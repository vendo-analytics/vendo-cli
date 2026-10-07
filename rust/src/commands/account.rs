//! Account and profile commands: `logout`, `profile *` and `mcp` (ports
//! of the matching files in `src/commands/`; `config *` moved under
//! `profile`, `whoami` and `logout --all` in CLI 1.1, VE-3827). `init` became
//! `login` (VE-3825, `commands/login.rs`); `whoami` and `doctor` became
//! `workspace` (VE-3891, `commands/workspace.rs`).

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    browse,
    config::{ConfigValueUpdates, unknown_vendo_profile},
    context::Ctx,
    output::{bold, confirm, dim, green, message_error_json, print_error, print_error_json, print_json, print_success},
    profile_display::{SwitchOptions, print_profile_list, profile_json, profile_list_cells, switch_profile_selection},
};

const NO_PROFILES: &str = "No profiles configured. Run `vendo login` to create one.";
const NO_PROFILES_YET: &str = "No profiles yet. Run `vendo login` to create one.";
const NOT_LOGGED_IN: &str = "Not currently logged in.";

/// `logout --json`: `{ "removed": [<profile name>…] }`, the profiles the command removed from the
/// config, none when it removed nothing (VE-3831). Not logged in, it is the JSON error instead.
fn print_removed(names: &[String]) {
    print_json(&json!({ "removed": names }));
}

pub fn logout(ctx: &Ctx, all: bool, yes: bool, json: bool) -> Result<()> {
    if all {
        // The TS CLI removed everything without asking (VE-3823, Yalcin 2026-10-06).
        if !confirm(yes, "Remove every saved profile?", "This removes every saved profile and its API key.")? {
            if json {
                print_removed(&[]);
            } else {
                println!("{}", dim("Cancelled."));
            }
            return Ok(());
        }
        let names: Vec<String> = ctx.store.profile_summaries().into_iter().map(|profile| profile.name).collect();
        let deleted = ctx.store.delete();
        if json {
            print_removed(if deleted { &names } else { &[] });
        } else if deleted {
            print_success("Logged out. All profiles removed.");
        } else {
            println!("{}", dim("No configuration file found."));
        }
        return Ok(());
    }
    let config = ctx.effective();
    if config.api_key.is_none() {
        // A VENDO_PROFILE that names no profile is the error that names it (Yalcin, 2026-10-06),
        // with exit 1 as for every other command: "not logged in" would hide that the profile is
        // not there while the active one may be logged in. --profile is as before.
        if let Some(name) = config.unknown_vendo_profile() {
            bail!(unknown_vendo_profile(name));
        }
        // With --json the JSON error on stderr and nothing on stdout; exit 0 either way, as the TS
        // CLI's text did (Yalcin, 2026-10-06).
        if json {
            print_error_json(&message_error_json(NOT_LOGGED_IN));
        } else {
            print_error(NOT_LOGGED_IN);
        }
        return Ok(());
    }
    let removed = ctx.store.clear_active_profile()?;
    if json {
        print_removed(removed.as_slice());
        return Ok(());
    }
    match removed {
        Some(name) => print_success(&format!("Logged out of profile \"{name}\".")),
        None => print_success("Logged out."),
    }
    Ok(())
}

pub fn profile_set(
    ctx: &Ctx,
    api_key: Option<String>,
    base_url: Option<String>,
    account: Option<String>,
    json: bool,
) -> Result<()> {
    let given = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
    if !given(&api_key) && !given(&base_url) && !given(&account) {
        bail!("Provide at least one option: --api-key <key>, --base-url <url>, or --account <id>");
    }
    let name = ctx.store.save_resolved_values(ConfigValueUpdates { api_key, base_url, account_id: account })?;
    let path = ctx.store.path().display().to_string();
    if json {
        // The profile it wrote, as `profile list --json` shows it, and the file (VE-3831).
        let profile = ctx.store.profile_summaries().into_iter().find(|profile| profile.name == name);
        print_json(&json!({ "profile": profile.as_ref().map(profile_json), "configPath": path }));
        return Ok(());
    }
    println!("{} {}", green("Configuration saved"), dim(&path));
    Ok(())
}

pub async fn profile_list(ctx: &Ctx, json: bool) -> Result<()> {
    let profiles = ctx.store.profile_summaries();
    if json {
        // What the list shows, one object per line of it (VE-3831).
        print_json(&json!({ "profiles": profiles.iter().map(profile_json).collect::<Vec<_>>() }));
        return Ok(());
    }
    // The lines, or at a terminal the same profiles to choose from (VE-3894), each offering `profile
    // switch` unless it is the saved active profile.
    let rows: Vec<Value> = profiles.iter().map(profile_json).collect();
    let cells = profiles.iter().map(profile_list_cells).collect();
    let print = || {
        print_profile_list(&profiles, true, "", NO_PROFILES);
    };
    browse::profiles(|| ctx.store.saved_active_profile(), &rows, cells, print).await
}

pub fn profile_switch(ctx: &Ctx, profile: Option<String>, account: Option<String>, json: bool) -> Result<()> {
    switch_profile_selection(
        ctx,
        &ctx.store.profile_summaries(),
        SwitchOptions {
            profile_name: profile,
            account_id: account,
            empty_message: NO_PROFILES_YET,
            list_command: "vendo profile list",
            profile_command: "vendo profile switch",
            verify_hint: "`vendo workspace`",
            json,
        },
    )
}

// ── mcp ────────────────────────────────────────────────────────────────────

const API_KEY_PLACEHOLDER: &str = "${VENDO_API_KEY}";

#[derive(Debug, PartialEq)]
pub struct McpClientConfig {
    pub endpoint: String,
    pub mcp_servers: Value,
    pub key_embedded: bool,
}

/// The MCP server is the web app's `/api/mcp` (stateless streamable HTTP);
/// a client only needs the URL and an `Authorization: Bearer` header.
pub fn build_mcp_client_config(base_url: &str, api_key: Option<&str>, show_key: bool) -> McpClientConfig {
    let endpoint = format!("{}/api/mcp", base_url.trim_end_matches('/'));
    let key = api_key.filter(|_| show_key);
    let authorization = format!("Bearer {}", key.unwrap_or(API_KEY_PLACEHOLDER));
    McpClientConfig {
        mcp_servers: json!({ "vendo": { "type": "http", "url": endpoint, "headers": { "Authorization": authorization } } }),
        endpoint,
        key_embedded: key.is_some(),
    }
}

pub fn mcp(ctx: &Ctx, json: bool, show_key: bool) {
    let config = ctx.effective();
    let mcp = build_mcp_client_config(&config.base_url, config.api_key.as_deref(), show_key);
    let block_value = json!({ "mcpServers": mcp.mcp_servers });
    if json {
        print_json(&block_value);
        return;
    }
    let block = serde_json::to_string_pretty(&block_value)
        .expect("JSON values always serialize")
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    println!();
    println!("{}", bold("Connect an MCP client to Vendo"));
    println!();
    println!("  Endpoint:   {}", mcp.endpoint);
    println!("  Transport:  streamable-http (stateless)");
    println!("  Auth:       Authorization: Bearer <api key>   (or OAuth in claude.ai)");
    println!();
    println!("  Add to your MCP client config (Claude Desktop, Cursor, Windsurf, …):");
    println!();
    println!("{block}");
    println!();
    if let (None, Some(name)) = (&config.api_key, config.unknown_vendo_profile()) {
        // Why there is no key: the profile VENDO_PROFILE names is not there (Yalcin, 2026-10-06).
        for line in unknown_vendo_profile(name).lines() {
            println!("{}", dim(&format!("  {}", line.trim_start())));
        }
    } else if config.api_key.is_none() {
        println!("{}", dim("  No API key configured — run `vendo login` or set VENDO_API_KEY first."));
    } else if !mcp.key_embedded {
        println!(
            "{}",
            dim(
                "  Replace ${VENDO_API_KEY} with your key (or set it in the client env), or re-run with --show-key to embed it."
            )
        );
    }
    println!(
        "{}",
        dim("  Tip: the MCP server lives on app2.vendodata.com — app.vendodata.com does not serve /api/mcp.")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_endpoint_comes_from_the_base_url() {
        let c = build_mcp_client_config("https://app2.vendodata.com", None, false);
        assert_eq!(c.endpoint, "https://app2.vendodata.com/api/mcp");
        assert_eq!(c.mcp_servers["vendo"]["type"], "http");
        assert_eq!(c.mcp_servers["vendo"]["url"], "https://app2.vendodata.com/api/mcp");
        assert_eq!(build_mcp_client_config("https://x.com///", None, false).endpoint, "https://x.com/api/mcp");
    }

    #[test]
    fn mcp_key_is_embedded_only_with_show_key() {
        let auth =
            |c: &McpClientConfig| c.mcp_servers["vendo"]["headers"]["Authorization"].as_str().unwrap().to_string();
        let placeholder = build_mcp_client_config("https://x.com", None, false);
        assert_eq!((auth(&placeholder).as_str(), placeholder.key_embedded), ("Bearer ${VENDO_API_KEY}", false));
        let hidden = build_mcp_client_config("https://x.com", Some("vendo_sk_secret"), false);
        assert_eq!((auth(&hidden).as_str(), hidden.key_embedded), ("Bearer ${VENDO_API_KEY}", false));
        let shown = build_mcp_client_config("https://x.com", Some("vendo_sk_secret"), true);
        assert_eq!((auth(&shown).as_str(), shown.key_embedded), ("Bearer vendo_sk_secret", true));
        let no_key = build_mcp_client_config("https://x.com", None, true);
        assert_eq!((auth(&no_key).as_str(), no_key.key_embedded), ("Bearer ${VENDO_API_KEY}", false));
    }
}

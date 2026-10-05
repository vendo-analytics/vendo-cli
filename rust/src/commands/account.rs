//! Account and profile commands: `init`, `logout`, `whoami`, `config *` and
//! `profile *` (ports of the matching files in `src/commands/`).

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    commands::login::{print_login_success, run_browser_login},
    config::ConfigValueUpdates,
    context::Ctx,
    identity::{Identity, fetch_identity},
    output::{
        bold, confirm, dim, green, js_join, js_nullish, js_string, js_template, print_error, print_json, print_success,
        run_action, yellow,
    },
    profile_display::{
        SwitchOptions, format_profile_list_line, print_current_profile_summary, print_profile_list,
        switch_profile_selection,
    },
    update_check,
};

const NO_PROFILES: &str = "No profiles configured. Run `vendo login` to create one.";
const NO_PROFILES_YET: &str = "No profiles yet. Run `vendo login` to create one.";

pub async fn init(ctx: &Ctx, env: Option<String>, base_url: Option<String>) -> Result<()> {
    println!("{}", bold("Vendo CLI Setup"));
    println!();

    let initial = ctx.effective();
    if initial.api_key.is_none() {
        println!("{}", dim("No API key found. Starting browser login..."));
        let base_url = ctx.store.resolve_login_base_url(env.as_deref(), base_url.as_deref())?;
        let result = run_browser_login(ctx, &base_url).await?;
        print_login_success(&result);
    } else {
        println!(
            "{}",
            dim(&format!(
                "Using existing profile {}.",
                initial.selected_profile.as_deref().unwrap_or("(legacy config)")
            ))
        );
    }

    let config = ctx.effective();
    let identity: Option<Identity> = match (&config.api_key, &config.account_id) {
        (Some(key), Some(account)) => fetch_identity(key, account, &config.base_url, ctx.debug).await.ok(),
        _ => None,
    };

    println!();
    println!("{}", bold("Setup summary"));
    println!("  Profile:     {}", config.selected_profile.clone().unwrap_or_else(|| dim("legacy config")));
    println!("  Base URL:    {}", config.base_url);
    println!("  Account ID:  {}", config.account_id.clone().unwrap_or_else(|| dim("missing")));
    if let Some(identity) = &identity {
        println!("  Auth:        {} as {}", green("verified"), identity.me.display_name());
    } else if config.api_key.is_some() && config.account_id.is_some() {
        println!("  Auth:        {} (API check failed)", yellow("not verified"));
    } else {
        println!("  Auth:        {} (account ID still required)", yellow("incomplete"));
    }

    println!();
    println!("{}", bold("Next steps"));
    println!("  vendo doctor");
    println!("  vendo whoami");
    println!("  vendo status");

    if config.account_id.is_none() {
        println!();
        println!(
            "{}",
            dim(
                "Set an account explicitly with `vendo config set --account <account-id>` if your login flow did not provide one."
            )
        );
    }

    print_success("Vendo CLI setup complete.");
    Ok(())
}

pub fn logout(ctx: &Ctx, all: bool) -> Result<()> {
    if all {
        if ctx.store.delete() {
            print_success("Logged out. All profiles removed.");
        } else {
            println!("{}", dim("No configuration file found."));
        }
        return Ok(());
    }
    if ctx.effective().api_key.is_none() {
        print_error("Not currently logged in.");
        return Ok(());
    }
    match ctx.store.clear_active_profile()? {
        Some(name) => print_success(&format!("Logged out of profile \"{name}\".")),
        None => print_success("Logged out."),
    }
    Ok(())
}

pub async fn whoami(ctx: &Ctx, json: bool) -> Result<()> {
    update_check::check(&ctx.update_cache_path()).await;
    let client = ctx.client()?;
    let mut res = run_action("Checking identity...", client.get("/me", &[])).await?;
    let config = ctx.effective();

    if json {
        let mut cfg = Map::new();
        if let Some(name) = &config.selected_profile {
            cfg.insert("selectedProfile".into(), json!(name));
        }
        cfg.insert("apiKeySource".into(), json!(config.api_key_source));
        cfg.insert("baseUrl".into(), json!(config.base_url));
        cfg.insert("baseUrlSource".into(), json!(config.base_url_source));
        if let Some(id) = &config.account_id {
            cfg.insert("accountId".into(), json!(id));
        }
        cfg.insert("accountIdSource".into(), json!(config.account_id_source));
        if let Value::Object(obj) = &mut res {
            obj.insert("config".into(), Value::Object(cfg));
        }
        print_json(&res);
        return Ok(());
    }

    // `${…}` of the `/me` fields, with the TS CLI's `??` fallbacks.
    let me = payload(&res);
    let field = |key: &str| me.get(key);
    let account_id = field("accountId");
    println!();
    println!("{}", bold(&js_template(js_nullish(js_nullish(field("accountName"), field("accountSlug")), account_id))));
    println!();
    println!("  Account:     {}", js_template(js_nullish(field("accountSlug"), account_id)));
    println!("  Account ID:  {}", js_template(account_id));
    println!("  Profile:     {}", config.selected_profile.clone().unwrap_or_else(|| dim("none selected")));
    println!("  Base URL:    {}", config.base_url);
    let api_key_id = field("apiKeyId").filter(|v| !v.is_null()).map(js_string);
    println!("  API Key:     {}", dim(api_key_id.as_deref().unwrap_or("unknown")));
    match field("scopes") {
        Some(Value::Array(scopes)) if !scopes.is_empty() => println!("  Scopes:      {}", js_join(scopes, ", ")),
        _ => println!("  Scopes:      {}", dim("full access")),
    }

    let overrides = config.env_override_names();
    if !overrides.is_empty() {
        println!();
        println!("{}", dim(&format!("  Env overrides active: {}", overrides.join(", "))));
    }

    let profiles = ctx.store.profile_summaries();
    if profiles.len() > 1 {
        println!();
        println!("{}", bold("  Profiles"));
        for profile in &profiles {
            println!("{}", format_profile_list_line(profile, false, "    "));
        }
        println!();
        println!(
            "{}",
            dim(
                "  Switch with `vendo profile switch`, target one command with `vendo --profile <name> ...`, or use `vendo config use` as a compatibility alias."
            )
        );
    }
    Ok(())
}

pub fn config_set(ctx: &Ctx, api_key: Option<String>, base_url: Option<String>, account: Option<String>) -> Result<()> {
    let given = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
    if !given(&api_key) && !given(&base_url) && !given(&account) {
        bail!("Provide at least one option: --api-key <key>, --base-url <url>, or --account <id>");
    }
    ctx.store.save_resolved_values(ConfigValueUpdates { api_key, base_url, account_id: account })?;
    println!("{} {}", green("Configuration saved"), dim(&ctx.store.path().display().to_string()));
    Ok(())
}

pub fn config_show(ctx: &Ctx) {
    let effective = ctx.effective();
    println!("{}", bold("Vendo CLI Config Inspect"));
    println!();
    println!("{}", dim("  Use `vendo profile current` for the day-to-day effective account view."));
    println!();
    println!("{}", bold("Effective Values"));
    println!();
    println!(
        "  API Key:        {} {}",
        effective.api_key.as_deref().map(crate::config::mask_api_key).unwrap_or_else(|| dim("not set")),
        dim(&format!("({})", effective.api_key_source.as_str()))
    );
    println!("  Base URL:       {} {}", effective.base_url, dim(&format!("({})", effective.base_url_source.as_str())));
    println!(
        "  Account ID:     {} {}",
        effective.account_id.clone().unwrap_or_else(|| dim("not set")),
        dim(&format!("({})", effective.account_id_source.as_str()))
    );
    println!("  Active Profile: {}", effective.selected_profile.clone().unwrap_or_else(|| dim("none selected")));
    println!("  Config Path:    {}", dim(&ctx.store.path().display().to_string()));
    println!();
    println!("{}", bold("Saved Profiles"));
    println!();
    print_profile_list(&ctx.store.profile_summaries(), true, "  ", NO_PROFILES);
}

pub fn config_use(ctx: &Ctx, profile: Option<String>, account: Option<String>) -> Result<()> {
    println!("{}", dim("Tip: prefer `vendo profile switch` for interactive profile changes."));
    println!();
    switch_profile_selection(
        ctx,
        &ctx.store.profile_summaries(),
        SwitchOptions {
            profile_name: profile,
            account_id: account,
            empty_message: NO_PROFILES_YET,
            list_command: "vendo profile list",
            profile_command: "vendo profile switch",
            verify_hint: "`vendo whoami`",
        },
    )
}

pub fn config_list(ctx: &Ctx) {
    println!("{}", dim("Tip: prefer `vendo profile list` for saved profile management."));
    println!();
    print_profile_list(&ctx.store.profile_summaries(), false, "", NO_PROFILES);
}

pub fn config_reset(ctx: &Ctx, yes: bool) {
    if !yes && !confirm("Delete all CLI configuration?") {
        println!("{}", dim("Cancelled."));
        return;
    }
    if ctx.store.delete() {
        print_success("Configuration deleted.");
    } else {
        println!("{}", dim("No configuration file found."));
    }
}

pub fn profile_list(ctx: &Ctx) {
    print_profile_list(&ctx.store.profile_summaries(), true, "", NO_PROFILES);
}

pub fn profile_current(ctx: &Ctx) {
    let config = ctx.effective();
    let profiles = ctx.store.profile_summaries();
    print_current_profile_summary(&config, profiles.iter().find(|p| p.active));
}

pub fn profile_switch(ctx: &Ctx, profile: Option<String>, account: Option<String>) -> Result<()> {
    switch_profile_selection(
        ctx,
        &ctx.store.profile_summaries(),
        SwitchOptions {
            profile_name: profile,
            account_id: account,
            empty_message: NO_PROFILES_YET,
            list_command: "vendo profile list",
            profile_command: "vendo profile switch",
            verify_hint: "`vendo profile current` or `vendo whoami`",
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
    if config.api_key.is_none() {
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

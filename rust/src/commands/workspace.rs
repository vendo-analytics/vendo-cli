//! `vendo workspace` (VE-3891, decided by Yalcin 2026-10-07): `whoami` and `doctor` as one command
//! with one screen, each fact once. First the account the CLI uses (its name and slug from `/me` as
//! the title, the account ID, profile, base URL and the API key masked with its key ID and scopes),
//! the env overrides, the saved profiles with the active one marked, then doctor's checks
//! ([`health::local_checks`], [`health::auth_check`]) with their fixes. Signed out, offline or with
//! no key it shows what the config gives and the checks, and it exits 1 when a check fails, as
//! doctor did. `whoami` and `doctor` are its hidden clap aliases, and `profile current` and
//! `config show` run it through `MOVED` (`cli.rs`): each prints exactly what it prints.
//!
//! `--json` keeps every key whoami's and doctor's JSON had, so their scripts keep working: `/me`'s
//! response as sent (`data`) and `config` (whoami's), then `summary`, `checks`, `suggestions`,
//! `identity` and `shell` (doctor's). `data` and `identity` are there when `/me` answered.

use std::{collections::BTreeSet, process::ExitCode};

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::{
    commands::health::{self, CheckStatus, DoctorCheck, DoctorEnv, Listed},
    config::{DEFAULT_BASE_URL, EffectiveConfig, ProfileSummary, mask_api_key, vendo_profile_overrides},
    context::Ctx,
    identity::{Identity, Me, fetch_identity},
    output::{bold, dim, green, print_json, red, run_action, yellow},
    update_check,
};

pub async fn run(ctx: &Ctx, json: bool) -> Result<ExitCode> {
    // whoami's notice of a newer release, at most once a day.
    update_check::check(&ctx.update_cache_path()).await;
    let config = ctx.effective();
    let env = DoctorEnv::current(ctx);
    let mut checks = health::local_checks(&env, &config);
    // `/me` takes the account (as `X-Account-Id`) with the key: without either nothing is asked,
    // and the checks say which is missing.
    let identity = match (&config.api_key, &config.account_id) {
        (Some(key), Some(account)) => {
            Some(run_action("Checking identity...", fetch_identity(key, account, &config.base_url, ctx.debug)).await)
        }
        _ => None,
    };
    checks.push(health::auth_check(identity.as_ref(), ctx.store.vendo_profile()));
    let identity = identity.and_then(Result::ok);
    let failed = checks.iter().any(|check| check.status == CheckStatus::Fail);
    let exit = if failed { ExitCode::from(1) } else { ExitCode::SUCCESS };

    if json {
        print_json(&workspace_json(identity.as_ref(), &config, &checks, env.shell.as_deref()));
        return Ok(exit);
    }
    let me = identity.as_ref().map(|identity| &identity.me);
    let profiles = ctx.store.profile_summaries();
    for line in screen(me, &config, &profiles, ctx.store.vendo_profile(), &checks) {
        println!("{line}");
    }
    Ok(exit)
}

// ── --json ─────────────────────────────────────────────────────────────────

/// whoami's keys, then doctor's.
fn workspace_json(
    identity: Option<&Identity>,
    config: &EffectiveConfig,
    checks: &[DoctorCheck],
    shell: Option<&str>,
) -> Value {
    let mut out = Map::new();
    if let Some(Value::Object(response)) = identity.map(|identity| &identity.response) {
        out.extend(response.clone());
    }
    out.insert("config".into(), config_json(config));
    let count = |status: CheckStatus| checks.iter().filter(|check| check.status == status).count();
    out.insert(
        "summary".into(),
        json!({ "ok": count(CheckStatus::Ok), "warn": count(CheckStatus::Warn), "fail": count(CheckStatus::Fail) }),
    );
    out.insert("checks".into(), json!(checks));
    let mut seen = BTreeSet::new();
    let suggestions: Vec<&String> =
        checks.iter().filter_map(|check| check.remediation.as_ref()).filter(|fix| seen.insert(*fix)).collect();
    out.insert("suggestions".into(), json!(suggestions));
    if let Some(identity) = identity {
        out.insert("identity".into(), identity.raw.clone());
    }
    out.insert("shell".into(), json!(shell.unwrap_or("unknown shell")));
    Value::Object(out)
}

/// whoami's `config`: what the CLI uses and where each value comes from.
fn config_json(config: &EffectiveConfig) -> Value {
    let mut out = Map::new();
    if let Some(name) = &config.selected_profile {
        out.insert("selectedProfile".into(), json!(name));
    }
    out.insert("apiKeySource".into(), json!(config.api_key_source));
    out.insert("baseUrl".into(), json!(config.base_url));
    out.insert("baseUrlSource".into(), json!(config.base_url_source));
    if let Some(id) = &config.account_id {
        out.insert("accountId".into(), json!(id));
    }
    out.insert("accountIdSource".into(), json!(config.account_id_source));
    Value::Object(out)
}

// ── the screen ─────────────────────────────────────────────────────────────

/// The screen, line by line. A value the CLI does not have (no profile selected, no key, no
/// account) has no line here: its check says so, with its fix.
fn screen(
    me: Option<&Me>,
    config: &EffectiveConfig,
    profiles: &[ProfileSummary],
    vendo_profile: Option<&str>,
    checks: &[DoctorCheck],
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(me) = me {
        lines.push(bold(&title(me)));
    }
    if let Some(id) = me.map(|me| me.account_id.as_str()).or(config.account_id.as_deref()) {
        lines.push(format!("  Account ID:  {id}"));
    }
    if let Some(name) = config.selected_profile.as_deref().filter(|name| !name.is_empty()) {
        lines.push(format!("  Profile:     {name}"));
    }
    lines.push(format!("  Base URL:    {}", config.base_url));
    if let Some(key) = &config.api_key {
        lines.push(format!("  API key:     {}", api_key(key, me)));
    }

    let overrides = config.env_override_names();
    if !overrides.is_empty() {
        lines.push(String::new());
        lines.push(dim(&format!("  Env overrides active: {}", overrides.join(", "))));
    }

    // One profile is the one above. Under the list, the note that VENDO_PROFILE overrides the
    // active profile, which the fixes below then leave out (each fact once; VE-3891 review).
    let listed = profiles.len() > 1;
    if listed {
        lines.push(String::new());
        lines.push(bold("Profiles"));
        lines.extend(profile_rows(profiles));
        if let Some(name) = vendo_profile {
            let overrides = vendo_profile_overrides(name);
            lines.push(dim(&format!("  {overrides}: change or unset VENDO_PROFILE to switch here.")));
        }
    }

    lines.push(String::new());
    lines.push(bold("Checks"));
    lines.extend(check_lines(checks, listed && vendo_profile.is_some()));
    lines
}

/// The account's name and slug (`T101 · t101`); the name alone when it has no slug, or the same
/// one; else the slug, else the ID.
fn title(me: &Me) -> String {
    match (me.account_name.as_deref(), me.account_slug.as_deref()) {
        (Some(name), Some(slug)) if name != slug => format!("{name} · {slug}"),
        _ => me.display_name().to_string(),
    }
}

/// The key masked as doctor masked it, then, once `/me` has answered, its key ID and scopes
/// (`full access` for none, as whoami said).
fn api_key(key: &str, me: Option<&Me>) -> String {
    let masked = mask_api_key(key);
    let Some(me) = me else { return masked };
    let key_id = me.api_key_id.as_deref().unwrap_or("unknown");
    let scopes = match me.scopes.as_deref() {
        Some(scopes) if !scopes.is_empty() => scopes.join(", "),
        _ => "full access".to_string(),
    };
    format!("{masked}  {}", dim(&format!("(key ID {key_id}, scopes {scopes})")))
}

/// The saved profiles in columns, `*` on the active one: the name, the account ID's first 8
/// characters and `…`, and the base URL without its scheme, left out for the default one as the
/// profile list leaves it out.
fn profile_rows(profiles: &[ProfileSummary]) -> Vec<String> {
    let account = |profile: &ProfileSummary| profile.account_id.as_deref().map_or("no account".into(), short_id);
    let width = |cell: &dyn Fn(&ProfileSummary) -> String| {
        profiles.iter().map(|profile| cell(profile).chars().count()).max().unwrap_or(0)
    };
    let (names, accounts) = (width(&|profile| profile.name.clone()), width(&account));
    profiles
        .iter()
        .map(|profile| {
            let marker = if profile.active { green("*") } else { " ".to_string() };
            let host = if profile.base_url == DEFAULT_BASE_URL { "" } else { without_scheme(&profile.base_url) };
            let row = format!("  {marker} {:<names$}  {:<accounts$}  {host}", profile.name, account(profile));
            row.trim_end().to_string()
        })
        .collect()
}

/// `28bb9a3b…` for `28bb9a3b-1c2d-…`; an ID of 8 characters or fewer whole.
fn short_id(id: &str) -> String {
    if id.chars().count() <= 8 { id.to_string() } else { format!("{}…", id.chars().take(8).collect::<String>()) }
}

/// `stg.vendodata.com` for `https://stg.vendodata.com/`.
fn without_scheme(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/')
}

/// The Checks section: each check in its few words after `[ok]`, `[warn]` or `[fail]`, with the
/// fix of each that did not pass under it, as [`DoctorCheck::listed`] places them. `noted`: the
/// profile list above says that VENDO_PROFILE overrides the active profile, so each fix shows
/// without that note ([`DoctorCheck::fix_without_override`]).
fn check_lines(checks: &[DoctorCheck], noted: bool) -> Vec<String> {
    // Each line's status, words and the checks it shows.
    let mut rows: Vec<(CheckStatus, String, Vec<&DoctorCheck>)> = Vec::new();
    for check in checks {
        match (check.listed, rows.last_mut()) {
            (Listed::WhenNotOk, _) if check.status == CheckStatus::Ok => {}
            (Listed::WithPrevious, Some((status, line, shown))) => {
                *status = (*status).max(check.status);
                line.push_str(", ");
                line.push_str(&check.line);
                shown.push(check);
            }
            _ => rows.push((check.status, check.line.clone(), vec![check])),
        }
    }
    let mut lines = Vec::new();
    for (status, line, shown) in rows {
        let marker = match status {
            CheckStatus::Ok => green("[ok]"),
            CheckStatus::Warn => yellow("[warn]"),
            CheckStatus::Fail => red("[fail]"),
        };
        lines.push(format!("  {marker} {line}"));
        for check in shown.iter().filter(|check| check.status != CheckStatus::Ok) {
            let without_note = check.fix_without_override.as_ref().filter(|_| noted);
            if let Some(fix) = without_note.or(check.remediation.as_ref()) {
                lines.push(format!("         {}", dim(&format!("Fix: {fix}"))));
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{STAGING_BASE_URL, Source};

    fn profile(name: &str, active: bool, account: Option<&str>, base_url: &str) -> ProfileSummary {
        ProfileSummary { name: name.into(), active, account_id: account.map(Into::into), base_url: base_url.into() }
    }

    fn me(name: Option<&str>, slug: Option<&str>) -> Me {
        Me {
            account_id: "73743172-2a4c-4f0e-9a8b-3f1d2c4b5a69".into(),
            account_name: name.map(Into::into),
            account_slug: slug.map(Into::into),
            api_key_id: Some("2d485183".into()),
            scopes: Some(vec!["*".into()]),
        }
    }

    fn config(api_key: Option<&str>, account: Option<&str>, profile: Option<&str>) -> EffectiveConfig {
        EffectiveConfig {
            config_exists: true,
            config_path: PathBuf::from("/h/.config/vendo/config.json"),
            selected_profile: profile.map(Into::into),
            selected_profile_exists: profile.is_some(),
            selected_by_vendo_profile: false,
            api_key: api_key.map(Into::into),
            api_key_source: if api_key.is_some() { Source::Profile } else { Source::Missing },
            base_url: STAGING_BASE_URL.into(),
            base_url_source: Source::Profile,
            account_id: account.map(Into::into),
            account_id_source: if account.is_some() { Source::Profile } else { Source::Missing },
        }
    }

    fn checked(name: &'static str, status: CheckStatus, line: &str, listed: Listed, fix: Option<&str>) -> DoctorCheck {
        DoctorCheck {
            name,
            status,
            detail: String::new(),
            remediation: fix.map(Into::into),
            fix_without_override: None,
            line: line.into(),
            listed,
        }
    }

    #[test]
    fn the_title_is_the_account_name_and_slug_each_once() {
        assert_eq!(title(&me(Some("T101"), Some("t101"))), "T101 · t101");
        assert_eq!(title(&me(Some("Acme"), Some("Acme"))), "Acme");
        assert_eq!(title(&me(Some("Acme"), None)), "Acme");
        assert_eq!(title(&me(None, Some("acme"))), "acme");
        assert_eq!(title(&me(None, None)), "73743172-2a4c-4f0e-9a8b-3f1d2c4b5a69");
    }

    #[test]
    fn the_key_is_masked_with_its_key_id_and_scopes_once_me_answers() {
        let key = "vendo_sk_fake_t101_eKuE";
        assert_eq!(api_key(key, Some(&me(None, None))), "vend...eKuE  (key ID 2d485183, scopes *)");
        assert_eq!(api_key(key, None), "vend...eKuE");
        let mut unknown = me(None, None);
        unknown.api_key_id = None;
        unknown.scopes = Some(vec![]);
        assert_eq!(api_key(key, Some(&unknown)), "vend...eKuE  (key ID unknown, scopes full access)");
        unknown.scopes = Some(vec!["read".into(), "write".into()]);
        assert_eq!(api_key(key, Some(&unknown)), "vend...eKuE  (key ID unknown, scopes read, write)");
    }

    #[test]
    fn profiles_line_up_with_short_ids_and_hosts() {
        let rows = profile_rows(&[
            profile("provin", false, Some("28bb9a3b-1c2d-4e5f-8a9b-0c1d2e3f4a5b"), STAGING_BASE_URL),
            profile("t101", true, Some("73743172-2a4c-4f0e-9a8b-3f1d2c4b5a69"), "https://stg.vendodata.com/"),
            profile("prod", false, Some("short"), DEFAULT_BASE_URL),
            profile("local", false, None, "http://127.0.0.1:3031"),
        ]);
        assert_eq!(
            rows,
            [
                "    provin  28bb9a3b…   stg.vendodata.com",
                "  * t101    73743172…   stg.vendodata.com",
                "    prod    short",
                "    local   no account  127.0.0.1:3031",
            ]
        );
        assert_eq!(short_id("12345678"), "12345678");
        assert_eq!(short_id("123456789"), "12345678…");
    }

    #[test]
    fn the_binary_and_path_share_a_line_and_passing_values_shown_above_are_left_out() {
        use CheckStatus::{Fail, Ok, Warn};
        let checks = [
            checked("CLI binary", Warn, "CLI 1.1.0 at /opt/vendo", Listed::Alone, Some("Reinstall.")),
            checked("PATH", Fail, "not on PATH", Listed::WithPrevious, Some("Add it to PATH.")),
            checked("Config file", Ok, "Config ~/.config/vendo/config.json", Listed::Alone, None),
            checked("Selected profile", Ok, "Selected profile: t101", Listed::WhenNotOk, None),
            checked("API key", Fail, "API key: Missing", Listed::WhenNotOk, Some("Run `vendo login`.")),
            checked("Shell completions", Ok, "Zsh completions installed", Listed::Alone, None),
        ];
        assert_eq!(
            check_lines(&checks, false),
            [
                "  [fail] CLI 1.1.0 at /opt/vendo, not on PATH",
                "         Fix: Reinstall.",
                "         Fix: Add it to PATH.",
                "  [ok] Config ~/.config/vendo/config.json",
                "  [fail] API key: Missing",
                "         Fix: Run `vendo login`.",
                "  [ok] Zsh completions installed",
            ]
        );
        // The worse status of the two, whichever comes first.
        let pair = |binary, path| {
            check_lines(
                &[
                    checked("CLI binary", binary, "CLI", Listed::Alone, None),
                    checked("PATH", path, "on PATH", Listed::WithPrevious, None),
                ],
                false,
            )
        };
        assert_eq!(pair(Ok, Ok), ["  [ok] CLI, on PATH"]);
        assert_eq!(pair(Warn, Ok), ["  [warn] CLI, on PATH"]);
        assert_eq!(pair(Ok, Fail), ["  [fail] CLI, on PATH"]);
    }

    #[test]
    fn the_screen_shows_what_the_config_has_when_me_did_not_answer() {
        let checks = [checked("API auth", CheckStatus::Fail, "API auth: fetch failed", Listed::Alone, None)];
        let two = [profile("a", true, Some("acct-a"), STAGING_BASE_URL), profile("b", false, None, STAGING_BASE_URL)];
        // A key, an account and a profile, but no answer: no title, no key ID.
        assert_eq!(
            screen(None, &config(Some("vendo_sk_fake_0000"), Some("acct-a"), Some("a")), &two, None, &checks),
            [
                "  Account ID:  acct-a",
                "  Profile:     a",
                "  Base URL:    https://stg.vendodata.com",
                "  API key:     vend...0000",
                "",
                "Profiles",
                "  * a  acct-a      stg.vendodata.com",
                "    b  no account  stg.vendodata.com",
                "",
                "Checks",
                "  [fail] API auth: fetch failed",
            ]
        );
        // Nothing: the base URL the CLI would use, and one profile is not listed.
        assert_eq!(
            screen(None, &config(None, None, None), &two[..1], None, &checks),
            ["  Base URL:    https://stg.vendodata.com", "", "Checks", "  [fail] API auth: fetch failed"]
        );
    }

    #[test]
    fn env_overrides_and_vendo_profile_are_noted() {
        let mut env = config(Some("vendo_sk_fake_0000"), Some("acct-a"), Some("b"));
        env.api_key_source = Source::Env;
        let two = [profile("a", false, Some("acct-a"), STAGING_BASE_URL), profile("b", true, None, STAGING_BASE_URL)];
        let lines = screen(Some(&me(Some("A"), None)), &env, &two, Some("b"), &[]);
        assert_eq!(
            lines,
            [
                "A",
                "  Account ID:  73743172-2a4c-4f0e-9a8b-3f1d2c4b5a69",
                "  Profile:     b",
                "  Base URL:    https://stg.vendodata.com",
                "  API key:     vend...0000  (key ID 2d485183, scopes *)",
                "",
                "  Env overrides active: VENDO_API_KEY",
                "",
                "Profiles",
                "    a  acct-a      stg.vendodata.com",
                "  * b  no account  stg.vendodata.com",
                "  VENDO_PROFILE=b overrides the active profile in this shell: change or unset VENDO_PROFILE to switch here.",
                "",
                "Checks",
            ]
        );
    }

    #[test]
    fn the_screen_says_once_that_vendo_profile_overrides_the_active_profile() {
        // Under the profile list when it lists the profiles, and then the fixes leave it out; with one
        // profile, which is not listed, in the fixes (VE-3891 review: each fact once).
        let note = "VENDO_PROFILE=b overrides the active profile in this shell";
        let mut check = checked(
            "API auth",
            CheckStatus::Fail,
            "API auth: fetch failed",
            Listed::Alone,
            Some(&format!("Check network access ({note}).")),
        );
        check.fix_without_override = Some("Check network access.".into());
        let checks = [check];
        let config = config(Some("vendo_sk_fake_0000"), Some("acct-a"), Some("b"));
        let two = [profile("a", false, Some("acct-a"), STAGING_BASE_URL), profile("b", true, None, STAGING_BASE_URL)];
        let lines = screen(None, &config, &two, Some("b"), &checks);
        assert_eq!(
            lines[lines.len() - 5..],
            [
                format!("  {note}: change or unset VENDO_PROFILE to switch here."),
                String::new(),
                "Checks".into(),
                "  [fail] API auth: fetch failed".into(),
                "         Fix: Check network access.".into(),
            ]
        );
        let lines = screen(None, &config, &two[1..], Some("b"), &checks);
        assert_eq!(
            lines[lines.len() - 3..],
            [
                "Checks".to_string(),
                "  [fail] API auth: fetch failed".into(),
                format!("         Fix: Check network access ({note}).")
            ]
        );
        assert_eq!(lines.iter().filter(|line| line.contains(note)).count(), 1);
    }

    #[test]
    fn the_json_has_whoamis_keys_then_doctors() {
        let identity = Identity {
            me: me(Some("T101"), Some("t101")),
            raw: json!({ "accountId": "a" }),
            response: json!({ "data": { "accountId": "a" } }),
        };
        let checks = [checked("API auth", CheckStatus::Ok, "Signed in as T101", Listed::Alone, None)];
        let config = config(Some("k"), Some("a"), Some("t101"));
        let keys = |value: Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            keys(workspace_json(Some(&identity), &config, &checks, Some("zsh"))),
            ["data", "config", "summary", "checks", "suggestions", "identity", "shell"]
        );
        let none = workspace_json(None, &config, &checks, None);
        assert_eq!(keys(none.clone()), ["config", "summary", "checks", "suggestions", "shell"]);
        assert_eq!(none["shell"], "unknown shell");
        assert_eq!(none["checks"][0], json!({ "name": "API auth", "status": "ok", "detail": "" }));
    }
}

//! `status`, `doctor` and `self-update` (ports of the matching files in
//! `src/commands/`).

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    client::payload,
    config::{EffectiveConfig, Source, mask_api_key},
    context::Ctx,
    identity::{Identity, IdentityError, fetch_identity},
    output::{bold, dim, gray, green, print_json, print_success, red, run_action, table, time_ago, yellow},
    update_check,
};

// ── status ─────────────────────────────────────────────────────────────────

pub async fn status(ctx: &Ctx, json: bool) -> Result<()> {
    update_check::check(&ctx.update_cache_path()).await;
    let client = ctx.client()?;
    let page = [("limit", Some("100".to_string()))];
    let failures = [
        ("status", Some("failed".to_string())),
        ("limit", Some("5".to_string())),
        ("sort", Some("created_at:desc".to_string())),
    ];
    let (apps, sources, integrations, jobs) = run_action("Fetching account status...", async {
        tokio::try_join!(
            client.get("/apps", &page),
            client.get("/sources", &page),
            client.get("/integrations", &page),
            client.get("/jobs", &failures),
        )
    })
    .await?;
    let (apps, sources, integrations, failed_jobs) =
        (payload(&apps), payload(&sources), payload(&integrations), payload(&jobs));

    if json {
        print_json(&json!({
            "apps": apps,
            "sources": sources,
            "integrations": integrations,
            "recentFailures": failed_jobs,
        }));
        return Ok(());
    }

    let rows = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let (apps, sources, integrations, failed_jobs) = (rows(apps), rows(sources), rows(integrations), rows(failed_jobs));
    let count = |items: &[Value], pred: &dyn Fn(&Value) -> bool| items.iter().filter(|i| pred(i)).count().to_string();
    let field_is = |key: &'static str, want: &'static str| move |v: &Value| v.get(key).and_then(Value::as_str) == Some(want);

    println!();
    let mut summary = table(&["", "Total", "Active", "Paused", "Errored"]);
    summary.add_row(vec![
        bold("Apps"),
        apps.len().to_string(),
        green(&count(&apps, &field_is("state", "active"))),
        gray(&count(&apps, &field_is("state", "inactive"))),
        red(&count(&apps, &|a| a.get("consecutiveFailureCount").and_then(Value::as_f64).is_some_and(|n| n > 0.0))),
    ]);
    summary.add_row(vec![
        bold("Sources"),
        sources.len().to_string(),
        green(&count(&sources, &field_is("state", "active"))),
        gray(&count(&sources, &field_is("integrationStatus", "paused"))),
        red(&count(&sources, &field_is("integrationStatus", "errored"))),
    ]);
    summary.add_row(vec![
        bold("Integrations"),
        integrations.len().to_string(),
        green(&count(&integrations, &field_is("state", "active"))),
        gray(&count(&integrations, &field_is("status", "paused"))),
        red(&count(&integrations, &field_is("status", "errored"))),
    ]);
    println!("{summary}");

    if failed_jobs.is_empty() {
        println!();
        println!("{}", green("No recent failures."));
    } else {
        println!();
        println!("{}", bold("Recent Failures"));
        let mut failures = table(&["Job ID", "Type", "Connector", "Error", "Failed"]);
        for job in &failed_jobs {
            let text = |key: &str| job.get(key).and_then(Value::as_str);
            failures.add_row(vec![
                dim(&text("id").unwrap_or_default().chars().take(8).collect::<String>()),
                text("jobType").unwrap_or_default().to_string(),
                text("connectorType").map(str::to_string).unwrap_or_else(|| dim("—")),
                truncate(text("errorMessage").unwrap_or("—"), 40),
                time_ago(text("finishedAt")),
            ]);
        }
        println!("{failures}");
    }

    println!();
    println!("{}", bold("Next steps"));
    match failed_jobs.first().and_then(|j| j.get("id")).and_then(Value::as_str) {
        Some(id) => {
            println!("  vendo jobs get {id}");
            println!("  vendo jobs list --status failed");
            println!("  vendo doctor");
        }
        None => {
            println!("  vendo jobs watch");
            println!("  vendo whoami");
            println!("  vendo doctor");
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}...", s.chars().take(max - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

// ── doctor ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

fn check(name: &'static str, status: CheckStatus, detail: String, remediation: Option<&str>) -> DoctorCheck {
    DoctorCheck { name, status, detail, remediation: remediation.map(str::to_string) }
}

/// Everything doctor inspects on the machine, so the checks are testable.
pub struct DoctorEnv {
    pub binary: PathBuf,
    pub standard_binary: PathBuf,
    pub path_var: String,
    pub shell: Option<String>,
    pub home: PathBuf,
}

const REINSTALL: &str = "Reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash` if you want the managed install path.";

/// The local checks, in the TS order (API auth is appended by the caller).
pub fn local_checks(env: &DoctorEnv, config: &EffectiveConfig) -> Vec<DoctorCheck> {
    use CheckStatus::*;
    let mut checks = Vec::new();
    let binary = env.binary.display().to_string();

    if env.binary == env.standard_binary {
        checks.push(check("CLI binary", Ok, binary.clone(), None));
    } else {
        checks.push(check(
            "CLI binary",
            Warn,
            format!("{binary} (standard install path is {})", env.standard_binary.display()),
            Some(REINSTALL),
        ));
    }

    let binary_dir = env.binary.parent().map(Path::to_path_buf).unwrap_or_default();
    let segments: Vec<&str> = env.path_var.split(':').filter(|s| !s.is_empty()).collect();
    let in_path = |dir: &Path| segments.iter().any(|s| Path::new(s) == dir);
    let standard_dir = env.standard_binary.parent().map(Path::to_path_buf).unwrap_or_default();
    if in_path(&binary_dir) || in_path(&standard_dir) {
        checks.push(check("PATH", Ok, format!("{} is available in PATH", binary_dir.display()), None));
    } else {
        checks.push(check(
            "PATH",
            Fail,
            format!("{} is not available in PATH", binary_dir.display()),
            Some(&path_remediation(env.shell.as_deref())),
        ));
    }

    let config_path = config.config_path.display().to_string();
    checks.push(if config.config_exists {
        check("Config file", Ok, config_path, None)
    } else {
        check("Config file", Warn, format!("{config_path} (not found yet)"), Some("Run `vendo init` to create and populate CLI config."))
    });

    checks.push(match &config.selected_profile {
        Some(name) if config.selected_profile_exists => check("Selected profile", Ok, name.clone(), None),
        Some(name) => check(
            "Selected profile",
            Warn,
            format!("{name} (not found in config)"),
            Some("Run `vendo config use` to switch profiles, or `vendo init` to create one."),
        ),
        None => check(
            "Selected profile",
            Warn,
            "No active profile selected".into(),
            Some("Run `vendo init` or `vendo config use <profile>`."),
        ),
    });

    checks.push(match &config.api_key {
        Some(key) => check("API key", Ok, format!("{} ({})", mask_api_key(key), format_source(config.api_key_source)), None),
        None => check("API key", Fail, "Missing".into(), Some("Run `vendo login` or `vendo config set --api-key <key>`.")),
    });

    checks.push(check("Base URL", Ok, format!("{} ({})", config.base_url, format_source(config.base_url_source)), None));

    checks.push(match &config.account_id {
        Some(id) => check("Account ID", Ok, format!("{id} ({})", format_source(config.account_id_source)), None),
        None => check(
            "Account ID",
            Fail,
            "Missing".into(),
            Some("Run `vendo config set --account <account-id>` or set `VENDO_ACCOUNT_ID`."),
        ),
    });

    checks.push(completion_check(env.shell.as_deref(), &env.home));
    checks
}

pub fn auth_check(result: Option<&Result<Identity, IdentityError>>) -> DoctorCheck {
    use CheckStatus::{Fail, Warn};
    match result {
        None => check("API auth", Warn, "Skipped because API key or account ID is missing".into(), None),
        Some(Ok(identity)) => check("API auth", CheckStatus::Ok, format!("Authenticated as {}", identity.me.display_name()), None),
        Some(Err(IdentityError::Http { status, status_text })) => check(
            "API auth",
            Fail,
            format!("HTTP {status}: {status_text}"),
            Some(api_auth_remediation(*status)),
        ),
        Some(Err(other)) => check(
            "API auth",
            Fail,
            other.to_string(),
            Some("Check network access and run `vendo whoami --debug` to inspect the failing request."),
        ),
    }
}

fn completion_check(shell: Option<&str>, home: &Path) -> DoctorCheck {
    use CheckStatus::*;
    let installed = |file: &str, rc: Option<&str>| {
        home.join(file).exists()
            && rc.is_none_or(|rc| {
                std::fs::read_to_string(home.join(rc)).is_ok_and(|text| text.contains("# >>> vendo completions >>>"))
            })
    };
    let missing = |label: &str, shell: &str| {
        check(
            "Shell completions",
            Warn,
            format!("{label} completions are not installed yet"),
            Some(&format!(
                "Run `vendo completions {shell}` or reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash`."
            )),
        )
    };
    match shell {
        Some("bash") if installed(".local/share/vendo/completions/vendo.bash", Some(".bashrc")) => {
            check("Shell completions", Ok, "Bash completions are installed in ~/.bashrc".into(), None)
        }
        Some("bash") => missing("Bash", "bash"),
        Some("zsh") if installed(".local/share/vendo/completions/vendo.zsh", Some(".zshrc")) => {
            check("Shell completions", Ok, "Zsh completions are installed in ~/.zshrc".into(), None)
        }
        Some("zsh") => missing("Zsh", "zsh"),
        Some("fish") if installed(".config/fish/completions/vendo.fish", None) => {
            check("Shell completions", Ok, "Fish completions are installed".into(), None)
        }
        Some("fish") => missing("Fish", "fish"),
        _ => check(
            "Shell completions",
            Warn,
            "Current shell could not be detected automatically".into(),
            Some("Run `vendo completions <shell>` manually after choosing your shell, or reinstall with the hosted installer."),
        ),
    }
}

fn path_remediation(shell: Option<&str>) -> String {
    let export_line = "export PATH=\"$HOME/.local/bin:$PATH\"";
    match shell {
        Some("bash") => format!("Add `{export_line}` to `~/.bashrc`, then restart your shell."),
        Some("zsh") => format!("Add `{export_line}` to `~/.zshrc`, then restart your shell."),
        Some("fish") => format!("Add `{export_line}` to `~/.config/fish/config.fish`, then restart your shell."),
        _ => format!("Add `{export_line}` to your shell profile, then restart your shell."),
    }
}

fn api_auth_remediation(status: u16) -> &'static str {
    match status {
        401 | 403 => "Run `vendo login` to refresh credentials, then retry `vendo whoami`.",
        404 => "Verify the configured account and base URL with `vendo whoami --debug`.",
        _ => "Run `vendo whoami --debug` to inspect the failing request and response.",
    }
}

fn format_source(source: Source) -> &'static str {
    match source {
        Source::Env => "from env",
        Source::Profile => "from profile",
        Source::Default => "default",
        Source::Missing => "missing",
    }
}

pub async fn doctor(ctx: &Ctx, json: bool) -> Result<ExitCode> {
    let config = ctx.effective();
    let shell = std::env::var("SHELL")
        .ok()
        .and_then(|s| s.rsplit('/').next().map(str::to_string))
        .filter(|s| !s.is_empty());
    let env = DoctorEnv {
        binary: std::env::current_exe().unwrap_or_default(),
        standard_binary: ctx.standard_binary_path(),
        path_var: std::env::var("PATH").unwrap_or_default(),
        shell: shell.clone(),
        home: ctx.home.clone(),
    };
    let mut checks = local_checks(&env, &config);
    let identity = match (&config.api_key, &config.account_id) {
        (Some(key), Some(account)) => Some(fetch_identity(key, account, &config.base_url, ctx.debug).await),
        _ => None,
    };
    checks.push(auth_check(identity.as_ref()));

    let summary = |status: CheckStatus| checks.iter().filter(|c| c.status == status).count();
    let (ok, warn, fail) = (summary(CheckStatus::Ok), summary(CheckStatus::Warn), summary(CheckStatus::Fail));
    let mut seen = BTreeSet::new();
    let suggestions: Vec<String> = checks
        .iter()
        .filter_map(|c| c.remediation.clone())
        .filter(|r| seen.insert(r.clone()))
        .collect();
    let exit = if fail > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS };

    if json {
        let mut out = json!({
            "summary": { "ok": ok, "warn": warn, "fail": fail },
            "checks": checks,
            "suggestions": suggestions,
        });
        if let Some(Ok(identity)) = &identity {
            out["identity"] = identity.raw.clone();
        }
        out["shell"] = json!(shell.as_deref().unwrap_or("unknown shell"));
        print_json(&out);
        return Ok(exit);
    }

    println!("{}", bold("Vendo CLI Doctor"));
    println!();
    for check in &checks {
        let marker = match check.status {
            CheckStatus::Ok => green("[ok]"),
            CheckStatus::Warn => yellow("[warn]"),
            CheckStatus::Fail => red("[fail]"),
        };
        println!("{marker} {}: {}", check.name, check.detail);
        if let (Some(fix), true) = (&check.remediation, check.status != CheckStatus::Ok) {
            println!("       {}", dim(&format!("Fix: {fix}")));
        }
    }
    println!();
    println!("{}", dim(&format!("Summary: {ok} ok, {warn} warnings, {fail} failures")));
    if !suggestions.is_empty() {
        println!();
        println!("{}", bold("Suggested next steps"));
        for suggestion in &suggestions {
            println!("  - {suggestion}");
        }
    }
    Ok(exit)
}

// ── self-update ────────────────────────────────────────────────────────────

const INSTALL_URL: &str = "https://app2.vendodata.com/install.sh";

pub fn self_update(ctx: &Ctx, version: Option<String>) -> Result<ExitCode> {
    let mut cmd = Command::new("bash");
    cmd.args(["-lc", &format!("curl -fsSL {INSTALL_URL} | bash")]);
    if let Some(version) = version {
        cmd.env("VENDO_VERSION", version);
    }
    let status = cmd.status().map_err(|err| anyhow!(err))?;
    if !status.success() {
        return Ok(ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8));
    }
    print_success("Vendo CLI updated.");
    let standard = ctx.standard_binary_path();
    if std::env::current_exe().ok().as_deref() != Some(standard.as_path()) {
        println!();
        println!(
            "{}",
            dim(&format!(
                "The installer writes to {}. If you use a custom path, switch to that binary after update.",
                standard.display()
            ))
        );
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::ApiError, config::DEFAULT_BASE_URL, identity::Me};

    fn config(api_key: Option<&str>, account: Option<&str>, profile: Option<(&str, bool)>, exists: bool) -> EffectiveConfig {
        EffectiveConfig {
            config_exists: exists,
            config_path: PathBuf::from("/h/.config/vendo/config.json"),
            selected_profile: profile.map(|p| p.0.to_string()),
            selected_profile_exists: profile.is_some_and(|p| p.1),
            api_key: api_key.map(Into::into),
            api_key_source: if api_key.is_some() { Source::Profile } else { Source::Missing },
            base_url: DEFAULT_BASE_URL.into(),
            base_url_source: Source::Default,
            account_id: account.map(Into::into),
            account_id_source: if account.is_some() { Source::Env } else { Source::Missing },
        }
    }

    fn env(home: &Path, binary: &str, path_var: &str, shell: Option<&str>) -> DoctorEnv {
        DoctorEnv {
            binary: PathBuf::from(binary),
            standard_binary: home.join(".local/bin/vendo"),
            path_var: path_var.into(),
            shell: shell.map(Into::into),
            home: home.to_path_buf(),
        }
    }

    #[test]
    fn healthy_install_is_all_ok() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".local/share/vendo/completions")).unwrap();
        std::fs::write(h.join(".local/share/vendo/completions/vendo.zsh"), "").unwrap();
        std::fs::write(h.join(".zshrc"), "# >>> vendo completions >>>\n").unwrap();
        let binary = h.join(".local/bin/vendo");
        let checks = local_checks(
            &env(h, binary.to_str().unwrap(), &format!("/usr/bin:{}", h.join(".local/bin").display()), Some("zsh")),
            &config(Some("vendo_sk_abcdefghij"), Some("acct"), Some(("p", true)), true),
        );
        assert!(checks.iter().all(|c| c.status == CheckStatus::Ok), "{checks:#?}");
        assert_eq!(checks[4].detail, "vend...ghij (from profile)");
        assert_eq!(checks[6].detail, "acct (from env)");
    }

    #[test]
    fn missing_pieces_warn_or_fail_with_fixes() {
        let home = tempfile::tempdir().unwrap();
        let checks = local_checks(&env(home.path(), "/opt/vendo/bin/vendo", "/usr/bin", Some("bash")), &config(None, None, None, false));
        let by_name = |n: &str| checks.iter().find(|c| c.name == n).unwrap().clone();
        assert_eq!(by_name("CLI binary").status, CheckStatus::Warn);
        assert_eq!(by_name("PATH").status, CheckStatus::Fail);
        assert_eq!(by_name("PATH").detail, "/opt/vendo/bin is not available in PATH");
        assert_eq!(
            by_name("PATH").remediation.as_deref(),
            Some("Add `export PATH=\"$HOME/.local/bin:$PATH\"` to `~/.bashrc`, then restart your shell.")
        );
        assert_eq!(by_name("Config file").detail, "/h/.config/vendo/config.json (not found yet)");
        assert_eq!(by_name("Selected profile").detail, "No active profile selected");
        assert_eq!((by_name("API key").status, by_name("Account ID").status), (CheckStatus::Fail, CheckStatus::Fail));
        assert_eq!(by_name("Shell completions").detail, "Bash completions are not installed yet");
    }

    #[test]
    fn auth_check_maps_identity_results() {
        assert_eq!(auth_check(None).status, CheckStatus::Warn);
        let ok = Ok(Identity {
            me: Me { account_id: "a".into(), account_name: Some("Acme".into()), account_slug: None, api_key_id: None, scopes: None },
            raw: json!({}),
        });
        assert_eq!(auth_check(Some(&ok)).detail, "Authenticated as Acme");
        let http = Err(IdentityError::Http { status: 401, status_text: "Unauthorized".into() });
        let c = auth_check(Some(&http));
        assert_eq!((c.status, c.detail.as_str()), (CheckStatus::Fail, "HTTP 401: Unauthorized"));
        assert_eq!(c.remediation.as_deref(), Some("Run `vendo login` to refresh credentials, then retry `vendo whoami`."));
        let net = Err(IdentityError::Transport(ApiError {
            message: "Request timed out".into(),
            status: 408,
            code: None,
            request_id: None,
            server_request_id: None,
            details: None,
            status_text: None,
        }));
        assert_eq!(auth_check(Some(&net)).detail, "Request timed out");
    }

    #[test]
    fn truncate_keeps_max_minus_one_then_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc...");
        assert_eq!(truncate("abcd", 4), "abcd");
    }
}

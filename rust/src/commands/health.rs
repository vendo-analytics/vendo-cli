//! `status`, the setup checks of `vendo workspace` (doctor's, VE-3891) and `self-update` (ports of
//! the matching files in `src/commands/`).

use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    client::payload,
    commands::completions::{self, Os, Setup},
    config::{EffectiveConfig, Source, mask_api_key, vendo_profile_overrides},
    context::Ctx,
    identity::{Identity, IdentityError},
    jobs::Job,
    output::{
        bold, dim, gray, green, js_length, js_slice, js_string, js_template, js_truthy, message_error_json,
        print_error_json, print_json, print_success, red, run_action, table, time_ago,
    },
    update_check::{self, INSTALL_COMMAND},
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
    let field_is =
        |key: &'static str, want: &'static str| move |v: &Value| v.get(key).and_then(Value::as_str) == Some(want);

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
        bold("Destinations"),
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
            let text = |key: &str| Job(job).text(key);
            failures.add_row(vec![
                dim(&js_slice(job.get("id").and_then(Value::as_str).unwrap_or_default(), 8)),
                text("jobType").unwrap_or_default(),
                text("connectorType").unwrap_or_else(|| dim("—")),
                truncate_error(job.get("errorMessage")),
                time_ago(job.get("finishedAt").and_then(Value::as_str)),
            ]);
        }
        println!("{failures}");
    }

    println!();
    println!("{}", bold("Next steps"));
    // `vendo workspace` where whoami and doctor were (VE-3891).
    match failed_jobs.first().filter(|job| js_truthy(job)) {
        Some(job) => {
            println!("  vendo jobs get {}", js_template(job.get("id")));
            println!("  vendo jobs list --status failed");
            println!("  vendo workspace");
        }
        None => {
            println!("  vendo jobs watch");
            println!("  vendo workspace");
        }
    }
    Ok(())
}

/// `truncate(job.errorMessage ?? '—', 40)`: strings longer than 40 UTF-16 units keep 39 and gain "...";
/// anything else (a number, say) has no `length` and prints as it is.
fn truncate_error(message: Option<&Value>) -> String {
    const MAX: usize = 40;
    match message.filter(|v| !v.is_null()) {
        None => "—".to_string(),
        Some(Value::String(s)) if js_length(s) > MAX => format!("{}...", js_slice(s, MAX - 1)),
        Some(other) => js_string(other),
    }
}

// ── the setup checks (doctor's) ────────────────────────────────────────────

/// Worst last, so the worse of two is their `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// How the `vendo workspace` screen lists a check (VE-3891).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listed {
    /// On a line of its own.
    Alone,
    /// At the end of the line before, after a comma, with the worse status of the two and both
    /// fixes: the binary's and PATH's checks share `CLI 1.1.0 at ~/.local/bin/vendo, on PATH`.
    WithPrevious,
    /// Only when it does not pass: the lines above the checks show its value (each fact once).
    WhenNotOk,
}

/// One check. The JSON is doctor's (`name`, `status`, `detail`, `remediation`); `line` and
/// `listed` are how the workspace screen shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    /// The fix without its note that VENDO_PROFILE overrides the active profile, where
    /// `remediation` ends with or starts with one: the workspace screen says that once, under its
    /// profile list when it lists the profiles, and there shows this (each fact once; VE-3891 review).
    #[serde(skip)]
    pub fix_without_override: Option<String>,
    /// Its words on the screen: those of the screen Yalcin agreed where it has them, else
    /// `<name>: <detail>` as doctor printed it.
    #[serde(skip)]
    pub line: String,
    #[serde(skip)]
    pub listed: Listed,
}

fn check(name: &'static str, status: CheckStatus, detail: String, remediation: Option<&str>) -> DoctorCheck {
    let line = format!("{name}: {detail}");
    DoctorCheck {
        name,
        status,
        detail,
        remediation: remediation.map(str::to_string),
        fix_without_override: None,
        line,
        listed: Listed::Alone,
    }
}

impl DoctorCheck {
    fn listed_as(mut self, line: String) -> Self {
        self.line = line;
        self
    }

    fn listed(mut self, listed: Listed) -> Self {
        self.listed = listed;
        self
    }

    /// `fix`, which sends the user to `vendo workspace`: when VENDO_PROFILE chose the profile it
    /// ends with the note that VENDO_PROFILE overrides the active profile (Yalcin, 2026-10-06), which
    /// the screen leaves out where its profile list says it ([`DoctorCheck::fix_without_override`]).
    fn fixed_by(mut self, fix: &str, vendo_profile: Option<&str>) -> Self {
        self.remediation = Some(match vendo_profile {
            Some(name) => {
                self.fix_without_override = Some(fix.to_string());
                format!("{} ({}).", fix.trim_end_matches('.'), vendo_profile_overrides(name))
            }
            None => fix.to_string(),
        });
        self
    }
}

/// `path` with HOME written `~`, as the workspace screen shows paths (VE-3891). Compared by
/// component, so `/h//.local/bin/vendo` is under `/h/`; with HOME `/` or empty, as it is.
fn home_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if home.parent().is_some() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// Everything the checks inspect on the machine, so they are testable.
pub struct DoctorEnv {
    pub binary: PathBuf,
    pub standard_binary: PathBuf,
    pub path_var: String,
    pub shell: Option<String>,
    pub home: PathBuf,
    /// Where bash reads its startup files (VE-3830).
    pub os: Os,
}

impl DoctorEnv {
    /// This machine: the running binary, `$PATH`, the shell `$SHELL` names, HOME and the system.
    pub fn current(ctx: &Ctx) -> Self {
        DoctorEnv {
            binary: running_binary(),
            standard_binary: doctor_standard_binary(&ctx.home),
            path_var: std::env::var("PATH").unwrap_or_default(),
            shell: completions::login_shell(),
            home: ctx.home.clone(),
            os: Os::current(),
        }
    }
}

const REINSTALL: &str =
    "Reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash` if you want the managed install path.";

/// Doctor's standard install path: the TS template `${homedir()}/.local/bin/vendo`,
/// so a HOME ending in `/` gives `//` as it did there.
pub fn doctor_standard_binary(home: &Path) -> PathBuf {
    let mut path = home.as_os_str().to_owned();
    path.push("/.local/bin/vendo");
    PathBuf::from(path)
}

/// The running binary as Node's `process.execPath` names it: symlinks resolved.
fn running_binary() -> PathBuf {
    std::env::current_exe().and_then(std::fs::canonicalize).unwrap_or_default()
}

/// The local checks, in the TS order (API auth is appended by the caller).
/// Paths are compared as plain strings, as the TS CLI did: a PATH entry with
/// a trailing slash doesn't match.
pub fn local_checks(env: &DoctorEnv, config: &EffectiveConfig) -> Vec<DoctorCheck> {
    use CheckStatus::*;
    let mut checks = Vec::new();
    let binary = env.binary.display().to_string();
    let cli = format!("CLI {} at {}", env!("CARGO_PKG_VERSION"), home_path(&env.binary, &env.home));

    if env.binary.as_os_str() == env.standard_binary.as_os_str() {
        checks.push(check("CLI binary", Ok, binary.clone(), None).listed_as(cli));
    } else {
        let standard = format!("(standard install path is {})", home_path(&env.standard_binary, &env.home));
        checks.push(
            check(
                "CLI binary",
                Warn,
                format!("{binary} (standard install path is {})", env.standard_binary.display()),
                Some(REINSTALL),
            )
            .listed_as(format!("{cli} {standard}")),
        );
    }

    let binary_dir = env.binary.parent().map(Path::to_path_buf).unwrap_or_default();
    let segments: Vec<&str> = env.path_var.split(':').filter(|s| !s.is_empty()).collect();
    let in_path = |dir: &Path| segments.iter().any(|s| std::ffi::OsStr::new(s) == dir.as_os_str());
    let standard_dir = env.standard_binary.parent().map(Path::to_path_buf).unwrap_or_default();
    checks.push(
        if in_path(&binary_dir) || in_path(&standard_dir) {
            check("PATH", Ok, format!("{} is available in PATH", binary_dir.display()), None)
                .listed_as("on PATH".into())
        } else {
            check(
                "PATH",
                Fail,
                format!("{} is not available in PATH", binary_dir.display()),
                Some(&path_remediation(env.shell.as_deref())),
            )
            .listed_as("not on PATH".into())
        }
        .listed(Listed::WithPrevious),
    );

    let config_path = config.config_path.display().to_string();
    let shown = format!("Config {}", home_path(&config.config_path, &env.home));
    checks.push(if config.config_exists {
        check("Config file", Ok, config_path, None).listed_as(shown)
    } else {
        check(
            "Config file",
            Warn,
            format!("{config_path} (not found yet)"),
            Some("Run `vendo login` to create and populate CLI config."),
        )
        .listed_as(format!("{shown} (not found yet)"))
    });

    // The profile, key, base URL and account: the workspace screen lists their values above the
    // checks, so it lists their checks only when they do not pass.
    checks.push(match config.selected_profile.as_ref().filter(|name| !name.is_empty()) {
        Some(name) if config.selected_profile_exists => check("Selected profile", Ok, name.clone(), None),
        // Switching profiles does not help while VENDO_PROFILE names the missing one (Yalcin, 2026-10-06).
        Some(name) if config.selected_by_vendo_profile => {
            let mut check = check(
                "Selected profile",
                Warn,
                format!("{name} (not found in config)"),
                Some(&format!(
                    "{}: run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.",
                    vendo_profile_overrides(name)
                )),
            );
            check.fix_without_override = Some(
                "Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.".into(),
            );
            check
        }
        Some(name) => check(
            "Selected profile",
            Warn,
            format!("{name} (not found in config)"),
            Some("Run `vendo profile switch` to switch profiles, or `vendo login` to create one."),
        ),
        None => check(
            "Selected profile",
            Warn,
            "No active profile selected".into(),
            Some("Run `vendo login` or `vendo profile switch <profile>`."),
        ),
    }
    .listed(Listed::WhenNotOk));

    checks.push(
        match &config.api_key {
            Some(key) => {
                check("API key", Ok, format!("{} ({})", mask_api_key(key), format_source(config.api_key_source)), None)
            }
            None => check(
                "API key",
                Fail,
                "Missing".into(),
                Some("Run `vendo login` or `vendo profile set --api-key <key>`."),
            ),
        }
        .listed(Listed::WhenNotOk),
    );

    checks.push(
        check("Base URL", Ok, format!("{} ({})", config.base_url, format_source(config.base_url_source)), None)
            .listed(Listed::WhenNotOk),
    );

    checks.push(
        match &config.account_id {
            Some(id) => check("Account ID", Ok, format!("{id} ({})", format_source(config.account_id_source)), None),
            None => check(
                "Account ID",
                Fail,
                "Missing".into(),
                Some("Run `vendo profile set --account <account-id>` or set `VENDO_ACCOUNT_ID`."),
            ),
        }
        .listed(Listed::WhenNotOk),
    );

    checks.push(completion_check(env.shell.as_deref(), &env.home, env.os));
    checks
}

/// The API check. `vendo_profile` is the profile `VENDO_PROFILE` chose (no `--profile`): each fix
/// sends the user to `vendo workspace`, so it says that VENDO_PROFILE overrides the active profile
/// (Yalcin, 2026-10-06).
pub fn auth_check(result: Option<&Result<Identity, IdentityError>>, vendo_profile: Option<&str>) -> DoctorCheck {
    use CheckStatus::{Fail, Warn};
    match result {
        None => check("API auth", Warn, "Skipped because API key or account ID is missing".into(), None),
        Some(Ok(identity)) => {
            let name = identity.me.display_name();
            check("API auth", CheckStatus::Ok, format!("Authenticated as {name}"), None)
                .listed_as(format!("Signed in as {name}"))
        }
        Some(Err(IdentityError::Http { status, status_text })) => {
            check("API auth", Fail, format!("HTTP {status}: {status_text}"), None)
                .fixed_by(api_auth_remediation(*status, vendo_profile.is_some()), vendo_profile)
        }
        Some(Err(other)) => check("API auth", Fail, other.to_string(), None).fixed_by(
            "Check network access and run `vendo workspace --debug` to inspect the failing request.",
            vendo_profile,
        ),
    }
}

/// `vendo completions <shell>` only prints a script, so the fixes point at the installer, which
/// sets completions up, and at bare `vendo completions`, which says how (VE-3830).
fn completion_check(shell: Option<&str>, home: &Path, os: Os) -> DoctorCheck {
    let setup = completions::setup(shell, home, os);
    let (status, fix) = match setup {
        Setup::Installed(..) => (CheckStatus::Ok, None),
        Setup::Missing(_) => (
            CheckStatus::Warn,
            Some(format!(
                "Reinstall with `{INSTALL_COMMAND}` to set them up, or run `vendo completions` for the manual steps."
            )),
        ),
        Setup::Unknown => {
            (CheckStatus::Warn, Some("Run `vendo completions` to see how to set them up for bash, zsh or fish.".into()))
        }
    };
    check("Shell completions", status, setup.detail(), fix.as_deref()).listed_as(setup.listed())
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

/// The fix for an HTTP error from `/me`, before [`DoctorCheck::fixed_by`] adds the VENDO_PROFILE note.
fn api_auth_remediation(status: u16, under_vendo_profile: bool) -> &'static str {
    match (status, under_vendo_profile) {
        // Under VENDO_PROFILE login saves the new key without making its profile active, and a plain
        // `vendo workspace` checks the profile VENDO_PROFILE names (Yalcin, 2026-10-06).
        (401 | 403, true) => {
            "Run `vendo login` to refresh credentials, then retry `vendo --profile <profile> workspace` with the profile it saved."
        }
        (401 | 403, false) => "Run `vendo login` to refresh credentials, then retry `vendo workspace`.",
        (404, _) => "Verify the configured account and base URL with `vendo workspace --debug`.",
        _ => "Run `vendo workspace --debug` to inspect the failing request and response.",
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

// ── self-update ────────────────────────────────────────────────────────────

const INSTALL_URL: &str = "https://app2.vendodata.com/install.sh";

/// Run the installer. With `--json` (VE-3831) its output goes to stderr, and stdout gets
/// `{ previousVersion, version, installPath, binaryPath }`: this CLI's version, what the installed
/// binary's `--version` prints (null when it cannot be run), where the installer wrote it, and the
/// binary that ran, which the text's note compares. A failed installer is the JSON error.
pub fn self_update(ctx: &Ctx, version: Option<String>, json: bool) -> Result<ExitCode> {
    let mut cmd = Command::new("bash");
    cmd.args(["-lc", &format!("curl -fsSL {INSTALL_URL} | bash")]);
    if let Some(version) = version.filter(|v| !v.is_empty()) {
        cmd.env("VENDO_VERSION", version);
    }
    if json {
        cmd.stdout(std::io::stderr());
    }
    let status = cmd.status().map_err(|err| anyhow!(err))?;
    if !status.success() {
        let code = status.code().unwrap_or(1).clamp(1, 255) as u8;
        if json {
            print_error_json(&message_error_json(&format!("The installer exited with code {code}.")));
        }
        return Ok(ExitCode::from(code));
    }
    // `join(homedir(), '.local', 'bin', 'vendo')` against `process.execPath`, as strings.
    let standard = ctx.standard_binary_path();
    if json {
        print_json(&json!({
            "previousVersion": env!("CARGO_PKG_VERSION"),
            "version": installed_version(&standard),
            "installPath": standard.display().to_string(),
            "binaryPath": running_binary().display().to_string(),
        }));
        return Ok(ExitCode::SUCCESS);
    }
    print_success("Vendo CLI updated.");
    if running_binary().as_os_str() != standard.as_os_str() {
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

/// What `<binary> --version` prints, when it runs and succeeds.
fn installed_version(binary: &Path) -> Option<String> {
    let out = Command::new(binary).arg("--version").stdin(std::process::Stdio::null()).output().ok()?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !version.is_empty()).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::ApiError, config::DEFAULT_BASE_URL, identity::Me};

    fn config(
        api_key: Option<&str>,
        account: Option<&str>,
        profile: Option<(&str, bool)>,
        exists: bool,
    ) -> EffectiveConfig {
        EffectiveConfig {
            config_exists: exists,
            config_path: PathBuf::from("/h/.config/vendo/config.json"),
            selected_profile: profile.map(|p| p.0.to_string()),
            selected_profile_exists: profile.is_some_and(|p| p.1),
            selected_by_vendo_profile: false,
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
            os: Os::Other,
        }
    }

    #[test]
    fn healthy_install_is_all_ok() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".local/share/vendo/completions")).unwrap();
        std::fs::write(h.join(".local/share/vendo/completions/vendo.zsh"), "# the script\n").unwrap();
        std::fs::write(h.join(".zshrc"), "# >>> vendo completions >>>\n").unwrap();
        let binary = h.join(".local/bin/vendo");
        let checks = local_checks(
            &env(h, binary.to_str().unwrap(), &format!("/usr/bin:{}", h.join(".local/bin").display()), Some("zsh")),
            &config(Some("vendo_sk_abcdefghij"), Some("acct"), Some(("p", true)), true),
        );
        assert!(checks.iter().all(|c| c.status == CheckStatus::Ok), "{checks:#?}");
        assert_eq!(checks[4].detail, "vend...ghij (from profile)");
        assert_eq!(checks[6].detail, "acct (from env)");
        // How the workspace screen lists them (VE-3891), paths under HOME as `~`.
        let listed: Vec<(&str, Listed)> = checks.iter().map(|c| (c.line.as_str(), c.listed)).collect();
        let cli = format!("CLI {} at ~/.local/bin/vendo", env!("CARGO_PKG_VERSION"));
        assert_eq!(
            listed,
            [
                (cli.as_str(), Listed::Alone),
                ("on PATH", Listed::WithPrevious),
                ("Config /h/.config/vendo/config.json", Listed::Alone),
                ("Selected profile: p", Listed::WhenNotOk),
                ("API key: vend...ghij (from profile)", Listed::WhenNotOk),
                ("Base URL: https://app2.vendodata.com (default)", Listed::WhenNotOk),
                ("Account ID: acct (from env)", Listed::WhenNotOk),
                ("Zsh completions installed", Listed::Alone),
            ]
        );
    }

    #[test]
    fn the_screen_writes_paths_under_home_with_a_tilde() {
        let home = Path::new("/Users/ana");
        assert_eq!(home_path(Path::new("/Users/ana/.config/vendo/config.json"), home), "~/.config/vendo/config.json");
        assert_eq!(
            home_path(Path::new("/Users/ana//.local/bin/vendo"), Path::new("/Users/ana/")),
            "~/.local/bin/vendo"
        );
        // Only under HOME, component by component.
        assert_eq!(home_path(Path::new("/Users/anabel/vendo"), home), "/Users/anabel/vendo");
        assert_eq!(home_path(Path::new("/opt/vendo/bin/vendo"), home), "/opt/vendo/bin/vendo");
        // A HOME of `/` or none would make every path `~`.
        assert_eq!(home_path(Path::new("/opt/vendo"), Path::new("/")), "/opt/vendo");
        assert_eq!(home_path(Path::new("/opt/vendo"), Path::new("")), "/opt/vendo");
    }

    #[test]
    fn missing_pieces_warn_or_fail_with_fixes() {
        let home = tempfile::tempdir().unwrap();
        let checks = local_checks(
            &env(home.path(), "/opt/vendo/bin/vendo", "/usr/bin", Some("bash")),
            &config(None, None, None, false),
        );
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
        // The workspace screen's words: the agreed ones, else doctor's `<name>: <detail>`.
        let line = |n: &str| by_name(n).line;
        assert_eq!(
            line("CLI binary"),
            format!(
                "CLI {} at /opt/vendo/bin/vendo (standard install path is ~/.local/bin/vendo)",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(line("PATH"), "not on PATH");
        assert_eq!(line("Config file"), "Config /h/.config/vendo/config.json (not found yet)");
        assert_eq!(line("Selected profile"), "Selected profile: No active profile selected");
        assert_eq!((line("API key"), line("Account ID")), ("API key: Missing".into(), "Account ID: Missing".into()));
        assert_eq!(line("Shell completions"), "Bash completions are not installed yet");
    }

    #[test]
    fn completions_set_up_by_hand_are_ok() {
        // The lines bare `vendo completions` gives for zsh, in ~/.zshrc without the installer (VE-3830).
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".zshrc"),
            "autoload -Uz compinit && compinit\neval \"$(vendo completions zsh)\"\n",
        )
        .unwrap();
        let check = completion_check(Some("zsh"), home.path(), Os::Other);
        assert_eq!(
            (check.status, check.detail.as_str(), check.remediation),
            (CheckStatus::Ok, "Zsh completions are installed in ~/.zshrc", None)
        );
        // The line alone fails on a stock zsh, where nothing has run compinit before it (Yalcin, 2026-10-06).
        std::fs::write(home.path().join(".zshrc"), "eval \"$(vendo completions zsh)\"\n").unwrap();
        let check = completion_check(Some("zsh"), home.path(), Os::Other);
        assert_eq!((check.status, check.detail.as_str()), (CheckStatus::Warn, "Zsh completions are not installed yet"));
    }

    #[test]
    fn on_macos_bash_completions_load_from_the_login_file_too() {
        // What install.sh leaves on macOS with only ~/.profile there: the block in ~/.bashrc and ~/.profile.
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".local/share/vendo/completions")).unwrap();
        std::fs::write(h.join(".local/share/vendo/completions/vendo.bash"), "# the script\n").unwrap();
        std::fs::write(h.join(".profile"), "# >>> vendo completions >>>\n").unwrap();
        let mut env = env(h, "/opt/vendo/bin/vendo", "/usr/bin", Some("bash"));
        let check = |env: &DoctorEnv| {
            let checks = local_checks(env, &config(None, None, None, true));
            let check = checks.into_iter().find(|c| c.name == "Shell completions").unwrap();
            (check.status, check.detail)
        };
        env.os = Os::MacOs;
        assert_eq!(check(&env), (CheckStatus::Ok, "Bash completions are installed in ~/.profile".into()));
        // A ~/.bash_profile beside it is what bash reads, so the block in ~/.profile loads nothing.
        std::fs::write(h.join(".bash_profile"), "export EDITOR=vi\n").unwrap();
        assert_eq!(check(&env), (CheckStatus::Warn, "Bash completions are not installed yet".into()));
        std::fs::remove_file(h.join(".bash_profile")).unwrap();
        std::fs::write(h.join(".bashrc"), "# >>> vendo completions >>>\n").unwrap();
        assert_eq!(check(&env), (CheckStatus::Ok, "Bash completions are installed in ~/.bashrc and ~/.profile".into()));
        // Elsewhere bash reads ~/.bashrc only.
        env.os = Os::Other;
        assert_eq!(check(&env), (CheckStatus::Ok, "Bash completions are installed in ~/.bashrc".into()));
        std::fs::remove_file(h.join(".bashrc")).unwrap();
        assert_eq!(check(&env), (CheckStatus::Warn, "Bash completions are not installed yet".into()));
    }

    #[test]
    fn auth_check_maps_identity_results() {
        assert_eq!(auth_check(None, None).status, CheckStatus::Warn);
        let ok = Ok(Identity {
            me: Me {
                account_id: "a".into(),
                account_name: Some("Acme".into()),
                account_slug: None,
                api_key_id: None,
                scopes: None,
            },
            raw: json!({}),
            response: json!({ "data": {} }),
        });
        assert_eq!(auth_check(Some(&ok), None).detail, "Authenticated as Acme");
        // The workspace screen's words (VE-3891).
        assert_eq!(auth_check(Some(&ok), None).line, "Signed in as Acme");
        assert_eq!(auth_check(None, None).line, "API auth: Skipped because API key or account ID is missing");
        let http = Err(IdentityError::Http { status: 401, status_text: "Unauthorized".into() });
        let c = auth_check(Some(&http), None);
        assert_eq!((c.status, c.detail.as_str()), (CheckStatus::Fail, "HTTP 401: Unauthorized"));
        assert_eq!(
            c.remediation.as_deref(),
            Some("Run `vendo login` to refresh credentials, then retry `vendo workspace`.")
        );
        let net = Err(IdentityError::Transport(ApiError {
            message: "Request timed out".into(),
            status: 408,
            code: None,
            request_id: None,
            server_request_id: None,
            details: None,
            status_text: None,
        }));
        assert_eq!(auth_check(Some(&net), None).detail, "Request timed out");
    }

    #[test]
    fn auth_fixes_under_vendo_profile_say_it_overrides_the_active_profile() {
        // Each fix sends the user to `vendo workspace`, which checks the profile VENDO_PROFILE names; after a
        // login under it the new key is in a profile it did not make active (Yalcin, 2026-10-06).
        let fix = |status: u16, vendo_profile: Option<&str>| {
            let http = Err(IdentityError::Http { status, status_text: "x".into() });
            auth_check(Some(&http), vendo_profile).remediation.unwrap()
        };
        let note = "(VENDO_PROFILE=beta overrides the active profile in this shell).";
        assert_eq!(
            fix(401, Some("beta")),
            format!(
                "Run `vendo login` to refresh credentials, then retry `vendo --profile <profile> workspace` with the profile it saved {note}"
            )
        );
        assert_eq!(fix(403, Some("beta")), fix(401, Some("beta")));
        assert_eq!(
            fix(404, Some("beta")),
            format!("Verify the configured account and base URL with `vendo workspace --debug` {note}")
        );
        assert_eq!(
            fix(500, Some("beta")),
            format!("Run `vendo workspace --debug` to inspect the failing request and response {note}")
        );
        let net = Err(IdentityError::Transport(ApiError {
            message: "Request timed out".into(),
            status: 408,
            code: None,
            request_id: None,
            server_request_id: None,
            details: None,
            status_text: None,
        }));
        assert_eq!(
            auth_check(Some(&net), Some("beta")).remediation.unwrap(),
            format!("Check network access and run `vendo workspace --debug` to inspect the failing request {note}")
        );
        // Without VENDO_PROFILE, as before.
        assert_eq!(fix(403, None), "Run `vendo login` to refresh credentials, then retry `vendo workspace`.");
        assert_eq!(fix(404, None), "Verify the configured account and base URL with `vendo workspace --debug`.");
        // The workspace screen says it once, under its profile list when it lists the profiles, and
        // there shows each fix without it (VE-3891 review: each fact once).
        let screen = |status: u16, vendo_profile: Option<&str>| {
            let http = Err(IdentityError::Http { status, status_text: "x".into() });
            auth_check(Some(&http), vendo_profile).fix_without_override
        };
        assert_eq!(
            screen(401, Some("beta")).as_deref(),
            Some(
                "Run `vendo login` to refresh credentials, then retry `vendo --profile <profile> workspace` with the profile it saved."
            )
        );
        assert_eq!(screen(403, Some("beta")), screen(401, Some("beta")));
        assert_eq!(
            screen(404, Some("beta")).as_deref(),
            Some("Verify the configured account and base URL with `vendo workspace --debug`.")
        );
        assert_eq!(
            screen(500, Some("beta")).as_deref(),
            Some("Run `vendo workspace --debug` to inspect the failing request and response.")
        );
        assert_eq!(
            auth_check(Some(&net), Some("beta")).fix_without_override.as_deref(),
            Some("Check network access and run `vendo workspace --debug` to inspect the failing request.")
        );
        // Without VENDO_PROFILE a fix has no note to leave out.
        assert_eq!((screen(401, None), screen(500, None)), (None, None));
        assert_eq!(auth_check(Some(&net), None).fix_without_override, None);
    }

    #[test]
    fn an_unknown_vendo_profile_is_a_warning_whose_fix_says_it_overrides_the_active_profile() {
        // A warning, so it never makes `vendo workspace` exit 1 by itself (VE-3891 review).
        let home = tempfile::tempdir().unwrap();
        let mut config = config(Some("vendo_sk_abcdefghij"), Some("acct"), Some(("nope", false)), true);
        config.selected_by_vendo_profile = true;
        let checks = local_checks(&env(home.path(), "/opt/vendo/bin/vendo", "/usr/bin", Some("zsh")), &config);
        let profile = checks.iter().find(|c| c.name == "Selected profile").unwrap();
        assert_eq!((profile.status, profile.detail.as_str()), (CheckStatus::Warn, "nope (not found in config)"));
        assert_eq!(
            profile.remediation.as_deref(),
            Some(
                "VENDO_PROFILE=nope overrides the active profile in this shell: run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile."
            )
        );
        assert_eq!(
            profile.fix_without_override.as_deref(),
            Some("Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.")
        );
        // An unknown --profile has no VENDO_PROFILE note.
        config.selected_by_vendo_profile = false;
        let checks = local_checks(&env(home.path(), "/opt/vendo/bin/vendo", "/usr/bin", Some("zsh")), &config);
        assert!(checks.iter().all(|c| c.fix_without_override.is_none()), "{checks:#?}");
    }

    #[test]
    fn error_messages_are_truncated_in_utf16_units_like_the_ts_cli() {
        // Expected values from Node: `truncate(m ?? '—', 40)` in src/commands/status.ts.
        let x = |n: usize| "x".repeat(n);
        let grin = |n: usize| "😀".repeat(n);
        assert_eq!(truncate_error(Some(&json!(x(41)))), format!("{}...", x(39)));
        assert_eq!(truncate_error(Some(&json!(x(40)))), x(40));
        // 20 emoji are exactly 40 UTF-16 units; 21 are cut inside the 20th, which prints as U+FFFD.
        assert_eq!(truncate_error(Some(&json!(grin(20)))), grin(20));
        assert_eq!(truncate_error(Some(&json!(grin(21)))), format!("{}\u{fffd}...", grin(19)));
        assert_eq!(truncate_error(Some(&json!("é".repeat(41)))), format!("{}...", "é".repeat(39)));
        assert_eq!(truncate_error(None), "—");
        assert_eq!(truncate_error(Some(&Value::Null)), "—");
        assert_eq!(truncate_error(Some(&json!(0))), "0");
        assert_eq!(truncate_error(Some(&json!(5))), "5");
    }

    #[test]
    fn an_empty_active_profile_name_is_no_profile() {
        let home = tempfile::tempdir().unwrap();
        let checks = local_checks(
            &env(home.path(), "/opt/vendo/bin/vendo", "/usr/bin", Some("bash")),
            &config(None, None, Some(("", false)), true),
        );
        let profile = checks.iter().find(|c| c.name == "Selected profile").unwrap();
        assert_eq!(profile.detail, "No active profile selected");
    }

    #[test]
    fn path_entries_are_compared_as_plain_strings() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        // A trailing slash or a `.` segment is a different string, as for `Array.includes` in TS.
        for path_var in ["/opt/vendo/bin/", "/opt/vendo/./bin", "/opt//vendo/bin"] {
            let checks =
                local_checks(&env(h, "/opt/vendo/bin/vendo", path_var, Some("zsh")), &config(None, None, None, true));
            let path = checks.iter().find(|c| c.name == "PATH").unwrap();
            assert_eq!(path.status, CheckStatus::Fail, "{path_var}");
        }
        let standard_dir_with_slash = format!("{}/", h.join(".local/bin").display());
        let checks = local_checks(
            &env(h, "/opt/vendo/bin/vendo", &standard_dir_with_slash, None),
            &config(None, None, None, true),
        );
        assert_eq!(checks.iter().find(|c| c.name == "PATH").unwrap().status, CheckStatus::Fail);
    }

    #[test]
    fn the_standard_path_is_the_home_string_plus_the_install_path() {
        // `${homedir()}/.local/bin/vendo`: a HOME ending in `/` gives `//`, which
        // never equals the running binary's canonical path.
        assert_eq!(doctor_standard_binary(Path::new("/h/")), PathBuf::from("/h//.local/bin/vendo"));
        let checks = local_checks(
            &DoctorEnv {
                binary: PathBuf::from("/h/.local/bin/vendo"),
                standard_binary: doctor_standard_binary(Path::new("/h/")),
                path_var: "/h/.local/bin".into(),
                shell: None,
                home: PathBuf::from("/h/"),
                os: Os::Other,
            },
            &config(None, None, None, true),
        );
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert_eq!(checks[0].detail, "/h/.local/bin/vendo (standard install path is /h//.local/bin/vendo)");
        assert_eq!(checks[1].status, CheckStatus::Ok);
    }
}

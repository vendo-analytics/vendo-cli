//! End-to-end checks of the built `vendo` binary (VE-3727): each test gets an
//! isolated HOME whose profiles point at a local stub (never a real API) and
//! a fresh update-check cache, so nothing leaves the machine.

use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

/// Nothing listens here: a test that forgets its stub fails locally.
const CLOSED: &str = "http://127.0.0.1:9";

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    /// Profiles `alpha` (active) and `beta` with fake keys on `base_url`.
    fn new(base_url: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".config/vendo");
        std::fs::create_dir_all(&dir).unwrap();
        let config = json!({
            "profiles": {
                "alpha": { "apiKey": "vendo_sk_fake_alpha_0000", "accountId": "acct-alpha", "baseUrl": base_url },
                "beta": { "apiKey": "vendo_sk_fake_beta_00000", "accountId": "acct-beta", "baseUrl": base_url },
            },
            "activeProfile": "alpha",
        });
        std::fs::write(dir.join("config.json"), serde_json::to_string_pretty(&config).unwrap()).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let cache = json!({ "lastCheck": now as u64, "latestVersion": env!("CARGO_PKG_VERSION") });
        std::fs::write(dir.join(".update-check"), cache.to_string()).unwrap();
        Sandbox { home }
    }

    /// The same profiles without API keys.
    fn without_api_keys(base_url: &str) -> Self {
        let sandbox = Sandbox::new(base_url);
        let mut config = sandbox.config();
        for profile in config["profiles"].as_object_mut().unwrap().values_mut() {
            profile.as_object_mut().unwrap().remove("apiKey");
        }
        std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
        sandbox
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vendo"));
        cmd.args(args).env("HOME", self.home.path()).stdin(Stdio::null());
        for var in ["VENDO_API_KEY", "VENDO_API_URL", "VENDO_ACCOUNT_ID", "VENDO_DEBUG"] {
            cmd.env_remove(var);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn config(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.home.path().join(".config/vendo/config.json")).unwrap())
            .unwrap()
    }
}

/// Run with stdout (and optionally stderr) connected to a pipe nobody reads,
/// like `vendo … | head -1` after `head` exits.
fn run_with_closed_output(sandbox: &Sandbox, args: &[&str], close_stderr: bool) -> Output {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut cmd = sandbox.command(args);
    if close_stderr {
        cmd.stderr(writer.try_clone().unwrap());
    } else {
        cmd.stderr(Stdio::piped());
    }
    cmd.stdout(writer).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_closed_stdout_is_ignored_like_nodes_console() {
    let sandbox = Sandbox::new(CLOSED);
    for shell in ["bash", "zsh", "fish"] {
        let out = run_with_closed_output(&sandbox, &["completions", shell], false);
        assert_eq!(out.status.code(), Some(0), "completions {shell}: {}", text(&out.stderr));
        assert_eq!(text(&out.stderr), "");
    }
    let out = run_with_closed_output(&sandbox, &["--version"], false);
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), String::new()));
}

#[tokio::test]
async fn a_closed_stdout_does_not_abort_a_listing() {
    let server = MockServer::start().await;
    let items: Vec<Value> = (0..2000).map(|i| json!({ "appType": format!("type_{i}"), "displayName": "T" })).collect();
    Mock::given(path("/api/v1/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": items })))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    for args in [&["catalog", "list", "--json"][..], &["catalog", "list"], &["catalog", "list", "--output", "appType"]]
    {
        let out = run_with_closed_output(&sandbox, args, false);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out.stderr));
        assert_eq!(text(&out.stderr), "", "{args:?}");
    }
    let out = run_with_closed_output(&sandbox, &["--debug", "catalog", "list"], true);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn the_global_profile_does_not_switch_profiles() {
    let sandbox = Sandbox::new(CLOSED);
    let out = sandbox.run(&["--profile", "beta", "profile", "switch"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), "Cancelled.\n".to_string()));
    assert_eq!(sandbox.config()["activeProfile"], "alpha");

    let out = sandbox.run(&["--profile", "alpha", "profile", "switch", "--account", "acct-beta"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("Done: Switched to profile beta."));
    assert_eq!(sandbox.config()["activeProfile"], "beta");
}

#[test]
fn version_prints_the_bare_number_anywhere() {
    let sandbox = Sandbox::new(CLOSED);
    for args in [&["--version"][..], &["-V"], &["whoami", "--version"], &["jobs", "list", "-V"]] {
        let out = sandbox.run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert_eq!(text(&out.stdout), format!("{}\n", env!("CARGO_PKG_VERSION")), "{args:?}");
    }
}

#[test]
fn empty_profile_names_and_accounts_are_unset() {
    let sandbox = Sandbox::new(CLOSED);
    for args in [
        &["profile", "switch", ""][..],
        &["profile", "switch", "--account", ""],
        &["--profile", "", "profile", "switch"],
    ] {
        let out = sandbox.run(args);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), "Cancelled.\n".to_string()), "{args:?}");
        assert_eq!(sandbox.config()["activeProfile"], "alpha", "{args:?}");
    }
    let out = sandbox.run(&["profile", "switch", "", "--account", "acct-beta"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(sandbox.config()["activeProfile"], "beta");

    let out = sandbox.run(&["--profile", "", "config", "set", "--account", "acct-x"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let config = sandbox.config();
    assert_eq!(
        (config["activeProfile"].as_str(), config["profiles"]["beta"]["accountId"].as_str()),
        (Some("beta"), Some("acct-x"))
    );
    assert!(config["profiles"].get("").is_none());
}

/// `self-update` runs `bash -lc "curl … | bash"`: a fake `bash` first on PATH
/// records the installer's VENDO_VERSION instead, so nothing is downloaded.
fn self_update_version(args: &[&str]) -> String {
    let sandbox = Sandbox::new(CLOSED);
    let bin = tempfile::tempdir().unwrap();
    let fake = bin.path().join("bash");
    std::fs::write(&fake, "#!/bin/sh\necho \"${VENDO_VERSION-unset}\" > \"$HOME/installer-version\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap_or_default());
    let out = sandbox.command(args).env("PATH", path).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    std::fs::read_to_string(sandbox.home.path().join("installer-version")).unwrap().trim().to_string()
}

#[cfg(unix)]
#[test]
fn self_update_passes_only_a_non_empty_version() {
    assert_eq!(self_update_version(&["self-update", "--version", "0.3.0"]), "0.3.0");
    assert_eq!(self_update_version(&["self-update", "--version", ""]), "unset");
    assert_eq!(self_update_version(&["self-update"]), "unset");
}

#[tokio::test]
async fn proxy_variables_are_ignored_like_node_fetch() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "accountId": "acct-alpha" } })))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    for var in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        let out = sandbox.command(&["whoami", "--json"]).env(var, CLOSED).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{var}: {}", text(&out.stderr));
    }
}

fn stderr_line(out: &Output) -> String {
    text(&out.stderr).lines().next().unwrap_or_default().to_string()
}

#[tokio::test]
async fn apps_report_the_first_bad_input_the_ts_cli_did() {
    let server = MockServer::start().await; // no catalog entry: the local role map
    let sandbox = Sandbox::new(&server.uri());
    let out = sandbox.run(&[
        "apps",
        "create",
        "--type",
        "t",
        "--name",
        "N",
        "--credentials-file",
        "nope.json",
        "--config-file",
        "nope2.json",
    ]);
    assert_eq!(
        stderr_line(&out),
        "Error: Failed to read nope.json: ENOENT: no such file or directory, open 'nope.json'"
    );

    let keyless = Sandbox::without_api_keys(CLOSED);
    for (args, expected) in [
        (
            &["apps", "create", "--type", "t", "--name", "N", "--credentials-file", "nope.json"][..],
            "Error: Failed to read nope.json: ENOENT: no such file or directory, open 'nope.json'",
        ),
        (
            &["apps", "create", "--type", "t", "--name", "N", "--config-file", "nope.json"],
            "Error: Failed to read nope.json: ENOENT: no such file or directory, open 'nope.json'",
        ),
        (&["apps", "update", "app-1"], "Error: Nothing to update — pass at least one flag."),
        (
            &["apps", "update", "app-1", "--role", "source"],
            "Error: No API key configured. Run `vendo login` or `vendo config set --api-key <key>` or set VENDO_API_KEY.",
        ),
    ] {
        let out = keyless.run(args);
        assert_eq!((out.status.code(), stderr_line(&out)), (Some(1), expected.to_string()), "{args:?}");
    }
}

#[tokio::test]
async fn a_role_the_catalog_refuses_creates_nothing() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/catalog/denied"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({ "error": { "message": "App type denied does not support role destination" } })),
        )
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    let creds = sandbox.home.path().join("creds.json");
    std::fs::write(&creds, r#"{"k":"v"}"#).unwrap();
    let out = sandbox.run(&[
        "apps",
        "create",
        "--type",
        "denied",
        "--name",
        "N",
        "--role",
        "destination",
        "--credentials-file",
        creds.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr_line(&out), "Error: App type denied does not support role destination");
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|r| r.method.as_str() == "GET"), "no POST may be sent");
}

#[cfg(unix)]
#[test]
fn doctor_names_the_running_binary_by_its_real_path() {
    let sandbox = Sandbox::new(CLOSED);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("vendo");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_vendo"), &link).unwrap();
    let out = Command::new(&link)
        .args(["doctor", "--json"])
        .env("HOME", sandbox.home.path())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("VENDO_API_KEY")
        .env_remove("VENDO_API_URL")
        .env_remove("VENDO_ACCOUNT_ID")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let real = std::fs::canonicalize(env!("CARGO_BIN_EXE_vendo")).unwrap();
    let detail = report["checks"][0]["detail"].as_str().unwrap();
    assert!(detail.starts_with(&format!("{} (standard install path is ", real.display())), "{detail}");
    assert_eq!(report["checks"][1]["detail"], format!("{} is not available in PATH", real.parent().unwrap().display()));
}

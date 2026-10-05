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

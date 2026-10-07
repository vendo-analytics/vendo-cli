//! End-to-end checks of the built `vendo` binary (VE-3727): each test gets an
//! isolated HOME whose profiles point at a local stub (never a real API) and
//! a fresh update-check cache, so nothing leaves the machine.

use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

/// Snapshots of every help screen and of each command's output (VE-3824),
/// recorded under `tests/snapshots/` with this file's sandbox and stub. Kept in
/// `tests/cli/` so cargo does not build it as a test target of its own.
#[path = "cli/snapshots.rs"]
mod snapshots;

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
        self.command_of(env!("CARGO_BIN_EXE_vendo"), args)
    }

    /// [`Sandbox::command`] running `program`, a copy of the binary elsewhere.
    fn command_of(&self, program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Command {
        let mut cmd = Command::new(program);
        // Dates and numbers follow the locale (VE-3728): pin it, and the time zone.
        cmd.args(args).env("HOME", self.home.path()).env("LANG", "C").env("TZ", "UTC").stdin(Stdio::null());
        for var in [
            "VENDO_API_KEY",
            "VENDO_API_URL",
            "VENDO_ACCOUNT_ID",
            "VENDO_PROFILE",
            "VENDO_DEBUG",
            "LC_ALL",
            "LC_MESSAGES",
        ] {
            cmd.env_remove(var);
        }
        // Output is piped, so no colour, whatever forces it in the caller's shell (VE-3824).
        cmd.env("NO_COLOR", "1");
        for var in ["FORCE_COLOR", "CLICOLOR_FORCE", "CLICOLOR", "IGNORE_IS_TERMINAL", "COLUMNS", "LINES"] {
            cmd.env_remove(var);
        }
        // Prompts as on a person's terminal, whatever turns them off where the tests run: `CI` is set
        // on CI runners (VE-3826), and `TERM=dumb` in some editors' shells.
        for var in ["CI", "VENDO_NO_INPUT", "TERM"] {
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
    // Bare, it explains itself on stderr (VE-3830).
    let out = run_with_closed_output(&sandbox, &["completions"], true);
    assert_eq!(out.status.code(), Some(0));
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
fn vendo_version_prints_what_the_version_flag_prints() {
    // VE-3893: the help lists `version`, which prints exactly what `--version` and `-V` print.
    let sandbox = Sandbox::new(CLOSED);
    let run = |args: &[&str]| {
        let out = sandbox.run(args);
        (out.status.code(), text(&out.stdout), text(&out.stderr))
    };
    let expected = (Some(0), format!("{}\n", env!("CARGO_PKG_VERSION")), String::new());
    for args in [&["version"][..], &["--version"], &["-V"], &["--profile", "beta", "version"], &["version", "--debug"]]
    {
        assert_eq!(run(args), expected, "vendo {}", args.join(" "));
    }
    // With --json, like every command (VE-3831): the version in an object.
    let (code, stdout, stderr) = run(&["version", "--json"]);
    assert_eq!((code, stderr), (Some(0), String::new()));
    assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), json!({ "version": env!("CARGO_PKG_VERSION") }));
    // The help works as before: `--help`, `-h` and `vendo help <command>`.
    assert_eq!(run(&["version", "--help"]).0, Some(0));
    for (args, same) in [
        (&["help", "version"][..], &["version", "--help"][..]),
        (&["version", "-h"], &["version", "--help"]),
        (&["help"], &["--help"]),
        (&["-h"], &["--help"]),
        (&["help", "apps", "list"], &["apps", "list", "--help"]),
        (&["apps", "list", "-h"], &["apps", "list", "--help"]),
    ] {
        assert_eq!(run(args), run(same), "vendo {}", args.join(" "));
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

    let out = sandbox.run(&["--profile", "", "profile", "set", "--account", "acct-x"]);
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
#[cfg(unix)]
fn self_update_version(args: &[&str]) -> String {
    let sandbox = Sandbox::new(CLOSED);
    let out = fake_installer(&sandbox, "echo \"${VENDO_VERSION-unset}\" > \"$HOME/installer-version\"\n", args);
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
        let out = sandbox.command(&["workspace", "--json"]).env(var, CLOSED).output().unwrap();
        let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
        let auth = printed["checks"].as_array().unwrap().iter().find(|c| c["name"] == "API auth").unwrap().clone();
        assert_eq!(auth["status"], "ok", "{var}: {auth} {}", text(&out.stderr));
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
            "Error: No API key configured. Run `vendo login` or `vendo profile set --api-key <key>` or set VENDO_API_KEY.",
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
fn workspace_names_the_running_binary_by_its_real_path() {
    let sandbox = Sandbox::new(CLOSED);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("vendo");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_vendo"), &link).unwrap();
    let out = Command::new(&link)
        .args(["workspace", "--json"])
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

// ── VE-3668: metrics, models and measurement ────────────────────────────────
// Expected output comes from the TypeScript CLI run against the same stub
// responses (dates are null where TS would print a locale date).

/// Serve `body` with `status` for `verb route` on `server`.
async fn serve(server: &MockServer, verb: &str, route: &str, status: u16, body: Value) {
    Mock::given(wiremock::matchers::method(verb))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

/// `METHOD path?query body` for every request the stub received, in order.
async fn sent(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap();
    requests
        .iter()
        .map(|r| {
            let query = r.url.query().map(|q| format!("?{q}")).unwrap_or_default();
            format!("{} {}{query} {}", r.method, r.url.path(), text(&r.body)).trim_end().to_string()
        })
        .collect()
}

/// Table and text rows as cells, the way the parity harness compares them:
/// border and column-gap differences are accepted, cell contents are not.
fn cells(stdout: &[u8]) -> Vec<Vec<String>> {
    text(stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.split("  ").map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect())
        .collect()
}

fn rows(expected: &[&[&str]]) -> Vec<Vec<String>> {
    expected.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
}

fn ok_output(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    text(&out.stdout)
}

/// A refused request: exit 1, `Error: <message>` and the CLI's `Request ID` line, as in TS.
fn assert_api_error(out: &Output, message: &str) {
    let stderr = text(&out.stderr);
    let (shown, request_id) = stderr.trim_end().rsplit_once("\nRequest ID: ").unwrap_or_else(|| panic!("{stderr}"));
    assert_eq!((out.status.code(), shown), (Some(1), format!("Error: {message}").as_str()));
    assert!(request_id.starts_with("cli-") && request_id.len() == 40, "{request_id}");
}

#[tokio::test]
async fn web_app_errors_print_the_servers_reason() {
    // The web-app routes answer `{ error: "<message>" }`; the v1 gateway's
    // `{ error: { code, message } }` and bodies that aren't JSON print as before (VE-3764).
    let server = MockServer::start().await;
    let zod = "[\n  {\n    \"code\": \"invalid_type\",\n    \"expected\": \"string\",\n    \"received\": \"undefined\",\n    \"path\": [\n      \"id\"\n    ],\n    \"message\": \"Required\"\n  }\n]";
    serve(&server, "POST", "/api/metrics", 400, json!({ "error": zod })).await;
    serve(&server, "DELETE", "/api/metrics/empty", 400, json!({ "error": "" })).await;
    serve(&server, "GET", "/api/measurement/ltv/customer/c1", 500, json!({ "error": "BigQuery is unavailable" })).await;
    let v1 = json!({ "error": { "code": "INTERNAL_ERROR", "message": "Methodologies are unavailable" } });
    serve(&server, "GET", "/api/measurement/methodologies", 500, v1).await;
    Mock::given(wiremock::matchers::method("PATCH"))
        .and(path("/api/metrics/html"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>"))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    let def = sandbox.home.path().join("def.json");
    std::fs::write(&def, r#"{"version":2,"reportType":"segmentation"}"#).unwrap();

    assert_api_error(&sandbox.run(&["metrics", "create", "--name", "x", "--definition", def.to_str().unwrap()]), zod);
    assert_api_error(&sandbox.run(&["measurement", "ltv", "customer", "c1"]), "BigQuery is unavailable");
    assert_api_error(&sandbox.run(&["measurement", "methodologies", "list"]), "Methodologies are unavailable");
    assert_api_error(&sandbox.run(&["metrics", "delete", "empty", "--yes"]), "HTTP 400");
    assert_api_error(&sandbox.run(&["metrics", "activate", "html"]), "HTTP 502: Bad Gateway");
}

const M1: &str = "6f1c2a9e-1111-4c3b-9a7e-000000000001";

fn metric(over: Value) -> Value {
    let mut row = json!({
        "id": M1, "access_scope": "workspace", "account_id": "acct-alpha", "name": "ROAS",
        "description": "Return on ad spend", "format": "multiplier", "higher_is_better": true, "unit": "x",
        "status": "active", "verified_at": null, "verified_by": null, "created_at": "2026-01-01T00:00:00Z",
        "updated_at": null, "definition": { "version": 2, "reportType": "segmentation" },
    });
    row.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    row
}

#[tokio::test]
async fn metrics_list_prints_the_table_fields_and_the_ts_envelope() {
    let server = MockServer::start().await;
    let body = json!({
        "metrics": [
            metric(json!({})),
            metric(json!({ "id": "short-id", "name": "Draft one", "status": "draft", "format": "number", "higher_is_better": false })),
            metric(json!({ "id": "abcdefabcdefabcdef", "name": "Archived", "status": "archived", "format": "currency", "metric_type": "legacy" })),
        ],
        "total": 3, "limit": 20, "offset": 0,
    });
    serve(&server, "GET", "/api/metrics", 200, body.clone()).await;
    let sandbox = Sandbox::new(&server.uri());

    let out = sandbox.run(&["metrics", "list"]);
    assert_eq!(
        cells(ok_output(&out).as_bytes()),
        rows(&[
            &["ID", "Name", "Format", "Status", "Updated"],
            &["6f1c2a9e...", "ROAS", "multiplier", "active", "—"],
            &["short-id", "Draft one", "number", "draft", "—"],
            // The API sends no type (VE-3856): one an older server sent is not shown.
            &["abcdefab...", "Archived", "currency", "archived", "—"],
            &["3 metrics"],
        ])
    );
    // The TS command wrapped the route's `{ metrics, total }` in its own envelope.
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&["metrics", "list", "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": body["metrics"], "meta": { "pagination": { "total": 3 } } }));
    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "list", "--output", "id"])),
        format!("{M1}\nshort-id\nabcdefabcdefabcdef\n")
    );
    assert_eq!(ok_output(&sandbox.run(&["metrics", "list", "--output", "higher_is_better"])), "true\nfalse\ntrue\n");
    assert_eq!(ok_output(&sandbox.run(&["metrics", "list", "--output", "metric_type"])), "legacy\n");
    ok_output(&sandbox.run(&["metrics", "list", "--status", "active", "--limit", "5", "--offset", "2"]));
    ok_output(&sandbox.run(&["metrics", "list", "--status", ""]));
    let requests = sent(&server).await;
    assert_eq!(requests[0], "GET /api/metrics?limit=20&offset=0");
    assert_eq!(requests[5], "GET /api/metrics?status=active&limit=5&offset=2");
    assert_eq!(requests[6], "GET /api/metrics?status=&limit=20&offset=0", "an empty --status is still sent, as in TS");
    let received = server.received_requests().await.unwrap();
    assert!(received.iter().all(|r| r.headers.get("x-account-id").is_none()), "web-app routes carry no account header");
}

#[tokio::test]
async fn metrics_get_prints_the_detail_view_like_ts() {
    let server = MockServer::start().await;
    serve(
        &server,
        "GET",
        &format!("/api/metrics/{M1}"),
        200,
        json!({ "metric": metric(json!({})), "registryWarning": "x" }),
    )
    .await;
    // The API sends no type (VE-3856): one an older server sent is not shown.
    let draft = metric(json!({
        "id": "short-id", "definition": null, "description": "", "unit": null, "higher_is_better": false,
        "status": "draft", "metric_type": "ratio",
    }));
    serve(&server, "GET", "/api/metrics/short-id", 200, json!({ "metric": draft })).await;
    let odd =
        metric(json!({ "id": "odd", "name": null, "definition": { "reportType": 7 }, "format": null, "status": null }));
    serve(&server, "GET", "/api/metrics/odd", 200, json!({ "metric": odd })).await;
    serve(&server, "GET", "/api/metrics/nometric", 200, json!({ "other": true })).await;
    serve(&server, "GET", "/api/metrics/missing", 404, json!({ "error": "Metric not found" })).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "get", M1])),
        format!(
            "\nROAS\n\n  ID:           {M1}\n  Format:       multiplier\n  Status:       active\n  Updated:      —\n  Description:  Return on ad spend\n  Unit:         x\n  Higher=Better: yes\n  Calculation:  segmentation\n"
        )
    );
    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "get", "short-id"])),
        "\nROAS\n\n  ID:           short-id\n  Format:       multiplier\n  Status:       draft\n  Updated:      —\n  Higher=Better: no\n  Calculation:  unknown\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "get", "odd"])),
        "\nnull\n\n  ID:           odd\n  Format:       null\n  Status:       null\n  Updated:      —\n  Description:  Return on ad spend\n  Unit:         x\n  Higher=Better: yes\n  Calculation:  unknown\n"
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&["metrics", "get", M1, "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": metric(json!({})) }));
    assert_eq!(ok_output(&sandbox.run(&["metrics", "get", "nometric", "--json"])), "{}\n");
    // The server's reason, not the generic 404 text (VE-3764).
    assert_api_error(&sandbox.run(&["metrics", "get", "missing"]), "Metric not found");
}

#[tokio::test]
async fn metrics_create_sends_the_query_spec_unchanged() {
    let server = MockServer::start().await;
    let draft = metric(json!({ "name": "CLI parity", "status": "draft" }));
    Mock::given(wiremock::matchers::method("POST"))
        .and(path("/api/metrics"))
        .and(wiremock::matchers::body_string_contains("\"format\":\"currency\""))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "metric": metric(json!({ "name": "CLI parity" })) })),
        )
        .mount(&server)
        .await;
    serve(&server, "POST", "/api/metrics", 201, json!({ "metric": draft, "registryWarning": "pending" })).await;
    let sandbox = Sandbox::new(&server.uri());
    let def = sandbox.home.path().join("def.query.json");
    // Big and tiny numbers go out the way JavaScript wrote them.
    std::fs::write(
        &def,
        r#"{"version":2,"reportType":"segmentation","metricOutput":{"kind":"measure","measureId":"m_a"},"big":12345678901234567890,"small":0.000001}"#,
    )
    .unwrap();
    let def = def.to_str().unwrap();
    let spec = r#"{"version":2,"reportType":"segmentation","metricOutput":{"kind":"measure","measureId":"m_a"},"big":12345678901234567000,"small":0.000001}"#;

    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "create", "--name", "CLI parity", "--definition", def])),
        format!(
            "\n✓ Metric \"CLI parity\" created\n  ID:     {M1}\n  Status: draft\n\n  The calculation could not compile. Update its definition before activating it.\n"
        )
    );
    assert_eq!(
        ok_output(&sandbox.run(&[
            "metrics",
            "create",
            "--name",
            "CLI parity",
            "--definition",
            def,
            "--description",
            "d",
            "--format",
            "currency",
            "--unit",
            "$",
        ])),
        format!("\n✓ Metric \"CLI parity\" created\n  ID:     {M1}\n  Status: active\n")
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&[
        "metrics",
        "create",
        "--name",
        "X",
        "--definition",
        def,
        "--json",
    ])))
    .unwrap();
    assert_eq!(printed, json!({ "data": metric(json!({ "name": "CLI parity", "status": "draft" })) }));
    ok_output(&sandbox.run(&[
        "metrics",
        "create",
        "--name",
        "",
        "--definition",
        def,
        "--description",
        "",
        "--unit",
        "",
    ]));
    let requests = sent(&server).await;
    assert_eq!(
        requests[0],
        format!(r#"POST /api/metrics {{"name":"CLI parity","definition":{spec},"format":"number"}}"#)
    );
    assert_eq!(
        requests[1],
        format!(
            r#"POST /api/metrics {{"name":"CLI parity","definition":{spec},"format":"currency","description":"d","unit":"$"}}"#
        )
    );
    assert_eq!(requests[3], format!(r#"POST /api/metrics {{"name":"","definition":{spec},"format":"number"}}"#));
}

#[tokio::test]
async fn a_metric_definition_that_cannot_be_read_names_the_file() {
    let keyless = Sandbox::without_api_keys(CLOSED);
    let empty = keyless.home.path().join("empty.json");
    std::fs::write(&empty, "").unwrap();
    let dir = keyless.home.path().to_str().unwrap().to_string();
    for (file, reason) in [
        (
            "/missing/metric.query.json".to_string(),
            "ENOENT: no such file or directory, open '/missing/metric.query.json'".to_string(),
        ),
        (empty.to_str().unwrap().to_string(), "Unexpected end of JSON input".to_string()),
        (dir.clone(), "EISDIR: illegal operation on a directory, read".to_string()),
    ] {
        for args in [
            vec!["metrics", "create", "--name", "X", "--definition", file.as_str()],
            vec!["metrics", "update", "m1", "--definition", file.as_str()],
        ] {
            let out = keyless.run(&args);
            assert_eq!(
                (out.status.code(), stderr_line(&out)),
                (Some(1), format!("Error: Failed to read Metric definition {file}: {reason}")),
                "{args:?}"
            );
        }
    }
}

#[tokio::test]
async fn metrics_update_activate_and_delete_like_ts() {
    let server = MockServer::start().await;
    let path_m1 = format!("/api/metrics/{M1}");
    serve(&server, "PATCH", &path_m1, 200, json!({ "metric": metric(json!({ "name": "New" })) })).await;
    serve(&server, "PATCH", "/api/metrics/refused", 400, json!({ "error": "Add a valid calculation first" })).await;
    serve(&server, "DELETE", &path_m1, 200, json!({ "deleted": true, "id": M1, "registryWarning": "pending" })).await;
    Mock::given(wiremock::matchers::method("DELETE"))
        .and(path("/api/metrics/gone"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    let def = sandbox.home.path().join("def.json");
    std::fs::write(&def, r#"{"version":2}"#).unwrap();

    // Nothing to send is refused before any key or request is needed.
    let keyless = Sandbox::without_api_keys(CLOSED);
    for args in [&["metrics", "update", M1][..], &["metrics", "update", M1, "--name", "", "--status", ""]] {
        let out = keyless.run(args);
        assert_eq!((out.status.code(), stderr_line(&out)), (Some(1), "Error: No updates provided".to_string()));
    }
    assert_eq!(
        ok_output(&sandbox.run(&[
            "metrics",
            "update",
            M1,
            "--name",
            "New",
            "--status",
            "active",
            "--unit",
            "u",
            "--format",
            "f",
            "--description",
            "d",
            "--definition",
            def.to_str().unwrap(),
        ])),
        "\n✓ Metric \"New\" updated\n"
    );
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["metrics", "update", M1, "--name", "New", "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": metric(json!({ "name": "New" })) }));
    assert_eq!(ok_output(&sandbox.run(&["metrics", "activate", M1])), "\n✓ Metric \"New\" is now active\n");
    // The server's reason, not `HTTP 400` (VE-3764).
    assert_api_error(&sandbox.run(&["metrics", "activate", "refused"]), "Add a valid calculation first");
    // Not a terminal: `--yes` confirms (VE-3823).
    for args in [&["metrics", "delete", M1, "--yes"][..], &["metrics", "delete", M1, "-y"]] {
        assert_eq!(ok_output(&sandbox.run(args)), "\n✓ Metric deleted\n", "{args:?}");
    }
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["metrics", "delete", M1, "--yes", "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": { "deleted": true, "id": M1, "registryWarning": "pending" } }));
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["metrics", "delete", "gone", "--yes", "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": {} }), "a 204 is an empty body, as in TS");

    let requests = sent(&server).await;
    assert_eq!(
        requests[0],
        format!(
            r#"PATCH {path_m1} {{"name":"New","description":"d","definition":{{"version":2}},"format":"f","unit":"u","status":"active"}}"#
        )
    );
    assert_eq!(requests[1], format!(r#"PATCH {path_m1} {{"name":"New"}}"#));
    assert_eq!(requests[2], format!(r#"PATCH {path_m1} {{"status":"active"}}"#));
    assert_eq!(requests[4], format!("DELETE {path_m1}"));
}

/// A model row as vendo-web-v2's `toModelRecord` (`lib/vendo/models/queries.ts`) builds it, in its key
/// order; the detail route adds `sqlQuery` and `definition`.
fn model(over: Value) -> Value {
    let mut row = json!({
        "id": "11111111-2222-4333-8444-555555555555", "state": "active", "accountId": "acct-alpha",
        "name": "orders_clean", "description": null, "modelType": "sql", "viewName": "orders_clean",
        "outputConfig": { "dataset_id": "prod", "write_mode": "replace" },
        "schedule": { "frequency_unit": "hours", "frequency_value": 6 },
        "columns": [{ "name": "order_id", "type": "STRING", "is_nullable": false }],
        "primaryKeyColumns": null, "incrementalColumn": null, "isValid": true, "validationError": null,
        "lastValidatedAt": null, "createdAt": null, "updatedAt": null, "managedBy": "customer", "sourceId": null,
        "sourceStream": null, "templateKey": null, "templateVersion": null,
    });
    row.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    row
}

#[tokio::test]
async fn models_list_and_get_like_ts() {
    let server = MockServer::start().await;
    let list = json!({
        "data": [model(json!({})), model(json!({ "id": "m2", "name": "broken", "isValid": false, "modelType": "bqml" }))],
        "meta": { "pagination": { "total": 7, "limit": 20, "offset": 0, "hasMore": false } },
    });
    serve(&server, "GET", "/api/v1/accounts/acct-alpha/models", 200, list.clone()).await;
    let detail = model(json!({
        "description": "Clean orders", "sqlQuery": "SELECT order_id\nFROM `p.d.orders`\n  WHERE 1 = 1",
        "primaryKeyColumns": ["order_id", "line_id"], "incrementalColumn": "updated_at",
        "validationError": "Table not found",
    }));
    serve(&server, "GET", "/api/v1/accounts/acct-alpha/models/mod-1", 200, json!({ "data": detail.clone() })).await;
    serve(
        &server,
        "GET",
        "/api/v1/accounts/acct-alpha/models/mod-2",
        200,
        json!({ "data": model(json!({ "isValid": false, "primaryKeyColumns": [], "sqlQuery": "" })) }),
    )
    .await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        cells(ok_output(&sandbox.run(&["models", "list"])).as_bytes()),
        rows(&[
            &["ID", "Name", "Type", "Valid", "Last Validated"],
            &["11111111...", "orders_clean", "sql", "yes", "—"],
            &["m2", "broken", "bqml", "no", "—"],
            &["7 models"],
        ])
    );
    // Verbatim: nested config keys stay snake_case (the TS client camelCased them).
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&["models", "list", "--json"]))).unwrap();
    assert_eq!(printed, list);
    assert_eq!(
        ok_output(&sandbox.run(&["models", "list", "--output", "id"])),
        "11111111-2222-4333-8444-555555555555\nm2\n"
    );
    ok_output(&sandbox.run(&["models", "list", "--valid", "--type", "sql"]));
    ok_output(&sandbox.run(&["models", "list", "--valid", "--invalid", "--limit", "3"]));
    assert_eq!(
        ok_output(&sandbox.run(&["models", "get", "mod-1"])),
        "\norders_clean (sql)\n\n  ID:            11111111-2222-4333-8444-555555555555\n  Type:          sql\n  Valid:         yes\n  Validated:     —\n  Created:       —\n  Description:   Clean orders\n  Primary Keys:  order_id, line_id\n  Incremental:   updated_at\n\n  Validation Error: Table not found\n\n  SQL Query:\n    SELECT order_id\n    FROM `p.d.orders`\n      WHERE 1 = 1\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["models", "get", "mod-2"])),
        "\norders_clean (sql)\n\n  ID:            11111111-2222-4333-8444-555555555555\n  Type:          sql\n  Valid:         no\n  Validated:     —\n  Created:       —\n"
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&["models", "get", "mod-1", "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": detail }));

    let requests = sent(&server).await;
    assert_eq!(requests[0], "GET /api/v1/accounts/acct-alpha/models?limit=20&offset=0");
    assert_eq!(requests[3], "GET /api/v1/accounts/acct-alpha/models?data_type=sql&limit=20&offset=0&is_valid=true");
    assert_eq!(requests[4], "GET /api/v1/accounts/acct-alpha/models?limit=3&offset=0&is_valid=false");
    let received = server.received_requests().await.unwrap();
    assert!(received.iter().all(|r| r.headers.get("x-account-id").is_some_and(|v| v == "acct-alpha")));
}

fn methodologies() -> Value {
    json!({ "data": { "methodologies": [
        { "id": "m-system-1", "account_id": null, "name": "Last click", "description": null, "click_path_model": "last_click",
          "ensemble_weights": { "click_path": 1 }, "signal_params": null, "is_system": true, "version": 1,
          "created_at": null, "updated_at": null },
        { "id": "m-account-2-with-a-long-id", "account_id": "acct-1", "name": "Blended", "description": "Click path and survey",
          "click_path_model": "linear",
          "ensemble_weights": { "click_path": 0.7, "survey_response_share": 0.3333333, "2": 5, "mmm": null },
          "signal_params": null, "is_system": false, "version": 3, "created_at": null, "updated_at": null },
        { "id": "m-3", "account_id": "acct-1", "name": "No weights", "description": "", "click_path_model": "first_click",
          "ensemble_weights": {}, "signal_params": null, "is_system": false, "version": null, "created_at": null,
          "updated_at": null },
    ] } })
}

#[tokio::test]
async fn measurement_methodologies_read_the_enveloped_list() {
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/measurement/methodologies", 200, methodologies()).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        cells(ok_output(&sandbox.run(&["measurement", "methodologies", "list"])).as_bytes()),
        rows(&[
            &["ID", "Name", "Click Path", "Scope", "Version", "Updated"],
            &["m-system-1", "Last click", "last_click", "system", "1", "—"],
            &["m-accoun...", "Blended", "linear", "account", "3", "—"],
            &["m-3", "No weights", "first_click", "account", "null", "—"],
            &["3 methodologys"],
        ])
    );
    let field = |name: &str| ok_output(&sandbox.run(&["measurement", "methodologies", "list", "--output", name]));
    assert_eq!(field("id"), "m-system-1\nm-account-2-with-a-long-id\nm-3\n");
    assert_eq!(field("clickPathModel"), "last_click\nlinear\nfirst_click\n");
    assert_eq!(field("click_path_model"), "last_click\nlinear\nfirst_click\n");
    assert_eq!(field("isSystem"), "true\nfalse\nfalse\n");
    assert_eq!(field("version"), "1\n3\n");
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["measurement", "methodologies", "list", "--json"]))).unwrap();
    assert_eq!(printed, methodologies(), "--json prints the server body unchanged");
    ok_output(&sandbox.run(&["measurement", "methodologies", "list", "--no-system"]));

    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "methodologies", "get", "m-account-2-with-a-long-id"])),
        "\nBlended (linear)\n\n  ID:             m-account-2-with-a-long-id\n  Scope:          account\n  Version:        3\n  Updated:        —\n  Description:   Click path and survey\n\n  Ensemble weights:\n    2             5\n    click_path    0.7\n    survey_response_share  0.333\n    mmm           0\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "methodologies", "get", "m-3"])),
        "\nNo weights (first_click)\n\n  ID:             m-3\n  Scope:          account\n  Version:        null\n  Updated:        —\n"
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&[
        "measurement",
        "methodologies",
        "get",
        "m-system-1",
        "--json",
    ])))
    .unwrap();
    assert_eq!(printed, methodologies()["data"]["methodologies"][0]);
    let out = sandbox.run(&["measurement", "methodologies", "get", "nope"]);
    assert_eq!((out.status.code(), stderr_line(&out)), (Some(1), "Error: Methodology nope not found".to_string()));

    let requests = sent(&server).await;
    assert_eq!(requests[7], "GET /api/measurement/methodologies?include_system=false");
    assert!(requests[8..].iter().all(|r| r == "GET /api/measurement/methodologies"), "get lists with system rows");
}

#[tokio::test]
async fn measurement_rules_preview_like_ts() {
    let server = MockServer::start().await;
    let previews = json!({
        "previews": [
            { "context": { "campaign_objective": "sales", "channel_grouping": null, "custom_label": "" }, "sample_count": 12,
              "resolved_methodology": { "id": "m1", "name": "Last click", "click_path_model": "last_click" },
              "matched_rule_id": null, "via": "default_fallback" },
            { "context": { "campaign_objective": null, "channel_grouping": "Paid Social", "custom_label": "promo" },
              "sample_count": 1234, "resolved_methodology": { "id": "m2", "name": "Blended", "click_path_model": "linear" },
              "matched_rule_id": "r1", "via": "rule" },
        ],
        "total_distinct_contexts": 2,
    });
    serve(&server, "POST", "/api/measurement/methodologies/rules/preview", 200, previews.clone()).await;
    let sandbox = Sandbox::new(&server.uri());
    let preview = |extra: &[&str]| {
        let mut args = vec!["measurement", "rules", "preview", "--from", "2026-09-01", "--to", "2026-09-30"];
        args.extend_from_slice(extra);
        sandbox.run(&args)
    };

    assert_eq!(
        cells(ok_output(&preview(&[])).as_bytes()),
        rows(&[
            &["Objective", "Channel", "Custom Label", "Methodology", "Via", "Sample"],
            &["sales", "—", "Last click", "default", "12"],
            &["—", "Paid Social", "promo", "Blended", "rule", "1234"],
            &["2 distinct contexts"],
        ])
    );
    let printed: Value = serde_json::from_str(&ok_output(&preview(&["--limit", "0x10", "--json"]))).unwrap();
    assert_eq!(printed, previews);
    ok_output(&preview(&["--limit", "1.5"]));
    let requests = sent(&server).await;
    let route = "POST /api/measurement/methodologies/rules/preview";
    assert_eq!(requests[0], format!(r#"{route} {{"from_date":"2026-09-01","to_date":"2026-09-30","limit":50}}"#));
    assert_eq!(requests[1], format!(r#"{route} {{"from_date":"2026-09-01","to_date":"2026-09-30","limit":16}}"#));
    assert_eq!(requests[2], format!(r#"{route} {{"from_date":"2026-09-01","to_date":"2026-09-30","limit":1.5}}"#));

    // Checked before the API key is needed, like the TS action.
    let keyless = Sandbox::without_api_keys(CLOSED);
    for limit in ["0", "201", "abc", "", "-0", "Infinity"] {
        let out = keyless.run(&["measurement", "rules", "preview", "--from", "x", "--to", "y", "--limit", limit]);
        assert_eq!(out.status.code(), Some(1), "{limit:?}");
        assert_eq!(
            text(&out.stderr),
            "Error: --limit must be 1..200\n\nUsage:\n  $ vendo measurement rules preview --from 2025-01-01 --to 2025-01-31 --limit 50\n",
            "{limit:?}"
        );
    }
}

#[tokio::test]
async fn measurement_rules_preview_reports_the_server_refusal() {
    let server = MockServer::start().await;
    let refusal =
        json!({ "error": { "code": "CONFLICT", "message": "No default methodology is configured for this account" } });
    serve(&server, "POST", "/api/measurement/methodologies/rules/preview", 409, refusal).await;
    let sandbox = Sandbox::new(&server.uri());
    let out = sandbox.run(&["measurement", "rules", "preview", "--from", "2026-09-01", "--to", "2026-09-30"]);
    assert_eq!(
        (out.status.code(), stderr_line(&out)),
        (Some(1), "Error: No default methodology is configured for this account".to_string())
    );
}

fn cohort_row(period: &str, size: Value, realised: Value) -> Value {
    json!({ "cohort_period": period, "cohort_granularity": "monthly", "segment_key": "all", "cohort_size": size,
            "realised": realised, "predicted": null })
}

#[tokio::test]
async fn measurement_ltv_list_formats_money_and_ratios_like_node() {
    let server = MockServer::start().await;
    let realised = |l30: Value, l90: Value, l12: Value, cac: Value, ratio: Value| {
        json!({ "ltv_30d": l30, "ltv_90d": l90, "ltv_12m": l12, "ltv_full": null, "cac": cac, "cac_ltv_ratio": ratio,
                "payback_period_days": null, "computed_at": null })
    };
    let body = json!({ "data": {
        "granularity": "monthly", "segment_key": "all",
        "cohorts": [
            cohort_row("2026-08-01", json!(1200), realised(json!(10), json!(1234.5), json!(null), json!(5), json!(0.5))),
            cohort_row("2026-07-01", json!(0), realised(json!(1.005), json!(2.675), json!(-1.005), json!(0), json!(0.125))),
            cohort_row("2026-06-01", json!(1234567.891), realised(json!(-0.001), json!(1e21), json!(0.30000000000000004), json!(-5), json!(2.675))),
            cohort_row("2026-05-01", json!(null), realised(json!(null), json!(null), json!(null), json!(null), json!(-0.001))),
        ],
        "total_returned": 4,
    } });
    serve(&server, "GET", "/api/measurement/ltv", 200, body.clone()).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        cells(ok_output(&sandbox.run(&["measurement", "ltv", "list"])).as_bytes()),
        rows(&[
            &["Cohort", "Segment", "Size", "LTV 30d", "LTV 90d", "LTV 12m", "CAC", "CAC:LTV"],
            &["2026-08-01", "all", "1,200", "$10.00", "$1,234.50", "—", "$5.00", "0.50"],
            &["2026-07-01", "all", "0", "$1.01", "$2.68", "-$1.01", "$0.00", "0.13"],
            &[
                "2026-06-01",
                "all",
                "1,234,567.891",
                "-$0.00",
                "$1,000,000,000,000,000,000,000.00",
                "$0.30",
                "-$5.00",
                "2.67"
            ],
            &["2026-05-01", "all", "—", "—", "—", "—", "—", "-0.00"],
            &["4 cohorts"],
        ])
    );
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["measurement", "ltv", "list", "--json"]))).unwrap();
    assert_eq!(printed, body);
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "ltv", "list", "--output", "cohort_period"])),
        "2026-08-01\n2026-07-01\n2026-06-01\n2026-05-01\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "ltv", "list", "--output", "cohortSize"])),
        "1200\n0\n1234567.891\n"
    );
    ok_output(&sandbox.run(&[
        "measurement",
        "ltv",
        "list",
        "--granularity",
        "weekly",
        "--segment",
        "channel:meta",
        "--from",
        "2026-01-01",
        "--to",
        "2026-06-30",
        "--limit",
        "5",
        "--no-predicted",
    ]));
    let requests = sent(&server).await;
    assert_eq!(requests[0], "GET /api/measurement/ltv?granularity=monthly&segment_key=all&limit=50");
    assert_eq!(
        requests[4],
        "GET /api/measurement/ltv?granularity=weekly&segment_key=channel%3Ameta&from_period=2026-01-01&to_period=2026-06-30&limit=5&include_predicted=false"
    );
}

#[tokio::test]
async fn measurement_ltv_cohort_and_customer_like_ts() {
    let server = MockServer::start().await;
    let cohort = json!({
        "cohort_period": "2026-08-01", "cohort_granularity": "monthly", "segment_key": "channel:meta", "cohort_size": 12345,
        "retention_matrix": [
            { "period_offset_days": 0, "retained_customers": 12345, "retained_revenue": 100.5, "retention_rate": 1 },
            { "period_offset_days": 30, "retained_customers": 100, "retained_revenue": 50, "retention_rate": 0.008 },
        ],
        "cumulative_curve": [
            { "period_offset_days": 0, "cumulative_gross_revenue": 100.5, "cumulative_revenue_after_cogs": 80 },
            { "period_offset_days": 30, "cumulative_gross_revenue": 1234.567, "cumulative_revenue_after_cogs": 999.999 },
        ],
        "prediction": { "method": "naive_decay", "ltv_30d_predicted": 10, "ltv_90d_predicted": null,
                        "ltv_12m_predicted": 30.456, "metadata": null, "computed_at": null },
    });
    serve(&server, "GET", "/api/measurement/ltv/cohort/2026-08-01", 200, cohort.clone()).await;
    let mut empty = cohort.clone();
    empty["cohort_size"] = json!(0);
    empty["retention_matrix"] = json!([]);
    empty["cumulative_curve"] = json!([]);
    empty["prediction"] = json!(null);
    serve(&server, "GET", "/api/measurement/ltv/cohort/2026-07-01", 200, empty).await;
    let mut no_offsets = cohort.clone();
    no_offsets["cumulative_curve"] =
        json!([{ "cumulative_gross_revenue": null, "cumulative_revenue_after_cogs": null }]);
    serve(&server, "GET", "/api/measurement/ltv/cohort/2026-06-01", 200, no_offsets).await;
    let customer = json!({
        "cohort": { "customer_id": "cust 1/2", "acquisition_date": "2026-01-05", "cohort_period_daily": "2026-01-05",
                    "cohort_period_weekly": "2026-01-05", "cohort_period_monthly": "2026-01-01",
                    "acquisition_channel": "Paid Social", "acquisition_campaign": null, "country": "AU", "is_reactivated": true },
        "revenue": [{ "revenue_period_daily": "2026-01-05", "period_offset_days": 0, "gross_revenue": 10 }],
        "realised": { "ltv_30d": 10, "ltv_90d": 10.5, "ltv_12m": null, "ltv_full": 1234.5678 },
    });
    serve(&server, "GET", "/api/measurement/ltv/customer/cust%201%2F2", 200, customer.clone()).await;
    serve(&server, "GET", "/api/measurement/ltv/customer/cust_%C3%A9%3F%23", 200, customer.clone()).await;
    let none = json!({ "cohort": null, "revenue": [],
                       "realised": { "ltv_30d": null, "ltv_90d": null, "ltv_12m": null, "ltv_full": null } });
    serve(&server, "GET", "/api/measurement/ltv/customer/parity-missing-customer", 200, none).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "ltv", "cohort", "2026-08-01"])),
        "\nCohort 2026-08-01 (monthly, segment=channel:meta)\n\n  Size:           12,345\n  Retention pts:  2\n  Curve points:   2\n  Cum revenue:    $1,234.57 (t+30d)\n  After COGS:     $1,000.00\n\n  Prediction:\n    Method:       naive_decay\n    LTV 30d:      $10.00\n    LTV 90d:      —\n    LTV 12m:      $30.46\n    Computed:     —\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&[
            "measurement",
            "ltv",
            "cohort",
            "2026-07-01",
            "--granularity",
            "weekly",
            "--segment",
            "channel:meta & co"
        ])),
        "\nCohort 2026-08-01 (monthly, segment=channel:meta)\n\n  Size:           0\n  Retention pts:  0\n  Curve points:   0\n  Prediction:     (none)\n"
    );
    assert!(
        ok_output(&sandbox.run(&["measurement", "ltv", "cohort", "2026-06-01"]))
            .contains("  Curve points:   1\n  Cum revenue:    $0.00 (t+0d)\n  After COGS:     $0.00\n")
    );
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["measurement", "ltv", "cohort", "2026-08-01", "--json"])))
            .unwrap();
    assert_eq!(printed, cohort);
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "ltv", "customer", "cust 1/2"])),
        "\nCustomer cust 1/2\n\n  Acquired:        2026-01-05\n  Channel:         Paid Social\n  Campaign:        —\n  Country:         AU\n  Reactivated:     yes\n  Monthly cohort:  2026-01-01\n\n  Realised LTV:\n    30d:           $10.00\n    90d:           $10.50\n    12m:           —\n    full:          $1,234.57\n    after-COGS variants in --json\n\n  Revenue points:  1\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "ltv", "customer", "parity-missing-customer"])),
        "\nCustomer parity-missing-customer\n  (no cohort row — customer not in customer_cohorts)\n\n  Realised LTV:\n    30d:           —\n    90d:           —\n    12m:           —\n    full:          —\n    after-COGS variants in --json\n\n  Revenue points:  0\n"
    );
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["measurement", "ltv", "customer", "cust_é?#", "--json"])))
            .unwrap();
    assert_eq!(printed, customer);

    let requests = sent(&server).await;
    assert_eq!(requests[0], "GET /api/measurement/ltv/cohort/2026-08-01?granularity=monthly&segment_key=all");
    assert_eq!(
        requests[1],
        "GET /api/measurement/ltv/cohort/2026-07-01?granularity=weekly&segment_key=channel%3Ameta+%26+co"
    );

    let keyless = Sandbox::without_api_keys(CLOSED);
    for period in ["2026-8-01", "2026-08-01x", "", "２０２６-08-01"] {
        let out = keyless.run(&["measurement", "ltv", "cohort", period]);
        assert_eq!(out.status.code(), Some(1), "{period:?}");
        assert_eq!(
            text(&out.stderr),
            "Error: cohort period must be YYYY-MM-DD\n\nUsage:\n  $ vendo measurement ltv cohort 2025-01-01\n"
        );
    }
    let out = keyless.run(&["measurement", "ltv", "customer", ""]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "Error: customerId is required\n\nUsage:\n  $ vendo measurement ltv customer cust_abc123\n"
    );
}

#[tokio::test]
async fn measurement_signals_like_ts() {
    let server = MockServer::start().await;
    let signals = json!({ "data": { "signals": [
        { "id": "click_path", "state": "live", "availability": { "available": true } },
        { "id": "mmm", "state": "stub", "availability": { "available": false, "reason": "Not enough spend history" } },
        { "id": "geo_lift", "state": "stub", "availability": null },
        { "id": "survey", "state": "live", "availability": { "available": 0, "reason": "" } },
    ] } });
    serve(&server, "GET", "/api/measurement/signals", 200, signals.clone()).await;
    let click_path = json!({ "status": {
        "enabled": true, "lastComputedAt": null, "sampleEstimates": [{ "tier_label": "a" }, { "tier_label": "b" }],
        "readiness": { "available": false, "readiness": [
            { "key": "clicks", "label": "Has click data", "ok": true, "detail": "ignored when ok" },
            { "key": "conv", "label": "Has conversions", "ok": false, "detail": "No conversions in 30 days" },
            { "key": "x", "label": "No detail", "ok": false },
        ] },
    } });
    Mock::given(path("/api/measurement/signals/click-path"))
        .and(wiremock::matchers::query_param("sampleLimit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": {
            "enabled": false, "lastComputedAt": null, "sampleEstimates": [],
            "readiness": { "available": false, "reason": "Signal disabled for this account" },
        } })))
        .mount(&server)
        .await;
    serve(&server, "GET", "/api/measurement/signals/click-path", 200, click_path.clone()).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        cells(ok_output(&sandbox.run(&["measurement", "signals", "list"])).as_bytes()),
        rows(&[
            &["Signal", "State", "Available", "Reason / Notes"],
            &["click_path", "live", "yes", "—"],
            &["mmm", "stub", "no", "Not enough spend history"],
            &["geo_lift", "stub", "—", "—"],
            &["survey", "live", "no"],
            &["4 signals"],
        ])
    );
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["measurement", "signals", "list", "--json"]))).unwrap();
    assert_eq!(printed, signals);
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "signals", "click-path"])),
        "\nClick-path signal\n\n  Enabled:        yes\n  Last computed:  —\n  Sample rows:    2\n\n  Readiness:\n    ✓ Has click data\n    ✗ Has conversions\n      No conversions in 30 days\n    ✗ No detail\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["measurement", "signals", "click-path", "--sample-limit", "50"])),
        "\nClick-path signal\n\n  Enabled:        no\n  Last computed:  —\n  Sample rows:    0\n  Note:           Signal disabled for this account\n"
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&[
        "measurement",
        "signals",
        "click-path",
        "--json",
        "--sample-limit",
        " 0x10 ",
    ])))
    .unwrap();
    assert_eq!(printed, click_path);
    ok_output(&sandbox.run(&["measurement", "signals", "click-path", "--sample-limit", "2.5"]));
    let requests = sent(&server).await;
    assert_eq!(requests[2], "GET /api/measurement/signals/click-path");
    assert_eq!(requests[4], "GET /api/measurement/signals/click-path?sampleLimit=16");
    assert_eq!(requests[5], "GET /api/measurement/signals/click-path?sampleLimit=2.5");

    let keyless = Sandbox::without_api_keys(CLOSED);
    for limit in ["0", "501", "abc", ""] {
        let out = keyless.run(&["measurement", "signals", "click-path", "--sample-limit", limit]);
        assert_eq!(out.status.code(), Some(1), "{limit:?}");
        assert_eq!(
            text(&out.stderr),
            "Error: --sample-limit must be 1..500\n\nUsage:\n  $ vendo measurement signals click-path --sample-limit 100\n"
        );
    }
}

#[test]
fn data_commands_need_an_api_key_like_ts() {
    let keyless = Sandbox::without_api_keys(CLOSED);
    for args in [
        &["metrics", "list"][..],
        &["metrics", "delete", "m1", "--yes"],
        &["models", "list"],
        &["measurement", "signals", "list"],
        &["measurement", "ltv", "cohort", "2026-01-01"],
    ] {
        let out = keyless.run(args);
        assert_eq!(
            (out.status.code(), stderr_line(&out)),
            (
                Some(1),
                "Error: No API key configured. Run `vendo login` or `vendo profile set --api-key <key>` or set VENDO_API_KEY."
                    .to_string()
            ),
            "{args:?}"
        );
    }
}

// ── VE-3713: dictionary ──────────────────────────────────────────────────────
// Fixtures follow src/__tests__/dictionary-contract.test.ts; expected output
// comes from the TypeScript CLI run against the same stub responses.

const DICTIONARY: &str = "/api/v1/accounts/acct-alpha/dictionary";
const COLUMN_ID: &str = "source:11111111-2222-4333-8444-555555555555/table:customers/col:email";

fn event_item() -> Value {
    json!({
        "subjectId": "0123456789abcdef0123456789abcdef", "subjectType": "event", "displayName": "Checkout Completed",
        "description": "A customer placed an order.", "dataType": null, "semanticType": null, "tags": [],
        "origin": "lexicon", "lastSeenAt": null, "status": "active",
    })
}

fn column_item() -> Value {
    let seen = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(72);
    json!({
        "subjectId": COLUMN_ID, "subjectType": "column", "displayName": "Customer email",
        "description": "Lowercased email of the latest customer record", "dataType": "string", "semanticType": "email",
        "tags": ["pii", "crm"], "origin": "bq_schema", "lastSeenAt": seen.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        "status": "deprecated",
    })
}

fn page(items: Vec<Value>, total: u64) -> Value {
    json!({ "data": items, "meta": { "pagination": { "total": total, "limit": 20, "offset": 0, "hasMore": false } } })
}

#[tokio::test]
async fn dictionary_list_and_search_read_one_subject_type() {
    let server = MockServer::start().await;
    let odd = json!({
        "subjectId": "fedcba9876543210fedcba9876543210", "subjectType": "event", "displayName": null, "description": "",
        "dataType": "", "semanticType": null, "tags": null, "origin": "", "lastSeenAt": null, "status": "warning",
    });
    let body = page(vec![event_item(), odd], 6);
    serve(&server, "GET", DICTIONARY, 200, body.clone()).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        cells(ok_output(&sandbox.run(&["dictionary", "list"])).as_bytes()),
        rows(&[
            &["Subject ID", "Display", "Description"],
            &["0123456789abcdef0123456789abcdef", "Checkout Completed", "A customer placed an order."],
            &["fedcba9876543210fedcba9876543210", "—", "—"],
            &["6 events"],
        ])
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&["dictionary", "list", "--json"]))).unwrap();
    assert_eq!(printed, body, "--json prints the server body unchanged");
    // `--output` reads the camelCase field names; null is skipped, "" prints an empty line.
    assert_eq!(ok_output(&sandbox.run(&["dictionary", "list", "--output", "displayName"])), "Checkout Completed\n");
    assert_eq!(
        ok_output(&sandbox.run(&["dictionary", "list", "--output", "description"])),
        "A customer placed an order.\n\n"
    );
    assert_eq!(
        cells(
            ok_output(&sandbox.run(&[
                "dictionary",
                "list",
                "--type",
                "prop",
                "-q",
                "email",
                "--limit",
                "5",
                "--offset",
                "10"
            ]))
            .as_bytes()
        )
        .last()
        .cloned(),
        Some(vec!["6 props".to_string()]),
        "the count names the --type"
    );
    ok_output(&sandbox.run(&["dictionary", "search", "email", "--type", "column", "--limit", "5"]));
    ok_output(&sandbox.run(&["dictionary", "search", "a b&c=d/é", "--output", "subjectId"]));
    ok_output(&sandbox.run(&["dictionary", "list", "--type", "column", "--query", ""]));

    let requests = sent(&server).await;
    assert_eq!(requests[0], format!("GET {DICTIONARY}?type=event&limit=20&offset=0"));
    assert_eq!(requests[4], format!("GET {DICTIONARY}?type=prop&q=email&limit=5&offset=10"));
    assert_eq!(requests[5], format!("GET {DICTIONARY}?type=column&q=email&limit=5&offset=0"));
    assert_eq!(requests[6], format!("GET {DICTIONARY}?type=event&q=a+b%26c%3Dd%2F%C3%A9&limit=20&offset=0"));
    assert_eq!(requests[7], format!("GET {DICTIONARY}?type=column&q=&limit=20&offset=0"));
    let received = server.received_requests().await.unwrap();
    assert!(received.iter().all(|r| r.headers.get("x-account-id").is_some_and(|v| v == "acct-alpha")));
}

#[tokio::test]
async fn dictionary_list_counts_the_rows_when_the_server_sends_no_total() {
    let server = MockServer::start().await;
    serve(&server, "GET", DICTIONARY, 200, json!({ "data": [event_item(), column_item()] })).await;
    let sandbox = Sandbox::new(&server.uri());
    let out = cells(ok_output(&sandbox.run(&["dictionary", "list", "--type", "audience"])).as_bytes());
    assert_eq!(out.last().cloned(), Some(vec!["2 audiences".to_string()]));
    assert_eq!(out[2], vec![COLUMN_ID, "Customer email", "Lowercased email of the latest customer record"]);
}

#[tokio::test]
async fn dictionary_list_prints_the_total_as_javascript_does() {
    // `printCount(res.meta?.pagination?.total ?? res.data.length, type)`: the total as a template
    // literal prints it, singular only for the number 1 (what the TS CLI printed for each).
    for (total, footer) in [(json!("1"), "1 events"), (json!(1), "1 event"), (json!(2.5), "2.5 events")] {
        let server = MockServer::start().await;
        let body = json!({ "data": [event_item()], "meta": { "pagination": { "total": total } } });
        serve(&server, "GET", DICTIONARY, 200, body).await;
        let sandbox = Sandbox::new(&server.uri());
        let out = cells(ok_output(&sandbox.run(&["dictionary", "list"])).as_bytes());
        assert_eq!(out.last().cloned(), Some(vec![footer.to_string()]), "total {total}");
    }
}

#[tokio::test]
async fn dictionary_get_prints_every_field_of_the_definition() {
    let server = MockServer::start().await;
    let column = column_item();
    let lookup = format!("{DICTIONARY}/lookup");
    Mock::given(path(lookup.clone()))
        .and(wiremock::matchers::query_param("subject_id", COLUMN_ID))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { "subjectId": COLUMN_ID, "found": true, "definition": column } })),
        )
        .mount(&server)
        .await;
    Mock::given(path(lookup.clone()))
        .and(wiremock::matchers::query_param("subject_id", "0123456789abcdef0123456789abcdef"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "subjectId": "0123456789abcdef0123456789abcdef", "found": true, "definition": event_item(),
        } })))
        .mount(&server)
        .await;
    Mock::given(path(lookup.clone()))
        .and(wiremock::matchers::query_param("subject_id", "fedcba9876543210fedcba9876543210"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "subjectId": "fedcba9876543210fedcba9876543210", "found": true, "definition": {
                "subjectId": "fedcba9876543210fedcba9876543210", "subjectType": null, "displayName": "",
                "description": null, "dataType": 7, "semanticType": ["x"], "tags": ["a", null, 3], "origin": null,
                "lastSeenAt": "garbage", "status": null,
            },
        } })))
        .mount(&server)
        .await;
    Mock::given(path(lookup.clone()))
        .and(wiremock::matchers::query_param("subject_id", "event:checkout"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": {
            "code": "COMMAND_REFUSED",
            "message": "event:checkout matches 2 events: 0123456789abcdef0123456789abcdef, fedcba9876543210fedcba9876543210. Use one of these subject IDs.",
        } })))
        .mount(&server)
        .await;
    serve(&server, "GET", &lookup, 200, json!({ "data": { "subjectId": "event:nope", "found": false } })).await;
    let sandbox = Sandbox::new(&server.uri());

    assert_eq!(
        ok_output(&sandbox.run(&["dictionary", "get", COLUMN_ID])),
        format!(
            "\nCustomer email (column)\n\n  Subject:      {COLUMN_ID}\n  Type:         column\n  Display:      Customer email\n  Data type:    string\n  Semantic:     email\n  Origin:       bq_schema\n  Status:       deprecated\n  Last seen:    3d ago\n  Tags:         pii, crm\n\n  Lowercased email of the latest customer record\n"
        )
    );
    assert_eq!(
        ok_output(&sandbox.run(&["dictionary", "get", "0123456789abcdef0123456789abcdef"])),
        "\nCheckout Completed (event)\n\n  Subject:      0123456789abcdef0123456789abcdef\n  Type:         event\n  Display:      Checkout Completed\n  Data type:    —\n  Semantic:     —\n  Origin:       lexicon\n  Status:       active\n  Last seen:    —\n  Tags:         —\n\n  A customer placed an order.\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["dictionary", "get", "fedcba9876543210fedcba9876543210"])),
        "\nfedcba9876543210fedcba9876543210 (null)\n\n  Subject:      fedcba9876543210fedcba9876543210\n  Type:         null\n  Display:      —\n  Data type:    —\n  Semantic:     x\n  Origin:       —\n  Status:       null\n  Last seen:    Invalid Date\n  Tags:         a, , 3\n"
    );
    let printed: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["dictionary", "get", COLUMN_ID, "--json"]))).unwrap();
    assert_eq!(printed, json!({ "data": { "subjectId": COLUMN_ID, "found": true, "definition": column } }));
    // The message names the argument as typed, not the server's echo.
    assert_eq!(
        ok_output(&sandbox.run(&["dictionary", "get", " event:nope "])),
        "\nNo dictionary entry for  event:nope \n"
    );
    let out = sandbox.run(&["dictionary", "get", "event:checkout"]);
    assert_eq!(
        (out.status.code(), stderr_line(&out)),
        (
            Some(1),
            "Error: event:checkout matches 2 events: 0123456789abcdef0123456789abcdef, fedcba9876543210fedcba9876543210. Use one of these subject IDs.".to_string()
        )
    );

    let requests = sent(&server).await;
    assert_eq!(
        requests[0],
        format!(
            "GET {lookup}?subject_id=source%3A11111111-2222-4333-8444-555555555555%2Ftable%3Acustomers%2Fcol%3Aemail"
        )
    );
    assert_eq!(requests[4], format!("GET {lookup}?subject_id=+event%3Anope+"));
}

#[tokio::test]
async fn dictionary_get_reports_an_entry_the_server_does_not_have() {
    let server = MockServer::start().await;
    let lookup = format!("{DICTIONARY}/lookup");
    for (id, data) in [
        ("event:half", json!({ "subjectId": "event:half", "found": true })),
        ("event:other", json!({ "subjectId": "event:other", "found": false, "definition": event_item() })),
    ] {
        Mock::given(path(lookup.clone()))
            .and(wiremock::matchers::query_param("subject_id", id))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data })))
            .mount(&server)
            .await;
    }
    let sandbox = Sandbox::new(&server.uri());
    for id in ["event:half", "event:other"] {
        assert_eq!(ok_output(&sandbox.run(&["dictionary", "get", id])), format!("\nNo dictionary entry for {id}\n"));
    }
}

#[tokio::test]
async fn dates_and_row_counts_follow_lang_like_node() {
    let server = MockServer::start().await;
    let job = json!({
        "id": "j-1", "status": "completed", "jobType": "import", "connectorType": "stripe",
        "startedAt": "2026-01-05T09:07:03Z", "finishedAt": "2026-01-05T10:07:03Z",
        "rowsProcessed": 1234567.891, "rowsWritten": 1234567,
    });
    Mock::given(path("/api/v1/accounts/acct-alpha/jobs/j-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": job })))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    // What `node dist/cli.js jobs get j-1` prints under each LANG with TZ=Australia/Sydney.
    for (lang, started, read, written) in [
        ("C", "1/5/2026", "1,234,567.891", "1,234,567"),
        ("en_AU.UTF-8", "05/01/2026", "1,234,567.891", "1,234,567"),
        ("de_DE.UTF-8", "5.1.2026", "1.234.567,891", "1.234.567"),
        ("ja_JP.UTF-8", "2026/1/5", "1,234,567.891", "1,234,567"),
    ] {
        let out =
            sandbox.command(&["jobs", "get", "j-1"]).env("LANG", lang).env("TZ", "Australia/Sydney").output().unwrap();
        let stdout = text(&out.stdout);
        assert!(stdout.contains(&format!("  Started:       {started}\n")), "{lang}: {stdout}");
        assert!(stdout.contains(&format!("  Rows Read:     {read}\n")), "{lang}: {stdout}");
        assert!(stdout.contains(&format!("  Rows Written:  {written}\n")), "{lang}: {stdout}");
    }
    // LC_ALL wins over LANG, as in Node.
    let out = sandbox.command(&["jobs", "get", "j-1"]).env("LANG", "de_DE.UTF-8").env("LC_ALL", "C").output().unwrap();
    assert!(text(&out.stdout).contains("  Rows Read:     1,234,567.891\n"));
}

#[tokio::test]
async fn json_output_matches_the_ts_cli_byte_for_byte() {
    // A body written the way the Node API writes it (`JSON.stringify`), with numbers serde_json
    // used to reformat: 0.000001 (was 1e-6) and 1e20 (was 1e+20).
    let body = r#"{"data":{"id":"j-1","a":0.000001,"b":1e+21,"c":1.5,"d":0,"e":9007199254740992,"f":12345678901234567000,"g":[100000000000000000000,2.5e-7]}}"#;
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/accounts/acct-alpha/jobs/j-1"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&server)
        .await;
    let out = Sandbox::new(&server.uri()).run(&["jobs", "get", "j-1", "--json"]);
    // `node dist/cli.js jobs get j-1 --json` against the same stub.
    let ts = "{\n  \"data\": {\n    \"id\": \"j-1\",\n    \"a\": 0.000001,\n    \"b\": 1e+21,\n    \"c\": 1.5,\n    \"d\": 0,\n    \"e\": 9007199254740992,\n    \"f\": 12345678901234567000,\n    \"g\": [\n      100000000000000000000,\n      2.5e-7\n    ]\n  }\n}\n";
    assert_eq!(text(&out.stdout), ts);
}

// ── VE-3823: delete, cancel and reset need --yes when no one is at a terminal ─
// Decided by Yalcin, 2026-10-05. The TS CLI answered its own "are you sure?"
// with yes when stdout was not a terminal, and `--json` skipped the question.

const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

/// A stub that answers every request, so a test sees what a command sent.
async fn stub_accepting_everything() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "id": ID } })))
        .mount(&server)
        .await;
    server
}

/// Each confirming command that calls the API: its arguments, what the
/// refusal says it would do, and the request a confirmed run sends.
fn confirming_commands() -> Vec<(Vec<&'static str>, String, String)> {
    let v1 = "/api/v1/accounts/acct-alpha";
    vec![
        (vec!["apps", "delete", ID], format!("This deletes app {ID}."), format!("DELETE {v1}/apps/{ID}")),
        (vec!["sources", "delete", ID], format!("This deletes source {ID}."), format!("DELETE {v1}/sources/{ID}")),
        (
            vec!["destinations", "delete", ID],
            format!("This deletes destination {ID}."),
            format!("DELETE {v1}/connections/{ID}"),
        ),
        (vec!["int", "delete", ID], format!("This deletes destination {ID}."), format!("DELETE {v1}/connections/{ID}")),
        (vec!["jobs", "cancel", ID], format!("This cancels job {ID}."), format!("POST {v1}/jobs/{ID}/cancel")),
        (
            vec!["metrics", "delete", ID],
            format!("This deletes metric {ID} and cannot be undone."),
            format!("DELETE /api/metrics/{ID}"),
        ),
    ]
}

/// Exit 1, nothing on stdout, and `Error: <what> Re-run with --yes to confirm.`
fn assert_needs_yes(out: &Output, what: &str) {
    assert_eq!(
        (out.status.code(), text(&out.stdout), text(&out.stderr)),
        (Some(1), String::new(), format!("Error: {what} Re-run with --yes to confirm.\n"))
    );
}

#[tokio::test]
async fn without_a_terminal_deletes_and_cancels_need_yes() {
    for (args, what, request) in confirming_commands() {
        let server = stub_accepting_everything().await;
        let sandbox = Sandbox::new(&server.uri());
        // Refused before any request, also with --json (which used to imply --yes), where the
        // refusal is the JSON error (VE-3831).
        assert_needs_yes(&sandbox.run(&args), &what);
        assert_needs_yes_json(&sandbox.run(&[&args[..], &["--json"]].concat()), &what);
        assert_eq!(sent(&server).await, Vec::<String>::new(), "{args:?} sent a request without --yes");

        for yes in [&["--yes"][..], &["-y"], &["--yes", "--json"]] {
            let out = sandbox.run(&[&args[..], yes].concat());
            assert_eq!(out.status.code(), Some(0), "{args:?} {yes:?}: {}", text(&out.stderr));
        }
        assert_eq!(sent(&server).await, vec![request; 3], "{args:?}");
    }
}

/// [`assert_needs_yes`] with `--json`: the refusal as the one-line JSON error (VE-3831).
fn assert_needs_yes_json(out: &Output, what: &str) {
    let error = json!({ "error": {
        "message": format!("{what} Re-run with --yes to confirm."), "code": null, "status": null, "requestId": null,
    } });
    assert_eq!(
        (out.status.code(), text(&out.stdout), text(&out.stderr)),
        (Some(1), String::new(), format!("{}\n", serde_json::to_string(&error).unwrap()))
    );
}

#[tokio::test]
async fn a_dry_run_needs_no_yes() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, printed) in [
        (["apps", "delete", ID, "--dry-run"], "[dry-run] Would delete app 550e8400...\n"),
        (["sources", "delete", ID, "--dry-run"], "[dry-run] Would delete source 550e8400...\n"),
        (["destinations", "delete", ID, "--dry-run"], "[dry-run] Would delete destination 550e8400...\n"),
        (["jobs", "cancel", ID, "--dry-run"], "[dry-run] Would cancel job 550e8400...\n"),
    ] {
        assert_eq!(ok_output(&sandbox.run(&args)), printed);
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[test]
fn without_a_terminal_config_reset_needs_yes() {
    // `config reset` runs `logout --all` since CLI 1.1 (VE-3827): its question, refusal and message.
    let sandbox = Sandbox::new(CLOSED);
    let file = sandbox.home.path().join(".config/vendo/config.json");
    assert_needs_yes(&sandbox.run(&["config", "reset"]), "This removes every saved profile and its API key.");
    assert!(file.exists(), "the refusal kept the configuration");
    assert_eq!(ok_output(&sandbox.run(&["config", "reset", "--yes"])), "Done: Logged out. All profiles removed.\n");
    assert!(!file.exists());
}

#[test]
fn without_a_terminal_logout_all_needs_yes() {
    let sandbox = Sandbox::new(CLOSED);
    let file = sandbox.home.path().join(".config/vendo/config.json");
    assert_needs_yes(&sandbox.run(&["logout", "--all"]), "This removes every saved profile and its API key.");
    assert!(file.exists(), "the refusal kept every profile");
    assert_eq!(ok_output(&sandbox.run(&["logout", "--all", "--yes"])), "Done: Logged out. All profiles removed.\n");
    assert!(!file.exists());
}

#[test]
fn logging_out_of_one_profile_still_needs_no_yes() {
    let sandbox = Sandbox::new(CLOSED);
    assert_eq!(ok_output(&sandbox.run(&["logout"])), "Done: Logged out of profile \"alpha\".\n");
    assert!(sandbox.config()["profiles"]["beta"]["apiKey"].is_string(), "the other profile is kept");
}

/// A pseudo-terminal: the controller the test reads and types into, and the
/// terminal end that `vendo` gets as stdin, stdout or stderr.
#[cfg(unix)]
fn pseudo_terminal() -> (std::fs::File, std::os::fd::OwnedFd) {
    // Wide enough that no menu row wraps (VE-3826): a new pseudo-terminal has no size.
    let (controller, terminal, _) = pseudo_terminal_sized(40, 120);
    (controller, terminal)
}

/// [`pseudo_terminal`] of `lines` × `columns`, and the terminal end's name (`/dev/ttys004`). The name
/// comes from ptsname, as the terminal end is opened by it: macOS's ttyname_r fails with ERANGE when
/// several threads call it at once, as the tests run.
#[cfg(unix)]
fn pseudo_terminal_sized(lines: u16, columns: u16) -> (std::fs::File, std::os::fd::OwnedFd, std::ffi::CString) {
    use std::os::fd::{FromRawFd, OwnedFd};
    // SAFETY: posix_openpt, grantpt, unlockpt and ptsname on a descriptor this
    // function owns; the name is copied before anything else can call ptsname.
    unsafe {
        let controller = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(controller >= 0, "posix_openpt failed");
        // Not inherited by the other tests' children (Command's own descriptors are close-on-exec too).
        libc::fcntl(controller, libc::F_SETFD, libc::FD_CLOEXEC);
        assert_eq!((libc::grantpt(controller), libc::unlockpt(controller)), (0, 0));
        let name = std::ffi::CStr::from_ptr(libc::ptsname(controller)).to_owned();
        let terminal = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        assert!(terminal >= 0, "open {name:?} failed");
        let size = libc::winsize { ws_row: lines, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(libc::ioctl(controller, libc::TIOCSWINSZ as _, &size), 0, "TIOCSWINSZ failed");
        (std::fs::File::from_raw_fd(controller), OwnedFd::from_raw_fd(terminal), name)
    }
}

/// `vendo` with stdin, stdout and stderr on a pseudo-terminal, like a person at
/// a terminal: the test waits for what the screen shows and types keys.
#[cfg(unix)]
struct OnTerminal {
    controller: std::fs::File,
    /// The terminal end's name (`/dev/ttys004`), for [`OnTerminal::takes_typed_end_of_input`].
    name: std::ffi::CString,
    /// The test's copy of the terminal end, kept until `vendo` exits ([`OnTerminal::finish`]): macOS
    /// drops what the terminal still holds for the reader when the last copy closes, so the output of
    /// a `vendo` that wrote and exited before the reader ran went missing.
    terminal: Option<std::os::fd::OwnedFd>,
    child: std::process::Child,
    /// Set once [`OnTerminal::exit_within`] or [`OnTerminal::stop`] has reaped `vendo` itself, so
    /// nothing kills or waits for its process ID again.
    reaped: bool,
    output: std::sync::mpsc::Receiver<Vec<u8>>,
    /// The thread that reads the controller into `output`; it holds a copy of the controller.
    reader: Option<std::thread::JoinHandle<()>>,
    /// Everything the terminal showed so far, as `vendo` wrote it.
    screen: String,
    /// The end of what [`OnTerminal::wait_for`] last found.
    seen: usize,
}

/// How `vendo` ended after [`OnTerminal::hang_up`]: its exit code (`None` when a signal stopped
/// it) and the processor time it used in all, user and system.
#[cfg(unix)]
#[derive(Debug)]
struct Ended {
    code: Option<i32>,
    cpu: std::time::Duration,
}

#[cfg(unix)]
impl OnTerminal {
    fn start(sandbox: &Sandbox, args: &[&str]) -> Self {
        Self::start_with(sandbox, args, (40, 120), "", None)
    }

    /// [`OnTerminal::start`] on a terminal of `lines` × `columns` that already shows `before`, as
    /// a shell's earlier output; `stderr` elsewhere than the terminal when given.
    fn start_with(sandbox: &Sandbox, args: &[&str], size: (u16, u16), before: &str, stderr: Option<Stdio>) -> Self {
        Self::spawn(sandbox.command(args), size, before, None, stderr)
    }

    /// [`OnTerminal::start`] with the environment variables `env` set, as `CI=true vendo …` runs.
    fn start_env(sandbox: &Sandbox, args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut cmd = sandbox.command(args);
        cmd.envs(env.iter().copied());
        Self::spawn(cmd, (40, 120), "", None, None)
    }

    /// [`OnTerminal::start`] on a terminal that is not `vendo`'s controlling terminal: `vendo` runs
    /// in a session of its own that has none, as when a program opens a pseudo-terminal for it
    /// (opened with `O_NOCTTY`, as [`pseudo_terminal_sized`] does). When that terminal hangs up, no
    /// SIGHUP ends `vendo` ([`OnTerminal::hang_up`]).
    fn start_detached(sandbox: &Sandbox, args: &[&str]) -> Self {
        Self::launch(sandbox.command(args), (40, 120), "", None, None, false)
    }

    /// `cmd` on a terminal of `lines` × `columns` that already shows `before`; stdin and stderr
    /// elsewhere than the terminal when given.
    fn spawn(cmd: Command, size: (u16, u16), before: &str, stdin: Option<Stdio>, stderr: Option<Stdio>) -> Self {
        Self::launch(cmd, size, before, stdin, stderr, true)
    }

    /// [`OnTerminal::spawn`], the terminal `vendo`'s controlling terminal when `controlling`.
    fn launch(
        cmd: Command,
        (lines, columns): (u16, u16),
        before: &str,
        stdin: Option<Stdio>,
        stderr: Option<Stdio>,
        controlling: bool,
    ) -> Self {
        Self::launch_on(pseudo_terminal_sized(lines, columns), cmd, before, stdin, stderr, controlling)
    }

    /// [`OnTerminal::launch`] on a pseudo-terminal the test made ([`pseudo_terminal_sized`]), so that
    /// it can open the terminal end by name first.
    fn launch_on(
        (controller, terminal, name): (std::fs::File, std::os::fd::OwnedFd, std::ffi::CString),
        mut cmd: Command,
        before: &str,
        stdin: Option<Stdio>,
        stderr: Option<Stdio>,
        controlling: bool,
    ) -> Self {
        use std::{
            io::{Read, Write},
            os::unix::process::CommandExt,
        };
        std::fs::File::from(terminal.try_clone().unwrap()).write_all(before.as_bytes()).unwrap();
        let kept = terminal.try_clone().unwrap();
        cmd.env("NO_COLOR", "1")
            .stdin(stdin.unwrap_or_else(|| terminal.try_clone().unwrap().into()))
            .stdout(terminal.try_clone().unwrap())
            .stderr(stderr.unwrap_or_else(|| terminal.into()));
        // The terminal is `vendo`'s own, as in a terminal window: its session's controlling
        // terminal, which `/dev/tty` opens, not the one running the tests (or none, in CI). Not
        // `controlling`: a session of its own all the same, with no controlling terminal.
        // SAFETY: setsid and ioctl only, between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() < 0 || (controlling && libc::ioctl(libc::STDOUT_FILENO, libc::TIOCSCTTY as _, 0) < 0)
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        // Close the test's other copies of the terminal end: once `vendo` has exited,
        // [`OnTerminal::finish`] closes the last one, and reading ends.
        drop(cmd);
        let (tx, output) = std::sync::mpsc::channel();
        let mut reader = controller.try_clone().unwrap();
        let reader = std::thread::spawn(move || {
            // Whole characters only: a read can end inside one (the hint's arrows).
            let (mut buf, mut read) = ([0u8; 4096], Vec::new());
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                read.extend_from_slice(&buf[..n]);
                let whole = match std::str::from_utf8(&read) {
                    Err(err) if err.error_len().is_none() => err.valid_up_to(),
                    _ => read.len(),
                };
                if tx.send(read.drain(..whole).collect()).is_err() {
                    break;
                }
            }
        });
        OnTerminal {
            controller,
            name,
            terminal: Some(kept),
            child,
            reaped: false,
            output,
            reader: Some(reader),
            screen: String::new(),
            seen: 0,
        }
    }

    /// Waits until the screen shows `expected` after what the last wait found, and returns what
    /// it showed from there up to and including `expected`. Fails after 20 seconds without it.
    fn wait_for(&mut self, expected: &str) -> String {
        loop {
            if let Some(at) = self.screen[self.seen..].find(expected) {
                let end = self.seen + at + expected.len();
                let shown = self.screen[self.seen..end].to_string();
                self.seen = end;
                return shown;
            }
            match self.output.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(chunk) => self.screen.push_str(&text(&chunk)),
                Err(_) => {
                    let _ = self.child.kill();
                    panic!("the terminal never showed {expected:?}; it showed {:?}", &self.screen[self.seen..]);
                }
            }
        }
    }

    fn press(&mut self, keys: &str) {
        use std::io::Write;
        (&self.controller).write_all(keys.as_bytes()).unwrap();
    }

    /// After Ctrl-D was pressed at the start of a line: whether that end of input is still waiting
    /// at the terminal, which it is when nothing read the terminal since. Reads it, from a
    /// descriptor of its own that does not wait, so `vendo`'s stays as it was.
    fn takes_typed_end_of_input(&mut self) -> bool {
        // SAFETY: open, read and close on a descriptor this function owns; `byte` outlives the read.
        let (read, err) = unsafe {
            let fd = libc::open(self.name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC);
            assert!(fd >= 0, "open {:?}: {}", self.name, std::io::Error::last_os_error());
            let mut byte = 0u8;
            let read = libc::read(fd, (&raw mut byte).cast(), 1);
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            (read, err)
        };
        match read {
            // A typed end of input reads as nothing, once.
            0 => true,
            -1 if err.kind() == std::io::ErrorKind::WouldBlock => false,
            _ => panic!("the terminal had more than an end of input waiting: {read} {err}"),
        }
    }

    /// Reads until `vendo` exits: everything the terminal showed after the last wait, and the exit code.
    /// A `vendo` that shows nothing for 20 seconds before it exits is stopped.
    fn finish(&mut self) -> (String, Option<i32>) {
        use std::{
            sync::mpsc::RecvTimeoutError,
            time::{Duration, Instant},
        };
        let mut shown = Instant::now();
        // Reading can end first: on macOS, when `vendo`, the terminal's session leader, exits.
        while self.child.try_wait().unwrap().is_none() && shown.elapsed() < Duration::from_secs(20) {
            match self.output.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    self.screen.push_str(&text(&chunk));
                    shown = Instant::now();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = self.child.kill();
        let code = self.child.wait().unwrap().code();
        // The last copy of the terminal end: reading ends after what the terminal still holds.
        self.terminal = None;
        while let Ok(chunk) = self.output.recv_timeout(Duration::from_secs(20)) {
            self.screen.push_str(&text(&chunk));
        }
        (self.screen[self.seen..].to_string(), code)
    }

    /// Types `bytes` as they are, also ones that are no whole character.
    fn press_bytes(&mut self, bytes: &[u8]) {
        use std::io::Write;
        (&self.controller).write_all(bytes).unwrap();
    }

    /// Hangs the terminal up, as a terminal window that closes, or a program that drops the
    /// pseudo-terminal it opened, does: every copy of the controller closes, and the test's copy of
    /// the terminal end too. `vendo` keeps its own (stdin, stdout and stderr) on a terminal that has
    /// hung up: reading it gives the end of the input (or an I/O error) at once, every time. What
    /// `vendo` shows after this is not read.
    fn hang_up(&mut self) {
        use std::io::Write;
        // The reader thread keeps its copy of the controller until what it reads has nowhere to
        // go: with its receiver dropped, a line written to the terminal end is the last it reads.
        self.output = std::sync::mpsc::channel().1;
        let kept = self.terminal.take().expect("the terminal hangs up once");
        let _ = std::fs::File::from(kept).write_all(b"\n");
        self.reader.take().expect("read until the terminal hangs up").join().unwrap();
        // The test's own copy of the controller, the last.
        self.controller = std::fs::File::open("/dev/null").unwrap();
    }

    /// After [`OnTerminal::hang_up`]: waits up to `limit` for `vendo` to exit and reaps it, with the
    /// processor time it used. `None` when it still runs then; [`OnTerminal::stop`] ends it.
    fn exit_within(&mut self, limit: std::time::Duration) -> Option<Ended> {
        let asked = std::time::Instant::now();
        loop {
            if let Some(ended) = self.reap(libc::WNOHANG) {
                return Some(ended);
            }
            if asked.elapsed() >= limit {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Kills `vendo` and reaps it, with the processor time it used.
    fn stop(&mut self) -> Ended {
        let _ = self.child.kill();
        self.reap(0).expect("a killed vendo is reaped")
    }

    /// `wait4` on `vendo` with `options`: how it ended once it has, and the processor time it used,
    /// which `Child::wait` does not tell.
    fn reap(&mut self, options: libc::c_int) -> Option<Ended> {
        assert!(!self.reaped, "vendo was reaped already");
        let pid = libc::pid_t::try_from(self.child.id()).unwrap();
        let mut status = 0;
        // SAFETY: `usage` is plain data that wait4 fills when it reaps the child.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: wait4 on the test's own child, writing into `status` and `usage`.
            match unsafe { libc::wait4(pid, &mut status, options, &mut usage) } {
                0 => return None,
                -1 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
                -1 => panic!("wait4: {}", std::io::Error::last_os_error()),
                _ => break,
            }
        }
        self.reaped = true;
        let time = |t: libc::timeval| {
            std::time::Duration::from_secs(t.tv_sec as u64) + std::time::Duration::from_micros(t.tv_usec as u64)
        };
        let code = libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status));
        Some(Ended { code, cpu: time(usage.ru_utime) + time(usage.ru_stime) })
    }
}

#[cfg(unix)]
impl Drop for OnTerminal {
    /// Stops a `vendo` still running when the test ends early, on a failed assertion, so it reads
    /// nothing from what the test leaves behind (login opens a browser on a line it reads). Not one
    /// the test reaped itself: its process ID may belong to another process by now.
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Run `vendo` on a pseudo-terminal ([`OnTerminal`]); `answer` is typed once
/// the `(y/N)` question shows. Returns what the terminal showed and the exit code.
#[cfg(unix)]
fn answer_on_terminal(sandbox: &Sandbox, args: &[&str], answer: &str) -> (String, Option<i32>) {
    answer_question(OnTerminal::start(sandbox, args), answer)
}

/// [`answer_on_terminal`] for a `vendo` already started on a terminal.
#[cfg(unix)]
fn answer_question(mut terminal: OnTerminal, answer: &str) -> (String, Option<i32>) {
    let question = terminal.wait_for("(y/N) ");
    terminal.press(answer);
    let (rest, code) = terminal.finish();
    (question + &rest, code)
}

#[cfg(unix)]
#[tokio::test]
async fn on_a_terminal_the_question_is_unchanged() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    // "n" changes nothing; the questions and "Cancelled" lines are the TS CLI's. `config reset` asks
    // what `logout --all`, which it runs since CLI 1.1 (VE-3827), asks.
    for (args, question, after) in [
        (&["apps", "delete", ID][..], "Delete app 550e8400...? (y/N) ", ""),
        (&["sources", "delete", ID, "--json"], "Delete source 550e8400...? (y/N) ", ""),
        (&["destinations", "delete", ID], "Delete destination 550e8400...? (y/N) ", ""),
        (&["jobs", "cancel", ID], "Cancel job 550e8400...? (y/N) ", ""),
        (
            &["metrics", "delete", ID, "--json"],
            "Delete metric 550e8400...? This cannot be undone. (y/N) ",
            "Cancelled\n",
        ),
        (&["config", "reset"], "Remove every saved profile? (y/N) ", "Cancelled.\n"),
        (&["logout", "--all"], "Remove every saved profile? (y/N) ", "Cancelled.\n"),
    ] {
        let (screen, code) = answer_on_terminal(&sandbox, args, "n\n");
        assert_eq!(code, Some(0), "{args:?}: {screen:?}");
        // The terminal echoes the answer and ends lines with CR LF.
        assert_eq!(screen.replace("\r\n", "\n"), format!("{question}n\n{after}"), "{args:?}");
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
    assert!(sandbox.home.path().join(".config/vendo/config.json").exists());

    // "y" goes ahead; --json asks first now instead of skipping the question.
    let (screen, code) = answer_on_terminal(&sandbox, &["apps", "delete", ID, "--json"], "y\n");
    assert_eq!(code, Some(0), "{screen:?}");
    assert!(screen.starts_with("Delete app 550e8400...? (y/N) y\r\n"), "{screen:?}");
    assert!(screen.contains(&format!("\"id\": \"{ID}\"")), "{screen:?}");
    assert_eq!(sent(&server).await, vec![format!("DELETE /api/v1/accounts/acct-alpha/apps/{ID}")]);
}

#[cfg(unix)]
#[tokio::test]
async fn a_question_needs_stdin_and_stdout_on_a_terminal() {
    use std::io::Write;
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let what = format!("This deletes app {ID}.");

    // A terminal to read from, but stdout goes to a pipe: nobody would see the question.
    let (_controller, terminal) = pseudo_terminal();
    let out = sandbox.command(&["apps", "delete", ID]).stdin(terminal).output().unwrap();
    assert_needs_yes(&out, &what);

    // Shown on a terminal, but the answer would come from a pipe: `echo y | vendo …` does not confirm.
    let (_controller, terminal) = pseudo_terminal();
    let mut child = sandbox
        .command(&["apps", "delete", ID])
        .stdin(Stdio::piped())
        .stdout(terminal)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        (out.status.code(), text(&out.stderr)),
        (Some(1), format!("Error: {what} Re-run with --yes to confirm.\n"))
    );

    assert_eq!(sent(&server).await, Vec::<String>::new());
}

// ── VE-3825: `vendo login` signs in, checks the key and prints the summary ───
// `login` does what `login` and `init` did (Yalcin, 2026-10-05, CLI 1.1): it signs in through the
// browser when there is no working key, checks the key with `/me` and prints the setup summary.
// `init` is its hidden alias. The browser is the test: stdin stays closed, so `vendo` never opens
// one; the test visits the printed sign-in URL instead.

/// The key the stub's sign-in page creates.
const NEW_KEY: &str = "vendo_sk_fake_new_000000";
const ALPHA_KEY: &str = "vendo_sk_fake_alpha_0000";
const GAMMA_KEY: &str = "vendo_sk_fake_gamma_0000";

/// The web app's `/cli-auth?port=&state=` page once the person approves: it creates an API key
/// (`createApiKey`) and redirects to the CLI's local callback with it.
pub struct SignInPage;

impl wiremock::Respond for SignInPage {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let param = |key: &str| {
            request.url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).unwrap_or_default()
        };
        let callback = format!(
            "http://127.0.0.1:{}/callback?key={NEW_KEY}&account=demo-account&account_id=acct-alpha&state={}",
            param("port"),
            param("state")
        );
        ResponseTemplate::new(302).insert_header("Location", callback.as_str())
    }
}

pub async fn mount_sign_in_page(server: &MockServer) {
    Mock::given(wiremock::matchers::method("GET")).and(path("/cli-auth")).respond_with(SignInPage).mount(server).await;
}

/// A stub whose `/me` accepts `keys` and answers `refusal` to any other, with the sign-in page.
async fn sign_in_stub(keys: &[&str], refusal: u16) -> MockServer {
    let server = MockServer::start().await;
    let me = json!({ "data": {
        "accountId": "acct-alpha", "accountName": "Demo Account (synthetic)", "accountSlug": "demo-account",
        "apiKeyId": "key_fake_0001", "scopes": [],
    } });
    for key in keys {
        Mock::given(wiremock::matchers::method("GET"))
            .and(path("/api/v1/me"))
            .and(wiremock::matchers::header("authorization", format!("Bearer {key}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(me.clone()))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let error = json!({ "error": { "code": "REFUSED", "message": "Refused by the stub" } });
    serve(&server, "GET", "/api/v1/me", refusal, error).await;
    mount_sign_in_page(&server).await;
    server
}

/// What reached the stub: `GET /cli-auth` (a key created), `GET /api/v1/me <key>` (a key checked).
async fn sign_in_requests(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap();
    requests
        .iter()
        .map(|r| match r.headers.get("authorization").and_then(|v| v.to_str().ok()) {
            Some(auth) => format!("{} {} {}", r.method, r.url.path(), auth.trim_start_matches("Bearer ")),
            None => format!("{} {}", r.method, r.url.path()),
        })
        .collect()
}

/// Run `cmd` as a person at the browser would: when `vendo` prints the sign-in URL, visit it (the
/// stub's sign-in page creates a key and sends it to the CLI's callback). Returns the exit code,
/// stdout with the callback's random port and state shown as `[port]` and `[state]`, and stderr.
pub async fn login_at_browser(cmd: Command) -> (Option<i32>, String, String) {
    browser_login(cmd, false).await
}

/// [`login_at_browser`] for `login --json`, which prints the sign-in URL on stderr: stderr is the
/// one shown with `[port]` and `[state]`.
async fn login_at_browser_json(cmd: Command) -> (Option<i32>, String, String) {
    browser_login(cmd, true).await
}

async fn browser_login(mut cmd: Command, url_on_stderr: bool) -> (Option<i32>, String, String) {
    use std::io::BufRead;
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let watched: Box<dyn std::io::Read> =
        if url_on_stderr { Box::new(child.stderr.take().unwrap()) } else { Box::new(child.stdout.take().unwrap()) };
    let mut watched = std::io::BufReader::new(watched);
    let (mut shown, mut line) = (String::new(), String::new());
    while watched.read_line(&mut line).unwrap() > 0 {
        if line.contains("/cli-auth?") {
            let url = reqwest::Url::parse(line.trim_end()).unwrap();
            assert_eq!(url.host_str(), Some("127.0.0.1"), "tests sign in at their local stub only: {url}");
            let page = reqwest::get(url.clone()).await.unwrap();
            assert_eq!(page.status(), 200);
            assert!(page.text().await.unwrap().contains("<h1>Authenticated</h1>"));
            for (key, value) in url.query_pairs() {
                line = line.replace(&format!("{key}={value}"), &format!("{key}=[{key}]"));
            }
        }
        shown.push_str(&line);
        line.clear();
    }
    let out = child.wait_with_output().unwrap();
    if url_on_stderr {
        (out.status.code(), text(&out.stdout), shown)
    } else {
        (out.status.code(), shown, text(&out.stderr))
    }
}

/// The summary and next steps a successful login ends with.
fn signed_in(profile: &str, base_url: &str, account_id: &str) -> String {
    format!(
        "\nSetup summary\n  Profile:     {profile}\n  Base URL:    {base_url}\n  Account ID:  {account_id}\n  Auth:        verified as Demo Account (synthetic)\n\nNext steps\n  vendo workspace\n  vendo status\nDone: Vendo CLI setup complete.\n"
    )
}

/// The lines before the browser sign-in's summary: `why`, then the sign-in URL on `base_url`.
fn browser_sign_in(why: &str, base_url: &str) -> String {
    format!(
        "{why}\nLogin at:\n{base_url}/cli-auth?port=[port]&state=[state]\nPress ENTER to open in the browser...\nWaiting for authorization...\n"
    )
}

#[tokio::test]
async fn login_without_a_key_signs_in_through_the_browser_and_checks_the_new_key() {
    let server = sign_in_stub(&[NEW_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::without_api_keys(&stub);
    let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login"])).await;
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let expected = browser_sign_in("No API key found. Starting browser login...", &stub);
    assert_eq!(stdout, expected + &signed_in("demo-account", &stub, "acct-alpha"));
    assert_eq!(sign_in_requests(&server).await, ["GET /cli-auth".to_string(), format!("GET /api/v1/me {NEW_KEY}")]);
    let config = sandbox.config();
    assert_eq!(config["activeProfile"], "demo-account");
    assert_eq!(
        config["profiles"]["demo-account"],
        json!({ "apiKey": NEW_KEY, "accountId": "acct-alpha", "baseUrl": stub })
    );
}

#[tokio::test]
async fn login_with_a_working_key_checks_it_and_creates_no_key() {
    let server = sign_in_stub(&[ALPHA_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::new(&stub);
    let before = sandbox.config();
    let expected = "Using existing profile alpha. Run `vendo login --force` to sign in again.\n".to_string()
        + &signed_in("alpha", &stub, "acct-alpha");
    // Naming the profile's own instance is the same login.
    for args in [&["login"][..], &["login", "--base-url", &stub]] {
        let (code, stdout, stderr) = login_at_browser(sandbox.command(args)).await;
        assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), expected.as_str(), ""), "{args:?}");
    }
    // Only checks: no sign-in page, so no new key.
    assert_eq!(sign_in_requests(&server).await, vec![format!("GET /api/v1/me {ALPHA_KEY}"); 2]);
    assert_eq!(sandbox.config(), before);
}

#[tokio::test]
async fn login_force_signs_in_again_without_checking_the_saved_key() {
    let server = sign_in_stub(&[ALPHA_KEY, NEW_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::new(&stub);
    let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login", "--force"])).await;
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let expected = browser_sign_in("Signing in again (--force). Starting browser login...", &stub);
    assert_eq!(stdout, expected + &signed_in("demo-account", &stub, "acct-alpha"));
    assert_eq!(sign_in_requests(&server).await, ["GET /cli-auth".to_string(), format!("GET /api/v1/me {NEW_KEY}")]);
    let config = sandbox.config();
    assert_eq!(config["activeProfile"], "demo-account");
    assert_eq!(config["profiles"]["alpha"]["apiKey"], ALPHA_KEY, "the other profiles are kept");
}

#[tokio::test]
async fn login_with_a_rejected_key_signs_in_through_the_browser() {
    for status in [401, 403] {
        let server = sign_in_stub(&[NEW_KEY], status).await;
        let stub = server.uri();
        let sandbox = Sandbox::new(&stub);
        let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login"])).await;
        assert_eq!((code, stderr.as_str()), (Some(0), ""), "{status}");
        let why = format!("The API key in profile alpha was rejected (HTTP {status}). Starting browser login...");
        assert_eq!(stdout, browser_sign_in(&why, &stub) + &signed_in("demo-account", &stub, "acct-alpha"));
        assert_eq!(
            sign_in_requests(&server).await,
            [format!("GET /api/v1/me {ALPHA_KEY}"), "GET /cli-auth".to_string(), format!("GET /api/v1/me {NEW_KEY}")]
        );
        assert_eq!(sandbox.config()["profiles"]["demo-account"]["apiKey"], NEW_KEY);
    }
}

#[tokio::test]
async fn login_that_cannot_check_the_key_creates_no_key() {
    // A server error, an answer that is not the API's, or no answer: the key may still work, so
    // nothing is replaced and login fails.
    let not_checked = |stub: &str| {
        "Using existing profile alpha. Run `vendo login --force` to sign in again.\n\nSetup summary\n  Profile:     alpha\n"
            .to_string()
            + &format!("  Base URL:    {stub}\n  Account ID:  acct-alpha\n  Auth:        not verified (API check failed)\n")
    };
    let error = |reason: &str| {
        format!(
            "Error: Could not verify the API key: {reason}. Nothing was changed: check your connection (`vendo workspace`) and run `vendo login` again.\n"
        )
    };
    for (status, reason) in
        [(500, "HTTP 500 Internal Server Error"), (404, "HTTP 404 Not Found"), (429, "HTTP 429 Too Many Requests")]
    {
        let server = sign_in_stub(&[], status).await;
        let stub = server.uri();
        let sandbox = Sandbox::new(&stub);
        let before = sandbox.config();
        let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login"])).await;
        assert_eq!((code, stdout, stderr), (Some(1), not_checked(&stub), error(reason)), "{status}");
        assert_eq!(sign_in_requests(&server).await, [format!("GET /api/v1/me {ALPHA_KEY}")], "{status}");
        assert_eq!(sandbox.config(), before);
    }
    let sandbox = Sandbox::new(CLOSED);
    let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login"])).await;
    assert_eq!((code, stdout, stderr), (Some(1), not_checked(CLOSED), error("fetch failed")));
}

#[tokio::test]
async fn a_new_key_that_cannot_be_checked_is_kept_and_login_fails() {
    // The sign-in worked, so its key is saved; a failed check never starts another sign-in.
    for (status, reason) in [(500, "HTTP 500 Internal Server Error"), (401, "HTTP 401 Unauthorized")] {
        let server = sign_in_stub(&[], status).await;
        let stub = server.uri();
        let sandbox = Sandbox::without_api_keys(&stub);
        let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login"])).await;
        let summary = format!(
            "\nSetup summary\n  Profile:     demo-account\n  Base URL:    {stub}\n  Account ID:  acct-alpha\n  Auth:        not verified (API check failed)\n"
        );
        let expected = browser_sign_in("No API key found. Starting browser login...", &stub) + &summary;
        let error = format!(
            "Error: Could not verify the API key: {reason}. The new key is saved in profile demo-account: run `vendo workspace` to check it again.\n"
        );
        assert_eq!((code, stdout, stderr), (Some(1), expected, error), "{status}");
        assert_eq!(sign_in_requests(&server).await, ["GET /cli-auth".to_string(), format!("GET /api/v1/me {NEW_KEY}")]);
        assert_eq!(sandbox.config()["profiles"]["demo-account"]["apiKey"], NEW_KEY, "{status}");
    }
}

#[tokio::test]
async fn a_key_without_an_account_is_kept_and_the_summary_says_what_is_missing() {
    let server = sign_in_stub(&[ALPHA_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::new(&stub);
    let mut config = sandbox.config();
    config["profiles"]["alpha"].as_object_mut().unwrap().remove("accountId");
    std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
    let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login"])).await;
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    assert_eq!(
        stdout,
        format!(
            "Using existing profile alpha. Run `vendo login --force` to sign in again.\n\nSetup summary\n  Profile:     alpha\n  Base URL:    {stub}\n  Account ID:  missing\n  Auth:        incomplete (account ID still required)\n\nNext steps\n  vendo workspace\n  vendo status\n\nSet an account explicitly with `vendo profile set --account <account-id>` if your login flow did not provide one.\nDone: Vendo CLI setup complete.\n"
        )
    );
    assert_eq!(sign_in_requests(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn login_for_another_instance_signs_in_there() {
    let saved = MockServer::start().await;
    let other = sign_in_stub(&[NEW_KEY], 401).await;
    let sandbox = Sandbox::new(&saved.uri());
    let (code, stdout, stderr) = login_at_browser(sandbox.command(&["login", "--base-url", &other.uri()])).await;
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let why = format!("Profile alpha is for {}. Starting browser login for {}...", saved.uri(), other.uri());
    assert_eq!(stdout, browser_sign_in(&why, &other.uri()) + &signed_in("demo-account", &other.uri(), "acct-alpha"));
    assert_eq!(sign_in_requests(&saved).await, Vec::<String>::new(), "the saved key is not for this instance");
    assert_eq!(sign_in_requests(&other).await, ["GET /cli-auth".to_string(), format!("GET /api/v1/me {NEW_KEY}")]);
    assert_eq!(sandbox.config()["profiles"]["demo-account"]["baseUrl"], other.uri());
}

#[tokio::test]
async fn headless_login_never_opens_a_browser() {
    let server = sign_in_stub(&[GAMMA_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::new(&stub);
    // Both flags: the key is checked and saved, with or without --force (it signs in with the key given).
    for force in [&[][..], &["--force"]] {
        let args =
            [&["login", "--api-key", GAMMA_KEY, "--account", "acct-gamma", "--base-url", &stub][..], force].concat();
        let (code, stdout, stderr) = login_at_browser(sandbox.command(&args)).await;
        assert_eq!((code, stdout, stderr), (Some(0), signed_in("demo-account", &stub, "acct-gamma"), String::new()));
    }
    let config = sandbox.config();
    assert_eq!(config["activeProfile"], "demo-account");
    assert_eq!(
        config["profiles"]["demo-account"],
        json!({ "apiKey": GAMMA_KEY, "accountId": "acct-gamma", "baseUrl": stub })
    );
    // A rejected key is refused and nothing is saved; one flag alone is refused before any request.
    let rejected = ["login", "--api-key", "vendo_sk_fake_bad_00000", "--account", "acct-gamma", "--base-url", &stub];
    let one_flag = ["login", "--api-key", GAMMA_KEY, "--base-url", &stub];
    for (args, error) in [
        (&rejected[..], "Error: Credential validation failed (HTTP 401). Check your API key and account ID.\n"),
        (
            &one_flag,
            "Error: Both --api-key and --account are required for headless login.\n  Example: vendo login --api-key <key> --account <id>\n",
        ),
    ] {
        let (code, stdout, stderr) = login_at_browser(sandbox.command(args)).await;
        assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(1), "", error), "{args:?}");
    }
    assert_eq!(sandbox.config(), config);
    let checks = [GAMMA_KEY, GAMMA_KEY, "vendo_sk_fake_bad_00000"].map(|key| format!("GET /api/v1/me {key}"));
    assert_eq!(sign_in_requests(&server).await, checks, "no sign-in page");
}

#[tokio::test]
async fn a_key_in_vendo_api_key_is_checked_and_never_opens_a_browser() {
    const ENV_KEY: &str = "vendo_sk_fake_env_000000";
    let server = sign_in_stub(&[ENV_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::without_api_keys(&stub);
    let before = sandbox.config();
    let with_key = |key: &str, args: &[&str]| {
        let mut cmd = sandbox.command(args);
        cmd.env("VENDO_API_KEY", key);
        cmd
    };
    let (code, stdout, stderr) = login_at_browser(with_key(ENV_KEY, &["login"])).await;
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    assert_eq!(stdout, "Using the API key in VENDO_API_KEY.\n".to_string() + &signed_in("alpha", &stub, "acct-alpha"));

    let unset = "`vendo login` does not open a browser while VENDO_API_KEY is set";
    for (key, args, error) in [
        (
            "vendo_sk_fake_bad_00000",
            &["login"][..],
            format!(
                "The API key in VENDO_API_KEY was rejected (HTTP 401). {unset}: set a working key, or unset it to sign in through the browser."
            ),
        ),
        (ENV_KEY, &["login", "--force"], format!("{unset}: unset it to sign in through the browser.")),
        (
            ENV_KEY,
            &["login", "--env", "staging"],
            format!(
                "VENDO_API_KEY is used with {stub}, not https://stg.vendodata.com. {unset}: unset it to sign in through the browser."
            ),
        ),
    ] {
        let (code, stdout, stderr) = login_at_browser(with_key(key, args)).await;
        assert_eq!((code, stdout.as_str(), stderr), (Some(1), "", format!("Error: {error}\n")), "{args:?}");
    }
    // --force and another instance are refused before any request.
    let checks = [ENV_KEY, "vendo_sk_fake_bad_00000"].map(|key| format!("GET /api/v1/me {key}"));
    assert_eq!(sign_in_requests(&server).await, checks);
    assert_eq!(sandbox.config(), before, "nothing saved");
}

#[tokio::test]
async fn init_prints_exactly_what_login_prints() {
    // `init` is a hidden alias of `login` since CLI 1.1 (VE-3825): the same output, requests and
    // saved profiles from the same start, in every state.
    async fn run(command: &str, state: &str) -> (Option<i32>, String, String, Vec<String>, String) {
        let accepted: &[&str] = if state == "rejected key" { &[NEW_KEY] } else { &[ALPHA_KEY, GAMMA_KEY, NEW_KEY] };
        let server = sign_in_stub(accepted, 401).await;
        let stub = server.uri();
        let sandbox = if state == "no key" { Sandbox::without_api_keys(&stub) } else { Sandbox::new(&stub) };
        let mut args = vec![command];
        match state {
            "--force" => args.push("--force"),
            "headless" => args.extend(["--api-key", GAMMA_KEY, "--account", "acct-gamma"]),
            _ => {}
        }
        let (code, stdout, stderr) = login_at_browser(sandbox.command(&args)).await;
        let config = sandbox.config().to_string();
        let shown = |s: String| s.replace(&stub, "[stub]");
        (code, shown(stdout), shown(stderr), sign_in_requests(&server).await, shown(config))
    }
    for state in ["no key", "working key", "--force", "headless", "rejected key"] {
        let login = run("login", state).await;
        assert_eq!(login.0, Some(0), "{state}: {login:?}");
        assert_eq!(run("init", state).await, login, "{state}");
    }
}

// ── VE-3829: catalog list shows the ready platforms, --all every one ────────
// Decided by Yalcin, 2026-10-05 (CLI 1.1). Bodies shaped like vendo-web-v2's
// `GET /api/v1/catalog` since VE-2436: an `availability` per entry, and `meta`
// counts taken after the category and role filters. The route has no pagination.

const CATALOG: &str = "/api/v1/catalog";

fn platform(app_type: &str, name: &str, availability: &str, reason: Value) -> Value {
    json!({
        "appType": app_type, "displayName": name, "category": "ads",
        "description": format!("{name} connector (synthetic)"), "logoUrl": null, "supportedRoles": ["source"],
        "lifecycle": "live", "selfServe": availability == "self_serve", "availability": availability,
        "requestAccessReason": reason, "provider": "vendo",
    })
}

/// `GET /api/v1/catalog`, asking for the request-access entries (`--all`) or not.
fn catalog_request(all: bool) -> wiremock::MockBuilder {
    let request = Mock::given(wiremock::matchers::method("GET")).and(path(CATALOG));
    if all {
        request.and(wiremock::matchers::query_param("include_request_access", "true"))
    } else {
        request.and(wiremock::matchers::query_param_is_missing("include_request_access"))
    }
}

async fn serve_catalog(server: &MockServer, all: bool, body: Value) {
    catalog_request(all).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(server).await;
}

const CATALOG_HEADER: [&str; 5] = ["App Type", "Name", "Category", "Roles", "Availability"];

#[tokio::test]
async fn catalog_list_shows_the_ready_platforms_and_counts_from_meta() {
    let server = MockServer::start().await;
    let ready = vec![
        platform("google_ads", "Google Ads", "self_serve", Value::Null),
        platform("meta_ads", "Meta Ads", "self_serve", Value::Null),
    ];
    // Two rows, but the API's counts: the footer prints those, not the rows.
    let meta = json!({ "total": 35, "selfServeTotal": 35, "requestAccessTotal": 560 });
    serve_catalog(&server, false, json!({ "data": ready, "meta": meta })).await;
    let sandbox = Sandbox::new(&server.uri());
    assert_eq!(
        cells(ok_output(&sandbox.run(&["catalog", "list"])).as_bytes()),
        rows(&[
            &CATALOG_HEADER,
            &["google_ads", "Google Ads", "ads", "source", "ready"],
            &["meta_ads", "Meta Ads", "ads", "source", "ready"],
            &["35 ready · 560 more on request (vendo catalog list --all)"],
        ])
    );
    // Filters go to the API, which counts within them; the CLI prints its counts as sent.
    ok_output(&sandbox.run(&["catalog", "list", "--category", "ads", "--role", "source"]));
    assert_eq!(sent(&server).await, [format!("GET {CATALOG}"), format!("GET {CATALOG}?category=ads&role=source")]);
}

#[tokio::test]
async fn catalog_list_footer_hint_repeats_the_filters() {
    // Yalcin, 2026-10-06: the counts are taken within --category and --role, so the hint that lists
    // the rest keeps them, --category first and the values as typed.
    let server = MockServer::start().await;
    let meta = json!({ "total": 9, "selfServeTotal": 9, "requestAccessTotal": 5 });
    let body = json!({ "data": [platform("google_ads", "Google Ads", "self_serve", Value::Null)], "meta": meta });
    serve_catalog(&server, false, body).await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, hint) in [
        (&["--category", "advertising"][..], "vendo catalog list --category advertising --all"),
        (&["--role", "source"], "vendo catalog list --role source --all"),
        (&["--role", "destination", "--category", "crm"], "vendo catalog list --category crm --role destination --all"),
        (&[], "vendo catalog list --all"),
    ] {
        let out = cells(ok_output(&sandbox.run(&[&["catalog", "list"][..], args].concat())).as_bytes());
        assert_eq!(out.last().cloned(), Some(vec![format!("9 ready · 5 more on request ({hint})")]), "{args:?}");
    }
}

#[tokio::test]
async fn catalog_get_shows_the_availability_the_list_shows() {
    // Yalcin, 2026-10-06: `Availability:` with the list's words, from the API's `availability`, in
    // place of `Self-Serve: yes/no`; `--json` prints the response as sent, as before.
    let server = MockServer::start().await;
    let entries = [
        platform("google_ads", "Google Ads", "self_serve", Value::Null),
        platform("tiktok_ads", "TikTok Ads", "request_access", json!("request_access_required")),
    ];
    for entry in &entries {
        serve(
            &server,
            "GET",
            &format!("{CATALOG}/{}", entry["appType"].as_str().unwrap()),
            200,
            json!({ "data": entry }),
        )
        .await;
    }
    let sandbox = Sandbox::new(&server.uri());
    let view = |app_type: &str, name: &str, words: &str| {
        format!(
            "\n{name} ({app_type})\n\n  Category:      ads\n  Roles:         source\n  Availability:  {words}\n  \
             Lifecycle:     live\n  Provider:      vendo\n\n  {name} connector (synthetic)\n"
        )
    };
    assert_eq!(ok_output(&sandbox.run(&["catalog", "get", "google_ads"])), view("google_ads", "Google Ads", "ready"));
    assert_eq!(
        ok_output(&sandbox.run(&["catalog", "get", "tiktok_ads"])),
        view("tiktok_ads", "TikTok Ads", "on request")
    );
    let printed: Value = serde_json::from_str(&ok_output(&sandbox.run(&["catalog", "get", "tiktok_ads", "--json"])))
        .expect("--json prints JSON");
    assert_eq!(printed, json!({ "data": entries[1] }));
}

#[tokio::test]
async fn catalog_get_from_an_api_without_availability_keeps_the_self_serve_line() {
    // Before VE-2436 the route sent `selfServe` and no `availability`: the view as it was.
    let server = MockServer::start().await;
    let old = json!({ "appType": "stripe", "displayName": "Stripe", "category": "payments",
                      "supportedRoles": ["source"], "selfServe": true });
    serve(&server, "GET", &format!("{CATALOG}/stripe"), 200, json!({ "data": old })).await;
    assert_eq!(
        ok_output(&Sandbox::new(&server.uri()).run(&["catalog", "get", "stripe"])),
        "\nStripe (stripe)\n\n  Category:    payments\n  Roles:       source\n  Self-Serve:  yes\n"
    );
}

#[tokio::test]
async fn catalog_list_footer_leaves_out_an_empty_request_access_count() {
    let server = MockServer::start().await;
    let meta = json!({ "total": 1, "selfServeTotal": 1, "requestAccessTotal": 0 });
    let body = json!({ "data": [platform("bigquery", "BigQuery", "self_serve", Value::Null)], "meta": meta });
    serve_catalog(&server, false, body).await;
    let out =
        cells(ok_output(&Sandbox::new(&server.uri()).run(&["catalog", "list", "--role", "destination"])).as_bytes());
    assert_eq!(out.last().cloned(), Some(vec!["1 ready".to_string()]));
}

#[tokio::test]
async fn catalog_list_all_lists_every_platform_with_its_availability() {
    let server = MockServer::start().await;
    // `self_serve`, then `request_access` for each of the route's three reasons.
    let every = vec![
        platform("google_ads", "Google Ads", "self_serve", Value::Null),
        platform("reddit_ads", "Reddit Ads", "request_access", json!("setup_unsupported")),
        platform("snapchat_ads", "Snapchat Ads", "request_access", json!("coming_soon")),
        platform("tiktok_ads", "TikTok Ads", "request_access", json!("request_access_required")),
    ];
    let meta = json!({ "total": 4, "selfServeTotal": 1, "requestAccessTotal": 3 });
    serve_catalog(&server, true, json!({ "data": every, "meta": meta })).await;
    let sandbox = Sandbox::new(&server.uri());
    assert_eq!(
        cells(ok_output(&sandbox.run(&["catalog", "list", "--all"])).as_bytes()),
        rows(&[
            &CATALOG_HEADER,
            &["google_ads", "Google Ads", "ads", "source", "ready"],
            &["reddit_ads", "Reddit Ads", "ads", "source", "on request"],
            &["snapchat_ads", "Snapchat Ads", "ads", "source", "on request"],
            &["tiktok_ads", "TikTok Ads", "ads", "source", "on request"],
            &["4 platforms"],
        ])
    );
    assert_eq!(
        ok_output(&sandbox.run(&["catalog", "list", "--all", "--output", "appType"])),
        "google_ads\nreddit_ads\nsnapchat_ads\ntiktok_ads\n"
    );
    ok_output(&sandbox.run(&["catalog", "list", "--category", "ads", "--role", "source", "--all"]));
    let all = "include_request_access=true";
    assert_eq!(
        sent(&server).await,
        [
            format!("GET {CATALOG}?{all}"),
            format!("GET {CATALOG}?{all}"),
            format!("GET {CATALOG}?category=ads&role=source&{all}"),
        ]
    );
}

#[tokio::test]
async fn catalog_list_json_prints_the_response_as_sent_with_or_without_all() {
    let server = MockServer::start().await;
    let ready = r#"{"data":[{"appType":"google_ads","selfServe":true,"availability":"self_serve","requestAccessReason":null}],"meta":{"total":1,"selfServeTotal":1,"requestAccessTotal":1}}"#;
    let every = r#"{"data":[{"appType":"google_ads","availability":"self_serve"},{"appType":"tiktok_ads","availability":"request_access"}],"meta":{"total":2,"selfServeTotal":1,"requestAccessTotal":1}}"#;
    for (all, body) in [(false, ready), (true, every)] {
        catalog_request(all)
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
    }
    let sandbox = Sandbox::new(&server.uri());
    // `JSON.stringify(res, null, 2)`, as every `--json` prints: the plain request and its output are
    // what they were before `--all` existed.
    assert_eq!(
        ok_output(&sandbox.run(&["catalog", "list", "--json"])),
        "{\n  \"data\": [\n    {\n      \"appType\": \"google_ads\",\n      \"selfServe\": true,\n      \"availability\": \"self_serve\",\n      \"requestAccessReason\": null\n    }\n  ],\n  \"meta\": {\n    \"total\": 1,\n    \"selfServeTotal\": 1,\n    \"requestAccessTotal\": 1\n  }\n}\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["catalog", "list", "--all", "--json"])),
        "{\n  \"data\": [\n    {\n      \"appType\": \"google_ads\",\n      \"availability\": \"self_serve\"\n    },\n    {\n      \"appType\": \"tiktok_ads\",\n      \"availability\": \"request_access\"\n    }\n  ],\n  \"meta\": {\n    \"total\": 2,\n    \"selfServeTotal\": 1,\n    \"requestAccessTotal\": 1\n  }\n}\n"
    );
    assert_eq!(sent(&server).await, [format!("GET {CATALOG}"), format!("GET {CATALOG}?include_request_access=true")]);
}

#[tokio::test]
async fn catalog_list_from_an_api_without_availability_keeps_the_count_line() {
    // Before VE-2436 the route sent neither `availability` nor the counts: no guess from
    // `selfServe` (the registry flag, true for nearly every entry), and today's count line.
    let server = MockServer::start().await;
    let old = json!({ "appType": "stripe", "displayName": "Stripe", "category": "payments",
                      "supportedRoles": ["source"], "selfServe": true });
    serve(&server, "GET", CATALOG, 200, json!({ "data": [old] })).await;
    assert_eq!(
        cells(ok_output(&Sandbox::new(&server.uri()).run(&["catalog", "list"])).as_bytes()),
        rows(&[&CATALOG_HEADER, &["stripe", "Stripe", "payments", "source"], &["1 platform"]])
    );
}

// ── VE-3831: easier for AI agents to drive ───────────────────────────────────
// Decided by Yalcin, 2026-10-05: errors as JSON with --json, `vendo commands`
// (tests in cli/snapshots.rs), --json on the commands that lacked it, the short
// IDs tables show, and VENDO_PROFILE.

/// The last line of stderr, which `--json` makes the error as JSON.
fn json_error(out: &Output) -> Value {
    let stderr = text(&out.stderr);
    let last = stderr.lines().last().unwrap_or_else(|| panic!("nothing on stderr"));
    serde_json::from_str(last).unwrap_or_else(|err| panic!("{err}: {stderr}"))
}

fn error_shape(message: &str, code: Value, status: Value, request_id: Value) -> Value {
    json!({ "error": { "message": message, "code": code, "status": status, "requestId": request_id } })
}

#[tokio::test]
async fn with_json_an_api_error_prints_message_code_status_and_request_id_on_one_stderr_line() {
    let server = MockServer::start().await;
    let missing = json!({ "error": { "code": "NOT_FOUND", "message": "Job not found", "details": { "id": ID } } });
    Mock::given(path(format!("/api/v1/accounts/acct-alpha/jobs/{ID}")))
        .respond_with(ResponseTemplate::new(404).set_body_json(missing).insert_header("x-request-id", "req_server_1"))
        .mount(&server)
        .await;
    // A web-app route answers its error as a string: no code (VE-3764).
    serve(&server, "GET", "/api/metrics/m-down", 500, json!({ "error": "BigQuery is unavailable" })).await;
    let sandbox = Sandbox::new(&server.uri());

    let out = sandbox.run(&["jobs", "get", ID, "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()));
    assert_eq!(
        text(&out.stderr),
        "{\"error\":{\"message\":\"Job not found\",\"code\":\"NOT_FOUND\",\"status\":404,\"requestId\":\"req_server_1\"}}\n"
    );
    // Without --json the error reads as before.
    let out = sandbox.run(&["jobs", "get", ID]);
    assert_eq!(
        (out.status.code(), text(&out.stderr)),
        (Some(1), "Error: Job not found\nRequest ID: req_server_1\n".into())
    );

    let out = sandbox.run(&["metrics", "get", "m-down", "--json"]);
    let error = json_error(&out);
    let request_id = error["error"]["requestId"].as_str().unwrap().to_string();
    assert!(request_id.starts_with("cli-") && request_id.len() == 40, "the CLI's own ID: {request_id}");
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()));
    assert_eq!(error, error_shape("BigQuery is unavailable", Value::Null, json!(500), json!(request_id)));
}

#[test]
fn with_json_an_error_the_cli_raises_has_only_a_message() {
    // No key, no answer, a bad flag value: nothing came from the API.
    let keyless = Sandbox::without_api_keys(CLOSED);
    let no_key =
        "No API key configured. Run `vendo login` or `vendo profile set --api-key <key>` or set VENDO_API_KEY.";
    for args in [&["metrics", "list", "--json"][..], &["apps", "get", ID, "--json"]] {
        let out = keyless.run(args);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()), "{args:?}");
        assert_eq!(json_error(&out), error_shape(no_key, Value::Null, Value::Null, Value::Null), "{args:?}");
    }
    let unreachable = Sandbox::new(CLOSED);
    let out = unreachable.run(&["apps", "list", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json_error(&out), error_shape("fetch failed", Value::Null, Value::Null, Value::Null));
    // A usage-style error the command reports itself (exit 1), without its examples.
    let preview = ["measurement", "rules", "preview", "--from", "2026-01-01", "--to", "2026-01-31", "--limit", "0"];
    let out = keyless.run(&[&preview[..], &["--json"]].concat());
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()));
    assert_eq!(
        text(&out.stderr),
        format!(
            "{}\n",
            serde_json::to_string(&error_shape("--limit must be 1..200", Value::Null, Value::Null, Value::Null))
                .unwrap()
        )
    );
    assert!(text(&keyless.run(&preview).stderr).starts_with("Error: --limit must be 1..200\n\nUsage:\n"));
}

#[test]
fn with_json_a_usage_error_is_json_and_still_exits_2() {
    let sandbox = Sandbox::new(CLOSED);
    for (args, message) in [
        (&["apps", "list", "--bogus", "--json"][..], "unexpected argument '--bogus' found"),
        (&["apps", "get", "--json"], "the following required arguments were not provided:\n  <appId>"),
        (&["jobs", "list", "--json", "--limit"], "a value is required for '--limit <n>' but none was supplied"),
        // A group takes no --json: still JSON, since it was asked for.
        (&["apps", "--json"], "unexpected argument '--json' found"),
        // clap's tips follow, one a line, so an agent can correct a typo (Yalcin, 2026-10-06).
        (&["apps", "lst", "--json"], "unrecognized subcommand 'lst'\ntip: a similar subcommand exists: 'list'"),
        (&["aps", "list", "--json"], "unrecognized subcommand 'aps'\ntip: a similar subcommand exists: 'apps'"),
        (
            &["apps", "list", "--json", "--stat", "active"],
            "unexpected argument '--stat' found\ntip: a similar argument exists: '--state'",
        ),
    ] {
        let out = sandbox.run(args);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), String::new()), "{args:?}");
        assert_eq!(
            text(&out.stderr),
            format!(
                "{}\n",
                serde_json::to_string(&error_shape(message, Value::Null, Value::Null, Value::Null)).unwrap()
            ),
            "{args:?}"
        );
    }
    // Without --json, clap's text as before; help is help.
    let out = sandbox.run(&["apps", "list", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).starts_with("error: unexpected argument '--bogus' found\n"), "{}", text(&out.stderr));
    let help = sandbox.run(&["apps", "list", "--json", "--help"]);
    assert_eq!((help.status.code(), text(&help.stderr)), (Some(0), String::new()));
    assert_eq!(text(&help.stdout), text(&sandbox.run(&["apps", "list", "--help"]).stdout));
    // A group without its command prints its help, as before.
    let out = sandbox.run(&["jobs"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).starts_with("Monitor sync jobs\n"));
}

#[tokio::test]
async fn refresh_source_json_keeps_the_response_on_stdout_and_adds_the_error_on_stderr() {
    let server = MockServer::start().await;
    let route = format!("/api/v1/accounts/acct-alpha/connections/{ID}/refresh-source");
    serve(&server, "POST", &route, 200, json!({ "data": { "status": "unavailable" } })).await;
    let sandbox = Sandbox::new(&server.uri());
    let out = sandbox.run(&["destinations", "refresh-source", ID, "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stdout), "{\n  \"data\": {\n    \"status\": \"unavailable\"\n  }\n}\n");
    let headline = "Could not trigger imports — check the source configuration.";
    assert_eq!(json_error(&out), error_shape(headline, Value::Null, Value::Null, Value::Null));
}

// ── --json on the profile commands and logout ──

#[test]
fn profile_list_json_prints_what_the_list_shows() {
    let sandbox = Sandbox::new("https://example.test");
    let profiles = json!({ "profiles": [
        { "name": "alpha", "active": true, "accountId": "acct-alpha", "baseUrl": "https://example.test" },
        { "name": "beta", "active": false, "accountId": "acct-beta", "baseUrl": "https://example.test" },
    ] });
    let printed = ok_output(&sandbox.run(&["profile", "list", "--json"]));
    assert_eq!(printed, format!("{}\n", serde_json::to_string_pretty(&profiles).unwrap()));
    // `config list`, its old name (VE-3827), prints the same.
    assert_eq!(ok_output(&sandbox.run(&["config", "list", "--json"])), printed);
    // --profile marks the profile it selects, as the list does.
    let beta: Value =
        serde_json::from_str(&ok_output(&sandbox.run(&["--profile", "beta", "profile", "list", "--json"]))).unwrap();
    assert_eq!((&beta["profiles"][0]["active"], &beta["profiles"][1]["active"]), (&json!(false), &json!(true)));
    // No profiles, no account.
    let empty = tempfile::tempdir().unwrap();
    let out = sandbox.command(&["profile", "list", "--json"]).env("HOME", empty.path()).output().unwrap();
    assert_eq!(ok_output(&out), "{\n  \"profiles\": []\n}\n");
    std::fs::write(
        sandbox.home.path().join(".config/vendo/config.json"),
        r#"{"profiles":{"x":{}},"activeProfile":"x"}"#,
    )
    .unwrap();
    assert_eq!(
        ok_output(&sandbox.run(&["profile", "list", "--json"])),
        "{\n  \"profiles\": [\n    {\n      \"name\": \"x\",\n      \"active\": true,\n      \"accountId\": null,\n      \"baseUrl\": \"https://app2.vendodata.com\"\n    }\n  ]\n}\n"
    );
}

#[test]
fn profile_switch_and_set_json_print_the_profile_they_changed() {
    let sandbox = Sandbox::new("https://example.test");
    let beta = json!({ "name": "beta", "active": true, "accountId": "acct-beta", "baseUrl": "https://example.test" });
    let printed = ok_output(&sandbox.run(&["profile", "switch", "beta", "--json"]));
    assert_eq!(printed, format!("{}\n", serde_json::to_string_pretty(&json!({ "profile": beta })).unwrap()));
    assert_eq!(sandbox.config()["activeProfile"], "beta");
    let out = sandbox.run(&["profile", "switch", "--account", "acct-alpha", "--json"]);
    assert_eq!(serde_json::from_str::<Value>(&ok_output(&out)).unwrap()["profile"]["name"], "alpha");
    // `config use`, its old name, prints the same.
    assert_eq!(ok_output(&sandbox.run(&["config", "use", "beta", "--json"])), printed);
    // Nothing chosen (no name, no one to pick one): nothing switched, as "Cancelled." says without --json.
    let out = sandbox.run(&["profile", "switch", "--json"]);
    assert_eq!((ok_output(&out), text(&out.stderr)), ("{\n  \"profile\": null\n}\n".to_string(), String::new()));
    assert_eq!(sandbox.config()["activeProfile"], "beta");
    // A profile that does not exist is an error, as JSON.
    let out = sandbox.run(&["profile", "switch", "nope", "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()));
    let message = json_error(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(message.starts_with("Profile \"nope\" not found.\n"), "{message}");

    let out = sandbox.run(&["profile", "set", "--account", "acct-b2", "--json"]);
    let config_path = sandbox.home.path().join(".config/vendo/config.json").display().to_string();
    let expected = json!({
        "profile": { "name": "beta", "active": true, "accountId": "acct-b2", "baseUrl": "https://example.test" },
        "configPath": config_path,
    });
    let printed = ok_output(&out);
    assert_eq!(printed, format!("{}\n", serde_json::to_string_pretty(&expected).unwrap()));
    assert_eq!(ok_output(&sandbox.run(&["config", "set", "--account", "acct-b2", "--json"])), printed);
    // The API key is never printed.
    let out = sandbox.run(&["profile", "set", "--api-key", "vendo_sk_fake_new_secret", "--json"]);
    assert!(!ok_output(&out).contains("vendo_sk_fake_new_secret"));
}

#[test]
fn logout_json_names_the_profiles_it_removed() {
    let sandbox = Sandbox::new(CLOSED);
    assert_eq!(ok_output(&sandbox.run(&["logout", "--json"])), "{\n  \"removed\": [\n    \"alpha\"\n  ]\n}\n");
    // Not logged in: the JSON error on stderr, nothing on stdout, and exit 0 like the text (Yalcin, 2026-10-06).
    let not_logged_in = format!(
        "{}\n",
        serde_json::to_string(&error_shape("Not currently logged in.", Value::Null, Value::Null, Value::Null)).unwrap()
    );
    let out = sandbox.run(&["logout", "--json"]);
    assert_eq!((ok_output(&out), text(&out.stderr)), (String::new(), not_logged_in.clone()));
    // --all asks first: without a terminal it needs --yes (VE-3823), refused as JSON.
    assert_needs_yes_json(
        &sandbox.run(&["logout", "--all", "--json"]),
        "This removes every saved profile and its API key.",
    );
    assert_needs_yes_json(
        &sandbox.run(&["config", "reset", "--json"]),
        "This removes every saved profile and its API key.",
    );
    let printed = ok_output(&sandbox.run(&["logout", "--all", "--yes", "--json"]));
    assert_eq!(printed, "{\n  \"removed\": [\n    \"beta\"\n  ]\n}\n");
    assert!(!sandbox.home.path().join(".config/vendo/config.json").exists());
    assert_eq!(ok_output(&sandbox.run(&["config", "reset", "--yes", "--json"])), "{\n  \"removed\": []\n}\n");
    // Without --json, as before.
    let keyless = Sandbox::without_api_keys(CLOSED);
    let out = keyless.run(&["logout"]);
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), "Error: Not currently logged in.\n".to_string()));
    let out = keyless.run(&["logout", "--json"]);
    assert_eq!((ok_output(&out), text(&out.stderr)), (String::new(), not_logged_in));
}

// ── --json on login, jobs watch and tail, completions and self-update ──

/// `login --json`'s summary.
fn login_summary(profile: &str, base_url: &str, account_id: &str, auth: &str, name: Value) -> String {
    let summary = json!({
        "profile": profile, "baseUrl": base_url, "accountId": account_id, "auth": auth, "accountName": name,
    });
    format!("{}\n", serde_json::to_string_pretty(&summary).unwrap())
}

#[tokio::test]
async fn login_json_prints_the_summary_and_says_the_rest_on_stderr() {
    let server = sign_in_stub(&[ALPHA_KEY, GAMMA_KEY, NEW_KEY], 401).await;
    let stub = server.uri();
    let verified = json!("Demo Account (synthetic)");
    // Headless: only the summary, never the key.
    let sandbox = Sandbox::new(&stub);
    let headless = ["login", "--api-key", GAMMA_KEY, "--account", "acct-gamma", "--base-url", &stub, "--json"];
    let out = sandbox.run(&headless);
    assert_eq!(
        (out.status.code(), text(&out.stdout), text(&out.stderr)),
        (Some(0), login_summary("demo-account", &stub, "acct-gamma", "verified", verified.clone()), String::new())
    );
    // A working key: what the text says on the way goes to stderr. `init`, its alias, is the same.
    for command in ["login", "init"] {
        let out = Sandbox::new(&stub).run(&[command, "--json"]);
        assert_eq!(
            (out.status.code(), text(&out.stdout), text(&out.stderr)),
            (
                Some(0),
                login_summary("alpha", &stub, "acct-alpha", "verified", verified.clone()),
                "Using existing profile alpha. Run `vendo login --force` to sign in again.\n".to_string()
            ),
            "{command}"
        );
    }
    // The browser sign-in prints its URL on stderr.
    let sandbox = Sandbox::without_api_keys(&stub);
    let (code, stdout, stderr) = login_at_browser_json(sandbox.command(&["login", "--json"])).await;
    assert_eq!(
        (code, stdout, stderr),
        (
            Some(0),
            login_summary("demo-account", &stub, "acct-alpha", "verified", verified),
            browser_sign_in("No API key found. Starting browser login...", &stub)
        )
    );
    assert_eq!(sandbox.config()["profiles"]["demo-account"]["apiKey"], NEW_KEY);
    // A key that cannot be checked: the summary, then the error as JSON (exit 1, as before).
    let down = sign_in_stub(&[], 500).await;
    let out = Sandbox::new(&down.uri()).run(&["login", "--json"]);
    assert_eq!(
        (out.status.code(), text(&out.stdout)),
        (Some(1), login_summary("alpha", &down.uri(), "acct-alpha", "unverified", Value::Null))
    );
    assert!(text(&out.stderr).starts_with("Using existing profile alpha."), "{}", text(&out.stderr));
    let message = "Could not verify the API key: HTTP 500 Internal Server Error. Nothing was changed: check your connection (`vendo workspace`) and run `vendo login` again.";
    assert_eq!(json_error(&out), error_shape(message, Value::Null, Value::Null, Value::Null));
}

#[tokio::test]
async fn jobs_tail_json_prints_the_job_when_tailing_ends() {
    let server = MockServer::start().await;
    let job = format!("{V1}/jobs/{ID}");
    let running = json!({ "data": { "id": ID, "status": "running", "jobType": "import" } });
    let done = json!({ "data": { "id": ID, "status": "completed", "jobType": "import", "rowsProcessed": 12 } });
    let failed =
        json!({ "data": { "id": ID, "status": "failed", "jobType": "import", "errorMessage": "Rate limited" } });
    let get = || Mock::given(wiremock::matchers::method("GET")).and(path(job.clone()));
    let pretty = |body: &Value| format!("{}\n", serde_json::to_string_pretty(body).unwrap());
    get().respond_with(ResponseTemplate::new(200).set_body_json(&running)).up_to_n_times(1).mount(&server).await;
    get().respond_with(ResponseTemplate::new(200).set_body_json(&done)).up_to_n_times(1).mount(&server).await;
    let sandbox = Sandbox::new(&server.uri());
    // Nothing while it runs; the last response, as `jobs get --json` prints it, when it ends.
    let out = sandbox.run(&["jobs", "tail", ID, "--interval", "0.05", "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout), text(&out.stderr)), (Some(0), pretty(&done), String::new()));
    assert_eq!(sent(&server).await, vec![format!("GET {job}"); 2]);
    // A failed job is printed the same, and what the text says of it is the JSON error; exit 0, as before.
    get().respond_with(ResponseTemplate::new(200).set_body_json(&failed)).mount(&server).await;
    let out = sandbox.run(&["jobs", "tail", ID, "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), pretty(&failed)));
    let message = "Job 550e8400... failed: Rate limited";
    assert_eq!(json_error(&out), error_shape(message, Value::Null, Value::Null, Value::Null));
    assert_eq!(text(&sandbox.run(&["jobs", "tail", ID]).stderr), format!("Error: {message}\n"));

    // The latest job of a source, then that job.
    let server = MockServer::start().await;
    let latest = json!({ "data": [done["data"].clone()] });
    Mock::given(wiremock::matchers::method("GET"))
        .and(path(format!("{V1}/jobs")))
        .and(wiremock::matchers::query_param("source_id", "src-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(latest))
        .mount(&server)
        .await;
    serve(&server, "GET", &job, 200, done.clone()).await;
    let out = Sandbox::new(&server.uri()).run(&["jobs", "tail", "--source", "src-1", "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout), text(&out.stderr)), (Some(0), pretty(&done), String::new()));
}

#[cfg(unix)]
#[tokio::test]
async fn jobs_watch_json_prints_each_poll_that_changed_on_one_line_until_stopped() {
    use std::io::{BufRead, Read};
    let server = MockServer::start().await;
    let first = json!({ "data": [{ "id": ID, "status": "running" }], "meta": { "pagination": { "total": 1 } } });
    let second = json!({ "data": [], "meta": { "pagination": { "total": 0 } } });
    let down = json!({ "error": { "code": "INTERNAL", "message": "Jobs are down" } });
    let jobs = || Mock::given(wiremock::matchers::method("GET")).and(path(format!("{V1}/jobs")));
    // The same list twice (printed once), a failed poll, then a new list.
    jobs().respond_with(ResponseTemplate::new(200).set_body_json(&first)).up_to_n_times(2).mount(&server).await;
    jobs().respond_with(ResponseTemplate::new(500).set_body_json(down)).up_to_n_times(1).mount(&server).await;
    jobs().respond_with(ResponseTemplate::new(200).set_body_json(&second)).mount(&server).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut child = sandbox
        .command(&["jobs", "watch", "--interval", "0.05", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut lines = Vec::new();
    for _ in 0..2 {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        lines.push(line);
    }
    // Ctrl-C stops it, as before.
    assert!(Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap().success());
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!((out.status.code(), rest.as_str()), (Some(0), ""), "{}", text(&out.stderr));
    let parsed: Vec<Value> = lines.iter().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(parsed, [first, second]);
    assert!(lines.iter().all(|line| line.ends_with("}\n")), "{lines:?}");
    // The failed poll, as the JSON error, once.
    let stderr = text(&out.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    let mut error = json_error(&out);
    assert!(error["error"]["requestId"].as_str().unwrap().starts_with("cli-"), "{error}");
    error["error"]["requestId"] = Value::Null;
    assert_eq!(error, error_shape("Jobs are down", json!("INTERNAL"), json!(500), Value::Null));
}

#[test]
fn completions_json_carries_the_script_or_bare_the_set_up() {
    let sandbox = Sandbox::new(CLOSED);
    for shell in ["bash", "zsh", "fish"] {
        let script = ok_output(&sandbox.run(&["completions", shell]));
        let out = sandbox.run(&["completions", shell, "--json"]);
        let printed: Value = serde_json::from_str(&ok_output(&out)).unwrap();
        assert_eq!(printed, json!({ "shell": shell, "script": script }), "{shell}");
        assert_eq!(text(&out.stderr), "", "{shell}");
    }
    // Bare: on stdout, what the explanation says of the shell `$SHELL` names.
    for (shell, expected) in [
        (Some("/bin/zsh"), json!({ "shell": "zsh", "installed": false })),
        (Some("/bin/tcsh"), json!({ "shell": null, "installed": null })),
        (None, json!({ "shell": null, "installed": null })),
    ] {
        let mut cmd = sandbox.command(&["completions", "--json"]);
        match shell {
            Some(shell) => cmd.env("SHELL", shell),
            None => cmd.env_remove("SHELL"),
        };
        let out = cmd.output().unwrap();
        assert_eq!(serde_json::from_str::<Value>(&ok_output(&out)).unwrap(), expected, "{shell:?}");
        assert_eq!(text(&out.stderr), "", "{shell:?}");
    }
}

// ── install.sh's completions (VE-3830) ──────────────────────────────────────

/// Run install.sh's `install_completions` for bash in `sandbox`'s HOME, with this `vendo` as the installed
/// binary and `uname -s` answering `system`: the summary it prints. Only the installer's functions load
/// (its last line, `main "$@"`, is left off), so nothing is downloaded.
#[cfg(unix)]
fn install_bash_completions(sandbox: &Sandbox, system: &str) -> String {
    let installer = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../install.sh")).unwrap();
    let functions = installer.strip_suffix("main \"$@\"\n").expect("install.sh ends by running main");
    let script = format!(
        "{functions}\nuname() {{ printf '%s\\n' \"$FAKE_UNAME\"; }}\nINSTALL_PATH=\"$VENDO_BINARY\"\n\
         install_completions\nprintf '%s\\n' \"$COMPLETIONS_SUMMARY\"\n"
    );
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &script])
        .env("HOME", sandbox.home.path())
        .env("SHELL", "/bin/bash")
        .env("FAKE_UNAME", system)
        .env("VENDO_BINARY", env!("CARGO_BIN_EXE_vendo"))
        .env_remove("VENDO_INSTALL_COMPLETIONS")
        .stdin(Stdio::null());
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    text(&out.stdout)
}

#[cfg(unix)]
#[test]
fn the_installer_adds_bash_completions_where_bash_reads_them() {
    const MARKER: &str = "# >>> vendo completions >>>";
    let login_files = [".bash_profile", ".bash_login", ".profile"];
    // (system, login files there before, the file the block goes into besides ~/.bashrc).
    for (system, present, expected) in [
        // macOS Terminal opens login shells, which read the first of the login files that exists.
        ("Darwin", &[".bash_profile"][..], Some(".bash_profile")),
        ("Darwin", &[".profile"][..], Some(".profile")),
        ("Darwin", &[".bash_login"][..], Some(".bash_login")),
        ("Darwin", &[".bash_profile", ".profile"][..], Some(".bash_profile")),
        ("Darwin", &[".bash_login", ".profile"][..], Some(".bash_login")),
        // None there: ~/.bash_profile, created with the block.
        ("Darwin", &[][..], Some(".bash_profile")),
        // Linux: ~/.bashrc only, as before.
        ("Linux", &[][..], None),
        ("Linux", &[".profile"][..], None),
    ] {
        let case = format!("{system} with {present:?}");
        let sandbox = Sandbox::new(CLOSED);
        let home = sandbox.home.path();
        for file in present {
            std::fs::write(home.join(file), "export EDITOR=vi\n").unwrap();
        }
        let summary = install_bash_completions(&sandbox, system);
        let script = std::fs::read_to_string(home.join(".local/share/vendo/completions/vendo.bash")).unwrap();
        assert!(script.contains("complete -F"), "{case}: the script is saved");
        let read = |file: &str| std::fs::read_to_string(home.join(file)).ok();
        assert_eq!(read(".bashrc").unwrap().matches(MARKER).count(), 1, "{case}: ~/.bashrc as before");
        for file in login_files {
            let contents = read(file);
            if Some(file) == expected {
                let contents = contents.unwrap_or_else(|| panic!("{case}: {file} is written"));
                assert_eq!(contents.matches(MARKER).count(), 1, "{case}: {file}");
                if present.contains(&file) {
                    assert!(contents.starts_with("export EDITOR=vi\n"), "{case}: {file} keeps what it had");
                }
            } else if present.contains(&file) {
                assert_eq!(contents.as_deref(), Some("export EDITOR=vi\n"), "{case}: {file} is left alone");
            } else {
                assert_eq!(contents, None, "{case}: {file} is not created");
            }
        }
        let loaded_from = match expected {
            Some(file) => format!("~/.bashrc and ~/{file}"),
            None => "~/.bashrc".to_string(),
        };
        assert_eq!(summary, format!("bash, loaded from {loaded_from}\n"), "{case}");
        // A second run adds no second block.
        assert_eq!(install_bash_completions(&sandbox, system), summary, "{case}");
        for file in [".bashrc"].into_iter().chain(expected) {
            assert_eq!(read(file).unwrap().matches(MARKER).count(), 1, "{case}: {file} after a second run");
        }
        // `vendo completions` and doctor find them where the installer says they load from, on this system.
        if (system == "Darwin") == cfg!(target_os = "macos") {
            let out = sandbox.command(&["completions"]).env("SHELL", "/bin/bash").output().unwrap();
            let detail = format!("Bash completions are installed in {loaded_from}.\n");
            assert!(text(&out.stderr).contains(&detail), "{case}: {}", text(&out.stderr));
            let out = sandbox.command(&["completions", "--json"]).env("SHELL", "/bin/bash").output().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&ok_output(&out)).unwrap(),
                json!({ "shell": "bash", "installed": true })
            );
        }
    }
}

/// A fake `bash` first on PATH that runs `script` in place of `curl … | bash`.
#[cfg(unix)]
fn fake_installer(sandbox: &Sandbox, script: &str, args: &[&str]) -> Output {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let fake = bin.path().join("bash");
    std::fs::write(&fake, format!("#!/bin/sh\n{script}")).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap_or_default());
    sandbox.command(args).env("PATH", path).output().unwrap()
}

#[cfg(unix)]
#[test]
fn self_update_json_prints_the_versions_and_paths_and_the_installer_on_stderr() {
    let sandbox = Sandbox::new(CLOSED);
    let installs = "echo \"Installing vendo\"\nmkdir -p \"$HOME/.local/bin\"\nprintf '#!/bin/sh\\necho 9.9.9\\n' > \"$HOME/.local/bin/vendo\"\nchmod +x \"$HOME/.local/bin/vendo\"\n";
    let out = fake_installer(&sandbox, installs, &["self-update", "--json"]);
    let install_path = sandbox.home.path().join(".local/bin/vendo");
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_vendo")).unwrap();
    let expected = json!({
        "previousVersion": env!("CARGO_PKG_VERSION"),
        "version": "9.9.9",
        "installPath": install_path.display().to_string(),
        "binaryPath": binary.display().to_string(),
    });
    assert_eq!(ok_output(&out), format!("{}\n", serde_json::to_string_pretty(&expected).unwrap()));
    assert_eq!(text(&out.stderr), "Installing vendo\n");
    // Without --json, as before.
    let out = fake_installer(&sandbox, installs, &["self-update"]);
    assert!(ok_output(&out).starts_with("Installing vendo\nDone: Vendo CLI updated.\n"), "{}", text(&out.stdout));
    // A failed installer: its exit code, as before, and the JSON error last on stderr.
    let out = fake_installer(&sandbox, "echo 'download failed' >&2\nexit 3\n", &["self-update", "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(3), String::new()));
    assert!(text(&out.stderr).starts_with("download failed\n"), "{}", text(&out.stderr));
    let error = error_shape("The installer exited with code 3.", Value::Null, Value::Null, Value::Null);
    assert_eq!(json_error(&out), error);
    // A version the installed binary does not print is null.
    let out = fake_installer(&sandbox, "rm -f \"$HOME/.local/bin/vendo\"\n", &["self-update", "--json"]);
    assert_eq!(serde_json::from_str::<Value>(&ok_output(&out)).unwrap()["version"], Value::Null);
}

// ── VENDO_PROFILE ──

async fn me_stub() -> MockServer {
    let server = MockServer::start().await;
    for account in ["acct-alpha", "acct-beta"] {
        Mock::given(path("/api/v1/me"))
            .and(wiremock::matchers::header("X-Account-Id", account))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "accountId": account } })))
            .mount(&server)
            .await;
    }
    server
}

fn workspace_config(sandbox: &Sandbox, args: &[&str], vendo_profile: Option<&str>) -> Value {
    let mut cmd = sandbox.command(&[args, &["workspace", "--json"]].concat());
    if let Some(name) = vendo_profile {
        cmd.env("VENDO_PROFILE", name);
    }
    let out = cmd.output().unwrap();
    assert_eq!(text(&out.stderr), "");
    let out: Value = serde_json::from_slice(&out.stdout).unwrap();
    json!([out["data"]["accountId"], out["config"]["selectedProfile"]])
}

#[tokio::test]
async fn vendo_profile_selects_the_profile_and_the_flag_overrides_it() {
    let server = me_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    assert_eq!(workspace_config(&sandbox, &[], None), json!(["acct-alpha", "alpha"]));
    assert_eq!(workspace_config(&sandbox, &[], Some("beta")), json!(["acct-beta", "beta"]));
    assert_eq!(workspace_config(&sandbox, &["--profile", "alpha"], Some("beta")), json!(["acct-alpha", "alpha"]));
    // Empty is unset.
    assert_eq!(workspace_config(&sandbox, &[], Some("")), json!(["acct-alpha", "alpha"]));
    // The profile commands act on it, as with --profile; the saved active profile stays.
    let out = sandbox.command(&["profile", "list"]).env("VENDO_PROFILE", "beta").output().unwrap();
    assert_eq!(
        cells(ok_output(&out).as_bytes()),
        rows(&[&["alpha", "acct-alpha", &server.uri()], &["* beta (active)", "acct-beta", &server.uri()]])
    );
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[tokio::test]
async fn workspace_shows_the_profile_vendo_profile_selects_as_it_shows_the_flag() {
    // It names the profile as it does for --profile and activeProfile; under the profile list it
    // says that VENDO_PROFILE overrides the active profile (Yalcin, 2026-10-06).
    let server = me_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let shown = |cmd: &mut Command| {
        let out = cmd.output().unwrap();
        assert_eq!(text(&out.stderr), "");
        text(&out.stdout)
    };
    let printed = shown(workspace(&sandbox, &["workspace"]).env("VENDO_PROFILE", "beta"));
    assert!(printed.contains("  Profile:     beta\n"), "{printed}");
    let note = "  VENDO_PROFILE=beta overrides the active profile in this shell: change or unset VENDO_PROFILE to switch here.\n";
    let flag = shown(&mut workspace(&sandbox, &["--profile", "beta", "workspace"]));
    assert!(flag.contains("\n  * beta "), "{flag}");
    assert_eq!(printed, flag.replace("\n\nChecks\n", &format!("\n{note}\nChecks\n")));
    // --profile wins over VENDO_PROFILE, so the note goes.
    let both = shown(workspace(&sandbox, &["--profile", "beta", "workspace"]).env("VENDO_PROFILE", "alpha"));
    assert_eq!(both, flag);

    let doctor = |vendo_profile: Option<&str>, args: &[&str]| {
        let mut cmd = sandbox.command(&[args, &["workspace", "--json"]].concat());
        if let Some(name) = vendo_profile {
            cmd.env("VENDO_PROFILE", name);
        }
        let out: Value = serde_json::from_str(&text(&cmd.output().unwrap().stdout)).unwrap();
        let check = out["checks"].as_array().unwrap().iter().find(|c| c["name"] == "Selected profile").unwrap().clone();
        (
            check["status"].as_str().unwrap().to_string(),
            check["detail"].as_str().unwrap().to_string(),
            check["remediation"].clone(),
        )
    };
    assert_eq!(doctor(None, &[]), ("ok".into(), "alpha".into(), Value::Null));
    assert_eq!(doctor(Some("beta"), &[]), ("ok".into(), "beta".into(), Value::Null));
    assert_eq!(doctor(Some("beta"), &["--profile", "alpha"]), ("ok".into(), "alpha".into(), Value::Null));
    let fix = "VENDO_PROFILE=nope overrides the active profile in this shell: run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.";
    assert_eq!(doctor(Some("nope"), &[]), ("warn".into(), "nope (not found in config)".into(), json!(fix)));
    // An unknown --profile is as before.
    let switch = "Run `vendo profile switch` to switch profiles, or `vendo login` to create one.";
    assert_eq!(
        doctor(None, &["--profile", "nope"]),
        ("warn".into(), "nope (not found in config)".into(), json!(switch))
    );
}

#[tokio::test]
async fn an_unknown_vendo_profile_is_an_error_that_names_it_and_says_how_to_fix_it() {
    let server = me_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let unknown = "Profile \"nope\" not found (VENDO_PROFILE selects it).\n  Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.";
    let saved = sandbox.config();
    // logout too, where it said "Not currently logged in." while the active profile was logged in.
    for args in [&["apps", "list"][..], &["jobs", "get", "1a2b3c4d"], &["logout"]] {
        let out = sandbox.command(args).env("VENDO_PROFILE", "nope").output().unwrap();
        assert_eq!(
            (out.status.code(), text(&out.stdout), text(&out.stderr)),
            (Some(1), String::new(), format!("Error: {unknown}\n")),
            "{args:?}"
        );
    }
    for args in [&["apps", "list", "--json"][..], &["logout", "--json"]] {
        let out = sandbox.command(args).env("VENDO_PROFILE", "nope").output().unwrap();
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()), "{args:?}");
        assert_eq!(json_error(&out), error_shape(unknown, Value::Null, Value::Null, Value::Null), "{args:?}");
    }
    assert_eq!(sandbox.config(), saved);
    // `vendo workspace` (whoami's place, VE-3891) shows what it can instead, as doctor did: the
    // profile's check, a warning, names it and says how to fix it, and with no key the key's check
    // fails, exit 1. That VENDO_PROFILE overrides the active profile is said once, under the profile
    // list, so the fix under the check leaves it out (VE-3891 review: each fact once).
    let out = workspace(&sandbox, &["workspace"]).env("VENDO_PROFILE", "nope").output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    let shown = text(&out.stdout);
    assert!(
        shown.starts_with("  Profile:     nope\n  Base URL:    https://app2.vendodata.com\n\nProfiles\n"),
        "{shown}"
    );
    assert!(
        shown.contains("\n  VENDO_PROFILE=nope overrides the active profile in this shell: change or unset VENDO_PROFILE to switch here.\n\nChecks\n"),
        "{shown}"
    );
    assert!(
        shown.contains("\n  [warn] Selected profile: nope (not found in config)\n         Fix: Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.\n  [fail] API key: Missing\n"),
        "{shown}"
    );
    assert_eq!(shown.matches("overrides the active profile").count(), 1, "{shown}");
    assert!(server.received_requests().await.unwrap().is_empty());
    // mcp prints the client config as ever; its hint names VENDO_PROFILE where it said there is no key.
    let out = sandbox.command(&["mcp"]).env("VENDO_PROFILE", "nope").output().unwrap();
    let no_key_hint = "  No API key configured — run `vendo login` or set VENDO_API_KEY first.\n";
    let flag = ok_output(&sandbox.run(&["--profile", "nope", "mcp"]));
    assert!(flag.contains(no_key_hint), "{flag}");
    assert_eq!(ok_output(&out), flag.replace(no_key_hint, &format!("  {unknown}\n")));
    // An unknown --profile is as before, also over VENDO_PROFILE.
    let no_key =
        "Error: No API key configured. Run `vendo login` or `vendo profile set --api-key <key>` or set VENDO_API_KEY.";
    for (args, vendo_profile) in
        [(&["--profile", "nope", "apps", "list"][..], None), (&["--profile", "nope", "apps", "list"], Some("beta"))]
    {
        let mut cmd = sandbox.command(args);
        if let Some(name) = vendo_profile {
            cmd.env("VENDO_PROFILE", name);
        }
        let out = cmd.output().unwrap();
        assert_eq!((out.status.code(), stderr_line(&out)), (Some(1), no_key.to_string()), "{args:?}");
    }
    let out = sandbox.command(&["--profile", "nope", "logout"]).env("VENDO_PROFILE", "beta").output().unwrap();
    assert_eq!(
        (out.status.code(), text(&out.stdout), text(&out.stderr)),
        (Some(0), String::new(), "Error: Not currently logged in.\n".to_string())
    );
    // profile switch needs no key: VENDO_PROFILE=nope is --profile nope there.
    let flag = sandbox.run(&["--profile", "nope", "profile", "switch"]);
    let env = sandbox.command(&["profile", "switch"]).env("VENDO_PROFILE", "nope").output().unwrap();
    assert_eq!((env.status.code(), &env.stdout, &env.stderr), (flag.status.code(), &flag.stdout, &flag.stderr));
    assert_eq!(sandbox.config(), saved);
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[test]
fn vendo_profile_never_changes_the_saved_active_profile() {
    // Decided by Yalcin, 2026-10-06: profile set and logout act on the profile VENDO_PROFILE names and
    // leave `activeProfile` as saved. --profile is unchanged: it makes the profile it sets active, and
    // logging out of it clears `activeProfile`.
    let sandbox = Sandbox::new("https://example.test");
    let with_env = |args: &[&str], name: &str| sandbox.command(args).env("VENDO_PROFILE", name).output().unwrap();
    ok_output(&with_env(&["profile", "set", "--account", "acct-b2"], "beta"));
    let config = sandbox.config();
    assert_eq!(
        (&config["profiles"]["beta"]["accountId"], &config["activeProfile"]),
        (&json!("acct-b2"), &json!("alpha"))
    );
    // A profile it names that does not exist yet is created, still without becoming active.
    let printed =
        ok_output(&with_env(&["profile", "set", "--api-key", GAMMA_KEY, "--account", "acct-gamma", "--json"], "gamma"));
    let set: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!((&set["profile"]["name"], &set["profile"]["active"]), (&json!("gamma"), &json!(true)));
    let config = sandbox.config();
    assert_eq!(
        (&config["profiles"]["gamma"], &config["activeProfile"]),
        (&json!({ "apiKey": GAMMA_KEY, "accountId": "acct-gamma" }), &json!("alpha"))
    );
    assert_eq!(ok_output(&with_env(&["logout"], "gamma")), "Done: Logged out of profile \"gamma\".\n");
    let config = sandbox.config();
    assert_eq!((config["profiles"].get("gamma"), &config["activeProfile"]), (None, &json!("alpha")));
    // Logging out of the active profile through VENDO_PROFILE leaves `activeProfile` as saved too.
    ok_output(&with_env(&["logout"], "alpha"));
    let config = sandbox.config();
    assert_eq!((config["profiles"].get("alpha"), &config["activeProfile"]), (None, &json!("alpha")));

    // --profile, as before.
    ok_output(&sandbox.run(&["--profile", "beta", "profile", "set", "--account", "acct-b3"]));
    assert_eq!(sandbox.config()["activeProfile"], "beta");
    ok_output(&sandbox.run(&["--profile", "beta", "logout"]));
    assert_eq!(sandbox.config(), json!({ "profiles": {} }));
}

#[test]
fn profile_switch_under_vendo_profile_switches_and_says_vendo_profile_still_overrides_it() {
    // An explicit request: it changes the saved active profile, then notes the override (Yalcin, 2026-10-06).
    let sandbox = Sandbox::new("https://example.test");
    let out = sandbox.command(&["profile", "switch", "beta"]).env("VENDO_PROFILE", "alpha").output().unwrap();
    assert_eq!(
        ok_output(&out),
        "Done: Switched to profile beta.\n  Account ID: acct-beta  Base URL: https://example.test\n  VENDO_PROFILE=alpha still overrides it in this shell: unset VENDO_PROFILE to use beta here.\n  Verify with `vendo workspace`.\n"
    );
    assert_eq!(sandbox.config()["activeProfile"], "beta");
    // With --json the profile says it is not the one in use; nothing else is printed.
    let out = sandbox.command(&["profile", "switch", "alpha", "--json"]).env("VENDO_PROFILE", "beta").output().unwrap();
    let switched: Value = serde_json::from_str(&ok_output(&out)).unwrap();
    assert_eq!(
        (&switched["profile"]["name"], &switched["profile"]["active"], text(&out.stderr)),
        (&json!("alpha"), &json!(false), String::new())
    );
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
    // Switching to the profile VENDO_PROFILE names: nothing overrides it, so no note.
    let out = sandbox.command(&["profile", "switch", "beta"]).env("VENDO_PROFILE", "beta").output().unwrap();
    assert_eq!(
        ok_output(&out),
        "Done: Switched to profile beta.\n  Account ID: acct-beta  Base URL: https://example.test\n  Verify with `vendo workspace`.\n"
    );
}

#[tokio::test]
async fn login_under_vendo_profile_saves_its_profile_without_making_it_active_and_says_so() {
    let server = sign_in_stub(&[GAMMA_KEY, NEW_KEY], 401).await;
    let stub = server.uri();
    let note = "Profile demo-account was saved but not made active: VENDO_PROFILE=beta overrides the active profile in this shell. Use it with VENDO_PROFILE=demo-account.\n";
    let summary = |account_id: &str| {
        signed_in("demo-account", &stub, account_id).replace("\n\nNext steps", &format!("\n\n{note}\nNext steps"))
    };
    // Headless.
    let sandbox = Sandbox::new(&stub);
    let headless = ["login", "--api-key", GAMMA_KEY, "--account", "acct-gamma", "--base-url", stub.as_str()];
    let out = sandbox.command(&headless).env("VENDO_PROFILE", "beta").output().unwrap();
    assert_eq!((ok_output(&out), text(&out.stderr)), (summary("acct-gamma"), String::new()));
    let config = sandbox.config();
    assert_eq!(
        (&config["profiles"]["demo-account"]["apiKey"], &config["activeProfile"]),
        (&json!(GAMMA_KEY), &json!("alpha"))
    );
    // With --json the note goes to stderr with the rest of what login says.
    let out = sandbox.command(&[&headless[..], &["--json"]].concat()).env("VENDO_PROFILE", "beta").output().unwrap();
    let verified = json!("Demo Account (synthetic)");
    assert_eq!(
        (ok_output(&out), text(&out.stderr)),
        (login_summary("demo-account", &stub, "acct-gamma", "verified", verified), note.to_string())
    );
    // Through the browser, for a VENDO_PROFILE no profile has yet.
    let sandbox = Sandbox::new(&stub);
    let mut login = sandbox.command(&["login", "--base-url", &stub]);
    login.env("VENDO_PROFILE", "fresh");
    let (code, stdout, stderr) = login_at_browser(login).await;
    let note_fresh = note.replace("VENDO_PROFILE=beta", "VENDO_PROFILE=fresh");
    let expected = browser_sign_in("No API key found. Starting browser login...", &stub)
        + &signed_in("demo-account", &stub, "acct-alpha")
            .replace("\n\nNext steps", &format!("\n\n{note_fresh}\nNext steps"));
    assert_eq!((code, stdout, stderr), (Some(0), expected, String::new()));
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
    // --profile, as before: the saved profile becomes active, and nothing is noted.
    let sandbox = Sandbox::new(&stub);
    let out = sandbox
        .command(&[&["--profile", "beta"][..], &headless].concat())
        .env("VENDO_PROFILE", "alpha")
        .output()
        .unwrap();
    assert_eq!(ok_output(&out), signed_in("demo-account", &stub, "acct-gamma"));
    assert_eq!(sandbox.config()["activeProfile"], "demo-account");
}

#[tokio::test]
async fn a_login_under_vendo_profile_to_the_saved_active_profile_does_not_say_it_was_not_made_active() {
    let server = sign_in_stub(&[GAMMA_KEY], 401).await;
    let stub = server.uri();
    let sandbox = Sandbox::new(&stub);
    let headless = ["login", "--api-key", GAMMA_KEY, "--account", "acct-gamma", "--base-url", stub.as_str()];
    let summary = signed_in("demo-account", &stub, "acct-gamma");
    // A plain login makes demo-account the saved active profile.
    assert_eq!(ok_output(&sandbox.run(&headless)), summary);
    assert_eq!(sandbox.config()["activeProfile"], "demo-account");
    // Again while VENDO_PROFILE names another: the profile is the active one, which VENDO_PROFILE overrides here.
    let note = "Profile demo-account was saved and is the active profile, but VENDO_PROFILE=beta overrides the active profile in this shell: unset VENDO_PROFILE to use demo-account here.\n";
    let out = sandbox.command(&headless).env("VENDO_PROFILE", "beta").output().unwrap();
    assert_eq!(
        (ok_output(&out), text(&out.stderr)),
        (summary.replace("\n\nNext steps", &format!("\n\n{note}\nNext steps")), String::new())
    );
    let out = sandbox.command(&[&headless[..], &["--json"]].concat()).env("VENDO_PROFILE", "beta").output().unwrap();
    let verified = json!("Demo Account (synthetic)");
    assert_eq!(
        (ok_output(&out), text(&out.stderr)),
        (login_summary("demo-account", &stub, "acct-gamma", "verified", verified), note.to_string())
    );
    // VENDO_PROFILE names it too: it is active and in use, so there is nothing to note.
    for args in [&headless[..], &[&headless[..], &["--json"]].concat()] {
        let out = sandbox.command(args).env("VENDO_PROFILE", "demo-account").output().unwrap();
        assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), String::new()), "{args:?}");
    }
    let out = sandbox.command(&headless).env("VENDO_PROFILE", "demo-account").output().unwrap();
    assert_eq!(ok_output(&out), summary);
    assert_eq!(sandbox.config()["activeProfile"], "demo-account");
}

#[tokio::test]
async fn workspace_fixes_for_a_rejected_key_under_vendo_profile_say_it_overrides_the_active_profile() {
    // Following "run `vendo login`, then retry `vendo workspace`" under VENDO_PROFILE checks the old key again:
    // login saves the new one in a profile it does not make active (Yalcin, 2026-10-06).
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/v1/me", 401, json!({ "error": { "message": "Invalid API key" } })).await;
    let sandbox = Sandbox::new(&server.uri());
    let auth_fix = |cmd: &mut Command| {
        let out: Value = serde_json::from_str(&text(&cmd.output().unwrap().stdout)).unwrap();
        out["checks"].as_array().unwrap().iter().find(|c| c["name"] == "API auth").unwrap()["remediation"].clone()
    };
    let fix = "Run `vendo login` to refresh credentials, then retry `vendo --profile <profile> workspace` with the profile it saved (VENDO_PROFILE=beta overrides the active profile in this shell).";
    assert_eq!(auth_fix(sandbox.command(&["workspace", "--json"]).env("VENDO_PROFILE", "beta")), json!(fix));
    let out = sandbox.command(&["workspace"]).env("VENDO_PROFILE", "beta").output().unwrap();
    let printed = text(&out.stdout);
    // On the screen the profile list says once that VENDO_PROFILE overrides the active profile, so the
    // fix under the check leaves it out (VE-3891 review: each fact once). The screen has no list of next
    // steps that repeats the fixes.
    let screen_fix = "Run `vendo login` to refresh credentials, then retry `vendo --profile <profile> workspace` with the profile it saved.";
    assert!(
        printed.contains(&format!("\n  [fail] API auth: HTTP 401: Unauthorized\n         Fix: {screen_fix}\n")),
        "{printed}"
    );
    assert!(
        printed.contains("\n  VENDO_PROFILE=beta overrides the active profile in this shell: change or unset VENDO_PROFILE to switch here.\n\nChecks\n"),
        "{printed}"
    );
    assert_eq!(printed.matches("overrides the active profile").count(), 1, "{printed}");
    assert_eq!(printed.matches(screen_fix).count(), 1, "{printed}");
    // Without VENDO_PROFILE, or with --profile over it, as before.
    let plain = json!("Run `vendo login` to refresh credentials, then retry `vendo workspace`.");
    assert_eq!(auth_fix(&mut sandbox.command(&["workspace", "--json"])), plain);
    assert_eq!(
        auth_fix(sandbox.command(&["--profile", "beta", "workspace", "--json"]).env("VENDO_PROFILE", "alpha")),
        plain
    );
    // With one profile the screen lists no profiles, so the fix says it, once.
    let mut config = sandbox.config();
    config["profiles"].as_object_mut().unwrap().remove("alpha");
    std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
    let out = sandbox.command(&["workspace"]).env("VENDO_PROFILE", "beta").output().unwrap();
    let alone = text(&out.stdout);
    assert!(!alone.contains("\nProfiles\n"), "{alone}");
    assert!(alone.contains(&format!("\n  [fail] API auth: HTTP 401: Unauthorized\n         Fix: {fix}\n")), "{alone}");
    assert_eq!(alone.matches("overrides the active profile").count(), 1, "{alone}");
}

#[cfg(unix)]
#[tokio::test]
async fn an_unknown_vendo_profile_is_a_warning_on_the_workspace_screen_and_a_key_from_the_env_still_counts() {
    // The profile's check is a warning, so with VENDO_API_KEY and VENDO_ACCOUNT_ID, which are still used,
    // every other check passes and it exits 0: exit 1 comes from a check that fails, as the key's does
    // without them (VE-3891 review).
    let server = MockServer::start().await;
    let me = json!({ "data": { "accountId": "acct-env", "accountName": "Env Account" } });
    serve(&server, "GET", "/api/v1/me", 200, me).await;
    let stub = server.uri();
    let installed = Installed::new(json!({
        "profiles": {
            "alpha": { "apiKey": "vendo_sk_fake_alpha_0000", "accountId": "acct-alpha", "baseUrl": stub },
            "beta": { "apiKey": "vendo_sk_fake_beta_00000", "accountId": "acct-beta", "baseUrl": stub },
        },
        "activeProfile": "alpha",
    }));
    let run = |words: &[&str]| {
        let mut cmd = installed.command(words);
        cmd.env("VENDO_PROFILE", "ghost").env("VENDO_API_URL", &stub);
        cmd.env("VENDO_API_KEY", "vendo_sk_fake_env_00000").env("VENDO_ACCOUNT_ID", "acct-env");
        cmd.output().unwrap()
    };
    let out = run(&["workspace"]);
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), String::new()));
    let shown = text(&out.stdout);
    assert!(
        shown.contains("\n  [warn] Selected profile: ghost (not found in config)\n         Fix: Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile.\n  [ok] Zsh completions installed\n  [ok] Signed in as Env Account\n"),
        "{shown}"
    );
    assert_eq!(shown.matches("overrides the active profile").count(), 1, "{shown}");
    let out = run(&["workspace", "--json"]);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((out.status.code(), &report["summary"]), (Some(0), &json!({ "ok": 8, "warn": 1, "fail": 0 })));
}

#[tokio::test]
async fn a_login_whose_new_key_cannot_be_checked_under_vendo_profile_says_how_to_check_it() {
    // The new key is checked with `/me`; when that fails, the error says how to check that profile again,
    // which `vendo workspace` alone would not while VENDO_PROFILE names another.
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/v1/me", 503, json!({ "error": { "message": "Down for the test" } })).await;
    mount_sign_in_page(&server).await;
    let stub = server.uri();
    let sandbox = Sandbox::without_api_keys(&stub);
    let mut login = sandbox.command(&["login"]);
    login.env("VENDO_PROFILE", "beta");
    let (code, _, stderr) = login_at_browser(login).await;
    assert_eq!(code, Some(1));
    assert!(
        stderr.ends_with(". The new key is saved in profile demo-account: run `vendo --profile demo-account workspace` to check it again (VENDO_PROFILE=beta overrides the active profile in this shell).\n"),
        "{stderr}"
    );
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

// ── short IDs ──

const V1: &str = "/api/v1/accounts/acct-alpha";

/// An ID whose first 8 characters are `prefix`.
fn uuid_with(prefix: &str, n: u64) -> String {
    format!("{prefix}-0000-4000-8000-{n:012x}")
}

/// A v1 list page as the API sends it.
fn list_page(ids: &[String], offset: usize, has_more: bool) -> Value {
    page_of(ids.iter().map(|id| json!({ "id": id })).collect(), offset, has_more)
}

/// A v1 list page of `items`.
fn page_of(items: Vec<Value>, offset: usize, has_more: bool) -> Value {
    let total = offset + items.len() + usize::from(has_more);
    json!({ "data": items, "meta": { "pagination": { "total": total, "limit": 100, "offset": offset, "hasMore": has_more } } })
}

/// The `n`th row of `resource`'s list (the API's path name), with the columns its table names it by.
fn listed_row(resource: &str, id: &str, n: u64) -> Value {
    match resource {
        "apps" => json!({ "id": id, "displayName": format!("Demo App {n}"), "appType": "shopify" }),
        "sources" => json!({ "id": id, "appName": format!("Demo App {n}"), "syncType": "shopify" }),
        "connections" => json!({
            "id": id, "sourceAppName": format!("Demo App {n}"), "destinationAppName": "Demo Warehouse",
            "dataType": "orders",
        }),
        "jobs" if n.is_multiple_of(2) => {
            json!({ "id": id, "jobType": "import", "connectorType": "shopify", "status": "failed" })
        }
        "jobs" => json!({ "id": id, "jobType": "export", "connectorType": "bigquery", "status": "completed" }),
        _ => json!({ "id": id, "name": format!("Demo {resource} {n}") }),
    }
}

/// Lists for every resource whose table shows short IDs: one ID starting `1a2b3c4d` and two
/// starting `5e6f7a8b`, each row named as its table names it ([`listed_row`]). Every other request
/// is answered `{ data: {} }`.
async fn short_id_stub() -> MockServer {
    let server = MockServer::start().await;
    let ids = [uuid_with("1a2b3c4d", 1), uuid_with("5e6f7a8b", 2), uuid_with("5e6f7a8b", 3)];
    let rows =
        |resource: &str| -> Vec<Value> { ids.iter().zip(1..).map(|(id, n)| listed_row(resource, id, n)).collect() };
    for resource in ["apps", "sources", "connections", "jobs", "models"] {
        Mock::given(wiremock::matchers::method("GET"))
            .and(path(format!("{V1}/{resource}")))
            .and(wiremock::matchers::query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page_of(rows(resource), 0, false)))
            .mount(&server)
            .await;
    }
    let metrics = rows("metrics");
    Mock::given(wiremock::matchers::method("GET"))
        .and(path("/api/metrics"))
        .and(wiremock::matchers::query_param("limit", "100"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "metrics": metrics, "total": 3, "limit": 100, "offset": 0 })),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
        .mount(&server)
        .await;
    server
}

fn listed(resource: &str) -> String {
    format!("GET {V1}/{resource}?limit=100&offset=0")
}

#[tokio::test]
async fn a_short_id_that_one_resource_starts_with_is_that_resource() {
    let full = uuid_with("1a2b3c4d", 1);
    for (args, requests) in [
        (vec!["apps", "get", "1a2b3c4d"], vec![listed("apps"), format!("GET {V1}/apps/{full}")]),
        (vec!["apps", "pause", "1a2b3c4d"], vec![listed("apps"), format!("POST {V1}/apps/{full}/pause")]),
        (vec!["sources", "get", "1a2b3c4d", "--json"], vec![listed("sources"), format!("GET {V1}/sources/{full}")]),
        (
            vec!["destinations", "resume", "1A2B3C4D"],
            vec![listed("connections"), format!("POST {V1}/connections/{full}/resume")],
        ),
        (vec!["jobs", "get", "1a2b3c4d"], vec![listed("jobs"), format!("GET {V1}/jobs/{full}")]),
        (vec!["models", "get", "1a2b3c4d"], vec![listed("models"), format!("GET {V1}/models/{full}")]),
        (
            vec!["metrics", "get", "1a2b3c4d"],
            vec!["GET /api/metrics?limit=100&offset=0".to_string(), format!("GET /api/metrics/{full}")],
        ),
        // IDs in flags too: a filter, and the app a new source belongs to.
        (
            vec!["jobs", "list", "--source", "1a2b3c4d", "--json"],
            vec![listed("sources"), format!("GET {V1}/jobs?source_id={full}&limit=20&offset=0")],
        ),
        (
            vec!["sources", "create", "--app", "1a2b3c4d", "--sync-type", "shopify", "--json"],
            vec![
                listed("apps"),
                format!(
                    "POST {V1}/sources {{\"appId\":\"{full}\",\"syncType\":\"shopify\",\"syncFrequencyValue\":24,\"syncFrequencyUnit\":\"hours\",\"runNow\":false}}"
                ),
            ],
        ),
    ] {
        let server = short_id_stub().await;
        let out = Sandbox::new(&server.uri()).run(&args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out.stderr));
        assert_eq!(sent(&server).await, requests, "{args:?}");
    }
    // The other commands that take these IDs: one lookup, first, then only the full ID goes out.
    for (args, listing) in [
        (&["apps", "update", "1a2b3c4d", "--name", "New"][..], listed("apps")),
        (&["apps", "update", "1a2b3c4d", "--role", "source", "--json"], listed("apps")),
        (&["sources", "update", "1a2b3c4d", "--frequency", "2"], listed("sources")),
        (&["sources", "sync", "1a2b3c4d", "--json"], listed("sources")),
        (&["sources", "sync", "1a2b3c4d", "--dry-run"], listed("sources")),
        (&["sources", "list", "--app", "1a2b3c4d", "--json"], listed("apps")),
        (&["destinations", "get", "1a2b3c4d", "--json"], listed("connections")),
        (&["destinations", "update", "1a2b3c4d", "--frequency", "2"], listed("connections")),
        (&["destinations", "refresh-source", "1a2b3c4d", "--json"], listed("connections")),
        (&["jobs", "list", "--integration", "1a2b3c4d"], listed("connections")),
        (&["metrics", "update", "1a2b3c4d", "--name", "New"], "GET /api/metrics?limit=100&offset=0".to_string()),
        (&["metrics", "activate", "1a2b3c4d"], "GET /api/metrics?limit=100&offset=0".to_string()),
    ] {
        let server = short_id_stub().await;
        let out = Sandbox::new(&server.uri()).run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out.stderr));
        let requests = sent(&server).await;
        assert_eq!(requests[0], listing, "{args:?}");
        assert!(requests.len() > 1, "{args:?}: {requests:?}");
        for request in &requests[1..] {
            assert!(!request.contains("limit=100"), "{args:?}: one lookup: {requests:?}");
            assert!(!request.replace(&full, "").contains("1a2b3c4d"), "{args:?}: the short ID went out: {request}");
        }
        assert!(requests[1..].iter().any(|r| r.contains(&full)), "{args:?}: {requests:?}");
    }
}

#[tokio::test]
async fn a_short_id_that_matches_nothing_or_whose_list_fails_is_sent_as_typed() {
    // The API answers as it did before short IDs: not found.
    for short in ["99999999", "99999999..."] {
        let server = short_id_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        assert_eq!(sandbox.run(&["apps", "get", short]).status.code(), Some(0), "{short}");
        assert_eq!(sent(&server).await, [listed("apps"), format!("GET {V1}/apps/{short}")], "{short}");
    }
    // A listing that fails changes nothing either: the request goes out as typed and reports its own error.
    let server = MockServer::start().await;
    serve(&server, "GET", &format!("{V1}/apps"), 500, json!({ "error": { "message": "Listing is down" } })).await;
    serve(&server, "GET", &format!("{V1}/apps/1a2b3c4d"), 404, json!({ "error": { "message": "App not found" } }))
        .await;
    let out = Sandbox::new(&server.uri()).run(&["apps", "get", "1a2b3c4d"]);
    assert_api_error(&out, "App not found");
    assert_eq!(sent(&server).await, [listed("apps"), format!("GET {V1}/apps/1a2b3c4d")]);
}

/// Drop a stub once the test's task has had its turn back from tokio. Dropping a `MockServer`
/// reads its state with `futures::executor::block_on`, whose wake-up tokio defers to its scheduler
/// once the task has spent its cooperative budget; a loop of stubs, mounts and lookups in one poll
/// can spend it, and the drop then waits forever. `yield_now` hands the task back to the
/// scheduler, which polls it again with a fresh budget.
async fn release(server: MockServer) {
    tokio::task::yield_now().await;
    drop(server);
}

/// The refusal of a short ID that `ids` start with, each shown with `labels` (VE-3831).
fn ambiguous(short: &str, plural: &str, ids: &[String], labels: &[&str]) -> String {
    let lines: Vec<String> = ids.iter().zip(labels).map(|(id, label)| format!("\n  {id}  {label}")).collect();
    format!("Short ID {short} matches {} {plural}:{}\nUse the full ID.", ids.len(), lines.concat())
}

#[tokio::test]
async fn a_short_id_that_several_ids_start_with_is_refused_with_the_matches() {
    // Decided by Yalcin, 2026-10-06: exit 1 before the command's request, naming each match by its full
    // ID and what its table shows of it, where sending it as typed made the API say "not found".
    let ids = [uuid_with("5e6f7a8b", 2), uuid_with("5e6f7a8b", 3)];
    let metrics_listed = "GET /api/metrics?limit=100&offset=0".to_string();
    for (args, plural, listing, labels) in [
        (&["apps", "get", "5e6f7a8b"][..], "apps", listed("apps"), ["Demo App 2  shopify", "Demo App 3  shopify"]),
        (&["sources", "get", "5e6f7a8b"], "sources", listed("sources"), ["Demo App 2  shopify", "Demo App 3  shopify"]),
        (
            &["destinations", "sync", "5E6F7A8B"],
            "destinations",
            listed("connections"),
            ["Demo App 2 → Demo Warehouse  orders", "Demo App 3 → Demo Warehouse  orders"],
        ),
        (
            &["jobs", "get", "5e6f7a8b"],
            "jobs",
            listed("jobs"),
            ["import  shopify  failed", "export  bigquery  completed"],
        ),
        (&["models", "get", "5e6f7a8b"], "models", listed("models"), ["Demo models 2", "Demo models 3"]),
        // Several in the default metrics list: the archived list is not read.
        (&["metrics", "get", "5e6f7a8b"], "metrics", metrics_listed.clone(), ["Demo metrics 2", "Demo metrics 3"]),
        // IDs in flags too.
        (
            &["jobs", "list", "--source", "5e6f7a8b"],
            "sources",
            listed("sources"),
            ["Demo App 2  shopify", "Demo App 3  shopify"],
        ),
        (
            &["sources", "create", "--app", "5e6f7a8b", "--sync-type", "shopify"],
            "apps",
            listed("apps"),
            ["Demo App 2  shopify", "Demo App 3  shopify"],
        ),
        // After a delete's consent, before the delete.
        (
            &["apps", "delete", "5e6f7a8b", "--yes"],
            "apps",
            listed("apps"),
            ["Demo App 2  shopify", "Demo App 3  shopify"],
        ),
        // As the table prints it, dots and all.
        (&["models", "get", "5e6f7a8b..."], "models", listed("models"), ["Demo models 2", "Demo models 3"]),
    ] {
        let server = short_id_stub().await;
        let out = Sandbox::new(&server.uri()).run(args);
        let short = args.iter().find(|arg| arg.to_lowercase().starts_with("5e6f7a8b")).unwrap();
        let expected = format!("Error: {}\n", ambiguous(short, plural, &ids, &labels));
        assert_eq!((out.status.code(), text(&out.stdout), text(&out.stderr)), (Some(1), String::new(), expected));
        assert_eq!(sent(&server).await, [listing], "{args:?}: nothing after the lookup");
        release(server).await;
    }
    // With --json, the JSON error: the message only.
    let server = short_id_stub().await;
    let out = Sandbox::new(&server.uri()).run(&["apps", "get", "5e6f7a8b", "--json"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(1), String::new()));
    let message = ambiguous("5e6f7a8b", "apps", &ids, &["Demo App 2  shopify", "Demo App 3  shopify"]);
    assert_eq!(json_error(&out), error_shape(&message, Value::Null, Value::Null, Value::Null));
    release(server).await;
}

#[tokio::test]
async fn a_refusal_lists_ten_matches_and_says_how_many_more() {
    let server = MockServer::start().await;
    let ids: Vec<String> = (1..=12).map(|n| uuid_with("abcdef01", n)).collect();
    // A row the list sends without the fields it is named by shows its ID alone.
    let mut rows: Vec<Value> = ids.iter().zip(1..).map(|(id, n)| listed_row("apps", id, n)).collect();
    rows[1] = json!({ "id": ids[1] });
    serve(&server, "GET", &format!("{V1}/apps"), 200, page_of(rows, 0, false)).await;
    let out = Sandbox::new(&server.uri()).run(&["apps", "get", "abcdef01"]);
    let mut expected = "Error: Short ID abcdef01 matches 12 apps:\n".to_string();
    for (n, id) in ids.iter().enumerate().take(10) {
        expected += &if n == 1 { format!("  {id}\n") } else { format!("  {id}  Demo App {}  shopify\n", n + 1) };
    }
    expected += "  and 2 more\nUse the full ID.\n";
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), expected));
    assert_eq!(sent(&server).await, [listed("apps")]);
}

#[tokio::test]
async fn a_short_id_as_tables_print_it_with_dots_is_looked_up_like_the_8_digits() {
    // Decided by Yalcin, 2026-10-06: the `1a2b3c4d...` a table prints works as `1a2b3c4d`.
    let full = uuid_with("1a2b3c4d", 1);
    for (args, requests) in [
        (vec!["apps", "get", "1a2b3c4d..."], vec![listed("apps"), format!("GET {V1}/apps/{full}")]),
        (
            vec!["jobs", "list", "--integration", "1A2B3C4D...", "--json"],
            vec![listed("connections"), format!("GET {V1}/jobs?integration_id={full}&limit=20&offset=0")],
        ),
        (
            vec!["metrics", "get", "1a2b3c4d..."],
            vec!["GET /api/metrics?limit=100&offset=0".to_string(), format!("GET /api/metrics/{full}")],
        ),
    ] {
        let server = short_id_stub().await;
        let out = Sandbox::new(&server.uri()).run(&args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out.stderr));
        assert_eq!(sent(&server).await, requests, "{args:?}");
    }
}

#[tokio::test]
async fn a_full_id_or_anything_but_eight_hex_digits_is_never_looked_up() {
    let server = short_id_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let full = uuid_with("1a2b3c4d", 1);
    let others =
        [full.as_str(), "1a2b3c4", "1a2b3c4d5", "1a2b3c4g", "app-1234", "1a2b3c4d..", "1a2b3c4d....", "1a2b3c4..."];
    for id in others {
        assert_eq!(sandbox.run(&["apps", "get", id]).status.code(), Some(0), "{id}");
    }
    let expected: Vec<String> = others.iter().map(|id| format!("GET {V1}/apps/{id}")).collect();
    assert_eq!(sent(&server).await, expected);
}

#[tokio::test]
async fn a_short_id_is_looked_up_only_after_consent() {
    // VE-3823: delete and cancel decide before any request. The refusal names the ID as typed.
    let full = uuid_with("1a2b3c4d", 1);
    for (args, what, listing, request) in [
        (
            ["apps", "delete", "1a2b3c4d"],
            "This deletes app 1a2b3c4d.",
            listed("apps"),
            format!("DELETE {V1}/apps/{full}"),
        ),
        (
            ["sources", "delete", "1a2b3c4d"],
            "This deletes source 1a2b3c4d.",
            listed("sources"),
            format!("DELETE {V1}/sources/{full}"),
        ),
        (
            ["destinations", "delete", "1a2b3c4d"],
            "This deletes destination 1a2b3c4d.",
            listed("connections"),
            format!("DELETE {V1}/connections/{full}"),
        ),
        (
            ["jobs", "cancel", "1a2b3c4d"],
            "This cancels job 1a2b3c4d.",
            listed("jobs"),
            format!("POST {V1}/jobs/{full}/cancel"),
        ),
        (
            ["metrics", "delete", "1a2b3c4d"],
            "This deletes metric 1a2b3c4d and cannot be undone.",
            "GET /api/metrics?limit=100&offset=0".to_string(),
            format!("DELETE /api/metrics/{full}"),
        ),
    ] {
        let server = short_id_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        assert_needs_yes(&sandbox.run(&args), what);
        assert_eq!(sent(&server).await, Vec::<String>::new(), "{args:?}");
        let out = sandbox.run(&[&args[..], &["--yes"]].concat());
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out.stderr));
        assert_eq!(sent(&server).await, [listing, request], "{args:?}");
    }
    // A dry run sends nothing, as before.
    let server = short_id_stub().await;
    assert_eq!(
        ok_output(&Sandbox::new(&server.uri()).run(&["apps", "delete", "1a2b3c4d", "--dry-run"])),
        "[dry-run] Would delete app 1a2b3c4d\n"
    );
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

/// `pages` pages of 100 jobs, each saying another follows unless it is the last and `last_ends`;
/// `place` puts IDs at (page, row).
async fn job_pages(pages: u64, last_ends: bool, place: &[(u64, usize, String)]) -> MockServer {
    let server = MockServer::start().await;
    for n in 0..pages {
        let mut ids: Vec<String> =
            (0..100).map(|i| uuid_with(&format!("{:08x}", 0x7000_0000 + n * 100 + i), i)).collect();
        for (_, row, id) in place.iter().filter(|(page, ..)| *page == n) {
            ids[*row] = id.clone();
        }
        let more = !(last_ends && n + 1 == pages);
        Mock::given(path(format!("{V1}/jobs")))
            .and(wiremock::matchers::query_param("offset", (n * 100).to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(list_page(&ids, (n * 100) as usize, more)))
            .mount(&server)
            .await;
    }
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_short_id_lookup_pages_through_the_list_up_to_five_pages() {
    // 100 per page (the API's most), the next page while `hasMore`: a match on one page is checked
    // against the rest, since one that several IDs start with is refused (VE-3831).
    let pages = |n: usize| (0..n).map(|i| format!("GET {V1}/jobs?limit=100&offset={}", i * 100));
    let found = uuid_with("abcdef01", 1);
    let server = job_pages(2, true, &[(1, 42, found.clone())]).await;
    assert_eq!(Sandbox::new(&server.uri()).run(&["jobs", "get", "abcdef01"]).status.code(), Some(0));
    assert_eq!(sent(&server).await, pages(2).chain([format!("GET {V1}/jobs/{found}")]).collect::<Vec<_>>());
    let twin = uuid_with("abcdef01", 2);
    let server = job_pages(3, true, &[(0, 3, found.clone()), (2, 7, twin.clone())]).await;
    let out = Sandbox::new(&server.uri()).run(&["jobs", "get", "abcdef01"]);
    let refused = format!("Error: Short ID abcdef01 matches 2 jobs:\n  {found}\n  {twin}\nUse the full ID.\n");
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), refused));
    assert_eq!(sent(&server).await, pages(3).collect::<Vec<_>>());
    // At most five pages (the key's 60 requests a minute): a job further back is sent as typed.
    let server = job_pages(6, false, &[(5, 0, uuid_with("abcdef02", 2))]).await;
    assert_eq!(Sandbox::new(&server.uri()).run(&["jobs", "get", "abcdef02"]).status.code(), Some(0));
    assert_eq!(sent(&server).await, pages(5).chain([format!("GET {V1}/jobs/abcdef02")]).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_row_read_on_two_pages_is_one_match() {
    // A job created between two page reads moves the next page down by one: the last row of one
    // page comes back first on the next. It is still one job.
    let found = uuid_with("abcdef01", 1);
    let server = job_pages(2, true, &[(0, 99, found.clone()), (1, 0, found.clone())]).await;
    let out = Sandbox::new(&server.uri()).run(&["jobs", "get", "abcdef01", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let pages = (0..2).map(|i| format!("GET {V1}/jobs?limit=100&offset={}", i * 100));
    assert_eq!(sent(&server).await, pages.chain([format!("GET {V1}/jobs/{found}")]).collect::<Vec<_>>());
}

/// The metrics route as vendo-web-v2 answers it: archived metrics only when asked for them
/// (`listMetrics`' `excludeArchivedWhenNoStatus`). Every other request is answered `{ metric: {} }`.
async fn metrics_stub(active: &str, archived: &str) -> MockServer {
    let server = MockServer::start().await;
    for (status, id) in [(None, active), (Some("archived"), archived)] {
        let list = Mock::given(wiremock::matchers::method("GET")).and(path("/api/metrics"));
        let list = match status {
            Some(status) => list.and(wiremock::matchers::query_param("status", status)),
            None => list.and(wiremock::matchers::query_param_is_missing("status")),
        };
        let page = json!({ "metrics": [{ "id": id }], "total": 1, "limit": 100, "offset": 0 });
        list.respond_with(ResponseTemplate::new(200).set_body_json(page)).mount(&server).await;
    }
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "metric": {} })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn an_archived_metrics_short_id_is_looked_up_in_the_archived_list_when_the_default_list_misses() {
    // `metrics list --status archived` shows archived metrics' short IDs, which the default list leaves out.
    let active = uuid_with("aaaaaaaa", 1);
    let archived = uuid_with("c0ffee01", 2);
    let listed = "GET /api/metrics?limit=100&offset=0".to_string();
    let archived_listed = "GET /api/metrics?status=archived&limit=100&offset=0".to_string();
    for (args, requests) in [
        (
            &["metrics", "get", "c0ffee01"][..],
            vec![listed.clone(), archived_listed.clone(), format!("GET /api/metrics/{archived}")],
        ),
        // Un-archiving it by its short ID.
        (
            &["metrics", "update", "c0ffee01", "--status", "active", "--json"],
            vec![
                listed.clone(),
                archived_listed.clone(),
                format!("PATCH /api/metrics/{archived} {{\"status\":\"active\"}}"),
            ],
        ),
        // A match in the default list reads nothing more.
        (&["metrics", "get", "aaaaaaaa"], vec![listed.clone(), format!("GET /api/metrics/{active}")]),
        // In neither: sent as typed.
        (
            &["metrics", "get", "99999999"],
            vec![listed.clone(), archived_listed.clone(), "GET /api/metrics/99999999".to_string()],
        ),
    ] {
        let server = metrics_stub(&active, &archived).await;
        let out = Sandbox::new(&server.uri()).run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out.stderr));
        assert_eq!(sent(&server).await, requests, "{args:?}");
    }
}

#[tokio::test]
async fn methodologies_get_takes_the_short_id_its_list_shows() {
    // `methodologies get` already reads the list: a short ID is found in it, with no extra request.
    let server = MockServer::start().await;
    let full = uuid_with("0d0e0f10", 9);
    let methodologies = json!({ "data": { "methodologies": [
        { "id": full, "name": "Blended", "click_path_model": "linear", "is_system": false, "version": 1 },
        { "id": uuid_with("5e6f7a8b", 1), "name": "A" }, { "id": uuid_with("5e6f7a8b", 2), "name": "B" },
    ] } });
    serve(&server, "GET", "/api/measurement/methodologies", 200, methodologies).await;
    let sandbox = Sandbox::new(&server.uri());
    let short = ok_output(&sandbox.run(&["measurement", "methodologies", "get", "0d0e0f10", "--json"]));
    assert_eq!(short, ok_output(&sandbox.run(&["measurement", "methodologies", "get", &full, "--json"])));
    let dots = ok_output(&sandbox.run(&["measurement", "methodologies", "get", "0D0E0F10...", "--json"]));
    assert_eq!(dots, short);
    assert_eq!(sent(&server).await.len(), 3);
    // Several start with it: refused with the matches, as the other short IDs are (VE-3831).
    let out = sandbox.run(&["measurement", "methodologies", "get", "5e6f7a8b"]);
    let ids = [uuid_with("5e6f7a8b", 1), uuid_with("5e6f7a8b", 2)];
    let expected = format!("Error: {}\n", ambiguous("5e6f7a8b", "methodologies", &ids, &["A", "B"]));
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), expected));
    // None does: not found, as for any other ID.
    let out = sandbox.run(&["measurement", "methodologies", "get", "99999999"]);
    assert_eq!((out.status.code(), stderr_line(&out)), (Some(1), "Error: Methodology 99999999 not found".to_string()));
}

// ── VE-3826: a bare group opens a menu of its commands on a terminal ────────
// Decided by Yalcin, 2026-10-05, CLI 1.1: where a person can answer and see it (stdin and stdout
// terminals, the rule `delete` asks by, and stderr, where the menu is drawn), `vendo apps` opens an
// arrow-key menu of the apps commands with type-to-filter, and Enter runs the chosen command as if
// it had been typed. Without a terminal nothing prompts: the group's help and exit 2, as before
// (snapshots.rs checks every group). A chosen command that needs an argument asks for it as it does
// when typed without it (VE-3881).

/// What the terminal shows without escape sequences (cursor moves, clearing, styles) and with
/// plain line ends.
fn plain(screen: &str) -> String {
    let mut out = String::new();
    let mut chars = screen.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI: `ESC [`, parameters, one final byte from `@` to `~`. Anything else: one more char.
                if chars.next_if_eq(&'[').is_some() {
                    while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
                } else {
                    chars.next();
                }
            }
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// What a terminal of `lines` × `columns` shows after `output`, its lines without trailing spaces,
/// and where its cursor is (line, column): enough of a VT100 to follow the menu (text that wraps at
/// the last column, CR, LF that scrolls at the bottom, cursor moves, erasing, saving and restoring
/// the cursor). A wide character such as 東 takes two columns, going to the next line when one is
/// left, as a terminal draws it (VE-3881); a character of no width (a combining mark) is left out.
/// Colours and modes change nothing.
fn screen(output: &str, lines: usize, columns: usize) -> (Vec<String>, (usize, usize)) {
    use unicode_width::UnicodeWidthChar;
    // The second column of a wide character.
    const RIGHT_HALF: char = '\0';
    let mut shown = vec![vec![' '; columns]; lines];
    let (mut line, mut column, mut saved, mut wrap): (usize, usize, _, _) = (0, 0, (0, 0), false);
    let line_feed = |shown: &mut Vec<Vec<char>>, line: &mut usize| {
        if *line + 1 == lines {
            shown.remove(0);
            shown.push(vec![' '; columns]);
        } else {
            *line += 1;
        }
    };
    let mut chars = output.chars();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('7') => saved = (line, column),
                Some('8') => ((line, column), wrap) = (saved, false),
                Some('[') => {
                    // Parameters, then one final character from `@` to `~`.
                    let (mut parameters, mut end) = (String::new(), None);
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            end = Some(c);
                            break;
                        }
                        parameters.push(c);
                    }
                    let n: usize = parameters.parse().unwrap_or(0);
                    match end {
                        Some('A') => line = line.saturating_sub(n.max(1)),
                        Some('B') => line = (line + n.max(1)).min(lines - 1),
                        Some('C') => column = (column + n.max(1)).min(columns - 1),
                        Some('D') => column = column.saturating_sub(n.max(1)),
                        Some('G') => column = (n.max(1) - 1).min(columns - 1),
                        Some('K') if n == 2 => shown[line].fill(' '),
                        Some('K') => shown[line][column..].fill(' '),
                        Some('J') => {
                            shown[line][column..].fill(' ');
                            shown[line + 1..].iter_mut().for_each(|line| line.fill(' '));
                        }
                        _ => continue,
                    }
                    wrap = false;
                }
                _ => {}
            },
            '\r' => (column, wrap) = (0, false),
            '\n' => {
                line_feed(&mut shown, &mut line);
                wrap = false;
            }
            c => {
                let width = c.width().unwrap_or(1);
                if width == 0 {
                    continue;
                }
                if wrap || column + width > columns {
                    line_feed(&mut shown, &mut line);
                    (column, wrap) = (0, false);
                }
                shown[line][column] = c;
                if width == 2 {
                    shown[line][column + 1] = RIGHT_HALF;
                }
                if column + width == columns {
                    (column, wrap) = (columns - 1, true);
                } else {
                    column += width;
                }
            }
        }
    }
    let shown = shown.iter().map(|line| line.iter().filter(|c| **c != RIGHT_HALF).collect::<String>());
    (shown.map(|line| line.trim_end().to_string()).collect(), (line, column))
}

/// The rows of `vendo <group> --help`'s `Commands:` list, without their indent: what its menu lists.
fn command_rows(sandbox: &Sandbox, group: &[&str]) -> Vec<String> {
    let screen = text(&sandbox.run(&[group, &["--help"]].concat()).stdout);
    let list = screen.split("Commands:\n").nth(1).unwrap().split("\n\n").next().unwrap();
    list.lines().map(|line| line.trim().to_string()).collect()
}

const MENU_APP: &str = "a1b2c3d4-0000-4000-8000-000000000001";

#[cfg(unix)]
#[tokio::test]
async fn a_bare_group_on_a_terminal_opens_a_menu_and_runs_the_chosen_command() {
    let server = MockServer::start().await;
    let app = json!({
        "id": MENU_APP, "appType": "shopify", "displayName": "Menu Shop", "permissions": ["performance_data"],
        "roles": ["source"], "state": "active", "accessStatus": "connected", "lastSyncAt": null,
    });
    serve(&server, "GET", "/api/v1/accounts/acct-beta/apps", 200, json!({ "data": [app] })).await;
    let sandbox = Sandbox::new(&server.uri());
    // `--profile` before the group stays with the command that runs.
    let mut terminal = OnTerminal::start(&sandbox, &["--profile", "beta", "apps"]);
    // Every apps command, with its description, as the group's help lists them.
    let rows = command_rows(&sandbox, &["apps"]);
    assert_eq!(rows.len(), 8, "{rows:?}");
    let menu = plain(&terminal.wait_for(rows.last().unwrap()));
    for row in &rows {
        assert!(menu.contains(row.as_str()), "{row:?} is not in the menu:\n{menu}");
    }
    // Typing filters the menu, by name and description in any case: with `get` highlighted, "LIS"
    // leaves only `list`, which Enter runs. The chosen command replaces the menu after the title.
    terminal.press("\u{1b}[B\u{1b}[B");
    terminal.press("LIS");
    terminal.wait_for("vendo apps LIS");
    terminal.press("\r");
    terminal.wait_for("vendo apps list");
    // `vendo --profile beta apps list` runs against the stub: at a terminal, its apps to choose from
    // (VE-3894), which Esc leaves.
    terminal.wait_for("type to filter · 1 app]");
    let listed = shown_from(&terminal, "? vendo apps list");
    assert_eq!(listed[1], "> a1b2c3d4...  Menu Shop  shopify  source  active  connected  —", "{listed:#?}");
    terminal.press("\u{1b}");
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran}");
    assert_eq!(sent(&server).await, ["GET /api/v1/accounts/acct-beta/apps?limit=20&offset=0"]);
}

#[cfg(unix)]
#[tokio::test]
async fn the_arrow_keys_move_through_the_menu() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps"]);
    terminal.wait_for("Update an app");
    // Down, down, up: the second command, `diagnose`.
    terminal.press("\u{1b}[B");
    terminal.press("\u{1b}[B");
    terminal.press("\u{1b}[A");
    terminal.press("\r");
    terminal.wait_for("vendo apps diagnose");
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran}");
    let mut requests = sent(&server).await;
    requests.sort();
    assert_eq!(
        requests,
        ["apps", "connections", "sources"].map(|list| format!("GET /api/v1/accounts/acct-alpha/{list}?limit=100"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_leave_the_menu_quietly() {
    // Like Ctrl-C or Ctrl-D at a question (VE-3823): exit 0, and nothing runs or fails. All three
    // leave the same screen: the title and `<canceled>` where the menu was, nothing of the menu,
    // and the cursor on the next line, where the shell's prompt goes. On an empty screen, and at
    // the bottom of a full one. (inquire left the whole menu standing for Ctrl-C, and the cursor
    // on its hint at the bottom of the screen.)
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let earlier: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    for (size, before) in [((40, 120), ""), ((16, 80), earlier.as_str())] {
        let mut screens = Vec::new();
        for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
            let mut terminal = OnTerminal::start_with(&sandbox, &["apps"], size, before, None);
            terminal.wait_for("Update an app");
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{name}: {rest:?}");
            let rest = plain(&rest).to_lowercase();
            assert!(!rest.contains("error") && !rest.contains("usage"), "{name}: {rest:?}");
            let (shown, cursor) = screen(&terminal.screen, size.0.into(), size.1.into());
            let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
            assert_eq!(
                (shown[last].as_str(), cursor),
                ("? vendo apps <canceled>", (last + 1, 0)),
                "{name}: {shown:#?}"
            );
            screens.push((shown, cursor));
        }
        assert!(screens.iter().all(|screen| *screen == screens[0]), "{screens:#?}");
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

/// How soon a `vendo` whose terminal hung up must exit, and the most processor time it may use in
/// all, from its start: one that spins on the terminal uses a whole core until it is killed.
#[cfg(unix)]
const HUNG_UP_EXIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(unix)]
const HUNG_UP_CPU: std::time::Duration = std::time::Duration::from_secs(1);

/// [`OnTerminal::hang_up`], then how `vendo` ended: whether it exited within [`HUNG_UP_EXIT`] (it is
/// stopped when not), and how.
#[cfg(unix)]
fn hang_up(terminal: &mut OnTerminal) -> (bool, Ended) {
    terminal.hang_up();
    match terminal.exit_within(HUNG_UP_EXIT) {
        Some(ended) => (true, ended),
        None => (false, terminal.stop()),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_menu_exits_as_ctrl_d_does_when_its_terminal_hangs_up() {
    // A menu left on a terminal that hung up, with no SIGHUP to end it (the terminal was not its
    // controlling terminal, and the program that opened it was gone): crossterm, which reads the
    // keys for inquire, read the end of the input (or an I/O error) at once, again and again, and
    // the CLI spun at a whole core for hours (2026-10-06). It exits 0 as for Ctrl-D, running
    // nothing, in every state the menu waits in: just opened, while typing filters it,
    // with half a key read (the first byte of a two-byte character, which crossterm waits to
    // complete), and in a chosen group's menu.
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut outcomes = Vec::new();
    for (case, group, menu, keys, then) in [
        ("just opened", "apps", "Update an app", &b""[..], None),
        ("filtering", "apps", "Update an app", b"LIS", Some("vendo apps LIS")),
        ("half a key", "apps", "Update an app", b"\xc3", None),
        (
            "a chosen group's menu",
            "measurement",
            "Measurement signal availability",
            b"ltv\r",
            Some("Show one customer"),
        ),
    ] {
        let mut terminal = OnTerminal::start_detached(&sandbox, &[group]);
        terminal.wait_for(menu);
        terminal.press_bytes(keys);
        match then {
            Some(shown) => _ = terminal.wait_for(shown),
            // Nothing more shows: time for crossterm to read what was typed and wait for more.
            None => std::thread::sleep(std::time::Duration::from_millis(300)),
        }
        let (exited, ended) = hang_up(&mut terminal);
        outcomes.push((case, exited, ended));
    }
    assert!(
        outcomes.iter().all(|(_, exited, ended)| *exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "each should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {outcomes:#?}"
    );
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn the_questions_and_login_do_not_spin_when_their_terminal_hangs_up() {
    // The y/N question reads the terminal with std, line by line: the end of the input or an error
    // ends the read, as Ctrl-D does, so it exits 0 and changes nothing. (The profile list is a menu
    // since VE-3892: `the_profile_list_exits_as_ctrl_d_does_when_its_terminal_hangs_up`.)
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start_detached(&sandbox, &["apps", "delete", ID]);
    terminal.wait_for("(y/N) ");
    let (exited, ended) = hang_up(&mut terminal);
    assert!(
        exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU,
        "it should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {ended:?}"
    );
    assert_eq!(sent(&server).await, Vec::<String>::new());

    // Login reads "Press ENTER to open in the browser..." on a thread of its own, once: the end of
    // the input is no ENTER, so no browser opens, and login waits for the sign-in as it does with
    // stdin closed, without spinning, until the browser comes back.
    let server = sign_in_stub(&[NEW_KEY], 401).await;
    let sandbox = Sandbox::without_api_keys(&server.uri());
    let mut terminal = OnTerminal::start_detached(&sandbox, &["login"]);
    let shown = plain(&terminal.wait_for("Waiting for authorization...\r\n"));
    terminal.hang_up();
    let waiting = terminal.exit_within(std::time::Duration::from_secs(2));
    assert!(waiting.is_none(), "login should wait for the sign-in: {waiting:?}");
    let url = reqwest::Url::parse(shown.lines().find(|line| line.contains("/cli-auth?")).unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"), "tests sign in at their local stub only: {url}");
    assert_eq!(reqwest::get(url).await.unwrap().status(), 200);
    let ended = terminal.exit_within(HUNG_UP_EXIT);
    assert!(
        ended.as_ref().is_some_and(|ended| ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "login should sign in and exit 0, using under {HUNG_UP_CPU:?} of processor time: {ended:?}"
    );
    assert_eq!(sandbox.config()["profiles"]["demo-account"]["apiKey"], NEW_KEY);
}

#[cfg(unix)]
#[tokio::test]
async fn the_menu_exits_as_ctrl_d_does_when_the_terminal_it_is_drawn_on_hangs_up() {
    // Keys from one terminal and the menu drawn on another (stdout and stderr), which hangs up
    // while the first stays up: the menu waited for a key on the first, then exited 2 with the
    // usage error on the terminal that had gone. It exits 0 as for Ctrl-D, running nothing, and
    // the terminal the keys come from is back in line mode.
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let (_up, keys, _) = pseudo_terminal_sized(40, 120);
    let watched = keys.try_clone().unwrap();
    let mut terminal = OnTerminal::launch(sandbox.command(&["apps"]), (40, 120), "", Some(keys.into()), None, false);
    terminal.wait_for("Update an app");
    let (exited, ended) = hang_up(&mut terminal);
    assert!(
        exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU,
        "it should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {ended:?}"
    );
    // SAFETY: tcgetattr fills `settings` from a descriptor this test owns.
    let settings = unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(std::os::fd::AsRawFd::as_raw_fd(&watched), &mut settings), 0);
        settings
    };
    assert_ne!(settings.c_lflag & libc::ICANON, 0, "the keys' terminal should be back in line mode");
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

/// Kills a process group when dropped: one a test's shell started, which the test cannot reap.
#[cfg(unix)]
struct KillGroup(libc::pid_t);

#[cfg(unix)]
impl Drop for KillGroup {
    fn drop(&mut self) {
        // SAFETY: kill only sends a signal.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

/// The first line a process writes to `path`, within 5 seconds.
#[cfg(unix)]
fn line_in(path: &std::path::Path) -> String {
    let asked = std::time::Instant::now();
    loop {
        if let Some((line, _)) = std::fs::read_to_string(path).ok().as_deref().and_then(|text| text.split_once('\n')) {
            return line.to_string();
        }
        assert!(asked.elapsed() < std::time::Duration::from_secs(5), "nothing was written to {}", path.display());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_menu_exits_as_ctrl_d_does_when_a_shell_takes_its_terminal_back_for_good() {
    // A program run from a shell started `vendo apps` and was gone while the menu was open: the
    // shell took its terminal back, and the menu was left in the background with nothing that
    // could bring it back (its process group orphaned). Every key typed at the shell then made
    // crossterm's read fail with EIO, at once, every time, and the CLI spun at a whole core for as
    // long as the window stayed open (2026-10-06). It exits 0 as for Ctrl-D, running nothing, and
    // leaves the terminal, the shell's now, as it is.
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let dir = tempfile::tempdir().unwrap();
    let [program, job, code, done] = ["program", "job", "code", "done"].map(|name| dir.path().join(name));
    // The shell, with job control as at a prompt (`sh` is bash on macOS, dash on Ubuntu), runs
    // the program in the foreground. The program, a shell with job control too, runs `vendo apps`
    // in a job of its own that it gives the terminal, and that job records how `vendo` ended.
    // Then it waits for its next command, which comes once `done` exists.
    let script = r#"set -m
sh -c "$PROGRAM" "$0" "$1" "$2" "$3"
printf 'the shell has the terminal'
while [ ! -e "$4" ]; do sleep 0.05; done
printf done"#;
    let vendo = sandbox.command(&["apps"]);
    let mut shell = Command::new("/bin/sh");
    shell.args(["-c", script]).arg(vendo.get_program()).args([&program, &job, &code, &done]);
    for (key, value) in vendo.get_envs() {
        match value {
            Some(value) => shell.env(key, value),
            None => shell.env_remove(key),
        };
    }
    shell
        .env("PROGRAM", r#"echo $$ >"$1"; set -m; sh -c "$JOB" "$0" "$2" "$3""#)
        .env("JOB", r#"echo $$ >"$1"; "$0" apps; echo $? >"$2""#);
    let mut terminal = OnTerminal::spawn(shell, (40, 120), "", None, None);
    terminal.wait_for("Update an app");
    let job = KillGroup(line_in(&job).parse().unwrap());
    let program: libc::pid_t = line_in(&program).parse().unwrap();
    // The program is gone: the shell takes the terminal back, and someone types at it.
    // SAFETY: kill only sends a signal, to the program's shell.
    unsafe {
        libc::kill(program, libc::SIGKILL);
    }
    terminal.wait_for("the shell has the terminal");
    terminal.press("l");
    let asked = std::time::Instant::now();
    let ended = loop {
        match std::fs::read_to_string(&code) {
            Ok(code) if code.ends_with('\n') => break Some(code),
            _ if asked.elapsed() >= HUNG_UP_EXIT => break None,
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    // Gone by now, unless it still runs.
    drop(job);
    std::fs::write(&done, "").unwrap();
    let shown = terminal.finish();
    assert_eq!(
        ended.as_deref(),
        Some("0\n"),
        "vendo should exit 0 within {HUNG_UP_EXIT:?} (the shell showed {shown:?})"
    );
    assert_eq!(shown, ("done".to_string(), Some(0)), "vendo should leave the shell's terminal as it is");
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn choosing_a_group_opens_its_menu() {
    // `vendo measurement` → `ltv` → the menu of `vendo measurement ltv`.
    let sandbox = Sandbox::new(CLOSED);
    let mut terminal = OnTerminal::start(&sandbox, &["measurement"]);
    let menu = plain(&terminal.wait_for("Measurement signal availability"));
    for row in command_rows(&sandbox, &["measurement"]) {
        assert!(menu.contains(row.as_str()), "{row:?} is not in the menu:\n{menu}");
    }
    terminal.press("ltv\r");
    terminal.wait_for("vendo measurement ltv");
    let rows = command_rows(&sandbox, &["measurement", "ltv"]);
    let menu = plain(&terminal.wait_for("Show one customer's cohort"));
    for row in &rows[..2] {
        assert!(menu.contains(row.as_str()), "{row:?} is not in the menu:\n{menu}");
    }
    terminal.press("\u{1b}");
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(0), "{rest:?}");
}

#[cfg(unix)]
#[test]
fn the_old_config_group_opens_the_profile_menu() {
    // `config` is the hidden old name of `profile` (VE-3827): the menu lists what `profile` does,
    // and nothing hidden.
    let sandbox = Sandbox::new(CLOSED);
    let mut terminal = OnTerminal::start(&sandbox, &["config"]);
    let rows = command_rows(&sandbox, &["profile"]);
    assert_eq!(rows.len(), 3, "{rows:?}");
    let menu = plain(&terminal.wait_for(rows.last().unwrap()));
    assert!(menu.contains("vendo profile"), "titled as the tree names it: {menu}");
    for row in &rows {
        assert!(menu.contains(row.as_str()), "{row:?} is not in the menu:\n{menu}");
    }
    assert!(!menu.contains("current") && !menu.contains("reset"), "{menu}");
    // Its first command, `list`, shows the profiles to choose from (VE-3894), titled as the tree names
    // it; Esc leaves.
    terminal.press("\r");
    terminal.wait_for("vendo profile list");
    terminal.wait_for("type to filter]");
    let rows = profile_list_rows(CLOSED, &["alpha", "beta"], "alpha");
    assert_eq!(shown_from(&terminal, "? vendo profile list"), profile_list("vendo profile list", &rows));
    terminal.press("\u{1b}");
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran}");
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[test]
fn the_menu_lists_only_visible_commands() {
    // `catalog credential-schema` is hidden (VE-3827).
    let sandbox = Sandbox::new(CLOSED);
    let mut terminal = OnTerminal::start(&sandbox, &["catalog"]);
    let menu = plain(&terminal.wait_for("Get details for a specific platform"));
    assert!(!menu.contains("credential-schema"), "{menu}");
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
}

#[cfg(unix)]
#[tokio::test]
async fn a_chosen_command_missing_its_app_asks_for_it_as_it_does_when_typed() {
    // VE-3881: `vendo apps` → get asks for the app as `vendo apps get` does, then runs.
    let server = apps_to_choose_stub("acct-alpha").await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps"]);
    terminal.wait_for("Update an app");
    terminal.press("get app\r");
    terminal.wait_for("vendo apps get");
    terminal.wait_for(APP_ROWS[2]);
    terminal.press("\r");
    terminal.wait_for("vendo apps get a1b2c3d4... (Menu Shop)");
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran:?}");
    assert!(plain(&ran).contains("Menu Shop"), "{ran:?}");
    assert_eq!(sent(&server).await, [format!("GET {V1}/apps?limit=100&offset=0"), format!("GET {V1}/apps/{MENU_APP}")]);
}

#[cfg(unix)]
#[test]
fn a_bare_group_needs_stdin_stdout_and_stderr_on_a_terminal_to_open_the_menu() {
    let sandbox = Sandbox::new(CLOSED);
    let help = text(&sandbox.run(&["apps", "--help"]).stdout);
    // Keys from a terminal, but stdout goes to a pipe (`vendo apps | less`): no menu, as `delete`
    // does not ask there.
    let (_controller, terminal) = pseudo_terminal();
    let out = sandbox.command(&["apps"]).stdin(terminal).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stdout), text(&out.stderr)), (Some(2), String::new(), help.clone()));
    // Shown on a terminal, but the keys would come from a pipe.
    let (_controller, terminal) = pseudo_terminal();
    let out = sandbox.command(&["apps"]).stdin(Stdio::piped()).stdout(terminal).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(2), help.clone()));
    // Keys and stdout on the terminal, but stderr, where the menu is drawn, goes to a file or
    // nowhere (`vendo apps 2>err.log`): the menu would wait for keys with nothing on the screen.
    let log = tempfile::NamedTempFile::new().unwrap();
    for stderr in [Stdio::from(log.reopen().unwrap()), Stdio::null()] {
        let mut terminal = OnTerminal::start_with(&sandbox, &["apps"], (40, 120), "", Some(stderr));
        assert_eq!(terminal.finish(), (String::new(), Some(2)));
    }
    assert_eq!(std::fs::read_to_string(log.path()).unwrap(), help);
    // Keys from the terminal opened for writing only (`vendo apps 0>/dev/ttys004`): a terminal, but
    // no key can be read from it. The menu waited for keys, and from the first one on crossterm's
    // read loop spun at a whole core on the error (EBADF) (2026-10-06).
    let pseudo_terminal = pseudo_terminal_sized(40, 120);
    let write_only = {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let name = std::ffi::OsStr::from_bytes(pseudo_terminal.2.as_bytes());
        std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOCTTY).open(name).unwrap()
    };
    let mut terminal =
        OnTerminal::launch_on(pseudo_terminal, sandbox.command(&["apps"]), "", Some(write_only.into()), None, true);
    let (shown, code) = terminal.finish();
    assert_eq!((plain(&shown).replace("\r\n", "\n"), code), (help, Some(2)));
}

#[cfg(unix)]
#[tokio::test]
async fn on_a_short_terminal_the_menu_scrolls_its_list() {
    // A menu taller than the screen drew over itself: the title went, rows went missing or showed
    // twice, two of them marked `>`. It lists as many commands as fit between the title and the
    // hint, with a line to spare, and scrolls; the one marked `>` is the one Enter runs.
    let server = MockServer::start().await;
    for list in ["apps", "connections"] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(Vec::new(), 0, false)).await;
    }
    let sandbox = Sandbox::new(&server.uri());
    for (group, (lines, columns), down, chosen) in [
        ("apps", (6, 120), 5, "delete"),
        // refresh-source's row takes two lines on 80 columns.
        ("destinations", (8, 80), 3, "refresh-source"),
    ] {
        let mut terminal = OnTerminal::start_with(&sandbox, &[group], (lines, columns), "", None);
        terminal.wait_for("type to filter]");
        terminal.press(&"\u{1b}[B".repeat(down));
        terminal.wait_for(&format!("> {chosen} "));
        // The end of that frame: inquire shows the cursor again.
        terminal.wait_for("\u{1b}[?25h");
        let (shown, _) = screen(&terminal.screen, lines.into(), columns.into());
        let hint = shown.iter().rposition(|line| !line.is_empty()).unwrap();
        assert_eq!(shown[0], format!("? vendo {group}"), "{shown:#?}");
        assert_eq!(shown[hint], "[↑↓ to move, enter to select, type to filter]", "{shown:#?}");
        assert!(hint < shown.len() - 1, "{shown:#?}");
        let marked: Vec<&String> = shown.iter().filter(|line| line.starts_with('>')).collect();
        assert!(marked.len() == 1 && marked[0].starts_with(&format!("> {chosen} ")), "{shown:#?}");
        // Both need an ID, asked for from a list of the account's (VE-3881): with none to choose
        // from, Enter ends in the usage error for the command shown.
        terminal.press("\r");
        terminal.wait_for(&format!("vendo {group} {chosen}"));
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(2), "{rest:?}");
        assert!(plain(&rest).contains(&format!("Usage: vendo {group} {chosen} <")), "{rest:?}");
    }
}

// ── VE-3881: a missing value is asked for at a terminal ─────────────────────
// Decided by Yalcin, 2026-10-07, CLI 1.1: where the group menu opens (VE-3826), a command missing a
// required value asks for it instead of failing: an app from an arrow-key list of the account's
// apps, with type-to-filter, a platform from the ones ready to connect, a name as a one-line
// question. What is chosen or typed runs the command exactly as if it had been typed. Esc, Ctrl-C
// and Ctrl-D leave quietly, running nothing. Without a terminal it is the usage error it was
// (`usage/` in snapshots.rs, `with_prompts_off_or_no_screen_to_ask_on_a_missing_value_is_the_usage_error`).

const CHOOSE_BQ: &str = "e5f6a7b8-0000-4000-8000-000000000002";
const CHOOSE_PIXEL: &str = "0c0d0e0f-0000-4000-8000-000000000003";

fn app_to_choose(id: &str, name: &str, app_type: &str, roles: &[&str], state: &str) -> Value {
    json!({
        "id": id, "appType": app_type, "displayName": name, "permissions": [], "roles": roles, "state": state,
        "accessStatus": "connected", "lastSyncAt": null,
    })
}

/// The apps to choose from, newest first as the route lists them.
fn apps_to_choose() -> Vec<Value> {
    vec![
        app_to_choose(MENU_APP, "Menu Shop", "shopify", &["source"], "active"),
        app_to_choose(CHOOSE_BQ, "Analytics BQ", "bigquery", &["source", "destination"], "active"),
        app_to_choose(CHOOSE_PIXEL, "Demo Pixel", "meta_ads", &["destination"], "inactive"),
    ]
}

/// [`apps_to_choose`] as the list shows them: plain text, each column padded to one width.
const APP_ROWS: [&str; 3] = [
    "a1b2c3d4...  Menu Shop     shopify   source               active",
    "e5f6a7b8...  Analytics BQ  bigquery  source, destination  active",
    "0c0d0e0f...  Demo Pixel    meta_ads  destination          inactive",
];

/// [`apps_to_choose`] in `account`: the list, and `{ data: <app> }` for every request about one
/// (get, pause, resume, delete, update).
async fn apps_to_choose_stub(account: &str) -> MockServer {
    apps_stub(account, apps_to_choose()).await
}

/// The ready platforms `GET /api/v1/catalog` lists (VE-3829), and each one's catalog entry.
async fn serve_platforms(server: &MockServer) {
    let ready = vec![
        platform("bigquery", "BigQuery", "self_serve", Value::Null),
        platform("shopify", "Shopify", "self_serve", Value::Null),
    ];
    serve_catalog(server, false, json!({ "data": ready, "meta": { "selfServeTotal": 2, "requestAccessTotal": 3 } }))
        .await;
    let defaults = json!({ "source": ["read_warehouse"], "destination": ["write_warehouse"] });
    for app_type in ["bigquery", "shopify"] {
        let entry = json!({ "data": { "appType": app_type, "defaultPermissions": defaults } });
        serve(server, "GET", &format!("{CATALOG}/{app_type}"), 200, entry).await;
    }
}

/// [`serve_platforms`] as the list shows them.
const PLATFORM_ROWS: [&str; 2] = ["BigQuery  bigquery  ads  source", "Shopify   shopify   ads  source"];

/// A terminal wide enough that a JSON error is one line.
const WIDE: (u16, u16) = (40, 400);

/// The lines the terminal shows, as [`screen`] reads them, up to its last line that shows anything.
fn shown_lines(terminal: &OnTerminal, (lines, columns): (usize, usize)) -> Vec<String> {
    let (mut shown, _) = screen(&terminal.screen, lines, columns);
    shown.truncate(shown.iter().rposition(|line| !line.is_empty()).map_or(0, |last| last + 1));
    shown
}

/// What the screen shows of a list: its lines between the title (the first line) and the hint.
fn listed_rows(terminal: &OnTerminal) -> Vec<String> {
    let shown = shown_lines(terminal, (40, 120));
    shown[1..shown.len() - 1].to_vec()
}

/// A credentials file in `sandbox`'s HOME, for `apps create --credentials-file`.
fn credentials_file(sandbox: &Sandbox) -> String {
    let file = sandbox.home.path().join("bq.json");
    std::fs::write(&file, r#"{ "type": "service_account" }"#).unwrap();
    file.display().to_string()
}

#[cfg(unix)]
#[tokio::test]
async fn a_missing_app_is_chosen_from_the_accounts_apps_and_the_command_runs_as_typed() {
    let server = apps_to_choose_stub("acct-alpha").await;
    let sandbox = Sandbox::new(&server.uri());
    let list = format!("GET {V1}/apps?limit=100&offset=0");
    for (command, flags) in [
        ("get", &[][..]),
        ("get", &["--json"]),
        ("pause", &[]),
        ("pause", &["--dry-run"]),
        ("resume", &["--output", "id"]),
        ("delete", &["--yes"]),
        ("update", &["--name", "Renamed"]),
        // No change asked for: the command says so after the choice, as when typed with the ID.
        ("update", &[]),
    ] {
        let args: Vec<&str> = ["apps", command].into_iter().chain(flags.iter().copied()).collect();
        let case = args.join(" ");
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, &args);
        terminal.wait_for("type to filter]");
        // Titled with the command; every app, newest first, the first marked; the menu's hint.
        let shown = shown_lines(&terminal, (40, 120));
        let mut expected = vec![format!("? vendo apps {command}")];
        expected
            .extend(APP_ROWS.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == 0 { '>' } else { ' ' })));
        expected.push("[↑↓ to move, enter to select, type to filter]".to_string());
        assert_eq!(shown, expected, "{case}");
        // Down to the second app, which Enter chooses: the answer names it by its short ID and name.
        terminal.press("\u{1b}[B");
        terminal.wait_for(&format!("> {}", APP_ROWS[1]));
        terminal.press("\r");
        let answer = format!("? vendo apps {command} e5f6a7b8... (Analytics BQ)");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        let asked = sent(&server).await[before..].to_vec();
        // The same command typed with the app's full ID, on a pipe.
        let typed_args: Vec<&str> = ["apps", command, CHOOSE_BQ].into_iter().chain(flags.iter().copied()).collect();
        let typed = sandbox.run(&typed_args);
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!(code, typed.status.code(), "{case}");
        assert_eq!((&asked[0], &asked[1..]), (&list, &typed_sent[..]), "{case}");
        // What it showed after the answer is what the typed command printed.
        let shown = shown_lines(&terminal, (40, 120));
        let answered = shown.iter().position(|line| *line == answer).unwrap();
        let after: Vec<&str> =
            shown[answered + 1..].iter().map(String::as_str).filter(|line| !line.is_empty()).collect();
        let typed_output = text(&typed.stdout) + &text(&typed.stderr);
        let printed: Vec<&str> = typed_output.lines().map(str::trim_end).filter(|line| !line.is_empty()).collect();
        assert_eq!(after, printed, "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_apps_by_name_short_id_or_any_column() {
    let server = apps_to_choose_stub("acct-alpha").await;
    let sandbox = Sandbox::new(&server.uri());
    for (typed, rows) in [
        ("ANALYTICS", &APP_ROWS[1..2]),
        ("0C0D0E0F...", &APP_ROWS[2..]),
        ("e5f6a7b8", &APP_ROWS[1..2]),
        ("destination", &APP_ROWS[1..]),
        ("shop", &APP_ROWS[..1]),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, &["apps", "get"]);
        terminal.wait_for("type to filter]");
        terminal.press(typed);
        terminal.wait_for(&format!("vendo apps get {typed}"));
        // The end of that frame: inquire shows the cursor again. It redraws only the lines that
        // changed, so the screen, not the stream, shows the rows left.
        terminal.wait_for("\u{1b}[?25h");
        let expected: Vec<String> =
            rows.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == 0 { '>' } else { ' ' })).collect();
        assert_eq!(listed_rows(&terminal), expected, "{typed}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{typed}");
    }
    // Enter chooses the first app left.
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "get"]);
    terminal.wait_for("type to filter]");
    terminal.press("pixel");
    terminal.wait_for("vendo apps get pixel");
    terminal.press("\r");
    terminal.wait_for("vendo apps get 0c0d0e0f... (Demo Pixel)");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await.last().unwrap(), &format!("GET {V1}/apps/{CHOOSE_PIXEL}"));
}

/// A second terminal a test gives `vendo` besides [`OnTerminal`]'s, read on a thread, so that the
/// test can wait for what it shows and type into it.
#[cfg(unix)]
struct SecondTerminal {
    controller: std::fs::File,
    /// The terminal end, kept until the test ends.
    terminal: std::os::fd::OwnedFd,
    output: std::sync::mpsc::Receiver<Vec<u8>>,
    shown: String,
}

#[cfg(unix)]
impl SecondTerminal {
    fn new() -> Self {
        use std::io::Read;
        let (controller, terminal, _) = pseudo_terminal_sized(40, 120);
        let mut reader = controller.try_clone().unwrap();
        let (tx, output) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        SecondTerminal { controller, terminal, output, shown: String::new() }
    }

    /// The terminal end, for `vendo`'s stdin or stderr.
    fn end(&self) -> Stdio {
        self.terminal.try_clone().unwrap().into()
    }

    fn wait_for(&mut self, expected: &str) {
        while !self.shown.contains(expected) {
            match self.output.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(chunk) => self.shown.push_str(&text(&chunk)),
                Err(_) => panic!("the second terminal never showed {expected:?}; it showed {:?}", self.shown),
            }
        }
    }

    fn press(&self, keys: &str) {
        use std::io::Write;
        (&self.controller).write_all(keys.as_bytes()).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_json_the_list_is_drawn_on_stderr_and_stdout_carries_only_the_json() {
    // `--profile beta` before the command, and `--json` after it, apply as typed.
    let server = apps_to_choose_stub("acct-beta").await;
    let sandbox = Sandbox::new(&server.uri());
    let mut keys = SecondTerminal::new();
    // The keys and the list on one terminal (stdin and stderr), stdout on another.
    let cmd = sandbox.command(&["--profile", "beta", "apps", "get", "--json"]);
    let mut terminal = OnTerminal::launch(cmd, (40, 120), "", Some(keys.end()), Some(keys.end()), true);
    keys.wait_for("type to filter]");
    keys.press("\r");
    let (stdout, code) = terminal.finish();
    keys.wait_for("vendo apps get a1b2c3d4... (Menu Shop)");
    let typed = sandbox.run(&["--profile", "beta", "apps", "get", MENU_APP, "--json"]);
    assert_eq!((code, plain(&stdout)), (Some(0), text(&typed.stdout)));
    let beta = "/api/v1/accounts/acct-beta/apps";
    assert_eq!(sent(&server).await[..2], [format!("GET {beta}?limit=100&offset=0"), format!("GET {beta}/{MENU_APP}")]);
}

#[cfg(unix)]
#[tokio::test]
async fn apps_delete_asks_which_app_then_asks_y_n_unless_yes() {
    let server = apps_to_choose_stub("acct-alpha").await;
    let sandbox = Sandbox::new(&server.uri());
    let list = format!("GET {V1}/apps?limit=100&offset=0");
    for (answer, deleted) in [("n\n", false), ("y\n", true)] {
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, &["apps", "delete"]);
        terminal.wait_for("type to filter]");
        terminal.press("\r");
        terminal.wait_for("vendo apps delete a1b2c3d4... (Menu Shop)");
        // The question names the app chosen, as it names one typed (VE-3823).
        let (shown, code) = answer_question(terminal, answer);
        assert_eq!(code, Some(0), "{shown:?}");
        assert!(plain(&shown).contains(&format!("Delete app a1b2c3d4...? (y/N) {answer}")), "{shown:?}");
        let mut expected = vec![list.clone()];
        expected.extend(deleted.then(|| format!("DELETE {V1}/apps/{MENU_APP}")));
        assert_eq!(sent(&server).await[before..], expected, "{answer:?}");
    }
    // `--yes`: no question.
    let before = sent(&server).await.len();
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "delete", "--yes"]);
    terminal.wait_for("type to filter]");
    terminal.press("\r");
    let (shown, code) = terminal.finish();
    assert_eq!(code, Some(0), "{shown:?}");
    assert!(!shown.contains("(y/N)") && plain(&shown).contains("App a1b2c3d4... deleted."), "{shown:?}");
    assert_eq!(sent(&server).await[before..], [list, format!("DELETE {V1}/apps/{MENU_APP}")]);
}

#[cfg(unix)]
#[tokio::test]
async fn apps_create_asks_for_a_ready_platform_then_a_name_and_creates_the_app_as_typed() {
    let server = MockServer::start().await;
    serve_platforms(&server).await;
    let created = app_to_choose(CHOOSE_BQ, "Analytics BQ", "bigquery", &["source"], "active");
    serve(&server, "POST", &format!("{V1}/apps"), 201, json!({ "data": created })).await;
    let sandbox = Sandbox::new(&server.uri());
    let credentials = credentials_file(&sandbox);
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "create", "--credentials-file", &credentials]);
    terminal.wait_for("type to filter]");
    // The platforms ready to connect, as `vendo catalog list` lists them by default.
    let expected: Vec<String> =
        PLATFORM_ROWS.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == 0 { '>' } else { ' ' })).collect();
    assert_eq!(listed_rows(&terminal), expected);
    assert_eq!(shown_lines(&terminal, (40, 120))[0], "? vendo apps create --type");
    terminal.press("SHOP");
    terminal.wait_for("vendo apps create --type SHOP");
    terminal.press("\r");
    terminal.wait_for("vendo apps create --type shopify");
    // Then the name, asked as a one-line question: an empty answer is refused.
    terminal.wait_for("vendo apps create --name");
    terminal.press("\r");
    terminal.wait_for("A response is required.");
    terminal.press("My Shop\r");
    terminal.wait_for("vendo apps create --name My Shop");
    let (_, code) = terminal.finish();
    let asked = sent(&server).await;
    // The same typed, on a pipe.
    let typed =
        sandbox.run(&["apps", "create", "--credentials-file", &credentials, "--type=shopify", "--name", "My Shop"]);
    let typed_sent = sent(&server).await[asked.len()..].to_vec();
    assert_eq!(code, Some(0));
    assert_eq!(typed.status.code(), Some(0), "{}", text(&typed.stderr));
    assert_eq!((asked[0].as_str(), &asked[1..]), ("GET /api/v1/catalog", &typed_sent[..]));
    assert_eq!(typed_sent.len(), 2, "{typed_sent:?}");
    assert!(
        typed_sent[1].starts_with(&format!("POST {V1}/apps ")) && typed_sent[1].contains("\"displayName\":\"My Shop\"")
    );
    let shown = shown_lines(&terminal, (40, 120));
    let answered = shown.iter().position(|line| line == "? vendo apps create --name My Shop").unwrap();
    let after: Vec<&str> = shown[answered + 1..].iter().map(String::as_str).filter(|line| !line.is_empty()).collect();
    let printed = text(&typed.stdout);
    assert_eq!(after, printed.lines().map(str::trim_end).filter(|line| !line.is_empty()).collect::<Vec<_>>());
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_the_list_or_the_question_leave_quietly() {
    // As at the group menu (VE-3826): exit 0, nothing run, and the screen the same for all three keys:
    // what was answered, the title and `<canceled>`, and the cursor on the next line.
    let server = apps_to_choose_stub("acct-alpha").await;
    serve_platforms(&server).await;
    let sandbox = Sandbox::new(&server.uri());
    let credentials = credentials_file(&sandbox);
    let earlier: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    let asking: [(&[&str], &str, &[&str]); 2] = [
        (&["apps", "get"], "", &["? vendo apps get <canceled>"]),
        (
            &["apps", "create", "--credentials-file", &credentials],
            "\r",
            &["? vendo apps create --type bigquery", "? vendo apps create --name <canceled>"],
        ),
    ];
    for (size, before) in [((40, 120), ""), ((16, 80), earlier.as_str())] {
        for (args, answers, last) in asking {
            let mut screens = Vec::new();
            for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
                let case = format!("{name} {}", args.join(" "));
                let requests = sent(&server).await.len();
                let mut terminal = OnTerminal::start_with(&sandbox, args, size, before, None);
                terminal.wait_for("type to filter]");
                terminal.press(answers);
                if !answers.is_empty() {
                    terminal.wait_for("vendo apps create --name");
                }
                terminal.press(key);
                let (rest, code) = terminal.finish();
                assert_eq!(code, Some(0), "{case}: {rest:?}");
                let (shown, cursor) = screen(&terminal.screen, size.0.into(), size.1.into());
                let end = shown.iter().rposition(|line| !line.is_empty()).unwrap() + 1;
                let tail: Vec<&str> = shown[end - last.len()..end].iter().map(String::as_str).collect();
                assert_eq!((tail.as_slice(), cursor), (last, (end, 0)), "{case}: {shown:#?}");
                let list = if answers.is_empty() {
                    format!("GET {V1}/apps?limit=100&offset=0")
                } else {
                    "GET /api/v1/catalog".into()
                };
                assert_eq!(sent(&server).await[requests..], [list], "{case}");
                screens.push((shown, cursor));
            }
            assert!(screens.iter().all(|screen| *screen == screens[0]), "{screens:#?}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_list_and_the_question_exit_as_ctrl_d_does_when_their_terminal_hangs_up() {
    let server = apps_to_choose_stub("acct-alpha").await;
    serve_platforms(&server).await;
    let sandbox = Sandbox::new(&server.uri());
    let credentials = credentials_file(&sandbox);
    let mut outcomes = Vec::new();
    let create = ["apps", "create", "--credentials-file", &credentials];
    for (args, answers) in [(&["apps", "get"][..], ""), (&create, "\r")] {
        let mut terminal = OnTerminal::start_detached(&sandbox, args);
        terminal.wait_for("type to filter]");
        terminal.press(answers);
        if !answers.is_empty() {
            terminal.wait_for("vendo apps create --name");
        }
        let (exited, ended) = hang_up(&mut terminal);
        outcomes.push((args, exited, ended));
    }
    assert!(
        outcomes.iter().all(|(_, exited, ended)| *exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "each should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {outcomes:#?}"
    );
    assert_eq!(sent(&server).await, [format!("GET {V1}/apps?limit=100&offset=0"), "GET /api/v1/catalog".to_string()]);
}

#[cfg(unix)]
#[tokio::test]
async fn with_nothing_to_choose_from_it_says_so_and_is_the_usage_error() {
    let server = MockServer::start().await;
    serve(&server, "GET", &format!("{V1}/apps"), 200, page_of(Vec::new(), 0, false)).await;
    serve_catalog(&server, false, json!({ "data": [], "meta": { "selfServeTotal": 0, "requestAccessTotal": 0 } }))
        .await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, nothing) in [
        (&["apps", "get"][..], "No apps to choose from."),
        (&["apps", "get", "--json"], "No apps to choose from."),
        (&["apps", "create"], "No platforms to choose from."),
    ] {
        let typed = sandbox.run(args);
        assert_eq!(typed.status.code(), Some(2));
        // Wide enough that the JSON error is one line.
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        let (_, code) = terminal.finish();
        let expected = format!("{nothing}\n{}", text(&typed.stderr));
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
        assert_eq!((code, shown), (Some(2), expected.trim_end().to_string()), "{args:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_list_that_fails_is_the_error_and_nothing_is_asked() {
    let server = MockServer::start().await;
    let refusal = json!({ "error": { "code": "INTERNAL_ERROR", "message": "Database unavailable" } });
    serve(&server, "GET", &format!("{V1}/apps"), 500, refusal).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "get"]);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (40, 120));
    assert_eq!((code, shown[0].as_str()), (Some(1), "Error: Database unavailable"), "{shown:#?}");
    assert!(shown.len() == 2 && shown[1].starts_with("Request ID: cli-"), "{shown:#?}");
    let mut terminal = OnTerminal::start_with(&sandbox, &["apps", "get", "--json"], WIDE, "", None);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into()));
    assert_eq!((code, shown.len()), (Some(1), 1), "{shown:#?}");
    let error: Value = serde_json::from_str(&shown[0]).unwrap();
    assert_eq!(
        (&error["error"]["message"], &error["error"]["code"], &error["error"]["status"]),
        (&json!("Database unavailable"), &json!("INTERNAL_ERROR"), &json!(500))
    );
    assert_eq!(sent(&server).await, vec![format!("GET {V1}/apps?limit=100&offset=0"); 2]);
}

#[cfg(unix)]
#[tokio::test]
async fn without_a_key_or_an_account_it_is_that_error_before_anything_is_asked() {
    let server = MockServer::start().await;
    serve_platforms(&server).await;
    // No key: the error `vendo apps list` gives, also for the platforms, which need a key.
    let sandbox = Sandbox::without_api_keys(&server.uri());
    let no_key = text(&sandbox.run(&["apps", "list"]).stderr);
    assert!(no_key.starts_with("Error: No API key configured."), "{no_key}");
    for args in [&["apps", "get"][..], &["apps", "create"]] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        assert_eq!((code, shown_lines(&terminal, (40, 120)).join("\n")), (Some(1), no_key.trim_end().to_string()));
    }
    let mut terminal = OnTerminal::start_with(&sandbox, &["apps", "get", "--json"], WIDE, "", None);
    let (_, code) = terminal.finish();
    let json_error = text(&sandbox.run(&["apps", "list", "--json"]).stderr);
    let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
    assert_eq!((code, shown), (Some(1), json_error.trim_end().to_string()));
    // A VENDO_PROFILE that names no profile: its error.
    let sandbox = Sandbox::new(&server.uri());
    let mut unknown = sandbox.command(&["apps", "list"]);
    let unknown = text(&unknown.env("VENDO_PROFILE", "gamma").output().unwrap().stderr);
    assert!(unknown.contains("VENDO_PROFILE"), "{unknown}");
    let mut terminal = OnTerminal::start_env(&sandbox, &["apps", "get"], &[("VENDO_PROFILE", "gamma")]);
    let (_, code) = terminal.finish();
    assert_eq!((code, shown_lines(&terminal, (40, 120)).join("\n")), (Some(1), unknown.trim_end().to_string()));
    assert_eq!(sent(&server).await, Vec::<String>::new());

    // A key but no account: the apps cannot be listed, so the error `vendo apps list` gives.
    let mut config = sandbox.config();
    for profile in config["profiles"].as_object_mut().unwrap().values_mut() {
        profile.as_object_mut().unwrap().remove("accountId");
    }
    std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
    let no_account = text(&sandbox.run(&["apps", "list"]).stderr);
    assert!(no_account.starts_with("Error: No account configured."), "{no_account}");
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "get"]);
    let (_, code) = terminal.finish();
    assert_eq!((code, shown_lines(&terminal, (40, 120)).join("\n")), (Some(1), no_account.trim_end().to_string()));
    assert_eq!(sent(&server).await, Vec::<String>::new());
    // The platforms need no account (the catalog route is the key's): they are listed.
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "create"]);
    terminal.wait_for(PLATFORM_ROWS[1]);
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await, ["GET /api/v1/catalog"]);
}

#[cfg(unix)]
#[tokio::test]
async fn of_more_than_500_apps_the_newest_500_are_listed_and_the_hint_says_so() {
    let server = MockServer::start().await;
    for page in 0..6u64 {
        let apps: Vec<Value> = (page * 100..page * 100 + 100)
            .map(|n| {
                app_to_choose(&uuid_with(&format!("{n:08x}"), n), &format!("App {n}"), "shopify", &["source"], "active")
            })
            .collect();
        Mock::given(wiremock::matchers::method("GET"))
            .and(path(format!("{V1}/apps")))
            .and(wiremock::matchers::query_param("offset", (page * 100).to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(page_of(apps, (page * 100) as usize, true)))
            .mount(&server)
            .await;
    }
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "get"]);
    terminal.wait_for("[↑↓ to move, enter to select, type to filter · newest 500 shown]");
    // The last of them can be found by typing.
    terminal.press("App 499");
    terminal.wait_for("000001f3...  App 499");
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    let pages: Vec<String> = (0..5).map(|page| format!("GET {V1}/apps?limit=100&offset={}", page * 100)).collect();
    assert_eq!(sent(&server).await, pages);
}

#[cfg(unix)]
#[tokio::test]
async fn keys_typed_before_a_list_or_question_opens_are_thrown_away() {
    // An Enter typed before the list opened (twice with the command, or again while
    // `Fetching apps...` showed) waited at the terminal, and inquire took it as Enter and chose the
    // first app, never shown: `apps pause` paused it and `apps delete --yes` deleted it. One typed
    // with the Enter that answered the menu, or the list before a question, did the same from
    // crossterm's queue. The list and the question now open with nothing typed ahead.
    let server = MockServer::start().await;
    let apps = format!("{V1}/apps");
    // The list answers after a while, as on a slow connection.
    Mock::given(wiremock::matchers::method("GET"))
        .and(path(apps.clone()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(page_of(apps_to_choose(), 0, false))
                .set_delay(std::time::Duration::from_millis(1500)),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path_regex(format!("^{apps}/.+$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": apps_to_choose()[0] })))
        .mount(&server)
        .await;
    serve_platforms(&server).await;
    let sandbox = Sandbox::new(&server.uri());
    let list = format!("GET {V1}/apps?limit=100&offset=0");
    // Typed with the command, and again while the list loads: the list opens and waits for a key.
    // With a `TERM`, as in a terminal window, the spinner shows while it loads.
    for args in [&["apps", "pause"][..], &["apps", "delete", "--yes"]] {
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start_env(&sandbox, args, &[("TERM", "xterm-256color")]);
        terminal.press("\r");
        terminal.wait_for("Fetching apps...");
        terminal.press("\r");
        terminal.wait_for("type to filter]");
        terminal.press("\u{1b}[B");
        terminal.wait_for(&format!("> {}", APP_ROWS[1]));
        terminal.press("\u{1b}");
        let (shown, code) = terminal.finish();
        assert_eq!(code, Some(0), "{args:?}: {shown:?}");
        assert!(plain(&shown).contains(&format!("vendo {} <canceled>", args[..2].join(" "))), "{args:?}: {shown:?}");
        assert_eq!(sent(&server).await[before..], [list.as_str()], "{args:?}");
    }
    // Typed with the Enter that chose `pause` in the menu.
    let before = sent(&server).await.len();
    let mut terminal = OnTerminal::start(&sandbox, &["apps"]);
    terminal.wait_for("Update an app");
    terminal.press("pause\r\r");
    terminal.wait_for("vendo apps pause");
    terminal.wait_for(APP_ROWS[2]);
    terminal.press("\u{1b}[B");
    terminal.wait_for(&format!("> {}", APP_ROWS[1]));
    terminal.press("\u{1b}");
    let (shown, code) = terminal.finish();
    assert_eq!(code, Some(0), "{shown:?}");
    assert!(plain(&shown).contains("vendo apps pause <canceled>"), "{shown:?}");
    assert_eq!(sent(&server).await[before..], [list]);
    // Typed with the Enter that chose a platform: the name is asked, not refused as empty.
    let before = sent(&server).await.len();
    let credentials = credentials_file(&sandbox);
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "create", "--credentials-file", &credentials]);
    terminal.wait_for("type to filter]");
    terminal.press("\r\r");
    terminal.wait_for("vendo apps create --type bigquery");
    terminal.wait_for("vendo apps create --name");
    terminal.press("x");
    terminal.wait_for("vendo apps create --name x");
    terminal.press("\u{1b}");
    let (_, code) = terminal.finish();
    assert_eq!(code, Some(0), "{:?}", terminal.screen);
    assert!(!terminal.screen.contains("A response is required."), "{:?}", terminal.screen);
    assert_eq!(sent(&server).await[before..], ["GET /api/v1/catalog"]);
}

#[cfg(unix)]
#[tokio::test]
async fn names_in_wide_characters_keep_the_columns_in_line_and_the_list_on_a_short_screen() {
    // A wide character such as 東 takes two columns on the screen. The list padded its columns and
    // fitted the screen counting one: a name in Japanese put the columns after it out of line, and
    // rows that took two lines, counted as one, ran the list off the top of a short screen, the
    // title and the row marked `>` with it, so that Enter chose an app never seen.
    let server = MockServer::start().await;
    let apps = vec![
        app_to_choose(MENU_APP, "東京ストア本店", "shopify", &["source"], "active"),
        app_to_choose(CHOOSE_BQ, "Plain Name", "bigquery", &["source"], "active"),
        app_to_choose(CHOOSE_PIXEL, "大阪", "meta_ads", &["destination"], "inactive"),
    ];
    serve(&server, "GET", &format!("{V1}/apps"), 200, page_of(apps, 0, false)).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "get"]);
    terminal.wait_for("type to filter]");
    assert_eq!(
        shown_lines(&terminal, (40, 120)),
        [
            "? vendo apps get",
            "> a1b2c3d4...  東京ストア本店  shopify   source       active",
            "  e5f6a7b8...  Plain Name      bigquery  source       active",
            "  0c0d0e0f...  大阪            meta_ads  destination  inactive",
            "[↑↓ to move, enter to select, type to filter]",
        ]
    );
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));

    // Eight apps whose rows take two lines each on 80 columns, on a screen of 10 lines.
    let server = MockServer::start().await;
    let name = |n: u64| format!("{}東京{n}", "東京ストア本店".repeat(3));
    let id = |n: u64| uuid_with(&format!("b000000{n}"), n);
    let apps: Vec<Value> = (0..8).map(|n| app_to_choose(&id(n), &name(n), "shopify", &["source"], "active")).collect();
    serve(&server, "GET", &format!("{V1}/apps"), 200, page_of(apps.clone(), 0, false)).await;
    serve(&server, "GET", &format!("{V1}/apps/{}", id(1)), 200, json!({ "data": apps[1] })).await;
    let sandbox = Sandbox::new(&server.uri());
    let (lines, columns) = (10, 80);
    let mut terminal = OnTerminal::start_with(&sandbox, &["apps", "get"], (lines, columns), "", None);
    terminal.wait_for("type to filter]");
    // The end of that frame: inquire shows the cursor again.
    terminal.wait_for("\u{1b}[?25h");
    // As many as fit between the title and the hint, with a line to spare, and the list scrolls. A
    // row's first line ends at `source`, in the 79th column: the 80th, the screen's last, stays free.
    let row = |mark: char, n: u64| [format!("{mark} b000000{n}...  {}  shopify  source", name(n)), "  active".into()];
    let mut expected = vec!["? vendo apps get".to_string()];
    expected.extend([row('>', 0), row(' ', 1), row('v', 2)].concat());
    expected.extend(["[↑↓ to move, enter to select, type to filter]".to_string(), String::new(), String::new()]);
    assert_eq!(screen(&terminal.screen, lines.into(), columns.into()).0, expected);
    // Down marks the second app, on the screen, and Enter chooses it.
    terminal.press("\u{1b}[B");
    terminal.wait_for("> b0000001...");
    terminal.wait_for("\u{1b}[?25h");
    let (shown, _) = screen(&terminal.screen, lines.into(), columns.into());
    assert_eq!((shown[0].as_str(), shown[3].as_str()), ("? vendo apps get", row('>', 1)[0].as_str()), "{shown:#?}");
    terminal.press("\r");
    terminal.wait_for(&format!("vendo apps get b0000001... ({})", name(1)));
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await, [format!("GET {V1}/apps?limit=100&offset=0"), format!("GET {V1}/apps/{}", id(1))]);
}

/// `line` as a list draws it on a screen `columns` wide, each line leaving the last column free:
/// broken every `columns - 1` characters (characters one column wide), without trailing spaces.
fn broken(line: &str, columns: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    chars.chunks(columns - 1).map(|chunk| chunk.iter().collect::<String>().trim_end().to_string()).collect()
}

#[cfg(unix)]
#[tokio::test]
async fn rows_that_wrap_stay_whole_when_the_list_or_menu_is_drawn_again() {
    // inquire draws again each line that changed, then erases to the end of the line. After a
    // character in the screen's last column the cursor waits there, and where the terminal follows
    // xterm (`screen` here) the erase took that character off the screen: from the first key on, a
    // row that filled a line lost a character there, a subject ID one at each place it wrapped. The
    // rows, the hint and the answered line leave the last column free.
    let (lines, columns) = (60, 40);
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // The menu of `vendo destinations` (VE-3826), whose rows take up to three lines on 40 columns:
    // down twice and up twice shows what it showed first, every line short of the last column.
    let mut terminal = OnTerminal::start_with(&sandbox, &["destinations"], (lines, columns), "", None);
    terminal.wait_for("\u{1b}[?25h");
    let first = screen(&terminal.screen, lines.into(), columns.into()).0;
    assert!(first.iter().all(|line| line.chars().count() < columns.into()), "{first:#?}");
    assert!(first.iter().filter(|line| !line.is_empty()).count() > 12, "no row wraps: {first:#?}");
    for key in ["\u{1b}[B", "\u{1b}[B", "\u{1b}[A", "\u{1b}[A"] {
        terminal.press(key);
        terminal.wait_for("\u{1b}[?25h");
    }
    assert_eq!(screen(&terminal.screen, lines.into(), columns.into()).0, first);
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));

    // `dictionary get`: an event's row takes two lines; down twice and up once marks the second.
    let mut terminal = OnTerminal::start_with(&sandbox, &["dictionary", "get"], (lines, columns), "", None);
    terminal.wait_for("> event");
    terminal.wait_for("\u{1b}[?25h");
    terminal.press("\r");
    terminal.wait_for("> Checkout Completed");
    terminal.wait_for("\u{1b}[?25h");
    for key in ["\u{1b}[B", "\u{1b}[B", "\u{1b}[A"] {
        terminal.press(key);
        terminal.wait_for("\u{1b}[?25h");
    }
    let (shown, _) = screen(&terminal.screen, lines.into(), columns.into());
    let mut expected = broken("? vendo dictionary get · subject type event", columns.into());
    expected.push("? vendo dictionary get".to_string());
    for (i, row) in EVENT_ROWS.iter().enumerate() {
        expected.extend(broken(&format!("{} {row}", if i == 1 { '>' } else { ' ' }), columns.into()));
    }
    expected.extend(broken(HINT, columns.into()));
    assert_eq!(shown[..expected.len()], expected[..], "{shown:#?}");
    assert!(shown[expected.len()..].iter().all(String::is_empty), "{shown:#?}");
    // The answered line, whole: the title, then the subject ID and the name.
    terminal.press("\r");
    terminal.wait_for("(Page Viewed)");
    terminal.wait_for("\u{1b}[?25h");
    let seen = terminal.seen;
    let (shown, _) = screen(&terminal.screen[..seen], lines.into(), columns.into());
    let answered = broken(&format!("? vendo dictionary get {EVENT_PAGE} (Page Viewed)"), columns.into());
    let at = expected.len() - EVENT_ROWS.len() * 2 - 3;
    assert_eq!(shown[at..at + 2], answered[..], "{shown:#?}");
    assert!(shown[at + 2..].iter().all(String::is_empty), "{shown:#?}");
    assert_eq!(terminal.finish().1, Some(0));
    let looked_up = sent(&server).await.last().cloned().unwrap();
    assert!(looked_up.contains(EVENT_PAGE), "{looked_up}");
}

// ── VE-3881, sources and destinations ────────────────────────────────────────
// A source or destination missing its ID is chosen from the account's list (newest first, as
// `vendo sources list` and `vendo destinations list` list them); `sources create` asks for its app
// from the apps and then for the app's type, the one sync type the API takes for it, in a list of
// that one row; `destinations create` asks for its app, then a data type from the 13 the API takes,
// then the config file's path as a one-line question.

const SOURCE_SHOP: &str = "5e6f7a8b-0000-4000-8000-000000000011";
const SOURCE_BQ: &str = "6f7a8b9c-0000-4000-8000-000000000012";
const SOURCE_BARE: &str = "7a8b9c0d-0000-4000-8000-000000000013";
const DEST_EVENTS: &str = "8b9c0d1e-0000-4000-8000-000000000021";
const DEST_AUDIENCES: &str = "9c0d1e2f-0000-4000-8000-000000000022";
const DEST_ONE_APP: &str = "0d1e2f3a-0000-4000-8000-000000000023";

/// The sources to choose from, newest first; the last has no app name.
fn sources_to_choose() -> Vec<Value> {
    let source = |id: &str, name: Value, sync_type: &str, status: &str| {
        json!({
            "id": id, "appId": MENU_APP, "appName": name, "syncType": sync_type, "state": "active",
            "integrationStatus": status, "lastSyncAt": null, "createdAt": null,
        })
    };
    vec![
        source(SOURCE_SHOP, json!("Menu Shop"), "shopify", "healthy"),
        source(SOURCE_BQ, json!("Analytics BQ"), "bigquery", "warning"),
        source(SOURCE_BARE, Value::Null, "meta_ads", "error"),
    ]
}

/// [`sources_to_choose`] as the list shows them: the table's dash for the missing name.
const SOURCE_ROWS: [&str; 3] = [
    "5e6f7a8b...  Menu Shop     shopify   healthy",
    "6f7a8b9c...  Analytics BQ  bigquery  warning",
    "7a8b9c0d...  —             meta_ads  error",
];

/// The destinations to choose from, newest first; the last has no source app.
fn destinations_to_choose() -> Vec<Value> {
    let destination = |id: &str, source: Value, destination: &str, data_type: &str, status: &str| {
        json!({
            "id": id, "sourceAppName": source, "destinationAppName": destination, "destinationAppId": CHOOSE_BQ,
            "dataType": data_type, "state": "active", "status": status, "lastSyncAt": null, "createdAt": null,
        })
    };
    vec![
        destination(DEST_EVENTS, json!("Menu Shop"), "Analytics BQ", "events", "active"),
        destination(DEST_AUDIENCES, json!("Analytics BQ"), "Demo Pixel", "audiences", "paused"),
        destination(DEST_ONE_APP, Value::Null, "Demo Pixel", "conversions", "error"),
    ]
}

/// [`destinations_to_choose`] as the list shows them: the two apps as `destinations get` titles them.
const DESTINATION_ROWS: [&str; 3] = [
    "8b9c0d1e...  Menu Shop → Analytics BQ   events       active",
    "9c0d1e2f...  Analytics BQ → Demo Pixel  audiences    paused",
    "0d1e2f3a...  — → Demo Pixel             conversions  error",
];

/// The data types `destinations create --data-type` lists: vendo-web-v2's `DataTypeSchema`, in its
/// order.
const DATA_TYPE_ROWS: [&str; 13] = [
    "events",
    "user_properties",
    "group_properties",
    "ad_data",
    "revenue",
    "contacts",
    "email_messages",
    "custom",
    "event",
    "user",
    "group",
    "audiences",
    "conversions",
];

/// [`apps_to_choose_stub`] with [`sources_to_choose`] and [`destinations_to_choose`] in acct-alpha:
/// the lists, `{ data: <item> }` for every request about one, no active jobs, a refresh-source
/// that finds the data there, and `create` for both.
async fn pipeline_to_choose_stub() -> MockServer {
    let server = apps_to_choose_stub("acct-alpha").await;
    serve(&server, "GET", &format!("{V1}/jobs"), 200, page_of(Vec::new(), 0, false)).await;
    for (list, items) in [("sources", sources_to_choose()), ("connections", destinations_to_choose())] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(items.clone(), 0, false)).await;
        serve(&server, "POST", &format!("{V1}/{list}"), 201, json!({ "data": items[0] })).await;
        for item in items {
            let id = item["id"].as_str().unwrap();
            // Before the catch-all below: wiremock answers with the first match mounted.
            let refresh = format!("{V1}/{list}/{id}/refresh-source");
            serve(&server, "POST", &refresh, 200, json!({ "data": { "status": "ready" } })).await;
            Mock::given(wiremock::matchers::path_regex(format!("^{V1}/{list}/{id}(/.*)?$")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": item })))
                .mount(&server)
                .await;
        }
    }
    server
}

/// What the screen shows after `answer`, without blank lines: the command's output.
fn after_answer(terminal: &OnTerminal, answer: &str) -> Vec<String> {
    let shown = shown_lines(terminal, (40, 120));
    let answered = shown.iter().position(|line| line == answer).unwrap_or_else(|| panic!("{answer:?}: {shown:#?}"));
    shown[answered + 1..].iter().filter(|line| !line.is_empty()).cloned().collect()
}

/// What `out` printed, stdout then stderr, as lines without blank ones.
fn printed_lines(out: &Output) -> Vec<String> {
    let printed = text(&out.stdout) + &text(&out.stderr);
    printed.lines().map(str::trim_end).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// `rows` as an open list shows them, the first marked.
fn marked(rows: &[&str]) -> Vec<String> {
    rows.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == 0 { '>' } else { ' ' })).collect()
}

#[cfg(unix)]
#[tokio::test]
async fn a_missing_source_or_destination_is_chosen_from_the_accounts_list_and_the_command_runs_as_typed() {
    let server = pipeline_to_choose_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let window = ["--from", "2026-06-29", "--to", "2026-07-02"];
    let sources = (&SOURCE_ROWS, SOURCE_BQ, "6f7a8b9c... (Analytics BQ)", "sources");
    let destinations = (&DESTINATION_ROWS, DEST_AUDIENCES, "9c0d1e2f... (Analytics BQ → Demo Pixel)", "connections");
    let mut cases: Vec<(Vec<&str>, _)> = Vec::new();
    for (command, flags) in [
        ("get", &[][..]),
        ("get", &["--json"]),
        ("sync", &[]),
        ("sync", &["--dry-run"]),
        ("pause", &[]),
        ("pause", &["--dry-run"]),
        ("resume", &["--output", "id"]),
        ("delete", &["--yes"]),
        ("update", &["--frequency", "6"]),
        // No change asked for: the command says so after the choice, as when typed with the ID.
        ("update", &[]),
    ] {
        cases.push(([&["sources", command][..], flags].concat(), sources));
    }
    for (command, flags) in [
        ("get", &[][..]),
        ("sync", &[]),
        ("refresh-source", &window),
        ("pause", &["--dry-run"]),
        ("resume", &[]),
        ("delete", &["--yes"]),
        ("update", &["--frequency", "2"]),
    ] {
        cases.push(([&["destinations", command][..], flags].concat(), destinations));
    }
    // The hidden names of destinations ask as destinations does, titled with the tree's name.
    cases.push((vec!["int", "get"], destinations));
    cases.push((vec!["integrations", "sync", "--dry-run"], destinations));
    for (args, (rows, chosen, named, list)) in cases {
        let case = args.join(" ");
        let path = format!("vendo {} {}", if args[0] == "sources" { "sources" } else { "destinations" }, args[1]);
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, &args);
        terminal.wait_for("type to filter]");
        // Titled with the command; every row, newest first, the first marked; the menu's hint.
        let mut expected = vec![format!("? {path}")];
        expected.extend(marked(rows));
        expected.push("[↑↓ to move, enter to select, type to filter]".to_string());
        assert_eq!(shown_lines(&terminal, (40, 120)), expected, "{case}");
        // Down to the second, which Enter chooses: the answer names it by its short ID and name.
        terminal.press("\u{1b}[B");
        terminal.wait_for(&format!("> {}", rows[1]));
        terminal.press("\r");
        let answer = format!("? {path} {named}");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        let mut asked = sent(&server).await[before..].to_vec();
        // The same command typed with the full ID, on a pipe.
        let typed_args: Vec<&str> = [&args[..2], &[chosen][..], &args[2..]].concat();
        let typed = sandbox.run(&typed_args);
        let mut typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!(code, typed.status.code(), "{case}");
        assert_eq!(asked.remove(0), format!("GET {V1}/{list}?limit=100&offset=0"), "{case}");
        // `get` and a dry run read the item and its active job at once, in either order.
        asked.sort();
        typed_sent.sort();
        assert_eq!(asked, typed_sent, "{case}");
        // What it showed after the answer is what the typed command printed.
        assert_eq!(after_answer(&terminal, &answer), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_sources_and_destinations_by_any_column() {
    let server = pipeline_to_choose_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, typed, rows) in [
        (&["sources", "get"][..], "META", &SOURCE_ROWS[2..]),
        (&["sources", "get"], "6f7a8b9c", &SOURCE_ROWS[1..2]),
        (&["sources", "get"], "warn", &SOURCE_ROWS[1..2]),
        (&["destinations", "get"], "demo pixel", &DESTINATION_ROWS[1..]),
        (&["destinations", "get"], "AUDIENCES", &DESTINATION_ROWS[1..2]),
        (&["destinations", "get"], "shop → a", &DESTINATION_ROWS[..1]),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        terminal.press(typed);
        terminal.wait_for(&format!("{} {typed}", args.join(" ")));
        // The end of that frame: inquire shows the cursor again.
        terminal.wait_for("\u{1b}[?25h");
        assert_eq!(listed_rows(&terminal), marked(rows), "{typed}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{typed}");
    }
    // Enter chooses the first row left.
    let mut terminal = OnTerminal::start(&sandbox, &["destinations", "get", "--json"]);
    terminal.wait_for("type to filter]");
    terminal.press("conversions");
    terminal.wait_for("vendo destinations get conversions");
    terminal.press("\r");
    terminal.wait_for("vendo destinations get 0d1e2f3a... (— → Demo Pixel)");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await.last().unwrap(), &format!("GET {V1}/connections/{DEST_ONE_APP}"));
    // A source without its app's name is answered by its short ID alone.
    let mut terminal = OnTerminal::start(&sandbox, &["sources", "get", "--json"]);
    terminal.wait_for("type to filter]");
    terminal.press("meta\r");
    let (_, code) = terminal.finish();
    assert_eq!(code, Some(0));
    assert!(shown_lines(&terminal, (40, 120)).iter().any(|line| line == "? vendo sources get 7a8b9c0d..."));
    assert_eq!(sent(&server).await.last().unwrap(), &format!("GET {V1}/sources/{SOURCE_BARE}"));
}

#[cfg(unix)]
#[tokio::test]
async fn sources_create_asks_for_the_app_then_its_type_and_creates_the_source_as_typed() {
    let server = pipeline_to_choose_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let apps = format!("GET {V1}/apps?limit=100&offset=0");
    let app_bq = format!("GET {V1}/apps/{CHOOSE_BQ}");
    // (words, the app's answer if listed, the type listed, the words typed instead, what the asking
    // sent before the command's own requests)
    let cases = [
        // Neither: the app from the account's apps, then its type, the one row.
        (
            &["sources", "create", "--run-now"][..],
            Some("a1b2c3d4... (Menu Shop)"),
            "shopify",
            vec!["sources", "create", "--run-now", "--app", MENU_APP, "--sync-type", "shopify"],
            vec![apps.clone()],
        ),
        // The app typed by its full ID: read for its type.
        (
            &["sources", "create", "--app", CHOOSE_BQ][..],
            None,
            "bigquery",
            vec!["sources", "create", "--app", CHOOSE_BQ, "--sync-type", "bigquery"],
            vec![app_bq.clone()],
        ),
        // By its short ID: looked up, read, and looked up again by the command (VE-3831).
        (
            &["sources", "create", "--app", "e5f6a7b8..."][..],
            None,
            "bigquery",
            vec!["sources", "create", "--app", "e5f6a7b8...", "--sync-type", "bigquery"],
            vec![apps.clone(), app_bq.clone()],
        ),
        // The type typed: the app from the list, not narrowed by it (Q3).
        (
            &["sources", "create", "--sync-type", "shopify", "--json"][..],
            Some("a1b2c3d4... (Menu Shop)"),
            "",
            vec!["sources", "create", "--sync-type", "shopify", "--json", "--app", MENU_APP],
            vec![apps.clone()],
        ),
    ];
    for (args, app, sync_type, typed_args, asking) in cases {
        let case = args.join(" ");
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, args);
        if let Some(app) = app {
            terminal.wait_for("type to filter]");
            let shown = shown_lines(&terminal, (40, 120));
            assert_eq!(shown[0], "? vendo sources create --app", "{case}");
            assert_eq!(shown[1..shown.len() - 1], marked(&APP_ROWS), "{case}");
            terminal.press("\r");
            terminal.wait_for(&format!("vendo sources create --app {app}"));
        }
        let mut last = app.map(|app| format!("? vendo sources create --app {app}"));
        if !sync_type.is_empty() {
            terminal.wait_for("vendo sources create --sync-type");
            terminal.wait_for("type to filter]");
            terminal.wait_for("\u{1b}[?25h");
            let shown = shown_lines(&terminal, (40, 120));
            let title = shown.iter().rposition(|line| line == "? vendo sources create --sync-type").unwrap();
            assert_eq!(
                shown[title + 1..],
                [format!("> {sync_type}"), "[↑↓ to move, enter to select, type to filter]".into()],
                "{case}"
            );
            terminal.press("\r");
            let answer = format!("? vendo sources create --sync-type {sync_type}");
            terminal.wait_for(&answer[2..]);
            last = Some(answer);
        }
        let (_, code) = terminal.finish();
        let asked = sent(&server).await[before..].to_vec();
        let typed = sandbox.run(&typed_args);
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{case}: {}", text(&typed.stderr));
        assert_eq!(asked[..asking.len()], asking[..], "{case}");
        assert_eq!(asked[asking.len()..], typed_sent[..], "{case}");
        assert!(typed_sent.last().unwrap().starts_with(&format!("POST {V1}/sources ")), "{case}: {typed_sent:?}");
        assert_eq!(after_answer(&terminal, &last.unwrap()), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_typed_app_whose_type_cannot_be_read_is_that_error_and_nothing_is_asked() {
    // Q3's default: the app's type cannot be listed, so the command stops with the error reading
    // the app gives, exit 1, as `vendo apps get` with the same ID does.
    let server = pipeline_to_choose_stub().await;
    let gone = "ffffffff-0000-4000-8000-000000000099";
    let not_found = json!({ "error": { "code": "NOT_FOUND", "message": "App not found" } });
    serve(&server, "GET", &format!("{V1}/apps/{gone}"), 404, not_found).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["sources", "create", "--app", gone]);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (40, 120));
    let typed = text(&sandbox.run(&["apps", "get", gone]).stderr);
    assert_eq!((code, shown[0].as_str()), (Some(1), typed.lines().next().unwrap()), "{shown:#?}");
    assert_eq!(shown[0], "Error: App not found");
    assert!(shown.len() == 2 && shown[1].starts_with("Request ID: "), "{shown:#?}");
    // With `--json`, the JSON error.
    let mut terminal =
        OnTerminal::start_with(&sandbox, &["sources", "create", "--app", gone, "--json"], WIDE, "", None);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into()));
    assert_eq!((code, shown.len()), (Some(1), 1), "{shown:#?}");
    let error: Value = serde_json::from_str(&shown[0]).unwrap();
    assert_eq!((&error["error"]["message"], &error["error"]["status"]), (&json!("App not found"), &json!(404)));
    // A short ID that several apps' IDs start with: the command's own refusal (VE-3831).
    let twins = vec![
        app_to_choose(&uuid_with("abcdef01", 1), "Twin One", "shopify", &["source"], "active"),
        app_to_choose(&uuid_with("abcdef01", 2), "Twin Two", "shopify", &["source"], "active"),
    ];
    let server = MockServer::start().await;
    serve(&server, "GET", &format!("{V1}/apps"), 200, page_of(twins, 0, false)).await;
    let sandbox = Sandbox::new(&server.uri());
    let refused = text(&sandbox.run(&["sources", "create", "--app", "abcdef01", "--sync-type", "shopify"]).stderr);
    assert!(refused.starts_with("Error: Short ID abcdef01 matches 2 apps:"), "{refused}");
    let mut terminal = OnTerminal::start(&sandbox, &["sources", "create", "--app", "abcdef01"]);
    let (_, code) = terminal.finish();
    assert_eq!((code, shown_lines(&terminal, (40, 120)).join("\n")), (Some(1), refused.trim_end().to_string()));
    assert!(!terminal.screen.contains("type to filter"));
}

#[cfg(unix)]
#[tokio::test]
async fn destinations_create_asks_for_the_app_the_data_type_and_the_config_file_in_clap_order() {
    let server = pipeline_to_choose_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let home = sandbox.home.path().to_path_buf();
    std::fs::write(home.join("tasks.json"), r#"{ "tasks": [{ "id": "page_views" }] }"#).unwrap();
    let in_home = |args: &[&str]| {
        let mut cmd = sandbox.command(args);
        cmd.current_dir(&home);
        cmd
    };
    // All three: the app, then a data type, then the path, relative to where vendo runs (Q8).
    let before = sent(&server).await.len();
    let args = ["destinations", "create", "--run-now"];
    let mut terminal = OnTerminal::spawn(in_home(&args), (40, 120), "", None, None);
    terminal.wait_for("type to filter]");
    let shown = shown_lines(&terminal, (40, 120));
    assert_eq!(shown[0], "? vendo destinations create --dest-app");
    assert_eq!(shown[1..shown.len() - 1], marked(&APP_ROWS));
    terminal.press("\u{1b}[B");
    terminal.wait_for(&format!("> {}", APP_ROWS[1]));
    terminal.press("\r");
    terminal.wait_for("vendo destinations create --dest-app e5f6a7b8... (Analytics BQ)");
    terminal.wait_for("vendo destinations create --data-type");
    terminal.wait_for("type to filter]");
    terminal.wait_for("\u{1b}[?25h");
    // Every data type the API takes, in its order.
    let shown = shown_lines(&terminal, (40, 120));
    let title = shown.iter().rposition(|line| line == "? vendo destinations create --data-type").unwrap();
    assert_eq!(shown[title + 1..shown.len() - 1], marked(&DATA_TYPE_ROWS));
    terminal.press("aud");
    terminal.wait_for("vendo destinations create --data-type aud");
    terminal.press("\r");
    terminal.wait_for("vendo destinations create --data-type audiences");
    // Then the path, as a one-line question: an empty answer is refused.
    terminal.wait_for("vendo destinations create --config-file");
    terminal.press("\r");
    terminal.wait_for("A response is required.");
    terminal.press("tasks.json\r");
    let answer = "? vendo destinations create --config-file tasks.json";
    terminal.wait_for(&answer[2..]);
    let (_, code) = terminal.finish();
    let asked = sent(&server).await[before..].to_vec();
    let typed_args =
        ["destinations", "create", "--run-now", "--dest-app", CHOOSE_BQ, "--data-type", "audiences", "--config-file"];
    let typed = in_home(&[&typed_args[..], &["tasks.json"]].concat()).output().unwrap();
    let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
    assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{}", text(&typed.stderr));
    assert_eq!((asked[0].clone(), &asked[1..]), (format!("GET {V1}/apps?limit=100&offset=0"), &typed_sent[..]));
    assert_eq!(typed_sent.len(), 1, "{typed_sent:?}");
    let body = &typed_sent[0];
    assert!(body.starts_with(&format!("POST {V1}/connections ")), "{body}");
    for part in [
        format!("\"destinationAppId\":\"{CHOOSE_BQ}\""),
        "\"dataType\":\"audiences\"".into(),
        "\"config\":{\"tasks\":[{\"id\":\"page_views\"}]}".into(),
    ] {
        assert!(body.contains(&part), "{part} in {body}");
    }
    assert_eq!(after_answer(&terminal, answer), printed_lines(&typed));

    // Only the path missing: the question alone, nothing sent before the command's requests; a
    // file that is not there is the command's error after the answer, as when typed.
    for (path, ok) in [("tasks.json", true), ("missing.json", false)] {
        let before = sent(&server).await.len();
        let args = ["destinations", "create", "--dest-app", "e5f6a7b8", "--data-type", "events"];
        let mut terminal = OnTerminal::spawn(in_home(&args), (40, 120), "", None, None);
        terminal.wait_for("vendo destinations create --config-file");
        terminal.press(&format!("{path}\r"));
        let answer = format!("? vendo destinations create --config-file {path}");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        assert!(!terminal.screen.contains("type to filter"), "{path}");
        let asked = sent(&server).await[before..].to_vec();
        let typed = in_home(&[&args[..], &["--config-file", path]].concat()).output().unwrap();
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(if ok { 0 } else { 1 }), code), "{path}");
        assert_eq!(asked, typed_sent, "{path}");
        assert_eq!(asked.len(), if ok { 2 } else { 0 }, "{path}: {asked:?}");
        assert_eq!(after_answer(&terminal, &answer), printed_lines(&typed), "{path}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_a_source_destination_type_or_path_leave_quietly() {
    let server = pipeline_to_choose_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let list = |of: &str| vec![format!("GET {V1}/{of}?limit=100&offset=0")];
    // (words, the line left, what was sent, whether a list or the question is open)
    let cases: [(&[&str], &str, Vec<String>, bool); 5] = [
        (&["sources", "delete"], "? vendo sources delete <canceled>", list("sources"), true),
        (&["int", "pause"], "? vendo destinations pause <canceled>", list("connections"), true),
        (
            &["sources", "create", "--app", CHOOSE_BQ],
            "? vendo sources create --sync-type <canceled>",
            vec![format!("GET {V1}/apps/{CHOOSE_BQ}")],
            true,
        ),
        (
            &["destinations", "create", "--dest-app", CHOOSE_BQ],
            "? vendo destinations create --data-type <canceled>",
            vec![],
            true,
        ),
        (
            &["destinations", "create", "--dest-app", CHOOSE_BQ, "--data-type", "events"],
            "? vendo destinations create --config-file <canceled>",
            vec![],
            false,
        ),
    ];
    for (args, last, requests, list) in cases {
        for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
            let case = format!("{name} {}", args.join(" "));
            let before = sent(&server).await.len();
            let mut terminal = OnTerminal::start(&sandbox, args);
            terminal.wait_for(&last[2..last.len() - " <canceled>".len()]);
            if list {
                terminal.wait_for("type to filter]");
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let shown = shown_lines(&terminal, (40, 120));
            assert_eq!(shown.last().map(String::as_str), Some(last), "{case}: {shown:#?}");
            assert_eq!(sent(&server).await[before..], requests[..], "{case}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_no_source_destination_or_app_to_choose_from_it_says_so_and_is_the_usage_error() {
    let server = MockServer::start().await;
    for list in ["apps", "sources", "connections"] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(Vec::new(), 0, false)).await;
    }
    let sandbox = Sandbox::new(&server.uri());
    for (args, nothing) in [
        (&["sources", "get"][..], "No sources to choose from."),
        (&["sources", "sync", "--json"], "No sources to choose from."),
        (&["destinations", "refresh-source", "--json"], "No destinations to choose from."),
        (&["int", "delete"], "No destinations to choose from."),
        (&["sources", "create"], "No apps to choose from."),
        (&["destinations", "create", "--data-type", "events"], "No apps to choose from."),
    ] {
        let typed = sandbox.run(args);
        assert_eq!(typed.status.code(), Some(2));
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        let (_, code) = terminal.finish();
        let expected = format!("{nothing}\n{}", text(&typed.stderr));
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
        assert_eq!((code, shown), (Some(2), expected.trim_end().to_string()), "{args:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_source_or_destination_list_that_fails_is_the_error_and_nothing_is_asked() {
    let server = MockServer::start().await;
    let refusal = json!({ "error": { "code": "RATE_LIMITED", "message": "Too many requests" } });
    for list in ["sources", "connections"] {
        serve(&server, "GET", &format!("{V1}/{list}"), 429, refusal.clone()).await;
    }
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["sources", "pause"]);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (40, 120));
    assert_eq!((code, shown[0].as_str()), (Some(1), "Error: Too many requests"), "{shown:#?}");
    assert!(!terminal.screen.contains("type to filter"));
    let mut terminal = OnTerminal::start_with(&sandbox, &["destinations", "get", "--json"], WIDE, "", None);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into()));
    assert_eq!((code, shown.len()), (Some(1), 1), "{shown:#?}");
    let error: Value = serde_json::from_str(&shown[0]).unwrap();
    assert_eq!(
        (&error["error"]["message"], &error["error"]["code"], &error["error"]["status"]),
        (&json!("Too many requests"), &json!("RATE_LIMITED"), &json!(429))
    );
    assert_eq!(
        sent(&server).await,
        [format!("GET {V1}/sources?limit=100&offset=0"), format!("GET {V1}/connections?limit=100&offset=0")]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sources_and_destinations_without_an_account_are_that_error_before_anything_is_asked() {
    let server = pipeline_to_choose_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut config = sandbox.config();
    for profile in config["profiles"].as_object_mut().unwrap().values_mut() {
        profile.as_object_mut().unwrap().remove("accountId");
    }
    std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
    let no_account = text(&sandbox.run(&["sources", "list"]).stderr);
    assert!(no_account.starts_with("Error: No account configured."), "{no_account}");
    for args in [
        &["sources", "get"][..],
        &["destinations", "sync"],
        &["sources", "create"],
        // The typed app is read for its type, in the account.
        &["sources", "create", "--app", CHOOSE_BQ],
        &["destinations", "create"],
        // By command, not by question: the data type and the path need no list, but the destination
        // they are for cannot be created without an account.
        &["destinations", "create", "--dest-app", CHOOSE_BQ],
        &["int", "create", "--dest-app", CHOOSE_BQ, "--data-type", "events"],
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        let shown = shown_lines(&terminal, (40, 120)).join("\n");
        assert_eq!((code, shown), (Some(1), no_account.trim_end().to_string()), "{args:?}");
    }
    let json_error = text(&sandbox.run(&["sources", "list", "--json"]).stderr);
    let args = ["destinations", "create", "--dest-app", CHOOSE_BQ, "--data-type", "events", "--json"];
    let mut terminal = OnTerminal::start_with(&sandbox, &args, WIDE, "", None);
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
    assert_eq!((code, shown), (Some(1), json_error.trim_end().to_string()));
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_source_json_chosen_from_the_list_keeps_its_response_on_stdout_and_adds_the_error() {
    // `destinations refresh-source --json` prints the response on stdout and, when the data cannot
    // be imported, the JSON error on stderr, exit 1 (VE-1603, VE-3831): the same once the
    // destination is chosen, the list drawn on stderr.
    let server = MockServer::start().await;
    serve(&server, "GET", &format!("{V1}/connections"), 200, page_of(destinations_to_choose(), 0, false)).await;
    let route = format!("{V1}/connections/{DEST_EVENTS}/refresh-source");
    serve(&server, "POST", &route, 200, json!({ "data": { "status": "unavailable" } })).await;
    let sandbox = Sandbox::new(&server.uri());
    let window = ["--from", "2026-06-29", "--to", "2026-07-02"];
    let mut keys = SecondTerminal::new();
    let cmd = sandbox.command(&[&["destinations", "refresh-source", "--json"][..], &window].concat());
    let mut terminal = OnTerminal::launch(cmd, (40, 120), "", Some(keys.end()), Some(keys.end()), true);
    keys.wait_for("type to filter]");
    keys.press("\r");
    let (stdout, code) = terminal.finish();
    keys.wait_for("vendo destinations refresh-source 8b9c0d1e... (Menu Shop → Analytics BQ)");
    let typed = sandbox.run(&[&["destinations", "refresh-source", DEST_EVENTS, "--json"][..], &window].concat());
    assert_eq!((code, plain(&stdout)), (Some(1), text(&typed.stdout)));
    assert_eq!(text(&typed.stdout), "{\n  \"data\": {\n    \"status\": \"unavailable\"\n  }\n}\n");
    let error = text(&typed.stderr);
    keys.wait_for(error.trim_end());
    let posted: Vec<String> = sent(&server).await.into_iter().filter(|r| r.starts_with("POST")).collect();
    assert_eq!(posted.len(), 2, "{posted:?}");
    assert_eq!(posted[0], posted[1]);
}

// ── VE-3881, jobs, models, metrics and the catalog ───────────────────────────
// A job, model or metric missing its ID is chosen from its list, newest first as `vendo jobs list`,
// `vendo models list` and `vendo metrics list` list them (the metrics without the archived ones, as
// `metrics list` leaves them out); `catalog get` and the hidden `catalog credential-schema` ask for a
// platform ready to connect, as `apps create --type` does; `metrics create` asks for its name and
// its definition file's path as one-line questions. The metrics and the platforms are the key's: no
// account is needed for them.

const JOB_RUNNING: &str = "1f2e3d4c-0000-4000-8000-000000000031";
const JOB_FAILED: &str = "2e3d4c5b-0000-4000-8000-000000000032";
const JOB_QUEUED: &str = "3d4c5b6a-0000-4000-8000-000000000033";
const MODEL_ORDERS: &str = "4c5b6a79-0000-4000-8000-000000000041";
const MODEL_LTV: &str = "5b6a7988-0000-4000-8000-000000000042";
const METRIC_ROAS: &str = "6a798897-0000-4000-8000-000000000051";
const METRIC_REVENUE: &str = "798897a6-0000-4000-8000-000000000052";
const METRIC_CTR: &str = "8897a6b5-0000-4000-8000-000000000053";

/// The time `minutes` ago, as the API sends it.
fn minutes_ago(minutes: i64) -> String {
    let at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes);
    at.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The jobs to choose from, newest first: one running for three and a half hours, one that failed
/// a day ago after half an hour (what `jobs get` shows of it does not change while a test runs),
/// and one queued with no platform and no time yet.
fn jobs_to_choose() -> Vec<Value> {
    let job = |id: &str, job_type: &str, connector: Value, status: &str, started: Value, finished: Value| {
        json!({
            "id": id, "jobType": job_type, "connectorType": connector, "status": status, "startedAt": started,
            "finishedAt": finished, "createdAt": started, "rowsProcessed": null, "rowsWritten": null,
        })
    };
    let (started, finished) = (json!(minutes_ago(26 * 60 + 30)), json!(minutes_ago(26 * 60)));
    vec![
        job(JOB_RUNNING, "import", json!("shopify"), "running", json!(minutes_ago(210)), Value::Null),
        job(JOB_FAILED, "export", json!("bigquery"), "failed", started, finished),
        job(JOB_QUEUED, "data_quality", Value::Null, "queued", Value::Null, Value::Null),
    ]
}

/// [`jobs_to_choose`] as the list shows them: the table's columns, its dash for no platform and no
/// time.
const JOB_ROWS: [&str; 3] = [
    "1f2e3d4c...  import        shopify   running  3h ago",
    "2e3d4c5b...  export        bigquery  failed   1d ago",
    "3d4c5b6a...  data_quality  —         queued   —",
];

/// The models to choose from, newest first.
fn models_to_choose() -> Vec<Value> {
    let model = |id: &str, name: &str, model_type: &str, valid: bool| {
        json!({
            "id": id, "name": name, "modelType": model_type, "isValid": valid, "lastValidatedAt": null,
            "createdAt": null, "description": null,
        })
    };
    vec![model(MODEL_ORDERS, "orders_clean", "sql", true), model(MODEL_LTV, "ltv_forecast", "bqml", false)]
}

/// [`models_to_choose`] as the list shows them.
const MODEL_ROWS: [&str; 2] = ["4c5b6a79...  orders_clean  sql   yes", "5b6a7988...  ltv_forecast  bqml  no"];

/// The metrics to choose from, newest first, as the web app's route sends them (snake_case).
fn metrics_to_choose() -> Vec<Value> {
    let metric = |id: &str, name: &str, format: &str, status: &str| {
        json!({
            "id": id, "name": name, "description": null, "format": format, "higher_is_better": true, "unit": null,
            "status": status, "created_at": null, "updated_at": null,
        })
    };
    vec![
        metric(METRIC_ROAS, "ROAS", "multiplier", "active"),
        metric(METRIC_REVENUE, "Total Revenue", "currency", "draft"),
        metric(METRIC_CTR, "CTR", "percentage", "active"),
    ]
}

/// [`metrics_to_choose`] as the list shows them.
const METRIC_ROWS: [&str; 3] = [
    "6a798897...  ROAS           multiplier  active",
    "798897a6...  Total Revenue  currency    draft",
    "8897a6b5...  CTR            percentage  active",
];

/// The catalog: the two ready platforms [`PLATFORM_ROWS`] shows, one more on request that only
/// `catalog list --all` lists, and each one's entry with its credential fields.
async fn serve_catalog_entries(server: &MockServer) {
    let ready = vec![
        platform("bigquery", "BigQuery", "self_serve", Value::Null),
        platform("shopify", "Shopify", "self_serve", Value::Null),
    ];
    let on_request = platform("hubspot", "HubSpot", "request_access", json!("Ask your account manager"));
    let meta = json!({ "selfServeTotal": 2, "requestAccessTotal": 1 });
    serve_catalog(server, false, json!({ "data": ready, "meta": meta })).await;
    let all = [ready, vec![on_request]].concat();
    serve_catalog(server, true, json!({ "data": all })).await;
    for mut entry in all {
        let field =
            json!({ "name": "apiKey", "label": "API key", "type": "secret", "description": "From its settings" });
        entry["credentialFields"] = json!([field]);
        let route = format!("{CATALOG}/{}", entry["appType"].as_str().unwrap());
        serve(server, "GET", &route, 200, json!({ "data": entry })).await;
    }
}

/// [`jobs_to_choose`], [`models_to_choose`] and [`metrics_to_choose`] in acct-alpha, and the catalog
/// ([`serve_catalog_entries`]): the lists, each item for every request about one (`{ data: <job> }`,
/// `{ metric: <metric> }`), cancelling a job and creating a metric.
async fn jobs_models_metrics_and_catalog_stub() -> MockServer {
    let server = MockServer::start().await;
    let jobs = jobs_to_choose();
    serve(&server, "GET", &format!("{V1}/jobs"), 200, page_of(jobs.clone(), 0, false)).await;
    for job in jobs {
        let id = job["id"].as_str().unwrap();
        let cancelled = json!({ "data": { "id": id, "status": "canceled" } });
        serve(&server, "POST", &format!("{V1}/jobs/{id}/cancel"), 200, cancelled).await;
        serve(&server, "GET", &format!("{V1}/jobs/{id}"), 200, json!({ "data": job })).await;
    }
    serve(&server, "GET", &format!("{V1}/models"), 200, page_of(models_to_choose(), 0, false)).await;
    for model in models_to_choose() {
        let route = format!("{V1}/models/{}", model["id"].as_str().unwrap());
        serve(&server, "GET", &route, 200, json!({ "data": model })).await;
    }
    let metrics = metrics_to_choose();
    let listed = json!({ "metrics": metrics, "total": metrics.len(), "limit": 100, "offset": 0 });
    serve(&server, "GET", "/api/metrics", 200, listed).await;
    for metric in metrics {
        Mock::given(wiremock::matchers::path_regex(format!("^/api/metrics/{}$", metric["id"].as_str().unwrap())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "metric": metric })))
            .mount(&server)
            .await;
    }
    let created = json!({ "metric": { "id": METRIC_ROAS, "name": "ROAS", "status": "active" } });
    serve(&server, "POST", "/api/metrics", 201, created).await;
    serve_catalog_entries(&server).await;
    server
}

/// `sandbox`'s profiles with their API keys and no account.
fn without_accounts(sandbox: &Sandbox) {
    let mut config = sandbox.config();
    for profile in config["profiles"].as_object_mut().unwrap().values_mut() {
        profile.as_object_mut().unwrap().remove("accountId");
    }
    std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
}

const METRICS_LISTED: &str = "GET /api/metrics?limit=100&offset=0";

#[cfg(unix)]
#[tokio::test]
async fn a_missing_job_model_or_metric_is_chosen_from_its_list_and_the_command_runs_as_typed() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // (the rows, the second one's full ID, how its answer names it, the list's request). A job has
    // no name: its short ID alone.
    let jobs = (&JOB_ROWS[..], JOB_FAILED, "2e3d4c5b...", format!("GET {V1}/jobs?limit=100&offset=0"));
    let models =
        (&MODEL_ROWS[..], MODEL_LTV, "5b6a7988... (ltv_forecast)", format!("GET {V1}/models?limit=100&offset=0"));
    let metrics = (&METRIC_ROWS[..], METRIC_REVENUE, "798897a6... (Total Revenue)", METRICS_LISTED.to_string());
    let cases: [(&[&str], _); 12] = [
        (&["jobs", "get"], &jobs),
        (&["jobs", "get", "--json"], &jobs),
        (&["jobs", "cancel", "--yes"], &jobs),
        (&["jobs", "cancel", "--dry-run"], &jobs),
        (&["jobs", "cancel", "--yes", "--output", "id"], &jobs),
        (&["models", "get"], &models),
        (&["models", "get", "--json"], &models),
        (&["metrics", "get"], &metrics),
        (&["metrics", "get", "--json"], &metrics),
        (&["metrics", "update", "--name", "Revenue"], &metrics),
        (&["metrics", "activate", "--json"], &metrics),
        (&["metrics", "delete", "--yes"], &metrics),
    ];
    for (args, (rows, chosen, named, list)) in cases {
        let case = args.join(" ");
        let path = format!("vendo {} {}", args[0], args[1]);
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        // Titled with the command; every row, newest first, the first marked; the menu's hint.
        let mut expected = vec![format!("? {path}")];
        expected.extend(marked(rows));
        expected.push("[↑↓ to move, enter to select, type to filter]".to_string());
        assert_eq!(shown_lines(&terminal, (40, 120)), expected, "{case}");
        // Down to the second, which Enter chooses.
        terminal.press("\u{1b}[B");
        terminal.wait_for(&format!("> {}", rows[1]));
        terminal.press("\r");
        let answer = format!("? {path} {named}");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        let mut asked = sent(&server).await[before..].to_vec();
        // The same command typed with the full ID, on a pipe.
        let typed = sandbox.run(&[&args[..2], &[*chosen][..], &args[2..]].concat());
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{case}: {}", text(&typed.stderr));
        assert_eq!(asked.remove(0), *list, "{case}");
        assert_eq!(asked, typed_sent, "{case}");
        // What it showed after the answer is what the typed command printed.
        assert_eq!(after_answer(&terminal, &answer), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_jobs_models_metrics_and_platforms_by_any_column() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let active = [METRIC_ROWS[0], METRIC_ROWS[2]];
    for (args, typed, rows) in [
        (&["jobs", "get"][..], "BIGQUERY", &JOB_ROWS[1..2]),
        (&["jobs", "get"], "3d4c5b6a", &JOB_ROWS[2..]),
        (&["jobs", "cancel"], "ago", &JOB_ROWS[..2]),
        (&["jobs", "cancel"], "QUEUED", &JOB_ROWS[2..]),
        (&["models", "get"], "LTV", &MODEL_ROWS[1..]),
        (&["models", "get"], "yes", &MODEL_ROWS[..1]),
        (&["metrics", "get"], "revenue", &METRIC_ROWS[1..2]),
        (&["metrics", "activate"], "active", &active[..]),
        (&["metrics", "delete"], "8897A6B5...", &METRIC_ROWS[2..]),
        (&["catalog", "get"], "SHOP", &PLATFORM_ROWS[1..]),
        (&["catalog", "credential-schema"], "big", &PLATFORM_ROWS[..1]),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        terminal.press(typed);
        terminal.wait_for(&format!("{} {typed}", args.join(" ")));
        // The end of that frame: inquire shows the cursor again.
        terminal.wait_for("\u{1b}[?25h");
        assert_eq!(listed_rows(&terminal), marked(rows), "{args:?} {typed}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{args:?} {typed}");
    }
    // Enter chooses the first row left.
    let mut terminal = OnTerminal::start(&sandbox, &["metrics", "get", "--json"]);
    terminal.wait_for("type to filter]");
    terminal.press("ctr");
    terminal.wait_for("vendo metrics get ctr");
    terminal.press("\r");
    terminal.wait_for("vendo metrics get 8897a6b5... (CTR)");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await.last().unwrap(), &format!("GET /api/metrics/{METRIC_CTR}"));
    // A job, which has no name, is answered by its short ID alone.
    let mut terminal = OnTerminal::start(&sandbox, &["jobs", "get", "--json"]);
    terminal.wait_for("type to filter]");
    terminal.press("data_quality\r");
    let (_, code) = terminal.finish();
    assert_eq!(code, Some(0));
    assert!(shown_lines(&terminal, (40, 120)).iter().any(|line| line == "? vendo jobs get 3d4c5b6a..."));
    assert_eq!(sent(&server).await.last().unwrap(), &format!("GET {V1}/jobs/{JOB_QUEUED}"));
}

#[cfg(unix)]
#[tokio::test]
async fn jobs_cancel_and_metrics_delete_ask_which_one_then_ask_y_n_unless_yes() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // (the words, the answer, the list's request, the question, the request once confirmed)
    for (args, answered, list, question, request) in [
        (
            ["jobs", "cancel"],
            "vendo jobs cancel 1f2e3d4c...",
            format!("GET {V1}/jobs?limit=100&offset=0"),
            "Cancel job 1f2e3d4c...? (y/N)",
            format!("POST {V1}/jobs/{JOB_RUNNING}/cancel"),
        ),
        (
            ["metrics", "delete"],
            "vendo metrics delete 6a798897... (ROAS)",
            METRICS_LISTED.to_string(),
            "Delete metric 6a798897...? This cannot be undone. (y/N)",
            format!("DELETE /api/metrics/{METRIC_ROAS}"),
        ),
    ] {
        for (answer, done) in [("n\n", false), ("y\n", true)] {
            let before = sent(&server).await.len();
            let mut terminal = OnTerminal::start(&sandbox, &args);
            terminal.wait_for("type to filter]");
            terminal.press("\r");
            terminal.wait_for(answered);
            // The question names the one chosen, as it names one typed (VE-3823).
            let (shown, code) = answer_question(terminal, answer);
            assert_eq!(code, Some(0), "{args:?}: {shown:?}");
            assert!(plain(&shown).contains(&format!("{question} {answer}")), "{args:?}: {shown:?}");
            let mut expected = vec![list.clone()];
            expected.extend(done.then(|| request.clone()));
            assert_eq!(sent(&server).await[before..], expected, "{args:?} {answer:?}");
        }
        // `--yes`: no question.
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, &[&args[..], &["--yes"]].concat());
        terminal.wait_for("type to filter]");
        terminal.press("\r");
        let (shown, code) = terminal.finish();
        assert_eq!(code, Some(0), "{args:?}: {shown:?}");
        assert!(!shown.contains("(y/N)"), "{args:?}: {shown:?}");
        assert_eq!(sent(&server).await[before..], [list, request], "{args:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn metrics_create_asks_for_the_name_then_the_definition_path_and_posts_the_files_query_spec_unchanged() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let home = sandbox.home.path().to_path_buf();
    // A QuerySpec v2 file, its keys in its own order.
    let file = r#"{ "version": 2, "reportType": "segmentation",
      "metricOutput": { "kind": "ratio", "numerator": "m_revenue", "denominator": "m_spend" }, "scale": 1.5 }"#;
    std::fs::write(home.join("roas.query.json"), file).unwrap();
    let definition = r#"{"version":2,"reportType":"segmentation","metricOutput":{"kind":"ratio","numerator":"m_revenue","denominator":"m_spend"},"scale":1.5}"#;
    let in_home = |args: &[&str]| {
        let mut cmd = sandbox.command(args);
        cmd.current_dir(&home);
        cmd
    };
    // Both: the name, then the path, relative to where vendo runs (Q8), each a one-line question; an
    // empty answer is refused.
    let args = ["metrics", "create", "--format", "multiplier"];
    let mut terminal = OnTerminal::spawn(in_home(&args), (40, 120), "", None, None);
    terminal.wait_for("vendo metrics create --name");
    terminal.press("\r");
    terminal.wait_for("A response is required.");
    terminal.press("ROAS\r");
    terminal.wait_for("vendo metrics create --definition");
    terminal.press("roas.query.json\r");
    let answer = "? vendo metrics create --definition roas.query.json";
    terminal.wait_for(&answer[2..]);
    let (_, code) = terminal.finish();
    assert!(!terminal.screen.contains("type to filter"));
    let shown = shown_lines(&terminal, (40, 120));
    assert!(shown.iter().any(|line| line == "? vendo metrics create --name ROAS"), "{shown:#?}");
    let asked = sent(&server).await;
    let typed = ["metrics", "create", "--format", "multiplier", "--name", "ROAS", "--definition", "roas.query.json"];
    let typed = in_home(&typed).output().unwrap();
    let typed_sent = sent(&server).await[asked.len()..].to_vec();
    assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{}", text(&typed.stderr));
    // Nothing before the command's own request, which carries the file's QuerySpec unchanged.
    let posted = format!(r#"POST /api/metrics {{"name":"ROAS","definition":{definition},"format":"multiplier"}}"#);
    assert_eq!((asked, typed_sent), (vec![posted.clone()], vec![posted]));
    assert_eq!(after_answer(&terminal, answer), printed_lines(&typed));

    // One missing: that question alone. A file that is not there is the command's error after the
    // answer, exit 1 with nothing sent, as when typed.
    for (args, asked_for, answer, ok) in [
        (&["metrics", "create", "--name", "ROAS"][..], "--definition", "roas.query.json", true),
        (&["metrics", "create", "--definition", "roas.query.json", "--json"], "--name", "Return on ad spend", true),
        (&["metrics", "create", "--name", "ROAS"], "--definition", "missing.json", false),
    ] {
        let case = format!("{} {asked_for} {answer}", args.join(" "));
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::spawn(in_home(args), (40, 120), "", None, None);
        let title = format!("vendo metrics create {asked_for}");
        terminal.wait_for(&title);
        terminal.press(&format!("{answer}\r"));
        let answered = format!("? {title} {answer}");
        terminal.wait_for(&answered[2..]);
        let (_, code) = terminal.finish();
        let asked = sent(&server).await[before..].to_vec();
        let typed = in_home(&[args, &[asked_for, answer][..]].concat()).output().unwrap();
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(if ok { 0 } else { 1 }), code), "{case}");
        assert_eq!((asked.len(), &asked), (usize::from(ok), &typed_sent), "{case}");
        assert_eq!(after_answer(&terminal, &answered), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn metrics_update_with_no_change_asks_for_the_metric_then_says_no_updates_provided() {
    // Q13's default (the literal decision): the metric is asked for, then the command fails as it
    // does typed with an ID and no change, in its own words (not the "Nothing to update" of apps,
    // sources and destinations), exit 1.
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["metrics", "update"]);
    terminal.wait_for("type to filter]");
    terminal.press("\r");
    let answer = "? vendo metrics update 6a798897... (ROAS)";
    terminal.wait_for(&answer[2..]);
    let (_, code) = terminal.finish();
    let typed = sandbox.run(&["metrics", "update", METRIC_ROAS]);
    assert_eq!((typed.status.code(), printed_lines(&typed)), (Some(1), vec!["Error: No updates provided".to_string()]));
    assert_eq!((code, after_answer(&terminal, answer)), (Some(1), printed_lines(&typed)));
    assert_eq!(sent(&server).await, [METRICS_LISTED]);
}

#[cfg(unix)]
#[tokio::test]
async fn catalog_get_lists_only_the_ready_platforms_with_a_key_and_no_account_and_shows_the_one_chosen() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // The catalog is the key's: typed, `catalog get shopify` works without an account, and so does
    // the list it is chosen from.
    without_accounts(&sandbox);
    for args in [
        &["catalog", "get"][..],
        &["catalog", "get", "--json"],
        // Hidden (VE-3827); asked for as `catalog get` asks (Q10's default).
        &["catalog", "credential-schema"],
        &["catalog", "credential-schema", "--json"],
    ] {
        let case = args.join(" ");
        let path = format!("vendo {} {}", args[0], args[1]);
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        // The platforms ready to connect, as `vendo catalog list` lists them by default: not the one
        // on request.
        let mut expected = vec![format!("? {path}")];
        expected.extend(marked(&PLATFORM_ROWS));
        expected.push("[↑↓ to move, enter to select, type to filter]".to_string());
        assert_eq!(shown_lines(&terminal, (40, 120)), expected, "{case}");
        terminal.press("\u{1b}[B");
        terminal.wait_for(&format!("> {}", PLATFORM_ROWS[1]));
        terminal.press("\r");
        let answer = format!("? {path} shopify");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        let asked = sent(&server).await[before..].to_vec();
        let typed = sandbox.run(&[&args[..2], &["shopify"][..], &args[2..]].concat());
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{case}: {}", text(&typed.stderr));
        assert_eq!((asked[0].as_str(), &asked[1..]), ("GET /api/v1/catalog", &typed_sent[..]), "{case}");
        assert_eq!(typed_sent, [format!("GET {CATALOG}/shopify")], "{case}");
        assert_eq!(after_answer(&terminal, &answer), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn jobs_and_models_without_an_account_are_that_error_and_metrics_are_asked_for_with_the_key_alone() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    without_accounts(&sandbox);
    let no_account = text(&sandbox.run(&["jobs", "list"]).stderr);
    assert!(no_account.starts_with("Error: No account configured."), "{no_account}");
    for args in [&["jobs", "get"][..], &["jobs", "cancel", "--yes"], &["models", "get"]] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        let shown = shown_lines(&terminal, (40, 120)).join("\n");
        assert_eq!((code, shown), (Some(1), no_account.trim_end().to_string()), "{args:?}");
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
    // The metrics are the key's (a web-app route, VE-3668): listed, and the one chosen is read.
    let mut terminal = OnTerminal::start(&sandbox, &["metrics", "get", "--json"]);
    terminal.wait_for("type to filter]");
    assert_eq!(listed_rows(&terminal), marked(&METRIC_ROWS));
    terminal.press("\r");
    assert_eq!(terminal.finish().1, Some(0));
    // And a new one's name is asked for.
    let mut terminal = OnTerminal::start(&sandbox, &["metrics", "create"]);
    terminal.wait_for("vendo metrics create --name");
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await, [METRICS_LISTED.to_string(), format!("GET /api/metrics/{METRIC_ROAS}")]);
}

#[cfg(unix)]
#[tokio::test]
async fn of_more_than_500_jobs_the_newest_500_are_listed_and_the_hint_says_so() {
    // Six pages of 100 jobs, newest first: the list reads five, as a short-ID lookup does.
    let server = job_pages(6, false, &[]).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["jobs", "get"]);
    terminal.wait_for("[↑↓ to move, enter to select, type to filter · newest 500 shown]");
    // The oldest of the 500 is found by typing, and Enter chooses it.
    terminal.press("700001f3");
    terminal.wait_for("> 700001f3...");
    terminal.press("\r");
    terminal.wait_for("vendo jobs get 700001f3...");
    assert_eq!(terminal.finish().1, Some(0));
    let mut pages: Vec<String> = (0..5).map(|page| format!("GET {V1}/jobs?limit=100&offset={}", page * 100)).collect();
    pages.push(format!("GET {V1}/jobs/{}", uuid_with("700001f3", 99)));
    assert_eq!(sent(&server).await, pages);
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_a_job_model_metric_platform_or_metric_question_leave_quietly() {
    let server = jobs_models_metrics_and_catalog_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // (words, the line left, what was sent, whether a list or a question is open)
    let cases: [(&[&str], &str, Vec<String>, bool); 7] = [
        (
            &["jobs", "cancel"],
            "? vendo jobs cancel <canceled>",
            vec![format!("GET {V1}/jobs?limit=100&offset=0")],
            true,
        ),
        (
            &["models", "get"],
            "? vendo models get <canceled>",
            vec![format!("GET {V1}/models?limit=100&offset=0")],
            true,
        ),
        (&["metrics", "delete"], "? vendo metrics delete <canceled>", vec![METRICS_LISTED.into()], true),
        (&["metrics", "update", "--name", "X"], "? vendo metrics update <canceled>", vec![METRICS_LISTED.into()], true),
        (&["catalog", "get"], "? vendo catalog get <canceled>", vec!["GET /api/v1/catalog".into()], true),
        (&["metrics", "create"], "? vendo metrics create --name <canceled>", vec![], false),
        (&["metrics", "create", "--name", "ROAS"], "? vendo metrics create --definition <canceled>", vec![], false),
    ];
    for (args, last, requests, list) in cases {
        for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
            let case = format!("{name} {}", args.join(" "));
            let before = sent(&server).await.len();
            let mut terminal = OnTerminal::start(&sandbox, args);
            terminal.wait_for(&last[2..last.len() - " <canceled>".len()]);
            if list {
                terminal.wait_for("type to filter]");
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let shown = shown_lines(&terminal, (40, 120));
            assert_eq!(shown.last().map(String::as_str), Some(last), "{case}: {shown:#?}");
            assert_eq!(sent(&server).await[before..], requests[..], "{case}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_no_job_model_metric_or_platform_to_choose_from_it_says_so_and_is_the_usage_error() {
    let server = MockServer::start().await;
    for list in ["jobs", "models"] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(Vec::new(), 0, false)).await;
    }
    serve(&server, "GET", "/api/metrics", 200, json!({ "metrics": [], "total": 0, "limit": 100, "offset": 0 })).await;
    serve_catalog(&server, false, json!({ "data": [], "meta": { "selfServeTotal": 0, "requestAccessTotal": 0 } }))
        .await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, nothing) in [
        (&["jobs", "get"][..], "No jobs to choose from."),
        (&["jobs", "cancel", "--json"], "No jobs to choose from."),
        (&["models", "get"], "No models to choose from."),
        (&["metrics", "activate"], "No metrics to choose from."),
        (&["metrics", "delete", "--json"], "No metrics to choose from."),
        (&["catalog", "get"], "No platforms to choose from."),
        (&["catalog", "credential-schema", "--json"], "No platforms to choose from."),
    ] {
        let typed = sandbox.run(args);
        assert_eq!(typed.status.code(), Some(2));
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        let (_, code) = terminal.finish();
        let expected = format!("{nothing}\n{}", text(&typed.stderr));
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
        assert_eq!((code, shown), (Some(2), expected.trim_end().to_string()), "{args:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_job_model_metric_or_platform_list_that_fails_is_the_error_and_nothing_is_asked() {
    let server = MockServer::start().await;
    let refusal = json!({ "error": { "code": "INTERNAL_ERROR", "message": "Database unavailable" } });
    for route in [format!("{V1}/jobs"), format!("{V1}/models"), CATALOG.to_string()] {
        serve(&server, "GET", &route, 500, refusal.clone()).await;
    }
    // The web app's routes send their error as a string, with no code (VE-3668, VE-3831).
    serve(&server, "GET", "/api/metrics", 500, json!({ "error": "BigQuery is unavailable" })).await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, message) in [
        (&["jobs", "get"][..], "Database unavailable"),
        (&["models", "get"], "Database unavailable"),
        (&["metrics", "update", "--name", "New"], "BigQuery is unavailable"),
        (&["catalog", "get"], "Database unavailable"),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        let shown = shown_lines(&terminal, (40, 120));
        assert_eq!((code, shown[0].as_str()), (Some(1), format!("Error: {message}").as_str()), "{shown:#?}");
        assert!(!terminal.screen.contains("type to filter"), "{args:?}");
    }
    for (args, message, code) in [
        (&["metrics", "get", "--json"][..], "BigQuery is unavailable", Value::Null),
        (&["jobs", "cancel", "--json"], "Database unavailable", json!("INTERNAL_ERROR")),
    ] {
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        let (_, exit) = terminal.finish();
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into()));
        assert_eq!((exit, shown.len()), (Some(1), 1), "{shown:#?}");
        let error: Value = serde_json::from_str(&shown[0]).unwrap();
        assert_eq!(
            (&error["error"]["message"], &error["error"]["code"], &error["error"]["status"]),
            (&json!(message), &code, &json!(500)),
            "{args:?}"
        );
    }
    assert_eq!(
        sent(&server).await,
        [
            format!("GET {V1}/jobs?limit=100&offset=0"),
            format!("GET {V1}/models?limit=100&offset=0"),
            METRICS_LISTED.to_string(),
            "GET /api/v1/catalog".to_string(),
            METRICS_LISTED.to_string(),
            format!("GET {V1}/jobs?limit=100&offset=0"),
        ]
    );
}

// ── VE-3881, measurement and the dictionary ──────────────────────────────────
// `measurement methodologies get` asks for a methodology from the ones `methodologies list` shows
// (the system's and the account's, one response); `measurement ltv cohort` for a cohort period from
// the cohorts `ltv list` shows for the typed `--granularity` and `--segment`, or their defaults
// (newest first, at most the route's 500); `dictionary get` for a subject type, then for one of that
// type's entries (Q11's default); `rules preview --from`/`--to`, `ltv customer` and `dictionary
// search` ask a one-line question. The measurement routes are the key's, so no account is needed
// for them; the dictionary is the account's.

const METHODOLOGY_BLENDED: &str = "0d0e0f10-0000-4000-8000-000000000061";
const METHODOLOGY_LAST_CLICK: &str = "1e1f2021-0000-4000-8000-000000000062";
const METHODOLOGY_MIX: &str = "2f303132-0000-4000-8000-000000000063";

/// The methodologies to choose from, as the web app's route sends them: the account's, then the
/// system's.
fn methodologies_to_choose() -> Value {
    let methodology = |id: &str, name: &str, model: &str, system: bool| {
        json!({
            "id": id, "name": name, "description": null, "click_path_model": model, "is_system": system,
            "version": 1, "updated_at": null, "ensemble_weights": null,
        })
    };
    json!({ "data": { "methodologies": [
        methodology(METHODOLOGY_BLENDED, "Blended", "linear", false),
        methodology(METHODOLOGY_LAST_CLICK, "Last Click", "last_click", true),
        methodology(METHODOLOGY_MIX, "Media Mix", "position_based", true),
    ] } })
}

/// [`methodologies_to_choose`] as the list shows them: short ID, name, scope and click-path model.
const METHODOLOGY_ROWS: [&str; 3] = [
    "0d0e0f10...  Blended     account  linear",
    "1e1f2021...  Last Click  system   last_click",
    "2f303132...  Media Mix   system   position_based",
];

/// `GET /api/measurement/ltv`'s answer: the cohorts of `granularity` and `segment`, each period with
/// its size, newest first as the route orders them.
fn cohorts(granularity: &str, segment: &str, periods: &[(&str, u64)]) -> Value {
    let cohorts: Vec<Value> = periods
        .iter()
        .map(|(period, size)| {
            json!({
                "cohort_period": period, "cohort_granularity": granularity, "segment_key": segment,
                "cohort_size": size, "predicted": null,
                "realised": { "ltv_30d": 12.5, "ltv_90d": null, "ltv_12m": null, "cac": null, "cac_ltv_ratio": null },
            })
        })
        .collect();
    let total = cohorts.len();
    json!({ "data": { "granularity": granularity, "segment_key": segment, "cohorts": cohorts, "total_returned": total } })
}

/// The monthly cohorts of every customer (`all`), the defaults of `ltv cohort`.
const MONTHLY: [(&str, u64); 3] = [("2026-09-01", 1204), ("2026-08-01", 987), ("2026-07-01", 15)];
/// [`MONTHLY`] as the list shows them: period, segment and size.
const COHORT_ROWS: [&str; 3] = ["2026-09-01  all  1,204", "2026-08-01  all  987", "2026-07-01  all  15"];
/// The weekly cohorts of `channel:meta`.
const WEEKLY: [(&str, u64); 2] = [("2026-09-28", 88), ("2026-09-21", 90)];
const WEEKLY_ROWS: [&str; 2] = ["2026-09-28  channel:meta  88", "2026-09-21  channel:meta  90"];

const EVENT_CHECKOUT: &str = "9f8e7d6c5b4a39281706f5e4d3c2b1a0";
const EVENT_PAGE: &str = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
const EVENT_UNNAMED: &str = "1b2c3d4e5f60718293a4b5c6d7e8f9a0";
const PROP_EMAIL: &str = "aa11bb22cc33dd44ee55ff6600112233";

/// One dictionary entry as `GET /dictionary` and the lookup send it.
fn dictionary_item(subject_id: &str, subject_type: &str, name: Value) -> Value {
    json!({
        "subjectId": subject_id, "subjectType": subject_type, "displayName": name, "description": "Synthetic entry",
        "dataType": "string", "semanticType": null, "tags": ["synthetic"], "origin": "registry", "lastSeenAt": null,
        "status": "active",
    })
}

/// The dictionary's entries by subject type: three events, the last without a name, and one property.
fn dictionary_entries() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "event",
            vec![
                dictionary_item(EVENT_CHECKOUT, "event", json!("Checkout Completed")),
                dictionary_item(EVENT_PAGE, "event", json!("Page Viewed")),
                dictionary_item(EVENT_UNNAMED, "event", Value::Null),
            ],
        ),
        ("prop", vec![dictionary_item(PROP_EMAIL, "prop", json!("Email"))]),
    ]
}

/// The subject types `dictionary get` asks for first, as the server accepts them, event first.
const SUBJECT_TYPE_ROWS: [&str; 7] = ["event", "prop", "group", "column", "metric", "model", "audience"];
/// The events as the list shows them: the name (the table's dash for none) and the subject ID.
const EVENT_ROWS: [&str; 3] = [
    "Checkout Completed  9f8e7d6c5b4a39281706f5e4d3c2b1a0",
    "Page Viewed         0a1b2c3d4e5f60718293a4b5c6d7e8f9",
    "—                   1b2c3d4e5f60718293a4b5c6d7e8f9a0",
];
const PROP_ROWS: [&str; 1] = ["Email  aa11bb22cc33dd44ee55ff6600112233"];

const METHODOLOGIES_LISTED: &str = "GET /api/measurement/methodologies";
const COHORTS_LISTED: &str =
    "GET /api/measurement/ltv?granularity=monthly&segment_key=all&limit=500&include_predicted=false";

/// The first page of the dictionary's entries of `subject_type`.
fn dictionary_listed(subject_type: &str) -> String {
    format!("GET {V1}/dictionary?type={subject_type}&limit=100&offset=0")
}

const HINT: &str = "[↑↓ to move, enter to select, type to filter]";

/// Lines as [`cells`] reads them, without the borders a table has at a terminal: piped, the same
/// table has none.
fn table_cells(lines: &[String]) -> Vec<Vec<String>> {
    let border = |c: char| c.is_whitespace() || "┌┐└┘├┤┼─╌┬┴╞╪╡═".contains(c);
    let text: String = lines
        .iter()
        .map(|line| line.chars().map(|c| if "│┆".contains(c) { ' ' } else { c }).collect::<String>())
        .filter(|line| !line.chars().all(border))
        .map(|line| line + "\n")
        .collect();
    cells(text.as_bytes())
}

/// [`methodologies_to_choose`], the [`MONTHLY`] cohorts (the [`WEEKLY`] ones of `channel:meta`),
/// each cohort's detail, any customer, a rules preview, and [`dictionary_entries`] in acct-alpha
/// with each entry's lookup (any other subject type has none).
async fn measurement_and_dictionary_stub() -> MockServer {
    use wiremock::matchers::{method, query_param};
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/measurement/methodologies", 200, methodologies_to_choose()).await;
    // Mounted first, matched first.
    Mock::given(method("GET"))
        .and(path("/api/measurement/ltv"))
        .and(query_param("granularity", "weekly"))
        .and(query_param("segment_key", "channel:meta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cohorts("weekly", "channel:meta", &WEEKLY)))
        .mount(&server)
        .await;
    serve(&server, "GET", "/api/measurement/ltv", 200, cohorts("monthly", "all", &MONTHLY)).await;
    for (granularity, segment, periods) in [("monthly", "all", &MONTHLY[..]), ("weekly", "channel:meta", &WEEKLY[..])] {
        for (period, size) in periods {
            let curve = json!({
                "period_offset_days": 30, "cumulative_gross_revenue": 1500.5, "cumulative_revenue_after_cogs": 900,
            });
            let detail = json!({
                "cohort_period": period, "cohort_granularity": granularity, "segment_key": segment,
                "cohort_size": size, "retention_matrix": [{}, {}], "cumulative_curve": [curve], "prediction": null,
            });
            serve(&server, "GET", &format!("/api/measurement/ltv/cohort/{period}"), 200, detail).await;
        }
    }
    let customer = json!({
        "cohort": {
            "acquisition_date": "2026-08-03", "acquisition_channel": "paid_social", "acquisition_campaign": null,
            "country": "AU", "is_reactivated": false, "cohort_period_monthly": "2026-08-01",
        },
        "realised": { "ltv_30d": 120.5, "ltv_90d": 240, "ltv_12m": null, "ltv_full": 240 },
        "revenue": [{}, {}],
    });
    Mock::given(wiremock::matchers::path_regex("^/api/measurement/ltv/customer/[^/]+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer))
        .mount(&server)
        .await;
    let preview = json!({
        "context": { "campaign_objective": "sales", "channel_grouping": "paid_social", "custom_label": null },
        "resolved_methodology": { "name": "Blended" }, "via": "rule", "sample_count": 12,
    });
    let previews = json!({ "previews": [preview], "total_distinct_contexts": 1 });
    serve(&server, "POST", "/api/measurement/methodologies/rules/preview", 200, previews).await;
    for (subject_type, items) in dictionary_entries() {
        Mock::given(method("GET"))
            .and(path(format!("{V1}/dictionary")))
            .and(query_param("type", subject_type))
            .respond_with(ResponseTemplate::new(200).set_body_json(page_of(items.clone(), 0, false)))
            .mount(&server)
            .await;
        for item in items {
            let id = item["subjectId"].as_str().unwrap().to_string();
            let found = json!({ "data": { "subjectId": id, "found": true, "definition": item } });
            Mock::given(method("GET"))
                .and(path(format!("{V1}/dictionary/lookup")))
                .and(query_param("subject_id", id.as_str()))
                .respond_with(ResponseTemplate::new(200).set_body_json(found))
                .mount(&server)
                .await;
        }
    }
    serve(&server, "GET", &format!("{V1}/dictionary"), 200, page_of(Vec::new(), 0, false)).await;
    server
}

#[cfg(unix)]
#[tokio::test]
async fn a_missing_methodology_or_cohort_is_chosen_from_its_list_and_the_command_runs_as_typed() {
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // (the rows, the second one's value, how its answer names it, the list's request). A cohort is
    // answered by its period, the value the command takes.
    let methodologies =
        (&METHODOLOGY_ROWS[..], METHODOLOGY_LAST_CLICK, "1e1f2021... (Last Click)", METHODOLOGIES_LISTED.to_string());
    let monthly = (&COHORT_ROWS[..], "2026-08-01", "2026-08-01", COHORTS_LISTED.to_string());
    let weekly_listed =
        "GET /api/measurement/ltv?granularity=weekly&segment_key=channel%3Ameta&limit=500&include_predicted=false";
    let weekly = (&WEEKLY_ROWS[..], "2026-09-21", "2026-09-21", weekly_listed.to_string());
    let cases: [(&[&str], _); 6] = [
        (&["measurement", "methodologies", "get"], &methodologies),
        (&["measurement", "methodologies", "get", "--json"], &methodologies),
        (&["measurement", "ltv", "cohort"], &monthly),
        (&["measurement", "ltv", "cohort", "--json"], &monthly),
        // The cohorts of the granularity and segment typed.
        (&["measurement", "ltv", "cohort", "--granularity", "weekly", "--segment", "channel:meta"], &weekly),
        (&["measurement", "ltv", "cohort", "--segment=channel:meta", "--json", "--granularity", "weekly"], &weekly),
    ];
    for (args, (rows, chosen, named, list)) in cases {
        let case = args.join(" ");
        let path = format!("vendo {}", args[..3].join(" "));
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        // Titled with the command; every row in the route's order, the first marked; the menu's hint.
        let mut expected = vec![format!("? {path}")];
        expected.extend(marked(rows));
        expected.push(HINT.to_string());
        assert_eq!(shown_lines(&terminal, (40, 120)), expected, "{case}");
        // Down to the second, which Enter chooses.
        terminal.press("\u{1b}[B");
        terminal.wait_for(&format!("> {}", rows[1]));
        terminal.press("\r");
        let answer = format!("? {path} {named}");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        let mut asked = sent(&server).await[before..].to_vec();
        // The same command typed with the value, on a pipe.
        let typed = sandbox.run(&[&args[..3], &[*chosen][..], &args[3..]].concat());
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{case}: {}", text(&typed.stderr));
        assert_eq!(asked.remove(0), *list, "{case}");
        assert_eq!(asked, typed_sent, "{case}");
        // What it showed after the answer is what the typed command printed.
        assert_eq!(after_answer(&terminal, &answer), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn dictionary_get_asks_for_a_subject_type_then_one_of_its_entries_and_runs_as_typed() {
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    // (the words, the type's row, that type's rows, the entry's row, its subject ID, how the answer
    // names it). Not a short ID: the dictionary's IDs are not looked up by their start (VE-3831).
    type Case<'a> = (&'a [&'a str], usize, &'a [&'a str], usize, &'a str, String);
    let cases: [Case; 4] = [
        (&["dictionary", "get"], 0, &EVENT_ROWS[..], 1, EVENT_PAGE, format!("{EVENT_PAGE} (Page Viewed)")),
        (
            &["dictionary", "get", "--json"],
            0,
            &EVENT_ROWS,
            0,
            EVENT_CHECKOUT,
            format!("{EVENT_CHECKOUT} (Checkout Completed)"),
        ),
        // An entry with no name: its subject ID alone.
        (&["dictionary", "get"], 0, &EVENT_ROWS, 2, EVENT_UNNAMED, EVENT_UNNAMED.to_string()),
        (&["dictionary", "get", "--json"], 1, &PROP_ROWS, 0, PROP_EMAIL, format!("{PROP_EMAIL} (Email)")),
    ];
    for (args, type_at, rows, entry_at, chosen, named) in cases {
        let case = format!("{} {chosen}", args.join(" "));
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        // First the subject type, event first as `dictionary list` lists events by default.
        let mut expected = vec!["? vendo dictionary get · subject type".to_string()];
        expected.extend(marked(&SUBJECT_TYPE_ROWS));
        expected.push(HINT.to_string());
        assert_eq!(shown_lines(&terminal, (40, 120)), expected, "{case}");
        let subject_type = SUBJECT_TYPE_ROWS[type_at];
        if type_at > 0 {
            terminal.press(&"\u{1b}[B".repeat(type_at));
            terminal.wait_for(&format!("> {subject_type}"));
        }
        terminal.press("\r");
        let typed_type = format!("? vendo dictionary get · subject type {subject_type}");
        terminal.wait_for(&typed_type[2..]);
        terminal.wait_for("type to filter]");
        // Then that type's entries: name and subject ID.
        let mut expected = vec![typed_type, "? vendo dictionary get".to_string()];
        expected.extend(marked(rows));
        expected.push(HINT.to_string());
        assert_eq!(shown_lines(&terminal, (40, 120)), expected, "{case}");
        if entry_at > 0 {
            terminal.press(&"\u{1b}[B".repeat(entry_at));
            terminal.wait_for(&format!("> {}", rows[entry_at]));
        }
        terminal.press("\r");
        let answer = format!("? vendo dictionary get {named}");
        terminal.wait_for(&answer[2..]);
        let (_, code) = terminal.finish();
        let mut asked = sent(&server).await[before..].to_vec();
        let typed = sandbox.run(&[&args[..2], &[chosen][..], &args[2..]].concat());
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{case}: {}", text(&typed.stderr));
        assert_eq!(asked.remove(0), dictionary_listed(subject_type), "{case}");
        assert_eq!(asked, typed_sent, "{case}");
        assert_eq!(after_answer(&terminal, &answer), printed_lines(&typed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rules_preview_ltv_customer_and_dictionary_search_ask_a_question_and_run_as_typed() {
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let preview = "vendo measurement rules preview";
    let (customer, search) = ("vendo measurement ltv customer", "vendo dictionary search");
    // (the title of the command, the words, each question's option and answer in clap's order, the
    // same command typed)
    type Case<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a str)], &'a [&'a str]);
    let cases: [Case; 10] = [
        (
            preview,
            &["measurement", "rules", "preview"],
            &[("--from", "2025-01-01"), ("--to", "2025-01-31")],
            &["measurement", "rules", "preview", "--from", "2025-01-01", "--to", "2025-01-31"],
        ),
        (
            preview,
            &["measurement", "rules", "preview", "--from", "2025-01-01", "--limit", "100"],
            &[("--to", "2025-01-31")],
            &["measurement", "rules", "preview", "--from", "2025-01-01", "--limit", "100", "--to", "2025-01-31"],
        ),
        (
            preview,
            &["measurement", "rules", "preview", "--to", "2025-01-31", "--json"],
            &[("--from", "2025-01-01")],
            &["measurement", "rules", "preview", "--to", "2025-01-31", "--json", "--from", "2025-01-01"],
        ),
        (
            customer,
            &["measurement", "ltv", "customer"],
            &[("", "cust_abc123")],
            &["measurement", "ltv", "customer", "cust_abc123"],
        ),
        (
            customer,
            &["measurement", "ltv", "customer", "--json"],
            &[("", "cust 1/2")],
            &["measurement", "ltv", "customer", "--json", "cust 1/2"],
        ),
        (search, &["dictionary", "search"], &[("", "checkout")], &["dictionary", "search", "checkout"]),
        (
            search,
            &["dictionary", "search", "--type", "prop", "--json"],
            &[("", "email")],
            &["dictionary", "search", "--type", "prop", "--json", "email"],
        ),
        // An answer that starts with `-` is the query, not an option.
        (search, &["dictionary", "search"], &[("", "-checkout")], &["dictionary", "search", "--", "-checkout"]),
        // And `help` is the query too, as when typed.
        (search, &["dictionary", "search"], &[("", "help")], &["dictionary", "search", "help"]),
        (
            search,
            &["dictionary", "search", "--limit", "5"],
            &[("", "page viewed")],
            &["dictionary", "search", "--limit", "5", "page viewed"],
        ),
    ];
    for (title, args, questions, typed_args) in cases {
        let case = args.join(" ");
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, args);
        let mut answered = Vec::new();
        for (option, answer) in questions {
            let title = if option.is_empty() { title.to_string() } else { format!("{title} {option}") };
            terminal.wait_for(&title);
            terminal.press(&format!("{answer}\r"));
            let line = format!("? {title} {answer}");
            terminal.wait_for(&line[2..]);
            answered.push(line);
        }
        let (_, code) = terminal.finish();
        assert!(!terminal.screen.contains("type to filter"), "{case}");
        let shown = shown_lines(&terminal, (40, 120));
        assert_eq!(shown[..answered.len()], answered[..], "{case}: {shown:#?}");
        let asked = sent(&server).await[before..].to_vec();
        let typed = sandbox.run(typed_args);
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{case}: {}", text(&typed.stderr));
        // Nothing is sent before the command's own request.
        assert_eq!((asked.len(), &asked), (1, &typed_sent), "{case}");
        let (after, printed) = (after_answer(&terminal, answered.last().unwrap()), printed_lines(&typed));
        if after.first().is_some_and(|line| line.starts_with('┌')) {
            assert_eq!(table_cells(&after), table_cells(&printed), "{case}");
        } else {
            assert_eq!(after, printed, "{case}");
        }
    }
    // An empty answer is refused.
    let before = sent(&server).await.len();
    let mut terminal = OnTerminal::start(&sandbox, &["measurement", "ltv", "customer"]);
    terminal.wait_for(customer);
    terminal.press("\r");
    terminal.wait_for("A response is required.");
    terminal.press("cust_abc123\r");
    terminal.wait_for("vendo measurement ltv customer cust_abc123");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await[before..], ["GET /api/measurement/ltv/customer/cust_abc123"]);
}

#[cfg(unix)]
#[tokio::test]
async fn a_one_line_answer_is_taken_without_the_spaces_and_line_ends_around_it() {
    // A shell takes a word without the spaces and the line end around it; the question took them
    // too. A line copied whole, or a spreadsheet's cell, brought its line end: the screen showed
    // `2025-01-01`, the API got `2025-01-01\n` and refused it, and a customer ID with it found no
    // cohort. A paste comes in brackets (`ESC[200~`…`ESC[201~`), as a terminal sends it.
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let paste = |text: &str| format!("\u{1b}[200~{text}\u{1b}[201~");
    let customer = "vendo measurement ltv customer";
    for keys in [paste(" cust_abc123\n"), paste("cust_abc123\r"), "  cust_abc123  ".to_string()] {
        let case = format!("{keys:?}");
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, &["measurement", "ltv", "customer"]);
        terminal.wait_for(customer);
        terminal.press(&keys);
        terminal.press("\r");
        let (_, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}");
        assert_eq!(sent(&server).await[before..], ["GET /api/measurement/ltv/customer/cust_abc123"], "{case}");
        // The answered line shows the answer as it is taken.
        assert_eq!(shown_lines(&terminal, (40, 120))[0], format!("? {customer} cust_abc123"), "{case}");
    }
    // Only spaces and line ends: refused as an empty answer, nothing sent.
    let before = sent(&server).await.len();
    let mut terminal = OnTerminal::start(&sandbox, &["measurement", "ltv", "customer"]);
    terminal.wait_for(customer);
    terminal.press(&paste("   \n"));
    terminal.press("\r");
    terminal.wait_for("A response is required.");
    assert_eq!(sent(&server).await.len(), before);
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await.len(), before);
    // Dates pasted with their line end, or typed with a space: the dates alone, as typed in a shell.
    let before = sent(&server).await.len();
    let preview = "vendo measurement rules preview";
    let mut terminal = OnTerminal::start(&sandbox, &["measurement", "rules", "preview"]);
    terminal.wait_for(&format!("{preview} --from"));
    terminal.press(&paste("2025-01-01\n"));
    terminal.press("\r");
    terminal.wait_for(&format!("{preview} --to"));
    terminal.press("2025-01-31 \r");
    let (_, code) = terminal.finish();
    let asked = sent(&server).await[before..].to_vec();
    let typed = sandbox.run(&["measurement", "rules", "preview", "--from", "2025-01-01", "--to", "2025-01-31"]);
    assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{}", text(&typed.stderr));
    assert_eq!(asked, sent(&server).await[before + asked.len()..], "{asked:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_methodologies_cohorts_subject_types_and_dictionary_entries() {
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let methodologies = ["measurement", "methodologies", "get"];
    let cohort = ["measurement", "ltv", "cohort"];
    // (the words, what answers the type first, the list's title, what is typed, the rows left)
    for (args, first, title, typed, rows) in [
        (&methodologies[..], "", "vendo measurement methodologies get", "SYSTEM", &METHODOLOGY_ROWS[1..]),
        (&methodologies, "", "vendo measurement methodologies get", "linear", &METHODOLOGY_ROWS[..1]),
        (&methodologies, "", "vendo measurement methodologies get", "2F303132...", &METHODOLOGY_ROWS[2..]),
        (&cohort, "", "vendo measurement ltv cohort", "2026-08", &COHORT_ROWS[1..2]),
        (&cohort, "", "vendo measurement ltv cohort", "1,204", &COHORT_ROWS[..1]),
        (&["dictionary", "get"], "", "vendo dictionary get · subject type", "PR", &SUBJECT_TYPE_ROWS[1..2]),
        (&["dictionary", "get"], "", "vendo dictionary get · subject type", "mo", &SUBJECT_TYPE_ROWS[5..6]),
        (&["dictionary", "get"], "\r", "vendo dictionary get", "viewed", &EVENT_ROWS[1..2]),
        (&["dictionary", "get"], "\r", "vendo dictionary get", "9F8E7D6C", &EVENT_ROWS[..1]),
        (&["dictionary", "get"], "\r", "vendo dictionary get", "e", &EVENT_ROWS[..]),
    ] {
        let case = format!("{} {typed}", args.join(" "));
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        if !first.is_empty() {
            terminal.press(first);
            terminal.wait_for("type to filter]");
        }
        terminal.press(typed);
        let filtered = format!("? {title} {typed}");
        terminal.wait_for(&filtered[2..]);
        // The end of that frame: inquire shows the cursor again.
        terminal.wait_for("\u{1b}[?25h");
        let shown = shown_lines(&terminal, (40, 120));
        let at = shown.iter().position(|line| *line == filtered).unwrap_or_else(|| panic!("{case}: {shown:#?}"));
        assert_eq!(shown[at + 1..shown.len() - 1], marked(rows)[..], "{case}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_a_measurement_or_dictionary_list_or_question_leave_quietly() {
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let event = dictionary_listed("event");
    // (words, what opens first, what answers it, the line left, what was sent, whether the last
    // prompt is a list)
    type Case<'a> = (&'a [&'a str], &'a str, &'a str, &'a str, Vec<String>, bool);
    let cases: [Case; 8] = [
        (
            &["measurement", "methodologies", "get"],
            "",
            "",
            "? vendo measurement methodologies get <canceled>",
            vec![METHODOLOGIES_LISTED.into()],
            true,
        ),
        (
            &["measurement", "ltv", "cohort"],
            "",
            "",
            "? vendo measurement ltv cohort <canceled>",
            vec![COHORTS_LISTED.into()],
            true,
        ),
        (&["dictionary", "get"], "", "", "? vendo dictionary get · subject type <canceled>", vec![], true),
        (&["dictionary", "get"], "type to filter]", "\r", "? vendo dictionary get <canceled>", vec![event], true),
        (
            &["measurement", "rules", "preview"],
            "",
            "",
            "? vendo measurement rules preview --from <canceled>",
            vec![],
            false,
        ),
        (
            &["measurement", "rules", "preview"],
            "vendo measurement rules preview --from",
            "2025-01-01\r",
            "? vendo measurement rules preview --to <canceled>",
            vec![],
            false,
        ),
        (&["measurement", "ltv", "customer"], "", "", "? vendo measurement ltv customer <canceled>", vec![], false),
        (&["dictionary", "search"], "", "", "? vendo dictionary search <canceled>", vec![], false),
    ];
    for (args, first, answers, last, requests, list) in cases {
        for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
            let case = format!("{name} {} {answers:?}", args.join(" "));
            let before = sent(&server).await.len();
            let mut terminal = OnTerminal::start(&sandbox, args);
            if !answers.is_empty() {
                terminal.wait_for(first);
                terminal.press(answers);
            }
            terminal.wait_for(&last[2..last.len() - " <canceled>".len()]);
            if list {
                terminal.wait_for("type to filter]");
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let shown = shown_lines(&terminal, (40, 120));
            assert_eq!(shown.last().map(String::as_str), Some(last), "{case}: {shown:#?}");
            assert_eq!(sent(&server).await[before..], requests[..], "{case}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_no_methodology_cohort_or_dictionary_entry_to_choose_from_it_says_so_and_is_the_usage_error() {
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/measurement/methodologies", 200, json!({ "data": { "methodologies": [] } })).await;
    serve(&server, "GET", "/api/measurement/ltv", 200, cohorts("monthly", "all", &[])).await;
    serve(&server, "GET", &format!("{V1}/dictionary"), 200, page_of(Vec::new(), 0, false)).await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, nothing) in [
        (&["measurement", "methodologies", "get"][..], "No methodologies to choose from."),
        (&["measurement", "methodologies", "get", "--json"], "No methodologies to choose from."),
        (&["measurement", "ltv", "cohort"], "No cohorts to choose from."),
        (&["measurement", "ltv", "cohort", "--json"], "No cohorts to choose from."),
    ] {
        let typed = sandbox.run(args);
        assert_eq!(typed.status.code(), Some(2));
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        let (_, code) = terminal.finish();
        let expected = format!("{nothing}\n{}", text(&typed.stderr));
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
        assert_eq!((code, shown), (Some(2), expected.trim_end().to_string()), "{args:?}");
    }
    // The dictionary: a type is chosen, then it has no entries.
    for args in [&["dictionary", "get"][..], &["dictionary", "get", "--json"]] {
        let typed = sandbox.run(args);
        assert_eq!(typed.status.code(), Some(2));
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        terminal.wait_for("type to filter]");
        terminal.press("\u{1b}[B");
        terminal.wait_for("> prop");
        terminal.press("\r");
        let (_, code) = terminal.finish();
        let expected = format!(
            "? vendo dictionary get · subject type prop\nNo prop entries to choose from.\n{}",
            text(&typed.stderr)
        );
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into())).join("\n");
        assert_eq!((code, shown), (Some(2), expected.trim_end().to_string()), "{args:?}");
    }
    let mut expected = vec![METHODOLOGIES_LISTED.to_string(); 2];
    expected.extend([COHORTS_LISTED.to_string(), COHORTS_LISTED.to_string()]);
    expected.extend([dictionary_listed("prop"), dictionary_listed("prop")]);
    assert_eq!(sent(&server).await, expected);
}

#[cfg(unix)]
#[tokio::test]
async fn a_methodology_cohort_or_dictionary_list_that_fails_is_the_error_and_nothing_more_is_asked() {
    let server = MockServer::start().await;
    // The web app's routes send their error as a string, with no code (VE-3668, VE-3831).
    serve(&server, "GET", "/api/measurement/methodologies", 500, json!({ "error": "BigQuery is unavailable" })).await;
    // A granularity the route does not know is its 400, as for the typed command.
    let refused = json!({ "error": "granularity must be one of daily | weekly | monthly" });
    serve(&server, "GET", "/api/measurement/ltv", 400, refused).await;
    let refusal = json!({ "error": { "code": "INTERNAL_ERROR", "message": "Database unavailable" } });
    serve(&server, "GET", &format!("{V1}/dictionary"), 500, refusal).await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, message) in [
        (&["measurement", "methodologies", "get"][..], "BigQuery is unavailable"),
        (
            &["measurement", "ltv", "cohort", "--granularity", "yearly"],
            "granularity must be one of daily | weekly | monthly",
        ),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        let shown = shown_lines(&terminal, (40, 120));
        assert_eq!((code, shown[0].as_str()), (Some(1), format!("Error: {message}").as_str()), "{shown:#?}");
        assert!(!terminal.screen.contains("type to filter"), "{args:?}");
    }
    // The dictionary's entries, once the type is chosen: its list opens no more.
    let mut terminal = OnTerminal::start(&sandbox, &["dictionary", "get"]);
    terminal.wait_for("type to filter]");
    let opened = terminal.screen.matches("type to filter]").count();
    terminal.press("\r");
    let (_, code) = terminal.finish();
    let shown = shown_lines(&terminal, (40, 120));
    assert_eq!(
        (code, &shown[..2]),
        (
            Some(1),
            &["? vendo dictionary get · subject type event".to_string(), "Error: Database unavailable".to_string()][..]
        ),
        "{shown:#?}"
    );
    assert_eq!(terminal.screen.matches("type to filter]").count(), opened, "{shown:#?}");
    for (args, first, message, code, status) in [
        (&["measurement", "methodologies", "get", "--json"][..], false, "BigQuery is unavailable", Value::Null, 500),
        (
            &["measurement", "ltv", "cohort", "--json", "--granularity", "yearly"],
            false,
            "granularity must be one of daily | weekly | monthly",
            Value::Null,
            400,
        ),
        (&["dictionary", "get", "--json"], true, "Database unavailable", json!("INTERNAL_ERROR"), 500),
    ] {
        let mut terminal = OnTerminal::start_with(&sandbox, args, WIDE, "", None);
        if first {
            terminal.wait_for("type to filter]");
            terminal.press("\r");
        }
        let (_, exit) = terminal.finish();
        let shown = shown_lines(&terminal, (WIDE.0.into(), WIDE.1.into()));
        assert_eq!((exit, shown.len()), (Some(1), 1 + usize::from(first)), "{shown:#?}");
        let error: Value = serde_json::from_str(shown.last().unwrap()).unwrap();
        assert_eq!(
            (&error["error"]["message"], &error["error"]["code"], &error["error"]["status"]),
            (&json!(message), &code, &json!(status)),
            "{args:?}"
        );
    }
    let yearly = "GET /api/measurement/ltv?granularity=yearly&segment_key=all&limit=500&include_predicted=false";
    let listed = [METHODOLOGIES_LISTED.to_string(), yearly.to_string(), dictionary_listed("event")];
    assert_eq!(sent(&server).await, [&listed[..], &listed[..]].concat());
}

#[cfg(unix)]
#[tokio::test]
async fn the_dictionary_needs_the_account_before_anything_is_asked_and_measurement_only_the_key() {
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    without_accounts(&sandbox);
    let no_account = text(&sandbox.run(&["dictionary", "list"]).stderr);
    assert!(no_account.starts_with("Error: No account configured."), "{no_account}");
    for args in [&["dictionary", "get"][..], &["dictionary", "search"], &["dictionary", "search", "--type", "prop"]] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        let shown = shown_lines(&terminal, (40, 120)).join("\n");
        assert_eq!((code, shown), (Some(1), no_account.trim_end().to_string()), "{args:?}");
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
    // The measurement routes are the key's (web-app routes, VE-3668): listed and asked without an
    // account.
    let mut terminal = OnTerminal::start(&sandbox, &["measurement", "methodologies", "get", "--json"]);
    terminal.wait_for("type to filter]");
    assert_eq!(listed_rows(&terminal), marked(&METHODOLOGY_ROWS));
    terminal.press("\r");
    assert_eq!(terminal.finish().1, Some(0));
    for (args, title) in [
        (&["measurement", "ltv", "cohort"][..], "type to filter]"),
        (&["measurement", "rules", "preview"], "vendo measurement rules preview --from"),
        (&["measurement", "ltv", "customer"], "vendo measurement ltv customer"),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for(title);
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{args:?}");
    }
    assert_eq!(sent(&server).await, [METHODOLOGIES_LISTED, METHODOLOGIES_LISTED, COHORTS_LISTED]);
    // No key: its error before anything is asked, a question too.
    let sandbox = Sandbox::without_api_keys(&server.uri());
    let no_key = text(&sandbox.run(&["measurement", "methodologies", "list"]).stderr);
    assert!(no_key.starts_with("Error: No API key configured."), "{no_key}");
    for args in [
        &["measurement", "methodologies", "get"][..],
        &["measurement", "ltv", "cohort"],
        &["measurement", "rules", "preview"],
        &["measurement", "ltv", "customer"],
        &["dictionary", "search"],
    ] {
        let mut terminal = OnTerminal::start(&sandbox, args);
        let (_, code) = terminal.finish();
        let shown = shown_lines(&terminal, (40, 120)).join("\n");
        assert_eq!((code, shown), (Some(1), no_key.trim_end().to_string()), "{args:?}");
    }
    assert_eq!(sent(&server).await.len(), 3);
}

#[cfg(unix)]
#[tokio::test]
async fn of_more_than_500_entries_the_first_500_are_listed_and_of_500_cohorts_the_hint_says_newest_500() {
    use wiremock::matchers::query_param;
    let server = MockServer::start().await;
    // Six pages of 100 events, in the route's order: the list reads five, as a short-ID lookup does.
    let id = |n: usize| format!("{n:032x}");
    for page in 0..6 {
        let items: Vec<Value> = (page * 100..page * 100 + 100)
            .map(|n| dictionary_item(&id(n), "event", json!(format!("Event {n}"))))
            .collect();
        Mock::given(path(format!("{V1}/dictionary")))
            .and(query_param("offset", (page * 100).to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(page_of(items, page * 100, true)))
            .mount(&server)
            .await;
    }
    serve(&server, "GET", &format!("{V1}/dictionary/lookup"), 200, json!({ "data": { "found": false } })).await;
    // The route sends at most 500 cohorts, newest first: 500 may leave older ones out.
    let periods: Vec<String> =
        (0..500).map(|n| format!("{:04}-{:02}-01", 2026 - (n + 3) / 12, 12 - (n + 3) % 12)).collect();
    let periods: Vec<(&str, u64)> = periods.iter().map(|period| (period.as_str(), 10)).collect();
    serve(&server, "GET", "/api/measurement/ltv", 200, cohorts("monthly", "all", &periods)).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["dictionary", "get"]);
    terminal.wait_for("type to filter]");
    terminal.press("\r");
    terminal.wait_for("[↑↓ to move, enter to select, type to filter · first 500 shown]");
    // The last of the 500 is found by typing, and Enter chooses it.
    terminal.press("Event 499");
    terminal.wait_for(&format!("> Event 499  {}", id(499)));
    terminal.press("\r");
    terminal.wait_for(&format!("vendo dictionary get {} (Event 499)", id(499)));
    assert_eq!(terminal.finish().1, Some(0));
    let mut pages: Vec<String> =
        (0..5).map(|page| format!("GET {V1}/dictionary?type=event&limit=100&offset={}", page * 100)).collect();
    pages.push(format!("GET {V1}/dictionary/lookup?subject_id={}", id(499)));
    assert_eq!(sent(&server).await, pages);
    let mut terminal = OnTerminal::start(&sandbox, &["measurement", "ltv", "cohort"]);
    terminal.wait_for("[↑↓ to move, enter to select, type to filter · newest 500 shown]");
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await[5 + 1..], [COHORTS_LISTED]);
}

#[cfg(unix)]
#[tokio::test]
async fn a_cohort_chosen_from_the_ltv_menu_is_asked_for_as_when_typed() {
    // `vendo measurement ltv` → cohort asks for the cohort as `vendo measurement ltv cohort` does.
    let server = measurement_and_dictionary_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["measurement", "ltv"]);
    terminal.wait_for("Show one customer's cohort");
    terminal.press("\u{1b}[B");
    terminal.wait_for("> cohort");
    terminal.press("\r");
    terminal.wait_for(COHORT_ROWS[2]);
    terminal.press("\r");
    terminal.wait_for("vendo measurement ltv cohort 2026-09-01");
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran:?}");
    assert!(plain(&ran).contains("Cohort 2026-09-01"), "{ran:?}");
    let cohort = "GET /api/measurement/ltv/cohort/2026-09-01?granularity=monthly&segment_key=all";
    assert_eq!(sent(&server).await, [COHORTS_LISTED, cohort]);
}

// ── VE-3826: CI and VENDO_NO_INPUT turn prompts off; no menu on TERM=dumb ───
// Decided by Yalcin, 2026-10-06, CLI 1.1. `CI` or `VENDO_NO_INPUT` set to anything but empty, `0`
// or `false` (in any case) turns every prompt off, also at a terminal: the y/N questions, the group
// menu and the profile list do what they do without a terminal, and login reads no "Press ENTER"
// at a terminal (a stdin that is no terminal it reads as before). There is no `--no-input` flag. On `TERM=dumb` a bare group is the usage error, as without a
// terminal; the questions still ask there. The profile list opens only where the group menu does
// (VE-3892; before it, the numbered picker asked where the questions ask).

/// Settings that turn prompts off.
#[cfg(unix)]
const PROMPTS_OFF: [(&str, &str); 6] = [
    ("CI", "true"),
    ("CI", "1"),
    ("CI", "TRUE"),
    ("CI", "woodpecker"),
    ("VENDO_NO_INPUT", "1"),
    ("VENDO_NO_INPUT", "true"),
];

/// Settings that leave them on.
#[cfg(unix)]
const PROMPTS_ON: [(&str, &str); 7] = [
    ("CI", "false"),
    ("CI", "0"),
    ("CI", "False"),
    ("CI", ""),
    ("VENDO_NO_INPUT", "0"),
    ("VENDO_NO_INPUT", "FALSE"),
    ("VENDO_NO_INPUT", ""),
];

#[cfg(unix)]
#[tokio::test]
async fn with_prompts_off_a_question_at_a_terminal_needs_yes_as_without_one() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut asking: Vec<(Vec<&str>, String)> =
        confirming_commands().into_iter().map(|(args, what, _)| (args, what)).collect();
    let reset = "This removes every saved profile and its API key.".to_string();
    asking.extend([(vec!["logout", "--all"], reset.clone()), (vec!["config", "reset"], reset)]);
    for (args, what) in &asking {
        for setting in [("CI", "true"), ("VENDO_NO_INPUT", "1")] {
            let (screen, code) = OnTerminal::start_env(&sandbox, args, &[setting]).finish();
            assert_eq!(
                (code, plain(&screen)),
                (Some(1), format!("Error: {what} Re-run with --yes to confirm.\n")),
                "{setting:?} {args:?}"
            );
        }
    }
    let (args, what) = &asking[0];
    for setting in PROMPTS_OFF {
        let (screen, code) = OnTerminal::start_env(&sandbox, args, &[setting]).finish();
        assert_eq!((code, plain(&screen)), (Some(1), format!("Error: {what} Re-run with --yes to confirm.\n")));
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
    assert!(sandbox.home.path().join(".config/vendo/config.json").exists());

    // `--yes` goes ahead.
    let (screen, code) = OnTerminal::start_env(&sandbox, &["apps", "delete", ID, "--yes"], &[("CI", "true")]).finish();
    assert_eq!(code, Some(0), "{screen:?}");
    assert_eq!(sent(&server).await, vec![format!("DELETE /api/v1/accounts/acct-alpha/apps/{ID}")]);
}

#[cfg(unix)]
#[tokio::test]
async fn with_ci_or_vendo_no_input_empty_0_or_false_the_questions_still_ask() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    for setting in PROMPTS_ON {
        let (screen, code) =
            answer_question(OnTerminal::start_env(&sandbox, &["apps", "delete", ID], &[setting]), "n\n");
        assert_eq!(
            (code, screen.replace("\r\n", "\n")),
            (Some(0), "Delete app 550e8400...? (y/N) n\n".to_string()),
            "{setting:?}"
        );
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn with_prompts_off_a_bare_group_at_a_terminal_is_the_usage_error() {
    let sandbox = Sandbox::new(CLOSED);
    let help = text(&sandbox.run(&["apps", "--help"]).stdout);
    for setting in PROMPTS_OFF {
        let (screen, code) = OnTerminal::start_env(&sandbox, &["apps"], &[setting]).finish();
        assert_eq!((code, plain(&screen)), (Some(2), help.clone()), "{setting:?}");
    }
    for setting in PROMPTS_ON {
        let mut terminal = OnTerminal::start_env(&sandbox, &["apps"], &[setting]);
        terminal.wait_for("Update an app");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{setting:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn on_a_dumb_terminal_a_bare_group_is_the_usage_error_and_the_questions_still_ask() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let help = text(&sandbox.run(&["apps", "--help"]).stdout);
    let (screen, code) = OnTerminal::start_env(&sandbox, &["apps"], &[("TERM", "dumb")]).finish();
    assert_eq!((code, plain(&screen)), (Some(2), help));
    // Any other terminal draws the menu.
    let mut terminal = OnTerminal::start_env(&sandbox, &["apps"], &[("TERM", "xterm-256color")]);
    terminal.wait_for("Update an app");
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));

    let terminal = OnTerminal::start_env(&sandbox, &["apps", "delete", ID], &[("TERM", "dumb")]);
    let (screen, code) = answer_question(terminal, "n\n");
    assert_eq!((code, screen.replace("\r\n", "\n")), (Some(0), "Delete app 550e8400...? (y/N) n\n".to_string()));
    // The profile list is a menu since VE-3892: off on `TERM=dumb`, so `profile switch` with no name
    // prints "Cancelled.", as without a terminal.
    let (screen, code) = OnTerminal::start_env(&sandbox, &["profile", "switch"], &[("TERM", "dumb")]).finish();
    assert_eq!((code, plain(&screen)), (Some(0), "Cancelled.\n".to_string()));
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn the_profile_list_opens_only_where_the_group_menu_does() {
    use std::io::Write;
    // `echo 1 | vendo profile switch` on a terminal: what the pipe holds is no answer. As without a
    // terminal, "Cancelled." and nothing switched. (Before VE-3892's list, the numbered picker read
    // "\n2\n" as every profile listed and beta picked.)
    for input in ["1\n", "\n2\n"] {
        let sandbox = Sandbox::new(CLOSED);
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(input.as_bytes()).unwrap();
        drop(writer);
        let cmd = sandbox.command(&["profile", "switch"]);
        let (screen, code) = OnTerminal::spawn(cmd, (40, 120), "", Some(reader.into()), None).finish();
        assert_eq!((code, plain(&screen)), (Some(0), "Cancelled.\n".to_string()), "{input:?}");
        assert_eq!(sandbox.config()["activeProfile"], "alpha", "{input:?}");
    }
    // Prompts off: the same at a terminal.
    for setting in PROMPTS_OFF {
        let sandbox = Sandbox::new(CLOSED);
        let (screen, code) = OnTerminal::start_env(&sandbox, &["profile", "switch"], &[setting]).finish();
        assert_eq!((code, plain(&screen)), (Some(0), "Cancelled.\n".to_string()), "{setting:?}");
        assert_eq!(sandbox.config()["activeProfile"], "alpha", "{setting:?}");
    }
    // Prompts on, at a terminal the list opens (VE-3892): down to beta, which Enter chooses.
    for setting in PROMPTS_ON {
        let sandbox = Sandbox::new(CLOSED);
        let mut terminal = OnTerminal::start_env(&sandbox, &["profile", "switch"], &[setting]);
        terminal.wait_for("type to filter]");
        terminal.press("\u{1b}[B\r");
        let (screen, code) = terminal.finish();
        assert_eq!(code, Some(0), "{setting:?} {screen:?}");
        assert!(plain(&screen).contains("Switched to profile beta."), "{setting:?} {screen:?}");
        assert_eq!(sandbox.config()["activeProfile"], "beta", "{setting:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_prompts_off_or_no_screen_to_ask_on_a_missing_value_is_the_usage_error() {
    // VE-3881: a command missing a required value asks for it only where the group menu opens.
    // With prompts off, on `TERM=dumb`, with stderr redirected or a stdin no key can be read from,
    // it is clap's usage error, byte for byte what a pipe gets (`usage/` records those), exit 2,
    // and nothing is sent: every command that requires a value.
    let server = MockServer::start().await;
    let sandbox = Sandbox::new(&server.uri());
    let commands = snapshots::commands_requiring_values(&sandbox);
    assert_eq!(commands.len(), 37);
    for (path, _) in &commands {
        let args: Vec<&str> = path.iter().map(String::as_str).collect();
        let piped = sandbox.run(&args);
        assert_eq!(piped.status.code(), Some(2), "vendo {}", args.join(" "));
        let expected = text(&piped.stderr);
        for setting in [("CI", "1"), ("VENDO_NO_INPUT", "1"), ("TERM", "dumb")] {
            let (screen, code) = OnTerminal::start_env(&sandbox, &args, &[setting]).finish();
            assert_eq!((code, plain(&screen)), (Some(2), expected.clone()), "{setting:?} vendo {}", args.join(" "));
        }
        let log = tempfile::NamedTempFile::new().unwrap();
        let mut terminal = OnTerminal::start_with(&sandbox, &args, (40, 120), "", Some(log.reopen().unwrap().into()));
        assert_eq!(terminal.finish(), (String::new(), Some(2)), "2>file vendo {}", args.join(" "));
        assert_eq!(std::fs::read_to_string(log.path()).unwrap(), expected, "2>file vendo {}", args.join(" "));
        let pseudo_terminal = pseudo_terminal_sized(40, 120);
        let write_only = {
            use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
            let name = std::ffi::OsStr::from_bytes(pseudo_terminal.2.as_bytes());
            std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOCTTY).open(name).unwrap()
        };
        let mut terminal =
            OnTerminal::launch_on(pseudo_terminal, sandbox.command(&args), "", Some(write_only.into()), None, true);
        let (screen, code) = terminal.finish();
        assert_eq!((code, plain(&screen)), (Some(2), expected), "0>write-only vendo {}", args.join(" "));
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn with_prompts_off_login_reads_no_press_enter_at_a_terminal_and_says_what_it_says_without_one() {
    // Without a terminal (stdin closed, as on a CI runner) login prints the sign-in URL and "Press
    // ENTER to open in the browser...", which nothing answers, and waits for the browser. With
    // prompts off it does that at a terminal too: it never reads the terminal. Ctrl-D is the key
    // pressed, so a login that reads it opens no browser either: it reads as no answer.
    for (setting, reads) in [
        (("CI", "true"), false),
        (("VENDO_NO_INPUT", "1"), false),
        (("CI", "0"), true),
        (("CI", "false"), true),
        (("VENDO_NO_INPUT", "0"), true),
    ] {
        let server = sign_in_stub(&[NEW_KEY], 401).await;
        let stub = server.uri();
        let sandbox = Sandbox::without_api_keys(&stub);
        let mut terminal = OnTerminal::start_env(&sandbox, &["login"], &[setting]);
        let shown = plain(&terminal.wait_for("Waiting for authorization...\r\n"));
        terminal.press("\u{4}");
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(terminal.takes_typed_end_of_input(), !reads, "{setting:?}: login should read the terminal: {reads}");
        sign_in_at_the_shown_url(terminal, &shown, &stub, &format!("{setting:?}")).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_prompts_off_login_still_reads_a_stdin_that_is_no_terminal() {
    // Without a terminal, login reads stdin whatever it is, as the TS CLI does: `echo | vendo login`
    // opens the browser. A pipe is no prompt, so prompts off leave that as it is. The pipe holds a
    // byte and no line end and stays open until `vendo` has exited, so the read never ends and no
    // browser opens: whether login read it shows in what the pipe still holds.
    use std::io::Write;
    for setting in [None, Some(("CI", "true")), Some(("VENDO_NO_INPUT", "1"))] {
        let server = sign_in_stub(&[NEW_KEY], 401).await;
        let stub = server.uri();
        let sandbox = Sandbox::without_api_keys(&stub);
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"x").unwrap();
        let held = reader.try_clone().unwrap();
        let mut cmd = sandbox.command(&["login"]);
        cmd.envs(setting);
        // After the writer, so dropped first: a failed assertion stops `vendo` before the pipe closes.
        let mut terminal = OnTerminal::spawn(cmd, (40, 120), "", Some(reader.into()), None);
        let shown = plain(&terminal.wait_for("Waiting for authorization...\r\n"));
        let asked = std::time::Instant::now();
        while unread(&held) > 0 && asked.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(unread(&held), 0, "{setting:?}: login should read the pipe");
        sign_in_at_the_shown_url(terminal, &shown, &stub, &format!("{setting:?}")).await;
        drop(writer);
    }
}

/// How many bytes `pipe` holds that nothing has read yet.
#[cfg(unix)]
fn unread(pipe: &std::io::PipeReader) -> usize {
    use std::os::fd::AsRawFd;
    let mut held: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int into `held`.
    assert_eq!(unsafe { libc::ioctl(pipe.as_raw_fd(), libc::FIONREAD as _, &mut held) }, 0);
    held as usize
}

/// The browser's part of a login on `terminal` that has shown `shown` up to "Waiting for
/// authorization...": visit the sign-in URL on the stub, then check that login showed what it shows
/// without a terminal and ends signed in.
#[cfg(unix)]
async fn sign_in_at_the_shown_url(mut terminal: OnTerminal, shown: &str, stub: &str, case: &str) {
    let url = reqwest::Url::parse(shown.lines().find(|line| line.contains("/cli-auth?")).unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"), "tests sign in at their local stub only: {url}");
    assert_eq!(reqwest::get(url.clone()).await.unwrap().status(), 200);
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(0), "{case} {rest:?}");
    let mut shown = shown.to_string();
    for (key, value) in url.query_pairs() {
        shown = shown.replace(&format!("{key}={value}"), &format!("{key}=[{key}]"));
    }
    assert_eq!(shown, browser_sign_in("No API key found. Starting browser login...", stub), "{case}");
    assert!(plain(&rest).ends_with(&signed_in("demo-account", stub, "acct-alpha")), "{case} {rest:?}");
}

// ── VE-3891: `vendo workspace` combines whoami and doctor ────────────────────

/// `vendo workspace <args>` on a test's machine: PATH and SHELL pinned, as its checks read both. The
/// test binary is neither where the installer puts it nor on PATH, and zsh has no completions.
fn workspace(sandbox: &Sandbox, words: &[&str]) -> Command {
    let mut cmd = sandbox.command(words);
    cmd.env("PATH", "/usr/bin:/bin").env("SHELL", "/bin/zsh");
    cmd
}

/// The checks a test's machine fails (see [`workspace`]): the binary and PATH on one line with
/// both fixes, and zsh's completions. Each ends with its line end.
fn machine_checks() -> (String, String) {
    let bin = std::fs::canonicalize(env!("CARGO_BIN_EXE_vendo")).unwrap();
    let version = env!("CARGO_PKG_VERSION");
    let cli = format!(
        "  [fail] CLI {version} at {} (standard install path is ~/.local/bin/vendo), not on PATH\n         Fix: Reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash` if you want the managed install path.\n         Fix: Add `export PATH=\"$HOME/.local/bin:$PATH\"` to `~/.zshrc`, then restart your shell.\n",
        bin.display()
    );
    let zsh = "  [warn] Zsh completions are not installed yet\n         Fix: Reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash` to set them up, or run `vendo completions` for the manual steps.\n";
    (cli, zsh.to_string())
}

/// `base_url` as the profile list shows it: without its scheme.
fn host(base_url: &str) -> &str {
    base_url.split_once("://").map_or(base_url, |(_, rest)| rest)
}

/// A sandbox where the CLI is installed as install.sh installs it, so every check passes: the binary
/// at `~/.local/bin/vendo` (a hard link to the test binary, a copy where that fails), that folder on
/// PATH, and zsh completions set up. HOME is the canonical path, as the running binary's path is
/// (macOS keeps temporary folders behind a symlink).
#[cfg(unix)]
struct Installed {
    sandbox: Sandbox,
    home: std::path::PathBuf,
}

#[cfg(unix)]
impl Installed {
    fn new(config: Value) -> Self {
        let sandbox = Sandbox::new(CLOSED);
        let home = std::fs::canonicalize(sandbox.home.path()).unwrap();
        std::fs::write(home.join(".config/vendo/config.json"), config.to_string()).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let binary = home.join(".local/bin/vendo");
        if std::fs::hard_link(env!("CARGO_BIN_EXE_vendo"), &binary).is_err() {
            std::fs::copy(env!("CARGO_BIN_EXE_vendo"), &binary).unwrap();
        }
        std::fs::create_dir_all(home.join(".local/share/vendo/completions")).unwrap();
        std::fs::write(home.join(".local/share/vendo/completions/vendo.zsh"), "# the script\n").unwrap();
        std::fs::write(home.join(".zshrc"), "# >>> vendo completions >>>\n").unwrap();
        Installed { sandbox, home }
    }

    fn command(&self, words: &[&str]) -> Command {
        let mut cmd = self.sandbox.command_of(self.home.join(".local/bin/vendo"), words);
        let path = format!("{}:/usr/bin:/bin", self.home.join(".local/bin").display());
        cmd.env("HOME", &self.home).env("PATH", path).env("SHELL", "/bin/zsh");
        cmd
    }
}

const T101: &str = "73743172-2a4c-4f0e-9a8b-3f1d2c4b5a69";
const T101_KEY_ID: &str = "2d485183-6b7c-4d8e-9f01-23456789abcd";

#[cfg(unix)]
#[tokio::test]
async fn workspace_shows_the_account_the_profiles_and_the_checks_on_one_screen() {
    // The screen Yalcin agreed (VE-3891, 2026-10-07), on a machine where every check passes: the
    // account once (name and slug as the title), the key masked with its key ID and scopes, the
    // profiles with the active one marked, then the checks doctor ran in a few words each. The
    // profile, key, base URL and account checks pass, and the lines above show them.
    let server = MockServer::start().await;
    let me = json!({
        "accountId": T101, "accountName": "T101", "accountSlug": "t101", "pictureUrl": null,
        "apiKeyId": T101_KEY_ID, "scopes": ["*"],
    });
    serve(&server, "GET", "/api/v1/me", 200, json!({ "data": me })).await;
    let stub = server.uri();
    let installed = Installed::new(json!({
        "profiles": {
            "provin": {
                "apiKey": "vendo_sk_fake_provin_0000", "accountId": "28bb9a3b-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
                "baseUrl": "https://stg.vendodata.com",
            },
            "t101": { "apiKey": "vendo_sk_fake_t101_eKuE", "accountId": T101, "baseUrl": stub },
            "vendo-cli-test": {
                "apiKey": "vendo_sk_fake_cli_test_0000", "accountId": "d3b82ae5-7f6e-4d5c-9b4a-3f2e1d0c9b8a",
                "baseUrl": "https://stg.vendodata.com",
            },
        },
        "activeProfile": "t101",
    }));
    let out = installed.command(&["workspace"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), String::new()));
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        text(&out.stdout),
        format!(
            "T101 · t101
  Account ID:  {T101}
  Profile:     t101
  Base URL:    {stub}
  API key:     vend...eKuE  (key ID {T101_KEY_ID}, scopes *)

Profiles
    provin          28bb9a3b…  stg.vendodata.com
  * t101            73743172…  {}
    vendo-cli-test  d3b82ae5…  stg.vendodata.com

Checks
  [ok] CLI {version} at ~/.local/bin/vendo, on PATH
  [ok] Config ~/.config/vendo/config.json
  [ok] Zsh completions installed
  [ok] Signed in as T101
",
            host(&stub)
        )
    );
}

#[tokio::test]
async fn signed_out_workspace_shows_what_it_can_and_each_check_with_its_fix() {
    // No config file and no key: nothing is asked of the API, and the screen says what the CLI
    // knows (the base URL it would use) and each check with its fix, exit 1 by doctor's rule.
    let sandbox = Sandbox::new(CLOSED);
    std::fs::remove_file(sandbox.home.path().join(".config/vendo/config.json")).unwrap();
    let out = workspace(&sandbox, &["workspace"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    let (cli, zsh) = machine_checks();
    assert_eq!(
        text(&out.stdout),
        format!(
            "  Base URL:    https://app2.vendodata.com

Checks
{cli}  [warn] Config ~/.config/vendo/config.json (not found yet)
         Fix: Run `vendo login` to create and populate CLI config.
  [warn] Selected profile: No active profile selected
         Fix: Run `vendo login` or `vendo profile switch <profile>`.
  [fail] API key: Missing
         Fix: Run `vendo login` or `vendo profile set --api-key <key>`.
  [fail] Account ID: Missing
         Fix: Run `vendo profile set --account <account-id>` or set `VENDO_ACCOUNT_ID`.
{zsh}  [warn] API auth: Skipped because API key or account ID is missing
"
        )
    );
}

#[tokio::test]
async fn workspace_without_a_key_unreachable_or_refused_shows_the_profile_and_the_failing_check() {
    let (cli, zsh) = machine_checks();
    let profiles =
        |base_url: &str| format!("Profiles\n  * alpha  acct-alp…  {0}\n    beta   acct-bet…  {0}\n", host(base_url));

    // Profiles without keys: no request, the API key's check fails with its fix.
    let keyless = Sandbox::without_api_keys(CLOSED);
    let out = workspace(&keyless, &["workspace"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    assert_eq!(
        text(&out.stdout),
        format!(
            "  Account ID:  acct-alpha\n  Profile:     alpha\n  Base URL:    {CLOSED}\n\n{}\nChecks\n{cli}  [ok] Config ~/.config/vendo/config.json\n  [fail] API key: Missing\n         Fix: Run `vendo login` or `vendo profile set --api-key <key>`.\n{zsh}  [warn] API auth: Skipped because API key or account ID is missing\n",
            profiles(CLOSED)
        )
    );

    // Offline: the key from the profile, masked, and the API check fails with doctor's fix.
    let offline = Sandbox::new(CLOSED);
    let out = workspace(&offline, &["workspace"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    assert_eq!(
        text(&out.stdout),
        format!(
            "  Account ID:  acct-alpha\n  Profile:     alpha\n  Base URL:    {CLOSED}\n  API key:     vend...0000\n\n{}\nChecks\n{cli}  [ok] Config ~/.config/vendo/config.json\n{zsh}  [fail] API auth: fetch failed\n         Fix: Check network access and run `vendo workspace --debug` to inspect the failing request.\n",
            profiles(CLOSED)
        )
    );

    // A refused key (401): its fix says to sign in again and check with `vendo workspace`.
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/v1/me", 401, json!({ "error": { "message": "Invalid API key" } })).await;
    let refused = Sandbox::new(&server.uri());
    let out = workspace(&refused, &["workspace"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    assert_eq!(
        text(&out.stdout),
        format!(
            "  Account ID:  acct-alpha\n  Profile:     alpha\n  Base URL:    {0}\n  API key:     vend...0000\n\n{1}\nChecks\n{cli}  [ok] Config ~/.config/vendo/config.json\n{zsh}  [fail] API auth: HTTP 401: Unauthorized\n         Fix: Run `vendo login` to refresh credentials, then retry `vendo workspace`.\n",
            server.uri(),
            profiles(&server.uri())
        )
    );
}

#[tokio::test]
async fn workspace_json_keeps_every_key_whoami_and_doctor_printed() {
    // Scripts read `vendo whoami --json` and `vendo doctor --json`; `vendo workspace --json` has every
    // key either had: whoami's (/me's response as sent, then `config`), then doctor's (`summary`,
    // `checks`, `suggestions`, `identity`, `shell`).
    let server = MockServer::start().await;
    let me = json!({
        "accountId": "acct-alpha", "accountName": "Demo Account (synthetic)", "accountSlug": "demo-account",
        "pictureUrl": null, "apiKeyId": "key_fake_0001", "scopes": [],
        "bigquery": { "projectId": "demo-project", "prodDatasetId": "demo_prod", "sourceDatasetId": "demo_source" },
    });
    serve(&server, "GET", "/api/v1/me", 200, json!({ "data": me })).await;
    let sandbox = Sandbox::new(&server.uri());
    let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    let out = workspace(&sandbox, &["workspace", "--json"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(keys(&printed), ["data", "config", "summary", "checks", "suggestions", "identity", "shell"]);
    // whoami's
    assert_eq!(printed["data"], me);
    assert_eq!(
        printed["config"],
        json!({
            "selectedProfile": "alpha", "apiKeySource": "profile", "baseUrl": server.uri(),
            "baseUrlSource": "profile", "accountId": "acct-alpha", "accountIdSource": "profile",
        })
    );
    assert_eq!(
        keys(&printed["config"]),
        ["selectedProfile", "apiKeySource", "baseUrl", "baseUrlSource", "accountId", "accountIdSource"]
    );
    // doctor's
    assert_eq!(printed["summary"], json!({ "ok": 6, "warn": 2, "fail": 1 }));
    let checks = printed["checks"].as_array().unwrap();
    let names: Vec<&str> = checks.iter().map(|check| check["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "CLI binary",
            "PATH",
            "Config file",
            "Selected profile",
            "API key",
            "Base URL",
            "Account ID",
            "Shell completions",
            "API auth"
        ]
    );
    for check in checks {
        let fields = keys(check);
        let expected: &[&str] = if check["status"] == "ok" {
            &["name", "status", "detail"]
        } else {
            &["name", "status", "detail", "remediation"]
        };
        assert_eq!(fields, expected, "{check}");
    }
    assert_eq!(
        checks[8],
        json!({ "name": "API auth", "status": "ok", "detail": "Authenticated as Demo Account (synthetic)" })
    );
    assert_eq!(printed["suggestions"].as_array().unwrap().len(), 3);
    assert_eq!(printed["identity"], me);
    assert_eq!(printed["shell"], "zsh");

    // Without an answer from /me, what whoami printed of it and doctor's identity are left out.
    let keyless = Sandbox::without_api_keys(&server.uri());
    let out = workspace(&keyless, &["workspace", "--json"]).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(1), String::new()));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(keys(&printed), ["config", "summary", "checks", "suggestions", "shell"]);
    assert_eq!(printed["config"]["apiKeySource"], "missing");
}

#[tokio::test]
async fn whoami_doctor_and_their_old_paths_print_exactly_what_workspace_prints() {
    // Hidden aliases (VE-3891): clap's for `whoami` and `doctor`, MOVED for `profile current` and
    // `config show`. Byte for byte, exit code and stderr too, signed in or not, text and --json.
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/v1/me", 200, json!({ "data": { "accountId": "acct-alpha", "accountName": "Acme" } }))
        .await;
    let refusing = MockServer::start().await;
    serve(&refusing, "GET", "/api/v1/me", 401, json!({ "error": { "message": "Invalid API key" } })).await;
    let sandboxes =
        [Sandbox::new(&server.uri()), Sandbox::without_api_keys(&server.uri()), Sandbox::new(&refusing.uri())];
    let run = |sandbox: &Sandbox, words: &[&str]| {
        let out = workspace(sandbox, words).output().unwrap();
        (out.status.code(), out.stdout, out.stderr)
    };
    for sandbox in &sandboxes {
        for json in [&[][..], &["--json"]] {
            let expected = run(sandbox, &[&["workspace"][..], json].concat());
            for old in [&["whoami"][..], &["doctor"], &["profile", "current"], &["config", "show"]] {
                let old = [old, json].concat();
                assert!(run(sandbox, &old) == expected, "vendo {} differs from vendo workspace", old.join(" "));
            }
        }
    }
    // Hidden: the main help and `vendo commands` name workspace only.
    let sandbox = &sandboxes[0];
    for words in [&["--help"][..], &["commands"]] {
        let shown = ok_output(&sandbox.run(words));
        assert!(shown.contains("\n  workspace ") || shown.contains("\nworkspace "), "{shown}");
        assert!(!shown.contains("whoami") && !shown.contains("doctor"), "{shown}");
    }
}

// ── VE-3892: `--profile` with no name, and `profile switch` with none, open the profile list ──────
// Decided by Yalcin 2026-10-07. Where the group menu opens (`output::can_show_menu`: stdin, stdout and
// stderr terminals, prompts on, `TERM` not `dumb`, the hang-up watch), the saved profiles show in an
// arrow-key list with type-to-filter, each row the active marker, the name, the account ID and the base
// URL's host. On its own (`vendo --profile`) the chosen profile becomes the active one exactly as
// `vendo profile switch <name>` makes it; at the end of a command (`vendo apps list --profile`) it is
// that command's alone, as if `--profile <name>` had been typed. `vendo profile switch` with no name
// opens the same list in place of its numbered picker. Without a terminal `--profile` with no name is
// clap's usage error as before (`usage/profile_flag`), and `profile switch` prints "Cancelled.".

/// The profile list's rows for `profiles` (named as [`Sandbox::new`] names them, `acct-<name>`) on
/// `base_url`, `*` on `active`: the name and account ID padded to the widest, then the host.
fn profile_rows(base_url: &str, profiles: &[&str], active: &str) -> Vec<String> {
    let host = base_url.split_once("://").map_or(base_url, |(_, rest)| rest);
    let row = |name: &&str| {
        let marker = if *name == active { '*' } else { ' ' };
        format!("{marker} {name:<5}  {:<10}  {host}", format!("acct-{name}"))
    };
    profiles.iter().map(row).collect()
}

/// What an open profile list titled `title` shows: the title, `rows` with the first marked, the hint.
fn profile_list(title: &str, rows: &[String]) -> Vec<String> {
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    [vec![format!("? {title}")], marked(&rows), vec![HINT.to_string()]].concat()
}

/// [`Sandbox::new`] with a third profile, `gamma`, after the other two.
fn with_gamma(sandbox: &Sandbox) {
    let mut config = sandbox.config();
    let alpha = config["profiles"]["alpha"].clone();
    let gamma = json!({ "apiKey": GAMMA_KEY, "accountId": "acct-gamma", "baseUrl": alpha["baseUrl"] });
    config["profiles"].as_object_mut().unwrap().insert("gamma".into(), gamma);
    std::fs::write(sandbox.home.path().join(".config/vendo/config.json"), config.to_string()).unwrap();
}

#[cfg(unix)]
#[test]
fn profile_switch_without_a_name_chooses_from_the_profile_list() {
    // Arrow keys, or typing to filter (any case); `config use`, its old name (VE-3827), is titled as the
    // tree names it. What follows the answer is what `vendo profile switch beta` prints.
    for (args, keys) in [(&["profile", "switch"][..], "\u{1b}[B\r"), (&["config", "use"], "BET\r")] {
        let sandbox = Sandbox::new(CLOSED);
        let case = args.join(" ");
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        let rows = profile_rows(CLOSED, &["alpha", "beta"], "alpha");
        assert_eq!(shown_lines(&terminal, (40, 120)), profile_list("vendo profile switch", &rows), "{case}");
        terminal.press(keys);
        terminal.wait_for("vendo profile switch beta");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        let typed = Sandbox::new(CLOSED).run(&["profile", "switch", "beta"]);
        assert_eq!(after_answer(&terminal, "? vendo profile switch beta"), printed_lines(&typed), "{case}");
        assert_eq!(sandbox.config()["activeProfile"], "beta", "{case}");
    }
}

#[cfg(unix)]
#[test]
fn profile_with_no_name_on_its_own_makes_the_chosen_profile_active_as_profile_switch_does() {
    for args in [&["--profile"][..], &["--debug", "--profile"]] {
        let sandbox = Sandbox::new(CLOSED);
        let case = args.join(" ");
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        let rows = profile_rows(CLOSED, &["alpha", "beta"], "alpha");
        assert_eq!(shown_lines(&terminal, (40, 120)), profile_list("vendo --profile", &rows), "{case}");
        terminal.press("\u{1b}[B\r");
        terminal.wait_for("vendo --profile beta");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        let typed = Sandbox::new(CLOSED).run(&["profile", "switch", "beta"]);
        assert_eq!(after_answer(&terminal, "? vendo --profile beta"), printed_lines(&typed), "{case}");
        assert_eq!(sandbox.config()["activeProfile"], "beta", "{case}");
    }
    // Under VENDO_PROFILE the list marks the profile it selects, as `profile list` does; the chosen one
    // becomes the saved active profile, and what follows says VENDO_PROFILE still overrides it in this
    // shell, as `profile switch` says.
    let sandbox = Sandbox::new(CLOSED);
    with_gamma(&sandbox);
    let mut terminal = OnTerminal::start_env(&sandbox, &["--profile"], &[("VENDO_PROFILE", "beta")]);
    terminal.wait_for("type to filter]");
    let rows = profile_rows(CLOSED, &["alpha", "beta", "gamma"], "beta");
    assert_eq!(shown_lines(&terminal, (40, 120)), profile_list("vendo --profile", &rows));
    terminal.press("gam\r");
    terminal.wait_for("vendo --profile gamma");
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(0), "{rest:?}");
    let twin = Sandbox::new(CLOSED);
    with_gamma(&twin);
    let typed = twin.command(&["profile", "switch", "gamma"]).env("VENDO_PROFILE", "beta").output().unwrap();
    let printed = printed_lines(&typed);
    assert!(printed.iter().any(|line| line.contains("VENDO_PROFILE=beta still overrides it")), "{printed:#?}");
    assert_eq!(after_answer(&terminal, "? vendo --profile gamma"), printed);
    assert_eq!(sandbox.config()["activeProfile"], "gamma");
}

#[cfg(unix)]
#[tokio::test]
async fn a_command_ending_in_profile_with_no_name_runs_with_the_chosen_profile_for_that_command_alone() {
    let server = MockServer::start().await;
    let app = |name: &str| {
        json!({
            "id": MENU_APP, "appType": "shopify", "displayName": name, "permissions": ["performance_data"],
            "roles": ["source"], "state": "active", "accessStatus": "connected", "lastSyncAt": null,
        })
    };
    serve(&server, "GET", "/api/v1/accounts/acct-alpha/apps", 200, json!({ "data": [app("Alpha Shop")] })).await;
    serve(&server, "GET", "/api/v1/accounts/acct-beta/apps", 200, json!({ "data": [app("Beta Shop")] })).await;
    let sandbox = Sandbox::new(&server.uri());
    let rows = |active: &str| profile_rows(&server.uri(), &["alpha", "beta"], active);
    /// The words, VENDO_PROFILE, the list's title, the profile its marker is on, the keys typed and the
    /// profile they choose.
    struct Case<'a>(&'a [&'a str], Option<&'a str>, &'a str, &'a str, &'a str, &'a str);
    // `config list` is `profile list`'s old name (VE-3827): titled as the tree names it.
    let cases = [
        Case(&["apps", "list", "--profile"], None, "vendo apps list --profile", "alpha", "\u{1b}[B\r", "beta"),
        Case(&["apps", "list", "--json", "--profile"], None, "vendo apps list --profile", "alpha", "BETA\r", "beta"),
        Case(&["config", "list", "--profile"], None, "vendo profile list --profile", "alpha", "\u{1b}[B\r", "beta"),
        // The flag, as typed, wins over VENDO_PROFILE, whose profile the list marks.
        Case(&["apps", "list", "--profile"], Some("beta"), "vendo apps list --profile", "beta", "\r", "alpha"),
    ];
    for Case(args, vendo_profile, title, active, keys, chosen) in cases {
        let case = format!("{vendo_profile:?} {}", args.join(" "));
        let env: Vec<(&str, &str)> = vendo_profile.map(|name| ("VENDO_PROFILE", name)).into_iter().collect();
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start_env(&sandbox, args, &env);
        terminal.wait_for("type to filter]");
        assert_eq!(shown_lines(&terminal, (40, 120)), profile_list(title, &rows(active)), "{case}");
        terminal.press(keys);
        let answer = format!("? {title} {chosen}");
        terminal.wait_for(&answer[2..]);
        // `apps list` at a terminal shows its table's rows to choose from (VE-3894), which Esc leaves;
        // `config list`, the profiles, each marked as the chosen profile marks them.
        let listed = (args[0] == "apps" && !args.contains(&"--json")).then(|| {
            terminal.wait_for("type to filter · 1 app]");
            let listed = shown_from(&terminal, "? vendo apps list");
            terminal.press("\u{1b}");
            listed
        });
        if args[0] == "config" {
            terminal.wait_for("vendo profile list");
            terminal.wait_for("type to filter]");
            let profiles = profile_list_rows(&server.uri(), &["alpha", "beta"], chosen);
            assert_eq!(shown_from(&terminal, "? vendo profile list"), profile_list("vendo profile list", &profiles));
            terminal.press("\u{1b}");
        }
        let (rest, code) = terminal.finish();
        let asked = sent(&server).await[before..].to_vec();
        // The same words with the profile typed after `--profile`, on a pipe.
        let typed_args: Vec<&str> = args.iter().copied().chain([chosen]).collect();
        let typed = sandbox.command(&typed_args).envs(env.iter().copied()).output().unwrap();
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!(code, typed.status.code(), "{case}: {rest:?}");
        assert_eq!((code, &asked), (Some(0), &typed_sent), "{case}");
        let printed = table_cells(&printed_lines(&typed));
        match listed {
            // The typed table's rows, of the chosen profile's apps.
            Some(listed) => {
                let rows: Vec<String> = listed[1..listed.len() - 1].iter().map(|row| row[2..].to_string()).collect();
                assert_eq!(table_cells(&rows), printed[1..printed.len() - 1], "{case}");
                assert_eq!(after_answer(&terminal, &answer), ["? vendo apps list <canceled>"], "{case}");
            }
            // The profiles, whose list Esc left.
            None if args[0] == "config" => {
                assert_eq!(after_answer(&terminal, &answer), ["? vendo profile list <canceled>"], "{case}");
                let profiles = profile_list_rows(&server.uri(), &["alpha", "beta"], chosen);
                assert_eq!(table_cells(&profiles), printed, "{case}: the lines typed");
            }
            // What it showed after the answer is what the typed command printed (a table has borders only
            // at a terminal).
            None => assert_eq!(table_cells(&after_answer(&terminal, &answer)), printed, "{case}"),
        }
        // For that command alone: the saved active profile stays.
        assert_eq!(sandbox.config()["activeProfile"], "alpha", "{case}");
    }
    let listed = |account: &str| format!("GET /api/v1/accounts/{account}/apps?limit=20&offset=0");
    // Each list run, at the terminal and then typed on a pipe.
    let (beta, alpha) = (listed("acct-beta"), listed("acct-alpha"));
    assert_eq!(sent(&server).await, [&beta, &beta, &beta, &beta, &alpha, &alpha].map(String::as_str));

    // After a group, as if typed: `vendo apps --profile beta` then opens the group's menu, whose first
    // command, `list`, runs with beta: beta's apps to choose from (VE-3894).
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "--profile"]);
    terminal.wait_for("type to filter]");
    assert_eq!(shown_lines(&terminal, (40, 120)), profile_list("vendo apps --profile", &rows("alpha")));
    terminal.press("\u{1b}[B\r");
    terminal.wait_for("vendo apps --profile beta");
    terminal.wait_for("Update an app");
    terminal.press("\r");
    terminal.wait_for("vendo apps list");
    terminal.wait_for("type to filter · 1 app]");
    let listed = shown_from(&terminal, "? vendo apps list");
    assert_eq!(listed[1], "> a1b2c3d4...  Beta Shop  shopify  source  active  connected  —", "{listed:#?}");
    terminal.press("\u{1b}");
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(0), "{rest:?}");
    assert_eq!(sent(&server).await[6..], [beta]);
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

/// The three ways to open the profile list, each with its title.
#[cfg(unix)]
const PROFILE_LISTS: [(&[&str], &str); 3] = [
    (&["--profile"], "vendo --profile"),
    (&["apps", "list", "--profile"], "vendo apps list --profile"),
    (&["profile", "switch"], "vendo profile switch"),
];

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_the_profile_list_leave_quietly() {
    // As at the group menu (VE-3826) and the lists for a missing value (VE-3881): exit 0, nothing run
    // or switched, the title and `<canceled>` where the list was, and the cursor on the next line.
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, title) in PROFILE_LISTS {
        let mut screens = Vec::new();
        for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
            let case = format!("{name} {}", args.join(" "));
            let mut terminal = OnTerminal::start(&sandbox, args);
            terminal.wait_for("type to filter]");
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let (shown, cursor) = screen(&terminal.screen, 40, 120);
            let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
            assert_eq!((shown[last].clone(), cursor), (format!("? {title} <canceled>"), (last + 1, 0)), "{case}");
            screens.push((shown, cursor));
        }
        assert!(screens.iter().all(|screen| *screen == screens[0]), "{screens:#?}");
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[tokio::test]
async fn the_profile_list_exits_as_ctrl_d_does_when_its_terminal_hangs_up() {
    // The list runs as the group menu runs, with its hang-up watch: on a terminal that hangs up it
    // exits 0 at once, running and switching nothing, and does not spin.
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut outcomes = Vec::new();
    for (args, _) in PROFILE_LISTS {
        let mut terminal = OnTerminal::start_detached(&sandbox, args);
        terminal.wait_for("type to filter]");
        let (exited, ended) = hang_up(&mut terminal);
        outcomes.push((args, exited, ended));
    }
    assert!(
        outcomes.iter().all(|(_, exited, ended)| *exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "each should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {outcomes:#?}"
    );
    assert_eq!(sent(&server).await, Vec::<String>::new());
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[tokio::test]
async fn where_the_list_cannot_open_profile_with_no_name_is_the_usage_error_and_profile_switch_says_cancelled() {
    // Byte for byte what a pipe gets (`usage/profile_flag` records it), exit 2, nothing read, sent or
    // switched: with prompts off, on `TERM=dumb`, with stdout or stderr elsewhere, or a stdin no key can be
    // read from.
    let server = MockServer::start().await;
    let sandbox = Sandbox::new(&server.uri());
    let write_only_terminal = || {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let pseudo_terminal = pseudo_terminal_sized(40, 120);
        let name = std::ffi::OsStr::from_bytes(pseudo_terminal.2.as_bytes());
        let write_only = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOCTTY).open(name).unwrap();
        (pseudo_terminal, write_only)
    };
    // Nothing is read either (`ask.rs`): a legacy flat config, which reading migrates and saves
    // (`ConfigStore::read`), stays byte for byte as it was (VE-3892 review).
    let legacy = Sandbox::new(&server.uri());
    let legacy_config = r#"{"apiKey":"vendo_sk_fake_alpha_0000","accountId":"acct-alpha"}"#;
    std::fs::write(legacy.home.path().join(".config/vendo/config.json"), legacy_config).unwrap();
    for sandbox in [&sandbox, &legacy] {
        let config_path = sandbox.home.path().join(".config/vendo/config.json");
        let saved = std::fs::read_to_string(&config_path).unwrap();
        let untouched = |case: &str| assert_eq!(std::fs::read_to_string(&config_path).unwrap(), saved, "{case}");
        for args in snapshots::PROFILE_WITHOUT_A_NAME {
            let case = args.join(" ");
            let piped = sandbox.run(args);
            assert_eq!((piped.status.code(), text(&piped.stdout)), (Some(2), String::new()), "{case}");
            untouched(&format!("<null vendo {case}"));
            let expected = text(&piped.stderr);
            for setting in [("CI", "1"), ("VENDO_NO_INPUT", "1"), ("TERM", "dumb")] {
                let (screen, code) = OnTerminal::start_env(sandbox, args, &[setting]).finish();
                assert_eq!((code, plain(&screen)), (Some(2), expected.clone()), "{setting:?} vendo {case}");
                untouched(&format!("{setting:?} vendo {case}"));
            }
            let (_controller, terminal) = pseudo_terminal();
            let out = sandbox.command(args).stdin(terminal).output().unwrap();
            let shown = (out.status.code(), text(&out.stdout), text(&out.stderr));
            assert_eq!(shown, (Some(2), String::new(), expected.clone()), "| vendo {case}");
            untouched(&format!("| vendo {case}"));
            let log = tempfile::NamedTempFile::new().unwrap();
            let stderr = Some(log.reopen().unwrap().into());
            let mut terminal = OnTerminal::start_with(sandbox, args, (40, 120), "", stderr);
            assert_eq!(terminal.finish(), (String::new(), Some(2)), "2>file vendo {case}");
            assert_eq!(std::fs::read_to_string(log.path()).unwrap(), expected, "2>file vendo {case}");
            untouched(&format!("2>file vendo {case}"));
            let (pseudo_terminal, write_only) = write_only_terminal();
            let cmd = sandbox.command(args);
            let mut terminal = OnTerminal::launch_on(pseudo_terminal, cmd, "", Some(write_only.into()), None, true);
            let (screen, code) = terminal.finish();
            assert_eq!((code, plain(&screen)), (Some(2), expected), "0>write-only vendo {case}");
            untouched(&format!("0>write-only vendo {case}"));
        }
    }
    // `profile switch` with no name prints "Cancelled." and switches nothing, as without a terminal (a
    // pipe and prompts off: `the_profile_list_opens_only_where_the_group_menu_does`).
    let cancelled = (Some(0), "Cancelled.\n".to_string());
    let (screen, code) = OnTerminal::start_env(&sandbox, &["profile", "switch"], &[("TERM", "dumb")]).finish();
    assert_eq!((code, plain(&screen)), cancelled, "TERM=dumb");
    let log = tempfile::NamedTempFile::new().unwrap();
    let stderr = Some(log.reopen().unwrap().into());
    let (screen, code) = OnTerminal::start_with(&sandbox, &["profile", "switch"], (40, 120), "", stderr).finish();
    assert_eq!(
        (code, plain(&screen), std::fs::read_to_string(log.path()).unwrap()),
        (cancelled.0, cancelled.1.clone(), String::new())
    );
    let (pseudo_terminal, write_only) = write_only_terminal();
    let cmd = sandbox.command(&["profile", "switch"]);
    let (screen, code) = OnTerminal::launch_on(pseudo_terminal, cmd, "", Some(write_only.into()), None, true).finish();
    assert_eq!((code, plain(&screen)), cancelled, "0>write-only");
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[test]
fn with_no_saved_profile_profile_with_no_name_says_so_and_is_the_usage_error() {
    // As a list for a missing value with nothing in it (VE-3881): it says so, then the usage error.
    // `profile switch` says what it said before.
    let sandbox = Sandbox::new(CLOSED);
    let empty = tempfile::tempdir().unwrap();
    for args in [&["--profile"][..], &["apps", "list", "--profile"]] {
        let piped = sandbox.command(args).env("HOME", empty.path()).output().unwrap();
        let mut cmd = sandbox.command(args);
        cmd.env("HOME", empty.path());
        let (screen, code) = OnTerminal::spawn(cmd, (40, 120), "", None, None).finish();
        let expected = format!("No profiles to choose from.\n{}", text(&piped.stderr));
        assert_eq!((code, plain(&screen)), (Some(2), expected), "{args:?}");
    }
    let mut cmd = sandbox.command(&["profile", "switch"]);
    cmd.env("HOME", empty.path());
    let (screen, code) = OnTerminal::spawn(cmd, (40, 120), "", None, None).finish();
    assert_eq!((code, plain(&screen)), (Some(0), "No profiles yet. Run `vendo login` to create one.\n".to_string()));
    assert!(!empty.path().join(".config").exists(), "nothing is saved");
}

// ── VE-3894: a list command's rows to choose from at a terminal ─────────────
// Decided by Yalcin, 2026-10-07, CLI 1.1 ("items first then actions, but still keep the actions so we
// can go directly to the action without the list too"): where the group menu opens (VE-3826), a list
// command shows the rows its table shows, from the same request, as an arrow-key list with
// type-to-filter, its footer after the hint. Enter shows the item as its group's `get` shows it, then
// the actions that apply to it and `back`; an action runs exactly as typed, Back opens the list again
// on that item, and Esc leaves. Elsewhere the table prints as before, byte for byte.

const OLD_APP: &str = "d4c3b2a1-0000-4000-8000-000000000004";

/// The apps a selectable list shows: one of each state (`deleted`, which the list does not normally
/// send, stands for any other), the last synced two hours ago.
fn apps_to_browse() -> Vec<Value> {
    let mut synced = app_to_choose(CHOOSE_PIXEL, "Demo Pixel", "meta_ads", &["destination"], "inactive");
    synced["lastSyncAt"] = json!(minutes_ago(130));
    vec![
        app_to_choose(MENU_APP, "Menu Shop", "shopify", &["source"], "active"),
        synced,
        app_to_choose(OLD_APP, "Old Shop", "shopify", &["source"], "deleted"),
    ]
}

/// [`apps_to_browse`] as the list shows them: the table's cells as plain text, padded per column.
const BROWSE_ROWS: [&str; 3] = [
    "a1b2c3d4...  Menu Shop   shopify   source       active    connected  —",
    "0c0d0e0f...  Demo Pixel  meta_ads  destination  inactive  paused     2h ago",
    "d4c3b2a1...  Old Shop    shopify   source       deleted   connected  —",
];

/// The list's hint: the menu's, then the table's footer.
const BROWSE_HINT: &str = "[↑↓ to move, enter to select, type to filter · 3 apps]";

/// `apps` in `account`: the list (any query) and `{ data: <app> }` for every request about one.
async fn apps_stub(account: &str, apps: Vec<Value>) -> MockServer {
    let server = MockServer::start().await;
    let route = format!("/api/v1/accounts/{account}/apps");
    serve(&server, "GET", &route, 200, page_of(apps.clone(), 0, false)).await;
    for app in apps {
        Mock::given(wiremock::matchers::path_regex(format!("^{route}/{}(/.*)?$", app["id"].as_str().unwrap())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": app })))
            .mount(&server)
            .await;
    }
    server
}

/// What an open apps list shows with the cursor on the row at `at`.
fn apps_list(at: usize) -> Vec<String> {
    let rows = BROWSE_ROWS.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == at { '>' } else { ' ' }));
    [vec!["? vendo apps list".to_string()], rows.collect(), vec![BROWSE_HINT.to_string()]].concat()
}

/// The action menu of an app, `actions` its rows before `back`, as the menu shows them.
fn app_actions(actions: &[&str]) -> Vec<String> {
    let about = |action: &str| match action {
        "pause" => "Pause an app",
        "resume" => "Resume a paused app",
        "update" => "Update an app",
        "delete" => "Delete an app (soft delete)",
        _ => "Back to the list",
    };
    let rows = actions.iter().chain(&["back"]).map(|action| format!("{action:<6}  {}", about(action)));
    let rows: Vec<String> = rows.collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    [vec!["? vendo apps".to_string()], marked(&rows), vec![HINT.to_string()]].concat()
}

/// The lines the terminal shows from the last one that is `first` on, as [`shown_lines`] reads them.
fn shown_from(terminal: &OnTerminal, first: &str) -> Vec<String> {
    let shown = shown_lines(terminal, (40, 120));
    let at = shown.iter().rposition(|line| line == first).unwrap_or_else(|| panic!("{first:?}: {shown:#?}"));
    shown[at..].to_vec()
}

#[cfg(unix)]
#[tokio::test]
async fn apps_list_at_a_terminal_lists_the_tables_rows_to_choose_from_with_its_footer_in_the_hint() {
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let piped = sandbox.run(&["apps", "list"]);
    let piped_sent = sent(&server).await;
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
    terminal.wait_for("type to filter · 3 apps]");
    // Titled with the command, every row the table shows (its cells, plain), the first marked, and
    // the table's footer after the hint.
    let shown = shown_lines(&terminal, (40, 120));
    assert_eq!(shown, apps_list(0));
    let table = table_cells(&printed_lines(&piped));
    let listed: Vec<String> = shown[1..shown.len() - 1].iter().map(|row| row[2..].to_string()).collect();
    assert_eq!(table_cells(&listed), table[1..table.len() - 1], "the table's rows, without its header and footer");
    assert_eq!(table.last().unwrap(), &["3 apps"]);
    terminal.press("\u{1b}");
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(0), "{rest:?}");
    // The list sent what the table sends, and nothing more.
    assert_eq!(sent(&server).await[piped_sent.len()..], piped_sent);
    assert_eq!(piped_sent, [format!("GET {V1}/apps?limit=20&offset=0")]);
}

#[cfg(unix)]
#[tokio::test]
async fn enter_shows_the_app_as_apps_get_does_then_the_actions_that_apply_to_its_state() {
    // Active: pause; inactive: resume; any other state: neither. Then update, delete and back, each
    // with its description as `vendo apps --help` lists it.
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let help = command_rows(&sandbox, &["apps"]);
    for (at, id, actions) in [
        (0, MENU_APP, &["pause", "update", "delete"][..]),
        (1, CHOOSE_PIXEL, &["resume", "update", "delete"]),
        (2, OLD_APP, &["update", "delete"]),
    ] {
        let before = sent(&server).await.len();
        let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
        terminal.wait_for("type to filter · 3 apps]");
        if at > 0 {
            terminal.press(&"\u{1b}[B".repeat(at));
            terminal.wait_for(&format!("> {}", BROWSE_ROWS[at]));
        }
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        let asked = sent(&server).await[before..].to_vec();
        // What `vendo apps get <full id>` sends and prints on a pipe.
        let typed = sandbox.run(&["apps", "get", id]);
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        assert_eq!(asked, [&[format!("GET {V1}/apps?limit=20&offset=0")][..], &typed_sent].concat(), "{id}");
        let name = BROWSE_ROWS[at][13..].split("  ").next().unwrap();
        let answer = format!("? vendo apps list {} ({name})", &BROWSE_ROWS[at][..11]);
        let shown = shown_from(&terminal, &answer);
        let menu = app_actions(actions);
        let details = &shown[1..shown.len() - menu.len()];
        let details: Vec<String> = details.iter().filter(|line| !line.is_empty()).cloned().collect();
        assert_eq!(details, printed_lines(&typed), "{id}");
        assert_eq!(shown[shown.len() - menu.len()..], menu, "{id}");
        // Each described as the group's help describes it.
        let described =
            |row: &str| row.split_once(' ').map(|(name, about)| (name.to_string(), about.trim().to_string()));
        for row in &menu[1..=actions.len()] {
            let (name, about) = described(&row[2..]).unwrap();
            assert!(help.iter().any(|line| described(line) == Some((name.clone(), about.clone()))), "{row}: {help:#?}");
        }
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{id}: {rest:?}");
        assert_eq!(sent(&server).await.len(), before + asked.len() + typed_sent.len(), "{id}: nothing more sent");
    }
}

/// Opens `vendo <args>` (an apps list), chooses the row at `at` and waits for its action menu.
#[cfg(unix)]
fn app_menu_of(sandbox: &Sandbox, args: &[&str], env: &[(&str, &str)], at: usize) -> OnTerminal {
    let mut terminal = OnTerminal::start_env(sandbox, args, env);
    terminal.wait_for("type to filter · 3 apps]");
    if at > 0 {
        terminal.press(&"\u{1b}[B".repeat(at));
        terminal.wait_for(&format!("> {}", BROWSE_ROWS[at]));
    }
    terminal.press("\r");
    terminal.wait_for("Back to the list");
    terminal.wait_for("type to filter]");
    terminal
}

#[cfg(unix)]
#[tokio::test]
async fn a_chosen_action_runs_exactly_as_the_typed_command_and_the_cli_ends_with_its_exit_code() {
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    /// The row chosen, its name, the keys that choose the action, the action, what the y/N question is
    /// answered (none when it asks none), and the same command typed with the full ID.
    struct Case<'a>(usize, &'a str, &'a str, &'a str, Option<&'a str>, &'a [&'a str]);
    let cases = [
        Case(0, "Menu Shop", "\r", "pause", None, &["apps", "pause", MENU_APP]),
        Case(1, "Demo Pixel", "\r", "resume", None, &["apps", "resume", CHOOSE_PIXEL]),
        // No change to make: it fails after the choice as when typed, exit 1 (VE-3881's Q13).
        Case(2, "Old Shop", "upd\r", "update", None, &["apps", "update", OLD_APP]),
        // The y/N of a delete (VE-3823) asks as when typed: n sends nothing, y deletes.
        Case(0, "Menu Shop", "\u{1b}[B\u{1b}[B\r", "delete", Some("n\n"), &[]),
        Case(1, "Demo Pixel", "DELETE\r", "delete", Some("y\n"), &["apps", "delete", CHOOSE_PIXEL, "--yes"]),
    ];
    for Case(at, name, keys, action, answer, typed_args) in cases {
        let case = format!("{action} {name} {answer:?}");
        let before = sent(&server).await.len();
        let mut terminal = app_menu_of(&sandbox, &["apps", "list"], &[], at);
        terminal.press(keys);
        let answered = format!("? vendo apps {action} {} ({name})", &BROWSE_ROWS[at][..11]);
        terminal.wait_for(&answered[2..]);
        if let Some(answer) = answer {
            terminal.wait_for("(y/N) ");
            terminal.press(answer);
        }
        let (rest, code) = terminal.finish();
        let asked = sent(&server).await[before..].to_vec();
        // The list and the app, then what the typed command sends, and nothing else.
        let typed = (!typed_args.is_empty()).then(|| sandbox.run(typed_args));
        let typed_sent = sent(&server).await[before + asked.len()..].to_vec();
        let shown = [MENU_APP, CHOOSE_PIXEL, OLD_APP][at];
        assert_eq!(
            asked[..2],
            [format!("GET {V1}/apps?limit=20&offset=0"), format!("GET {V1}/apps/{shown}")],
            "{case}"
        );
        assert_eq!(asked[2..], typed_sent, "{case}");
        match typed {
            Some(typed) => {
                assert_eq!(code, typed.status.code(), "{case}: {rest:?}");
                // What it showed after the answer (and the y/N question, answered) is what the typed
                // command printed.
                let mut after = after_answer(&terminal, &answered);
                after.retain(|line| !line.contains("(y/N)"));
                assert_eq!(after, printed_lines(&typed), "{case}");
            }
            None => {
                assert_eq!(code, Some(0), "{case}: {rest:?}");
                assert!(asked.len() == 2, "{case}: {asked:#?}");
                assert_eq!(after_answer(&terminal, &answered), ["Delete app a1b2c3d4...? (y/N) n"], "{case}");
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn back_opens_the_list_again_on_the_item_with_no_request_and_the_filter_cleared() {
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
    terminal.wait_for("type to filter · 3 apps]");
    // Filtered down to Demo Pixel, which Enter shows.
    terminal.press("pixel");
    terminal.wait_for("vendo apps list pixel");
    terminal.press("\r");
    terminal.wait_for("Back to the list");
    terminal.wait_for("type to filter]");
    let shown_once = sent(&server).await;
    terminal.press("back\r");
    terminal.wait_for("type to filter · 3 apps]");
    terminal.wait_for("\u{1b}[?25h");
    // The same rows, the filter cleared, the cursor on the app just viewed; nothing sent.
    assert_eq!(shown_from(&terminal, "? vendo apps list"), apps_list(1));
    assert_eq!(sent(&server).await, shown_once);
    // The app's answered line and its details stay; the action menu leaves no line (answered
    // `back`, it read as a command, `vendo apps back`, which does not exist).
    let shown = shown_lines(&terminal, (40, 120));
    let first = shown.iter().position(|line| line == "? vendo apps list 0c0d0e0f... (Demo Pixel)").unwrap();
    assert!(!shown.iter().any(|line| line.ends_with("vendo apps back")), "{shown:#?}");
    let again = first + shown[first..].iter().position(|line| line == "? vendo apps list").unwrap();
    let typed = sandbox.run(&["apps", "get", CHOOSE_PIXEL]);
    let details: Vec<String> = shown[first + 1..again].iter().filter(|line| !line.is_empty()).cloned().collect();
    assert_eq!(details, printed_lines(&typed));
    // Enter shows the same app again, as the first time.
    terminal.press("\r");
    terminal.wait_for("Back to the list");
    terminal.wait_for("type to filter]");
    terminal.press("\u{1b}");
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(0), "{rest:?}");
    let listed = format!("GET {V1}/apps?limit=20&offset=0");
    let pixel = format!("GET {V1}/apps/{CHOOSE_PIXEL}");
    assert_eq!(sent(&server).await, [&listed, &pixel, &pixel, &pixel].map(String::as_str));
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_the_list_or_an_items_actions_leave_quietly_and_run_nothing() {
    // As at the group menu (VE-3826): exit 0, nothing run, the title and `<canceled>` where the list
    // or the menu was, and the cursor on the next line. At the list, at an app's actions, and at the
    // list opened again after Back.
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let listed = format!("GET {V1}/apps?limit=20&offset=0");
    let menu = format!("GET {V1}/apps/{MENU_APP}");
    for (step, title, requests) in [
        ("list", "vendo apps list", vec![listed.clone()]),
        ("actions", "vendo apps", vec![listed.clone(), menu.clone()]),
        ("list after back", "vendo apps list", vec![listed.clone(), menu.clone()]),
    ] {
        for (key, name) in [("\u{1b}", "Esc"), ("\u{3}", "Ctrl-C"), ("\u{4}", "Ctrl-D")] {
            let case = format!("{name} at the {step}");
            let before = sent(&server).await.len();
            let mut terminal = if step == "list" {
                let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
                terminal.wait_for("type to filter · 3 apps]");
                terminal
            } else {
                app_menu_of(&sandbox, &["apps", "list"], &[], 0)
            };
            if step == "list after back" {
                terminal.press("back\r");
                terminal.wait_for("type to filter · 3 apps]");
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let rest = plain(&rest).to_lowercase();
            assert!(!rest.contains("error") && !rest.contains("usage"), "{case}: {rest:?}");
            let (shown, cursor) = screen(&terminal.screen, 40, 120);
            let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
            assert_eq!((shown[last].clone(), cursor), (format!("? {title} <canceled>"), (last + 1, 0)), "{case}");
            assert_eq!(sent(&server).await[before..], requests, "{case}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_list_and_an_items_actions_exit_as_ctrl_d_does_when_their_terminal_hangs_up() {
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut outcomes = Vec::new();
    for step in ["list", "actions"] {
        let mut terminal = OnTerminal::start_detached(&sandbox, &["apps", "list"]);
        terminal.wait_for("type to filter · 3 apps]");
        if step == "actions" {
            terminal.press("\r");
            terminal.wait_for("Back to the list");
            terminal.wait_for("type to filter]");
        }
        let (exited, ended) = hang_up(&mut terminal);
        outcomes.push((step, exited, ended));
    }
    assert!(
        outcomes.iter().all(|(_, exited, ended)| *exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "each should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {outcomes:#?}"
    );
    let listed = format!("GET {V1}/apps?limit=20&offset=0");
    assert_eq!(sent(&server).await, [listed.clone(), listed, format!("GET {V1}/apps/{MENU_APP}")]);
}

#[cfg(unix)]
#[tokio::test]
async fn the_profile_and_debug_the_list_ran_with_carry_into_the_action() {
    // `--profile` before or after the command, VENDO_PROFILE, the profile chosen for `--profile` typed
    // with no name (VE-3892), and the group menu (VE-3826): the action goes to the profile the list
    // listed, as typed. `--debug` too.
    let server = apps_stub("acct-beta", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let pause = format!("POST /api/v1/accounts/acct-beta/apps/{MENU_APP}/pause");
    let answered = "? vendo apps pause a1b2c3d4... (Menu Shop)";
    for (args, env) in [
        (&["--profile", "beta", "apps", "list"][..], &[][..]),
        (&["apps", "list", "--profile", "beta"], &[]),
        (&["apps", "list"], &[("VENDO_PROFILE", "beta")]),
        (&["--debug", "apps", "list", "--profile=beta"], &[]),
        (&["apps", "list"], &[("VENDO_PROFILE", "beta"), ("VENDO_DEBUG", "1")]),
    ] {
        let case = format!("{env:?} {}", args.join(" "));
        let mut terminal = app_menu_of(&sandbox, args, env, 0);
        terminal.press("\r");
        terminal.wait_for(&answered[2..]);
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(sent(&server).await.last(), Some(&pause), "{case}");
        let after = after_answer(&terminal, answered);
        let debug = args.contains(&"--debug") || env.contains(&("VENDO_DEBUG", "1"));
        // The screen breaks the debug lines; joined, the request is the pause.
        let joined = after.concat();
        let debugged = joined.contains(&format!(
            "[debug] request method=POST url={}/api/v1/accounts/acct-beta/apps/{MENU_APP}/pause",
            server.uri()
        ));
        assert_eq!(debugged, debug, "{case}: {after:#?}");
        assert_eq!(after.last().map(String::as_str), Some("Done: App a1b2c3d4... paused."), "{case}");
    }
    // Chosen for `--profile` typed with no name, then from the group's menu.
    for (args, keys) in [(&["apps", "list", "--profile"][..], "beta\r"), (&["apps", "--profile"], "beta\r")] {
        let case = args.join(" ");
        let mut terminal = OnTerminal::start(&sandbox, args);
        terminal.wait_for("type to filter]");
        terminal.press(keys);
        terminal.wait_for(&format!("vendo {} beta", args.join(" ")));
        if args[1] == "--profile" {
            terminal.wait_for("Update an app");
            terminal.press("\r");
            terminal.wait_for("vendo apps list");
        }
        terminal.wait_for("type to filter · 3 apps]");
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        terminal.press("\r");
        terminal.wait_for(&answered[2..]);
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(sent(&server).await.last(), Some(&pause), "{case}");
    }
    // For the command alone: the saved active profile stays.
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_list_by_any_column_and_a_cell_with_a_line_break_stays_on_its_row() {
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    for (typed, rows) in [
        ("SHOPIFY", &[BROWSE_ROWS[0], BROWSE_ROWS[2]][..]),
        ("paused", &BROWSE_ROWS[1..2]),
        ("2h ago", &BROWSE_ROWS[1..2]),
        ("d4c3b2a1", &BROWSE_ROWS[2..]),
        ("deleted", &BROWSE_ROWS[2..]),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
        terminal.wait_for("type to filter · 3 apps]");
        terminal.press(typed);
        terminal.wait_for(&format!("vendo apps list {typed}"));
        terminal.wait_for("\u{1b}[?25h");
        assert_eq!(listed_rows(&terminal), marked(rows), "{typed}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{typed}");
    }
    // Line ends and other control characters in a cell show as spaces: the row stays one line, and so
    // does the answered line.
    let mut broken = app_to_choose(MENU_APP, "Two\nLine\tShop", "shopify", &["source"], "active");
    broken["accessStatus"] = json!("needs\r\nattention");
    let server = apps_stub("acct-alpha", vec![broken]).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
    terminal.wait_for("type to filter · 1 app]");
    assert_eq!(
        shown_lines(&terminal, (40, 120)),
        [
            "? vendo apps list",
            "> a1b2c3d4...  Two Line Shop  shopify  source  active  needs  attention  —",
            "[↑↓ to move, enter to select, type to filter · 1 app]",
        ]
    );
    terminal.press("\r");
    terminal.wait_for("Back to the list");
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    assert!(shown_lines(&terminal, (40, 120)).contains(&"? vendo apps list a1b2c3d4... (Two Line Shop)".to_string()));
}

#[cfg(unix)]
#[tokio::test]
async fn an_app_that_cannot_be_read_ends_with_the_error_apps_get_gives() {
    // Deleted since the list loaded (or a 429, or the network): `apps get`'s error, exit 1.
    let server = MockServer::start().await;
    serve(&server, "GET", &format!("{V1}/apps"), 200, page_of(apps_to_browse(), 0, false)).await;
    let missing = json!({ "error": { "code": "NOT_FOUND", "message": "App not found" } });
    Mock::given(path(format!("{V1}/apps/{OLD_APP}")))
        .respond_with(ResponseTemplate::new(404).set_body_json(missing).insert_header("x-request-id", "req_server_1"))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = OnTerminal::start(&sandbox, &["apps", "list"]);
    terminal.wait_for("type to filter · 3 apps]");
    terminal.press("old\r");
    let answered = "? vendo apps list d4c3b2a1... (Old Shop)";
    terminal.wait_for(&answered[2..]);
    let (rest, code) = terminal.finish();
    let typed = sandbox.run(&["apps", "get", OLD_APP]);
    assert_eq!((code, typed.status.code()), (Some(1), Some(1)), "{rest:?}");
    assert_eq!(after_answer(&terminal, answered), printed_lines(&typed));
    assert_eq!(printed_lines(&typed), ["Error: App not found", "Request ID: req_server_1"]);
}

/// The ways `vendo <args>` runs at a terminal where the list does not open, each with what it shows:
/// prompts off, `TERM=dumb`, stderr redirected, a stdin no key can be read from.
#[cfg(unix)]
fn where_no_list_opens(sandbox: &Sandbox, args: &[&str]) -> Vec<(String, Option<i32>, String)> {
    let mut shown = Vec::new();
    for setting in [("CI", "1"), ("VENDO_NO_INPUT", "1"), ("TERM", "dumb")] {
        let (screen, code) = OnTerminal::start_env(sandbox, args, &[setting]).finish();
        shown.push((format!("{setting:?}"), code, plain(&screen)));
    }
    let log = tempfile::NamedTempFile::new().unwrap();
    let (screen, code) =
        OnTerminal::start_with(sandbox, args, (40, 120), "", Some(log.reopen().unwrap().into())).finish();
    assert_eq!(std::fs::read_to_string(log.path()).unwrap(), "", "2>file: nothing on stderr");
    shown.push(("2>file".into(), code, plain(&screen)));
    let pseudo_terminal = pseudo_terminal_sized(40, 120);
    let write_only = {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let name = std::ffi::OsStr::from_bytes(pseudo_terminal.2.as_bytes());
        std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOCTTY).open(name).unwrap()
    };
    let cmd = sandbox.command(args);
    let (screen, code) = OnTerminal::launch_on(pseudo_terminal, cmd, "", Some(write_only.into()), None, true).finish();
    shown.push(("0>write-only".into(), code, plain(&screen)));
    shown
}

#[cfg(unix)]
#[tokio::test]
async fn where_the_list_cannot_open_and_with_json_or_output_apps_list_prints_what_it_printed() {
    // Byte for byte: piped (`output/` records it), with stdout piped from a terminal, and at a terminal
    // with prompts off, on `TERM=dumb`, with stderr redirected or a write-only stdin, where it prints the
    // table a terminal shows (borders and all), as before. `--json` and `--output`, an empty one too,
    // print what they print on a pipe; the empty `--output` the table. Nothing more is sent.
    let server = apps_stub("acct-alpha", apps_to_browse()).await;
    let sandbox = Sandbox::new(&server.uri());
    let piped = sandbox.run(&["apps", "list"]);
    let (reference, code) = OnTerminal::start_env(&sandbox, &["apps", "list"], &[("CI", "1")]).finish();
    assert_eq!(code, Some(0), "{reference:?}");
    let reference = plain(&reference);
    // The table a terminal shows: the piped table's cells, with borders.
    let bordered: Vec<String> = reference.lines().map(str::to_string).collect();
    assert_eq!(table_cells(&bordered), cells(&piped.stdout));
    assert!(reference.starts_with("┌") && reference.ends_with("3 apps\n"), "{reference}");
    for (case, code, screen) in where_no_list_opens(&sandbox, &["apps", "list"]) {
        assert_eq!((code, screen), (Some(0), reference.clone()), "{case}");
    }
    // Keys and the screen on a terminal, stdout piped (`vendo apps list | less`).
    let (_controller, terminal) = pseudo_terminal();
    let terminal = std::fs::File::from(terminal);
    let out =
        sandbox.command(&["apps", "list"]).stdin(terminal.try_clone().unwrap()).stderr(terminal).output().unwrap();
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), text(&piped.stdout)));
    for flags in [&["--json"][..], &["--output", "id"], &["--output", "state"]] {
        let args: Vec<&str> = ["apps", "list"].into_iter().chain(flags.iter().copied()).collect();
        let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
        let piped = sandbox.run(&args);
        assert_eq!((code, plain(&screen)), (Some(0), text(&piped.stdout)), "{flags:?}");
    }
    let (screen, code) = OnTerminal::start(&sandbox, &["apps", "list", "--output", ""]).finish();
    assert_eq!((code, plain(&screen)), (Some(0), reference.clone()), "--output ''");
    let listed = format!("GET {V1}/apps?limit=20&offset=0");
    assert!(sent(&server).await.iter().all(|request| *request == listed), "{:#?}", sent(&server).await);
    assert_eq!(sent(&server).await.len(), 2 + 5 + 1 + 3 + 3 + 1);
}

#[cfg(unix)]
#[tokio::test]
async fn an_empty_list_prints_the_table_and_its_count_at_a_terminal_too() {
    let server = apps_stub("acct-alpha", Vec::new()).await;
    let sandbox = Sandbox::new(&server.uri());
    let (reference, code) = OnTerminal::start_env(&sandbox, &["apps", "list"], &[("CI", "1")]).finish();
    assert_eq!(code, Some(0));
    let (screen, code) = OnTerminal::start(&sandbox, &["apps", "list"]).finish();
    assert_eq!((code, plain(&screen)), (Some(0), plain(&reference)));
    assert!(plain(&screen).ends_with("0 apps\n"), "{screen:?}");
    assert_eq!(text(&sandbox.run(&["apps", "list"]).stdout).lines().last(), Some("0 apps"));
}

// ── VE-3894: sources, destinations and jobs ──────────────────────────────────
// `vendo sources list`, `vendo destinations list` (its hidden `integrations list` and `int list` too) and
// `vendo jobs list` at a terminal, as `vendo apps list` above: the rows their tables show, from the
// same requests (the sources' and destinations' with the active jobs that feed Progress), to choose
// from. Enter shows the item as its group's `get` does, then the actions that apply to it as that
// request returned it: for a source or destination `sync` when active, `refresh-source` for a
// destination with a source app, `pause` when active, `resume` when inactive, `update` and `delete`;
// for a job `tail` and `cancel` while it is queued, pending or running; then `back`.

/// The sources a selectable list shows: one active, whose import [`JOB_RUNNING`] runs, one inactive
/// that failed twice and last synced two hours ago, and one in another state with no app name.
fn sources_to_browse() -> Vec<Value> {
    let source = |id: &str, name: Value, sync_type: &str, state: &str, status: &str| {
        json!({
            "id": id, "appId": MENU_APP, "appName": name, "syncType": sync_type, "state": state,
            "integrationStatus": status, "consecutiveFailures": 0, "lastSyncAt": null, "createdAt": null,
        })
    };
    let mut failing = source(SOURCE_BQ, json!("Analytics BQ"), "bigquery", "inactive", "warning");
    failing["consecutiveFailures"] = json!(2);
    failing["lastSyncAt"] = json!(minutes_ago(130));
    vec![
        source(SOURCE_SHOP, json!("Menu Shop"), "shopify", "active", "healthy"),
        failing,
        source(SOURCE_BARE, Value::Null, "meta_ads", "deleted", "error"),
    ]
}

/// The destinations a selectable list shows: one active, one inactive whose export [`JOB_QUEUED`]
/// waits, last synced two hours ago, and one in another state with no source app.
fn destinations_to_browse() -> Vec<Value> {
    let destination =
        |id: &str, source: Option<(&str, &str)>, name: &str, data_type: &str, state: &str, status: &str| {
            json!({
                "id": id, "sourceAppId": source.map(|(id, _)| id), "sourceAppName": source.map(|(_, name)| name),
                "destinationAppId": CHOOSE_PIXEL, "destinationAppName": name, "dataType": data_type, "state": state,
                "status": status, "lastSyncAt": null, "createdAt": null,
            })
        };
    let waiting = Some((CHOOSE_BQ, "Analytics BQ"));
    let mut paused = destination(DEST_AUDIENCES, waiting, "Demo Pixel", "audiences", "inactive", "paused");
    paused["lastSyncAt"] = json!(minutes_ago(130));
    vec![
        destination(DEST_EVENTS, Some((MENU_APP, "Menu Shop")), "Analytics BQ", "events", "active", "active"),
        paused,
        destination(DEST_ONE_APP, None, "Demo Pixel", "conversions", "deleted", "error"),
    ]
}

/// The jobs a selectable list shows, newest first: [`SOURCE_SHOP`]'s import running for three and a
/// half hours at 45%, an export that failed a day ago after half an hour, and [`DEST_AUDIENCES`]'s
/// queued with no platform and no time yet. A running job's duration changes every minute: each test
/// makes its stub, and so these, just before it reads them.
fn jobs_to_browse() -> Vec<Value> {
    let job = |id: &str, job_type: &str, connector: Value, status: &str, started: Value, finished: Value| {
        json!({
            "id": id, "jobType": job_type, "connectorType": connector, "status": status, "startedAt": started,
            "finishedAt": finished, "createdAt": started, "rowsProcessed": null, "rowsWritten": null,
        })
    };
    let mut running = job(JOB_RUNNING, "import", json!("shopify"), "running", json!(minutes_ago(210)), Value::Null);
    running["progressPct"] = json!(45);
    running["sourceId"] = json!(SOURCE_SHOP);
    let (started, finished) = (json!(minutes_ago(26 * 60 + 30)), json!(minutes_ago(26 * 60)));
    let mut failed = job(JOB_FAILED, "export", json!("bigquery"), "failed", started, finished);
    failed["errorMessage"] = json!("Rate limited");
    failed["integrationId"] = json!(DEST_EVENTS);
    let mut queued = job(JOB_QUEUED, "data_quality", Value::Null, "queued", Value::Null, Value::Null);
    queued["integrationId"] = json!(DEST_AUDIENCES);
    vec![running, failed, queued]
}

/// [`sources_to_browse`], [`destinations_to_browse`] and [`jobs_to_browse`] in acct-alpha: the lists
/// (`jobs list` any `GET /jobs` not sorted as the active jobs are asked for), the active jobs as the API
/// sends them (the running and queued ones; [`JOB_RUNNING`] for [`SOURCE_SHOP`], [`JOB_QUEUED`] for
/// [`DEST_AUDIENCES`], none for another source or destination), `{ data: <item> }` for every request
/// about one, a refresh-source that finds the data there, and cancelling a job.
async fn pipeline_to_browse_stub() -> MockServer {
    use wiremock::matchers::{method, path_regex, query_param};
    let server = MockServer::start().await;
    let jobs = jobs_to_browse();
    let route = format!("{V1}/jobs");
    let page = |jobs: &[&Value]| page_of(jobs.iter().map(|job| (*job).clone()).collect(), 0, false);
    let active = || Mock::given(method("GET")).and(path(route.clone())).and(query_param("sort", "created_at:desc"));
    for (key, id, job) in [("source_id", SOURCE_SHOP, &jobs[0]), ("integration_id", DEST_AUDIENCES, &jobs[2])] {
        let found = ResponseTemplate::new(200).set_body_json(page(&[job]));
        active().and(query_param(key, id)).respond_with(found).mount(&server).await;
    }
    let none = ResponseTemplate::new(200).set_body_json(page(&[]));
    active().and(query_param("limit", "1")).respond_with(none).mount(&server).await;
    let running = ResponseTemplate::new(200).set_body_json(page(&[&jobs[0], &jobs[2]]));
    active().respond_with(running).mount(&server).await;
    serve(&server, "GET", &route, 200, page_of(jobs.clone(), 0, false)).await;
    for job in &jobs {
        let id = job["id"].as_str().unwrap();
        let cancelled = json!({ "data": { "id": id, "status": "canceled" } });
        serve(&server, "POST", &format!("{route}/{id}/cancel"), 200, cancelled).await;
        serve(&server, "GET", &format!("{route}/{id}"), 200, json!({ "data": job })).await;
    }
    for (list, items) in [("sources", sources_to_browse()), ("connections", destinations_to_browse())] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(items.clone(), 0, false)).await;
        for item in items {
            let id = item["id"].as_str().unwrap();
            // Before the catch-all below: wiremock answers with the first match mounted.
            let refresh = format!("{V1}/{list}/{id}/refresh-source");
            serve(&server, "POST", &refresh, 200, json!({ "data": { "status": "ready" } })).await;
            Mock::given(path_regex(format!("^{V1}/{list}/{id}(/.*)?$")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": item })))
                .mount(&server)
                .await;
        }
    }
    server
}

/// A list command of VE-3894's second group as its selectable list shows [`pipeline_to_browse_stub`]'s
/// items.
#[derive(Clone, Copy, Debug)]
struct Browsed {
    /// The group as the tree names it.
    group: &'static str,
    /// The items, in the list's order.
    ids: [&'static str; 3],
    /// The items as the list shows them: the table's cells as plain text, padded per column.
    rows: [&'static str; 3],
    /// What an answered line names each item by after its short ID; nothing for a job.
    names: [&'static str; 3],
    /// The table's footer, which follows the list's hint.
    footer: &'static str,
    /// The actions each item offers before `back`.
    actions: [&'static [&'static str]; 3],
}

const SOURCES_BROWSED: Browsed = Browsed {
    group: "sources",
    ids: [SOURCE_SHOP, SOURCE_BQ, SOURCE_BARE],
    rows: [
        "5e6f7a8b...  Menu Shop     shopify   healthy  45%  0  —",
        "6f7a8b9c...  Analytics BQ  bigquery  warning  —    2  2h ago",
        "7a8b9c0d...  —             meta_ads  error    —    0  —",
    ],
    names: ["Menu Shop", "Analytics BQ", ""],
    footer: "3 sources",
    actions: [&["sync", "pause", "update", "delete"], &["resume", "update", "delete"], &["update", "delete"]],
};

const DESTINATIONS_BROWSED: Browsed = Browsed {
    group: "destinations",
    ids: [DEST_EVENTS, DEST_AUDIENCES, DEST_ONE_APP],
    rows: [
        "8b9c0d1e...  Menu Shop     Analytics BQ  events       active  —       —",
        "9c0d1e2f...  Analytics BQ  Demo Pixel    audiences    paused  queued  2h ago",
        "0d1e2f3a...  —             Demo Pixel    conversions  error   —       —",
    ],
    names: ["Menu Shop → Analytics BQ", "Analytics BQ → Demo Pixel", "— → Demo Pixel"],
    footer: "3 destinations",
    actions: [
        &["sync", "refresh-source", "pause", "update", "delete"],
        &["refresh-source", "resume", "update", "delete"],
        &["update", "delete"],
    ],
};

const JOBS_BROWSED: Browsed = Browsed {
    group: "jobs",
    ids: [JOB_RUNNING, JOB_FAILED, JOB_QUEUED],
    rows: [
        "1f2e3d4c...  import        shopify   running  45%     3h ago  3h 30m",
        "2e3d4c5b...  export        bigquery  failed   —       1d ago  30m",
        "3d4c5b6a...  data_quality  —         queued   queued  —       —",
    ],
    names: ["", "", ""],
    footer: "3 jobs",
    actions: [&["tail", "cancel"], &[], &["tail", "cancel"]],
};

const PIPELINE_BROWSED: [Browsed; 3] = [SOURCES_BROWSED, DESTINATIONS_BROWSED, JOBS_BROWSED];

impl Browsed {
    /// The open list's title.
    fn title(self) -> String {
        format!("? vendo {} list", self.group)
    }

    /// The end of the list's hint: the table's footer.
    fn hint_end(self) -> String {
        format!("type to filter · {}]", self.footer)
    }

    /// What the open list shows with the cursor on the row at `at`.
    fn list(self, at: usize) -> Vec<String> {
        let rows = self.rows.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == at { '>' } else { ' ' }));
        let hint = format!("[↑↓ to move, enter to select, {}", self.hint_end());
        [vec![self.title()], rows.collect(), vec![hint]].concat()
    }

    /// How an answered line names the item at `at`: its short ID, then its name in brackets.
    fn named(self, at: usize) -> String {
        let short = &self.rows[at][..11];
        if self.names[at].is_empty() { short.to_string() } else { format!("{short} ({})", self.names[at]) }
    }

    /// The requests the table sends, and `get` too: the sources' and destinations' with their active jobs.
    fn reads(self) -> usize {
        if self.group == "jobs" { 1 } else { 2 }
    }

    /// The items as the stub lists them, and the route it lists them at.
    fn items(self) -> (Vec<Value>, &'static str) {
        match self.group {
            "sources" => (sources_to_browse(), "sources"),
            "destinations" => (destinations_to_browse(), "connections"),
            _ => (jobs_to_browse(), "jobs"),
        }
    }

    /// The action menu of the item at `at`: its actions, then `back`, each described as
    /// `vendo <group> --help` lists it.
    fn menu(self, sandbox: &Sandbox, at: usize) -> Vec<String> {
        let help = command_rows(sandbox, &[self.group]);
        let about = |action: &str| match action {
            "back" => "Back to the list".to_string(),
            _ => help
                .iter()
                .find_map(|row| row.strip_prefix(action).filter(|about| about.starts_with(' ')))
                .map(|about| about.trim().to_string())
                .unwrap_or_else(|| panic!("{action}: {help:#?}")),
        };
        let names: Vec<&str> = self.actions[at].iter().copied().chain(["back"]).collect();
        let width = names.iter().map(|name| name.len()).max().unwrap();
        let rows: Vec<String> = names.iter().map(|name| format!("{name:<width$}  {}", about(name))).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        [vec![format!("? vendo {}", self.group)], marked(&rows), vec![HINT.to_string()]].concat()
    }

    /// Opens `vendo <args>`, this list, and waits for it.
    fn open(self, sandbox: &Sandbox, args: &[&str]) -> OnTerminal {
        let mut terminal = OnTerminal::start(sandbox, args);
        terminal.wait_for(&self.hint_end());
        terminal
    }

    /// Opens `vendo <args>`, chooses the row at `at` and waits for its action menu.
    fn menu_of(self, sandbox: &Sandbox, args: &[&str], at: usize) -> OnTerminal {
        let mut terminal = self.open(sandbox, args);
        if at > 0 {
            terminal.press(&"\u{1b}[B".repeat(at));
            terminal.wait_for(&format!("> {}", self.rows[at]));
        }
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        terminal
    }
}

/// `requests` in order, for requests sent together (a list and its active jobs, an item and its
/// active job), which reach the stub in either order.
fn in_order(mut requests: Vec<String>) -> Vec<String> {
    requests.sort();
    requests
}

/// `lines` without what changes from one run to the next: the window of a refresh-source, which ends
/// now, as it prints it and in the body of its request.
fn without_window(lines: Vec<String>) -> Vec<String> {
    let line = |line: String| match line.split_once("/refresh-source ") {
        Some((request, _)) => format!("{request}/refresh-source <window>"),
        None if line.starts_with("Window: ") => "Window: <window>".to_string(),
        None => line,
    };
    lines.into_iter().map(line).collect()
}

#[cfg(unix)]
#[tokio::test]
async fn sources_destinations_and_jobs_list_at_a_terminal_list_the_tables_rows_from_the_same_requests() {
    // Titled with the command as the tree names it (`vendo destinations list` for `int list`), every row
    // the table shows (its cells, plain), the first marked, the table's footer after the hint. The
    // flags go into the requests as typed, short IDs looked up as typed: the same requests as the
    // table's, the sources' and destinations' active jobs (Progress) too, and nothing more.
    let active_jobs = format!("GET {V1}/jobs?status=running%2Cpending%2Cqueued&limit=100&sort=created_at%3Adesc");
    let cases: [(Browsed, &[&str], &[String]); 7] = [
        (SOURCES_BROWSED, &["sources", "list"], &[format!("GET {V1}/sources?limit=20&offset=0"), active_jobs.clone()]),
        (
            SOURCES_BROWSED,
            &["sources", "list", "--state", "active", "--limit", "5"],
            &[format!("GET {V1}/sources?state=active&limit=5&offset=0"), active_jobs.clone()],
        ),
        (
            DESTINATIONS_BROWSED,
            &["destinations", "list"],
            &[format!("GET {V1}/connections?limit=20&offset=0"), active_jobs.clone()],
        ),
        (
            DESTINATIONS_BROWSED,
            &["integrations", "list", "--type", "events"],
            &[format!("GET {V1}/connections?data_type=events&limit=20&offset=0"), active_jobs.clone()],
        ),
        (
            DESTINATIONS_BROWSED,
            &["int", "list"],
            &[format!("GET {V1}/connections?limit=20&offset=0"), active_jobs.clone()],
        ),
        (JOBS_BROWSED, &["jobs", "list"], &[format!("GET {V1}/jobs?limit=20&offset=0")]),
        (
            JOBS_BROWSED,
            &["jobs", "list", "--source", "5e6f7a8b", "--status", "running"],
            &[
                format!("GET {V1}/sources?limit=100&offset=0"),
                format!("GET {V1}/jobs?status=running&source_id={SOURCE_SHOP}&limit=20&offset=0"),
            ],
        ),
    ];
    for (browsed, args, requests) in cases {
        let case = args.join(" ");
        let server = pipeline_to_browse_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let piped = sandbox.run(args);
        let piped_sent = sent(&server).await;
        assert_eq!(in_order(piped_sent.clone()), in_order(requests.to_vec()), "{case}");
        let mut terminal = browsed.open(&sandbox, args);
        let shown = shown_lines(&terminal, (40, 120));
        assert_eq!(shown, browsed.list(0), "{case}");
        let table = table_cells(&printed_lines(&piped));
        let listed: Vec<String> = shown[1..shown.len() - 1].iter().map(|row| row[2..].to_string()).collect();
        assert_eq!(
            table_cells(&listed),
            table[1..table.len() - 1],
            "{case}: the table's rows, without header and footer"
        );
        assert_eq!(table.last().unwrap(), &[browsed.footer], "{case}");
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(in_order(sent(&server).await[piped_sent.len()..].to_vec()), in_order(piped_sent), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn enter_on_a_source_destination_or_job_shows_it_as_get_does_then_the_actions_that_apply_to_it() {
    // A source or destination: sync when active, refresh-source with a source app (destinations),
    // pause when active, resume when inactive, neither in another state, then update and delete. A
    // job: tail and cancel while queued or running, nothing once it failed. Then back, each described
    // as the group's help describes it.
    for browsed in PIPELINE_BROWSED {
        for at in 0..3 {
            let case = format!("{} {}", browsed.group, browsed.ids[at]);
            let server = pipeline_to_browse_stub().await;
            let sandbox = Sandbox::new(&server.uri());
            let mut terminal = browsed.menu_of(&sandbox, &[browsed.group, "list"], at);
            let asked = sent(&server).await;
            // What `vendo <group> get <full id>` sends and prints on a pipe.
            let typed = sandbox.run(&[browsed.group, "get", browsed.ids[at]]);
            let typed_sent = sent(&server).await[asked.len()..].to_vec();
            assert_eq!(asked.len(), browsed.reads() * 2, "{case}: {asked:#?}");
            assert_eq!(in_order(asked[browsed.reads()..].to_vec()), in_order(typed_sent), "{case}");
            let shown = shown_from(&terminal, &format!("{} {}", browsed.title(), browsed.named(at)));
            let menu = browsed.menu(&sandbox, at);
            let details = &shown[1..shown.len() - menu.len()];
            let details: Vec<String> = details.iter().filter(|line| !line.is_empty()).cloned().collect();
            assert_eq!(details, printed_lines(&typed), "{case}");
            assert_eq!(shown[shown.len() - menu.len()..], menu, "{case}");
            terminal.press("\u{1b}");
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            assert_eq!(sent(&server).await.len(), asked.len() + browsed.reads(), "{case}: nothing more sent");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_chosen_source_destination_or_job_action_runs_exactly_as_typed_and_the_cli_ends_with_its_exit_code() {
    /// The list, the words that open it, the row chosen, the keys that choose the action, the action,
    /// what the y/N question is answered (none when it asks none), and the same command typed with the
    /// full ID (none when nothing is sent).
    struct Case(
        Browsed,
        &'static [&'static str],
        usize,
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static [&'static str],
    );
    let cases = [
        // Its import runs: the sync says so, as when typed.
        Case(SOURCES_BROWSED, &["sources", "list"], 0, "\r", "sync", None, &["sources", "sync", SOURCE_SHOP]),
        Case(SOURCES_BROWSED, &["sources", "list"], 0, "pause\r", "pause", None, &["sources", "pause", SOURCE_SHOP]),
        Case(SOURCES_BROWSED, &["sources", "list"], 1, "\r", "resume", None, &["sources", "resume", SOURCE_BQ]),
        // No change to make: it fails after the choice as when typed, exit 1 (VE-3881's Q13).
        Case(SOURCES_BROWSED, &["sources", "list"], 2, "\r", "update", None, &["sources", "update", SOURCE_BARE]),
        // The y/N of a delete (VE-3823) asks as when typed: n sends nothing, y deletes.
        Case(SOURCES_BROWSED, &["sources", "list"], 0, "delete\r", "delete", Some("n\n"), &[]),
        Case(
            SOURCES_BROWSED,
            &["sources", "list"],
            1,
            "delete\r",
            "delete",
            Some("y\n"),
            &["sources", "delete", SOURCE_BQ, "--yes"],
        ),
        // No active job: the sync is triggered.
        Case(
            DESTINATIONS_BROWSED,
            &["destinations", "list"],
            0,
            "\r",
            "sync",
            None,
            &["destinations", "sync", DEST_EVENTS],
        ),
        Case(
            DESTINATIONS_BROWSED,
            &["destinations", "list"],
            1,
            "\r",
            "refresh-source",
            None,
            &["destinations", "refresh-source", DEST_AUDIENCES],
        ),
        // From the hidden aliases: the action runs as the tree names it.
        Case(
            DESTINATIONS_BROWSED,
            &["int", "list"],
            0,
            "pause\r",
            "pause",
            None,
            &["destinations", "pause", DEST_EVENTS],
        ),
        Case(
            DESTINATIONS_BROWSED,
            &["integrations", "list"],
            1,
            "resume\r",
            "resume",
            None,
            &["destinations", "resume", DEST_AUDIENCES],
        ),
        Case(DESTINATIONS_BROWSED, &["destinations", "list"], 2, "delete\r", "delete", Some("n\n"), &[]),
        Case(
            DESTINATIONS_BROWSED,
            &["int", "list"],
            2,
            "delete\r",
            "delete",
            Some("y\n"),
            &["destinations", "delete", DEST_ONE_APP, "--yes"],
        ),
        // A job's cancel asks y/N as when typed.
        Case(JOBS_BROWSED, &["jobs", "list"], 0, "cancel\r", "cancel", Some("n\n"), &[]),
        Case(
            JOBS_BROWSED,
            &["jobs", "list"],
            2,
            "cancel\r",
            "cancel",
            Some("y\n"),
            &["jobs", "cancel", JOB_QUEUED, "--yes"],
        ),
    ];
    for Case(browsed, args, at, keys, action, answer, typed_args) in cases {
        let case = format!("{} {action} {} {answer:?}", args.join(" "), browsed.ids[at]);
        let server = pipeline_to_browse_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = browsed.menu_of(&sandbox, args, at);
        terminal.press(keys);
        let answered = format!("? vendo {} {action} {}", browsed.group, browsed.named(at));
        terminal.wait_for(&answered[2..]);
        if let Some(answer) = answer {
            terminal.wait_for("(y/N) ");
            terminal.press(answer);
        }
        let (rest, code) = terminal.finish();
        let asked = sent(&server).await;
        // The list and the item, then what the typed command sends, and nothing else.
        let typed = (!typed_args.is_empty()).then(|| sandbox.run(typed_args));
        let typed_sent = sent(&server).await[asked.len()..].to_vec();
        let shown = 2 * browsed.reads();
        assert!(asked[..shown].iter().all(|request| request.starts_with("GET ")), "{case}: {asked:#?}");
        assert_eq!(without_window(asked[shown..].to_vec()), without_window(typed_sent), "{case}");
        match typed {
            Some(typed) => {
                assert_eq!(code, typed.status.code(), "{case}: {rest:?}");
                // What it showed after the answer (and the y/N question, answered) is what the typed
                // command printed.
                let mut after = after_answer(&terminal, &answered);
                after.retain(|line| !line.contains("(y/N)"));
                assert_eq!(without_window(after), without_window(printed_lines(&typed)), "{case}");
            }
            None => {
                assert_eq!(code, Some(0), "{case}: {rest:?}");
                assert_eq!(asked.len(), shown, "{case}: {asked:#?}");
                let noun = match browsed.group {
                    "sources" => "Delete source",
                    "destinations" => "Delete destination",
                    _ => "Cancel job",
                };
                let question = format!("{noun} {}? (y/N) n", &browsed.rows[at][..11]);
                assert_eq!(after_answer(&terminal, &answered), [question], "{case}");
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn tail_on_a_running_job_follows_it_exactly_as_jobs_tail_does() {
    // `jobs get`'s request finds it running, offering tail; tail's first poll finds it completed.
    use wiremock::matchers::method;
    let server = pipeline_to_browse_stub().await;
    let mut done = jobs_to_browse()[0].clone();
    done["status"] = json!("completed");
    done["finishedAt"] = json!(minutes_ago(1));
    done["progressPct"] = json!(100);
    (done["rowsProcessed"], done["rowsWritten"]) = (json!(1200), json!(1200));
    // Before the stub's own answer for it.
    let job = || Mock::given(method("GET")).and(path(format!("{V1}/jobs/{JOB_RUNNING}")));
    let running = ResponseTemplate::new(200).set_body_json(json!({ "data": jobs_to_browse()[0] }));
    job().respond_with(running).up_to_n_times(1).with_priority(1).mount(&server).await;
    let done = ResponseTemplate::new(200).set_body_json(json!({ "data": done }));
    job().respond_with(done).with_priority(1).mount(&server).await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = JOBS_BROWSED.menu_of(&sandbox, &["jobs", "list"], 0);
    terminal.press("\r");
    let answered = "? vendo jobs tail 1f2e3d4c...";
    terminal.wait_for(&answered[2..]);
    let (rest, code) = terminal.finish();
    let asked = sent(&server).await;
    let typed = sandbox.run(&["jobs", "tail", JOB_RUNNING]);
    let typed_sent = sent(&server).await[asked.len()..].to_vec();
    assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{rest:?}");
    let item = format!("GET {V1}/jobs/{JOB_RUNNING}");
    assert_eq!(asked, [format!("GET {V1}/jobs?limit=20&offset=0"), item.clone(), item.clone()]);
    assert_eq!(typed_sent, [item]);
    let after = after_answer(&terminal, answered);
    assert_eq!(after, printed_lines(&typed));
    assert_eq!(
        after.last().map(String::as_str),
        Some("Done: Job 1f2e3d4c... completed. 1,200 rows processed, 1,200 written.")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn back_opens_the_source_destination_or_job_list_again_on_the_item_with_no_request() {
    for (browsed, filter) in
        [(SOURCES_BROWSED, "bigquery"), (DESTINATIONS_BROWSED, "audiences"), (JOBS_BROWSED, "export")]
    {
        let server = pipeline_to_browse_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = browsed.open(&sandbox, &[browsed.group, "list"]);
        // Filtered down to the second row, which Enter shows.
        terminal.press(filter);
        terminal.wait_for(&format!("vendo {} list {filter}", browsed.group));
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        let shown_once = sent(&server).await;
        terminal.press("back\r");
        terminal.wait_for(&browsed.hint_end());
        terminal.wait_for("\u{1b}[?25h");
        // The same rows, the filter cleared, the cursor on the item just viewed; nothing sent.
        assert_eq!(shown_from(&terminal, &browsed.title()), browsed.list(1), "{}", browsed.group);
        assert_eq!(sent(&server).await, shown_once, "{}", browsed.group);
        // The action menu leaves no line: answered `back`, it read as a command.
        let back = format!("vendo {} back", browsed.group);
        assert!(!shown_lines(&terminal, (40, 120)).iter().any(|line| line.ends_with(&back)), "{}", browsed.group);
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{}: {rest:?}", browsed.group);
        assert_eq!(sent(&server).await, shown_once, "{}", browsed.group);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_sources_destinations_and_jobs_by_any_column() {
    for (browsed, typed, rows) in [
        (SOURCES_BROWSED, "45%", &[0][..]),
        (SOURCES_BROWSED, "2h ago", &[1]),
        (SOURCES_BROWSED, "META", &[2]),
        (DESTINATIONS_BROWSED, "queued", &[1]),
        (DESTINATIONS_BROWSED, "demo pixel", &[1, 2]),
        (DESTINATIONS_BROWSED, "events", &[0]),
        (JOBS_BROWSED, "3h", &[0]),
        (JOBS_BROWSED, "failed", &[1]),
        (JOBS_BROWSED, "QUEUED", &[2]),
    ] {
        let case = format!("{} {typed}", browsed.group);
        let server = pipeline_to_browse_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = browsed.open(&sandbox, &[browsed.group, "list"]);
        terminal.press(typed);
        terminal.wait_for(&format!("vendo {} list {typed}", browsed.group));
        terminal.wait_for("\u{1b}[?25h");
        let expected: Vec<&str> = rows.iter().map(|at| browsed.rows[*at]).collect();
        assert_eq!(listed_rows(&terminal), marked(&expected), "{case}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_a_source_destination_or_job_list_or_its_actions_leave_quietly() {
    // As at the group menu (VE-3826): exit 0, nothing run, the title and `<canceled>` where the list
    // or the menu was, and the cursor on the next line.
    for browsed in PIPELINE_BROWSED {
        let list = format!("vendo {} list", browsed.group);
        let actions = format!("vendo {}", browsed.group);
        for (step, key, title, reads) in
            [("list", "\u{1b}", &list, 1), ("actions", "\u{3}", &actions, 2), ("list after back", "\u{4}", &list, 2)]
        {
            let case = format!("{} at the {step}", browsed.group);
            let server = pipeline_to_browse_stub().await;
            let sandbox = Sandbox::new(&server.uri());
            let mut terminal = if step == "list" {
                browsed.open(&sandbox, &[browsed.group, "list"])
            } else {
                browsed.menu_of(&sandbox, &[browsed.group, "list"], 0)
            };
            if step == "list after back" {
                terminal.press("back\r");
                terminal.wait_for(&browsed.hint_end());
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let rest = plain(&rest).to_lowercase();
            assert!(!rest.contains("error") && !rest.contains("usage"), "{case}: {rest:?}");
            let (shown, cursor) = screen(&terminal.screen, 40, 120);
            let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
            assert_eq!((shown[last].clone(), cursor), (format!("? {title} <canceled>"), (last + 1, 0)), "{case}");
            let asked = sent(&server).await;
            assert_eq!(asked.len(), browsed.reads() * reads, "{case}: {asked:#?}");
            assert!(asked.iter().all(|request| request.starts_with("GET ")), "{case}: {asked:#?}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_source_destination_or_job_that_cannot_be_read_ends_with_the_error_get_gives() {
    // Deleted since the list loaded (or a 429, or the network): `get`'s error, exit 1.
    for (browsed, message) in [
        (SOURCES_BROWSED, "Source not found"),
        (DESTINATIONS_BROWSED, "Destination not found"),
        (JOBS_BROWSED, "Job not found"),
    ] {
        let server = MockServer::start().await;
        let (items, route) = browsed.items();
        let missing = json!({ "error": { "code": "NOT_FOUND", "message": message } });
        Mock::given(path(format!("{V1}/{route}/{}", browsed.ids[2])))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(missing).insert_header("x-request-id", "req_server_1"),
            )
            .mount(&server)
            .await;
        serve(&server, "GET", &format!("{V1}/{route}"), 200, page_of(items, 0, false)).await;
        serve(&server, "GET", &format!("{V1}/jobs"), 200, page_of(Vec::new(), 0, false)).await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = browsed.open(&sandbox, &[browsed.group, "list"]);
        // No active job here: the Progress column shows none.
        terminal.press("\u{1b}[B\u{1b}[B");
        terminal.wait_for(&format!("> {}", &browsed.rows[2][..11]));
        terminal.press("\r");
        let answered = format!("{} {}", browsed.title(), browsed.named(2));
        terminal.wait_for(&answered[2..]);
        let (rest, code) = terminal.finish();
        let typed = sandbox.run(&[browsed.group, "get", browsed.ids[2]]);
        assert_eq!((code, typed.status.code()), (Some(1), Some(1)), "{}: {rest:?}", browsed.group);
        assert_eq!(after_answer(&terminal, &answered), printed_lines(&typed), "{}", browsed.group);
        assert_eq!(printed_lines(&typed), [format!("Error: {message}"), "Request ID: req_server_1".to_string()]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn where_the_list_cannot_open_and_with_json_or_output_sources_destinations_and_jobs_list_print_what_they_printed()
{
    // Byte for byte, as `vendo apps list` (above): piped (`output/` records it), with stdout piped from
    // a terminal, and at a terminal with prompts off, on `TERM=dumb`, with stderr redirected or a
    // write-only stdin, where it prints the table a terminal shows. `--json` and `--output` print
    // what they print on a pipe; an empty `--output` the table.
    for (args, footer) in [
        (&["sources", "list"][..], "3 sources"),
        (&["destinations", "list"], "3 destinations"),
        (&["int", "list"], "3 destinations"),
        (&["jobs", "list"], "3 jobs"),
    ] {
        let case = args.join(" ");
        let server = pipeline_to_browse_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let piped = sandbox.run(args);
        let (reference, code) = OnTerminal::start_env(&sandbox, args, &[("CI", "1")]).finish();
        assert_eq!(code, Some(0), "{case}: {reference:?}");
        let reference = plain(&reference);
        let bordered: Vec<String> = reference.lines().map(str::to_string).collect();
        assert_eq!(table_cells(&bordered), cells(&piped.stdout), "{case}");
        assert!(reference.starts_with("┌") && reference.ends_with(&format!("{footer}\n")), "{case}: {reference}");
        for (setting, code, screen) in where_no_list_opens(&sandbox, args) {
            assert_eq!((code, screen), (Some(0), reference.clone()), "{case}: {setting}");
        }
        let (_controller, terminal) = pseudo_terminal();
        let terminal = std::fs::File::from(terminal);
        let out = sandbox.command(args).stdin(terminal.try_clone().unwrap()).stderr(terminal).output().unwrap();
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), text(&piped.stdout)), "{case}");
        for flags in [&["--json"][..], &["--output", "id"]] {
            let args: Vec<&str> = args.iter().chain(flags).copied().collect();
            let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
            let piped = sandbox.run(&args);
            assert_eq!((code, plain(&screen)), (Some(0), text(&piped.stdout)), "{case} {flags:?}");
        }
        let args: Vec<&str> = args.iter().chain(&["--output", ""]).copied().collect();
        let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
        assert_eq!((code, plain(&screen)), (Some(0), reference.clone()), "{case} --output ''");
        assert!(sent(&server).await.iter().all(|request| request.starts_with("GET ")), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_empty_source_destination_or_job_list_prints_the_table_and_its_count_at_a_terminal_too() {
    let server = MockServer::start().await;
    for list in ["sources", "connections", "jobs"] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(Vec::new(), 0, false)).await;
    }
    let sandbox = Sandbox::new(&server.uri());
    for (args, footer) in [
        (&["sources", "list"], "0 sources"),
        (&["destinations", "list"], "0 destinations"),
        (&["jobs", "list"], "0 jobs"),
    ] {
        let (reference, code) = OnTerminal::start_env(&sandbox, args, &[("CI", "1")]).finish();
        assert_eq!(code, Some(0), "{args:?}");
        let (screen, code) = OnTerminal::start(&sandbox, args).finish();
        assert_eq!((code, plain(&screen)), (Some(0), plain(&reference)), "{args:?}");
        assert!(plain(&screen).ends_with(&format!("{footer}\n")), "{args:?}: {screen:?}");
        assert_eq!(text(&sandbox.run(args).stdout).lines().last(), Some(footer), "{args:?}");
    }
}

// ── VE-3894: catalog, dictionary, metrics and models ─────────────────────────
// `vendo catalog list`, `vendo dictionary list`, `vendo metrics list` and `vendo models list` at a
// terminal, as `vendo apps list` above: the rows their tables show, from the same request, to choose
// from. Enter shows the item as its group's `get` does, then the actions that apply to it: a draft
// metric `activate`, every metric `update` and `delete`; a platform, a dictionary entry or a model
// nothing (never the hidden `catalog credential-schema`); then `back`. `dictionary search`, which
// prints the same table, prints it as before.

/// The events a selectable list shows: one whose description has a line break, one with no
/// description, one with no name.
fn dictionary_to_browse() -> Vec<Value> {
    let mut checkout = dictionary_item(EVENT_CHECKOUT, "event", json!("Checkout Completed"));
    checkout["description"] = json!("A customer placed an order.\nPaid orders only.");
    let mut page = dictionary_item(EVENT_PAGE, "event", json!("Page Viewed"));
    page["description"] = Value::Null;
    vec![checkout, page, dictionary_item(EVENT_UNNAMED, "event", Value::Null)]
}

/// [`jobs_models_metrics_and_catalog_stub`] (the catalog, [`models_to_choose`], [`metrics_to_choose`],
/// each item for every request about one), with [`dictionary_to_browse`] in acct-alpha for any list
/// of the dictionary and each entry's lookup.
async fn catalog_dictionary_metrics_and_models_stub() -> MockServer {
    use wiremock::matchers::{method, query_param};
    let server = jobs_models_metrics_and_catalog_stub().await;
    let items = dictionary_to_browse();
    for item in &items {
        let id = item["subjectId"].as_str().unwrap();
        let found = json!({ "data": { "subjectId": id, "found": true, "definition": item } });
        Mock::given(method("GET"))
            .and(path(format!("{V1}/dictionary/lookup")))
            .and(query_param("subject_id", id))
            .respond_with(ResponseTemplate::new(200).set_body_json(found))
            .mount(&server)
            .await;
    }
    serve(&server, "GET", &format!("{V1}/dictionary"), 200, page_of(items, 0, false)).await;
    server
}

/// A list command of VE-3894's third group as its selectable list shows
/// [`catalog_dictionary_metrics_and_models_stub`]'s items.
#[derive(Clone, Copy, Debug)]
struct Selectable {
    /// The group as the tree names it.
    group: &'static str,
    /// The items' IDs as `get` takes them, in the list's order.
    ids: &'static [&'static str],
    /// The items as the list shows them: the table's cells as plain text on one line, padded per column.
    rows: &'static [&'static str],
    /// How an answered line names each item.
    answers: &'static [&'static str],
    /// The table's footer, which follows the list's hint.
    footer: &'static str,
    /// The actions each item offers before `back`.
    actions: &'static [&'static [&'static str]],
}

const CATALOG_SELECTABLE: Selectable = Selectable {
    group: "catalog",
    ids: &["bigquery", "shopify"],
    rows: &["bigquery  BigQuery  ads  source  ready", "shopify   Shopify   ads  source  ready"],
    answers: &["bigquery", "shopify"],
    footer: "2 ready · 1 more on request (vendo catalog list --all)",
    actions: &[&[], &[]],
};

/// `vendo catalog list --all`: the platform on request too, and the plain count.
const CATALOG_ALL_SELECTABLE: Selectable = Selectable {
    ids: &["bigquery", "shopify", "hubspot"],
    rows: &[
        "bigquery  BigQuery  ads  source  ready",
        "shopify   Shopify   ads  source  ready",
        "hubspot   HubSpot   ads  source  on request",
    ],
    answers: &["bigquery", "shopify", "hubspot"],
    footer: "3 platforms",
    actions: &[&[], &[], &[]],
    ..CATALOG_SELECTABLE
};

const DICTIONARY_SELECTABLE: Selectable = Selectable {
    group: "dictionary",
    ids: &[EVENT_CHECKOUT, EVENT_PAGE, EVENT_UNNAMED],
    // The line break of the first description is a space: one row, one line.
    rows: &[
        "9f8e7d6c5b4a39281706f5e4d3c2b1a0  Checkout Completed  A customer placed an order. Paid orders only.",
        "0a1b2c3d4e5f60718293a4b5c6d7e8f9  Page Viewed         —",
        "1b2c3d4e5f60718293a4b5c6d7e8f9a0  —                   Synthetic entry",
    ],
    answers: &[
        "9f8e7d6c5b4a39281706f5e4d3c2b1a0 (Checkout Completed)",
        "0a1b2c3d4e5f60718293a4b5c6d7e8f9 (Page Viewed)",
        "1b2c3d4e5f60718293a4b5c6d7e8f9a0",
    ],
    footer: "3 events",
    actions: &[&[], &[], &[]],
};

const METRICS_SELECTABLE: Selectable = Selectable {
    group: "metrics",
    ids: &[METRIC_ROAS, METRIC_REVENUE, METRIC_CTR],
    rows: &[
        "6a798897...  ROAS           multiplier  active  —",
        "798897a6...  Total Revenue  currency    draft   —",
        "8897a6b5...  CTR            percentage  active  —",
    ],
    answers: &["6a798897... (ROAS)", "798897a6... (Total Revenue)", "8897a6b5... (CTR)"],
    footer: "3 metrics",
    actions: &[&["update", "delete"], &["activate", "update", "delete"], &["update", "delete"]],
};

const MODELS_SELECTABLE: Selectable = Selectable {
    group: "models",
    ids: &[MODEL_ORDERS, MODEL_LTV],
    rows: &["4c5b6a79...  orders_clean  sql   yes  —", "5b6a7988...  ltv_forecast  bqml  no   —"],
    answers: &["4c5b6a79... (orders_clean)", "5b6a7988... (ltv_forecast)"],
    footer: "2 models",
    actions: &[&[], &[]],
};

const SELECTABLE: [Selectable; 4] = [CATALOG_SELECTABLE, DICTIONARY_SELECTABLE, METRICS_SELECTABLE, MODELS_SELECTABLE];

impl Selectable {
    /// The open list's title.
    fn title(self) -> String {
        format!("? vendo {} list", self.group)
    }

    /// The end of the list's hint: the table's footer.
    fn hint_end(self) -> String {
        format!("type to filter · {}]", self.footer)
    }

    /// What the open list shows with the cursor on the row at `at`.
    fn list(self, at: usize) -> Vec<String> {
        let rows = self.rows.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == at { '>' } else { ' ' }));
        let hint = format!("[↑↓ to move, enter to select, {}", self.hint_end());
        [vec![self.title()], rows.collect(), vec![hint]].concat()
    }

    /// The request `vendo <group> get <id>` sends for the item at `at`, and Enter too.
    fn item(self, at: usize) -> String {
        let id = self.ids[at];
        match self.group {
            "catalog" => format!("GET {CATALOG}/{id}"),
            "dictionary" => format!("GET {V1}/dictionary/lookup?subject_id={id}"),
            "metrics" => format!("GET /api/metrics/{id}"),
            _ => format!("GET {V1}/models/{id}"),
        }
    }

    /// The action menu of the item at `at`: its actions, then `back`, each described as
    /// `vendo <group> --help` lists it.
    fn menu(self, sandbox: &Sandbox, at: usize) -> Vec<String> {
        let help = command_rows(sandbox, &[self.group]);
        let about = |action: &str| match action {
            "back" => "Back to the list".to_string(),
            _ => help
                .iter()
                .find_map(|row| row.strip_prefix(action).filter(|about| about.starts_with(' ')))
                .map(|about| about.trim().to_string())
                .unwrap_or_else(|| panic!("{action}: {help:#?}")),
        };
        let names: Vec<&str> = self.actions[at].iter().copied().chain(["back"]).collect();
        let width = names.iter().map(|name| name.len()).max().unwrap();
        let rows: Vec<String> = names.iter().map(|name| format!("{name:<width$}  {}", about(name))).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        [vec![format!("? vendo {}", self.group)], marked(&rows), vec![HINT.to_string()]].concat()
    }

    /// Opens `vendo <args>`, this list, and waits for it.
    fn open(self, sandbox: &Sandbox, args: &[&str]) -> OnTerminal {
        let mut terminal = OnTerminal::start(sandbox, args);
        terminal.wait_for(&self.hint_end());
        terminal
    }

    /// Opens `vendo <group> list`, chooses the row at `at` and waits for its action menu.
    fn menu_of(self, sandbox: &Sandbox, at: usize) -> OnTerminal {
        let mut terminal = self.open(sandbox, &[self.group, "list"]);
        if at > 0 {
            terminal.press(&"\u{1b}[B".repeat(at));
            terminal.wait_for(&format!("> {}", self.rows[at]));
        }
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        terminal
    }
}

#[cfg(unix)]
#[tokio::test]
async fn catalog_dictionary_metrics_and_models_list_at_a_terminal_list_the_tables_rows_from_the_same_request() {
    // Titled with the command, every row the table shows (its cells, plain, on one line), the first
    // marked, the table's footer after the hint (the catalog's `2 ready · 1 more on request …`). The
    // flags go into the request as typed: the table's request, and nothing more.
    let cases: [(Selectable, &[&str], String); 8] = [
        (CATALOG_SELECTABLE, &["catalog", "list"], format!("GET {CATALOG}")),
        (CATALOG_ALL_SELECTABLE, &["catalog", "list", "--all"], format!("GET {CATALOG}?include_request_access=true")),
        (DICTIONARY_SELECTABLE, &["dictionary", "list"], format!("GET {V1}/dictionary?type=event&limit=20&offset=0")),
        (
            DICTIONARY_SELECTABLE,
            &["dictionary", "list", "--query", "order", "--limit", "5"],
            format!("GET {V1}/dictionary?type=event&q=order&limit=5&offset=0"),
        ),
        (METRICS_SELECTABLE, &["metrics", "list"], "GET /api/metrics?limit=20&offset=0".to_string()),
        (
            METRICS_SELECTABLE,
            &["metrics", "list", "--status", "draft", "--offset", "3"],
            "GET /api/metrics?status=draft&limit=20&offset=3".to_string(),
        ),
        (MODELS_SELECTABLE, &["models", "list"], format!("GET {V1}/models?limit=20&offset=0")),
        (MODELS_SELECTABLE, &["models", "list", "--valid"], format!("GET {V1}/models?limit=20&offset=0&is_valid=true")),
    ];
    for (selectable, args, request) in cases {
        let case = args.join(" ");
        let server = catalog_dictionary_metrics_and_models_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let piped = sandbox.run(args);
        assert_eq!(sent(&server).await, std::slice::from_ref(&request), "{case}");
        let mut terminal = selectable.open(&sandbox, args);
        let shown = shown_lines(&terminal, (40, 120));
        assert_eq!(shown, selectable.list(0), "{case}");
        let table = table_cells(&printed_lines(&piped));
        assert_eq!(table.last().unwrap(), &[selectable.footer], "{case}");
        if selectable.group != "dictionary" {
            let listed: Vec<String> = shown[1..shown.len() - 1].iter().map(|row| row[2..].to_string()).collect();
            assert_eq!(table_cells(&listed), table[1..table.len() - 1], "{case}: the table's rows");
        } else {
            // The table breaks the description over two lines; the list keeps the row on one.
            let piped = text(&piped.stdout);
            assert!(piped.contains("A customer placed an order.") && piped.contains("Paid orders only."), "{piped}");
            assert!(!piped.contains("order. Paid"), "{piped}");
        }
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(sent(&server).await, [request.clone(), request], "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn enter_on_a_platform_dictionary_entry_metric_or_model_shows_it_as_get_does_then_the_actions_that_apply() {
    // A draft metric: activate, update, delete; any other metric update and delete; a platform, a
    // dictionary entry or a model nothing but back. Each described as the group's help describes it.
    for selectable in SELECTABLE {
        for at in 0..selectable.ids.len() {
            let case = format!("{} {}", selectable.group, selectable.ids[at]);
            let server = catalog_dictionary_metrics_and_models_stub().await;
            let sandbox = Sandbox::new(&server.uri());
            let mut terminal = selectable.menu_of(&sandbox, at);
            let asked = sent(&server).await;
            assert_eq!(asked.len(), 2, "{case}: {asked:#?}");
            assert_eq!(asked[1], selectable.item(at), "{case}");
            // What `vendo <group> get <full id>` sends and prints on a pipe.
            let typed = sandbox.run(&[selectable.group, "get", selectable.ids[at]]);
            assert_eq!(sent(&server).await[2..], [selectable.item(at)], "{case}");
            let shown = shown_from(&terminal, &format!("{} {}", selectable.title(), selectable.answers[at]));
            let menu = selectable.menu(&sandbox, at);
            let details = &shown[1..shown.len() - menu.len()];
            let details: Vec<String> = details.iter().filter(|line| !line.is_empty()).cloned().collect();
            assert_eq!(details, printed_lines(&typed), "{case}");
            assert_eq!(shown[shown.len() - menu.len()..], menu, "{case}");
            assert!(!menu.iter().any(|row| row.contains("credential-schema")), "{case}");
            terminal.press("\u{1b}");
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            assert_eq!(sent(&server).await.len(), 3, "{case}: nothing more sent");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_chosen_metric_action_runs_exactly_as_typed_and_the_cli_ends_with_its_exit_code() {
    /// The row chosen, the keys that choose the action, the action, what the y/N question is answered
    /// (none when it asks none), and the same command typed with the full ID (none when nothing is sent).
    struct Case(usize, &'static str, &'static str, Option<&'static str>, &'static [&'static str]);
    let cases = [
        Case(1, "\r", "activate", None, &["metrics", "activate", METRIC_REVENUE]),
        // No change to make: it fails after the choice as when typed, exit 1 (VE-3881's Q13).
        Case(0, "\r", "update", None, &["metrics", "update", METRIC_ROAS]),
        Case(1, "update\r", "update", None, &["metrics", "update", METRIC_REVENUE]),
        // The y/N of a delete (VE-3823) asks as when typed: n sends nothing, y deletes.
        Case(0, "delete\r", "delete", Some("n\n"), &[]),
        Case(2, "delete\r", "delete", Some("y\n"), &["metrics", "delete", METRIC_CTR, "--yes"]),
    ];
    let selectable = METRICS_SELECTABLE;
    for Case(at, keys, action, answer, typed_args) in cases {
        let case = format!("{action} {} {answer:?}", selectable.ids[at]);
        let server = catalog_dictionary_metrics_and_models_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = selectable.menu_of(&sandbox, at);
        terminal.press(keys);
        let answered = format!("? vendo metrics {action} {}", selectable.answers[at]);
        terminal.wait_for(&answered[2..]);
        if let Some(answer) = answer {
            terminal.wait_for("(y/N) ");
            terminal.press(answer);
        }
        let (rest, code) = terminal.finish();
        let asked = sent(&server).await;
        assert_eq!(asked[..2], ["GET /api/metrics?limit=20&offset=0".to_string(), selectable.item(at)], "{case}");
        if typed_args.is_empty() {
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            assert_eq!(asked.len(), 2, "{case}: {asked:#?}");
            let question = format!("Delete metric {}? This cannot be undone. (y/N) n", &selectable.rows[at][..11]);
            assert_eq!(after_answer(&terminal, &answered), [question, "Cancelled".to_string()], "{case}");
            continue;
        }
        // The list and the item, then what the typed command sends, and nothing else.
        let typed = sandbox.run(typed_args);
        let typed_sent = sent(&server).await[asked.len()..].to_vec();
        assert_eq!(asked[2..], typed_sent, "{case}");
        assert_eq!(code, typed.status.code(), "{case}: {rest:?}");
        let mut after = after_answer(&terminal, &answered);
        after.retain(|line| !line.contains("(y/N)"));
        assert_eq!(after, printed_lines(&typed), "{case}");
    }
    // What those were: activate's PATCH, update's usage of its flags (nothing sent), delete's DELETE.
    let server = catalog_dictionary_metrics_and_models_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let out = sandbox.run(&["metrics", "update", METRIC_ROAS]);
    assert_eq!((out.status.code(), printed_lines(&out)), (Some(1), vec!["Error: No updates provided".to_string()]));
    sandbox.run(&["metrics", "activate", METRIC_REVENUE]);
    sandbox.run(&["metrics", "delete", METRIC_CTR, "--yes"]);
    assert_eq!(
        sent(&server).await,
        [
            format!("PATCH /api/metrics/{METRIC_REVENUE} {{\"status\":\"active\"}}"),
            format!("DELETE /api/metrics/{METRIC_CTR}"),
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn back_opens_the_platform_dictionary_metric_or_model_list_again_on_the_item_with_no_request() {
    for (selectable, filter) in [
        (CATALOG_SELECTABLE, "shopify"),
        (DICTIONARY_SELECTABLE, "page viewed"),
        (METRICS_SELECTABLE, "revenue"),
        (MODELS_SELECTABLE, "bqml"),
    ] {
        let server = catalog_dictionary_metrics_and_models_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = selectable.open(&sandbox, &[selectable.group, "list"]);
        // Filtered down to the second row, which Enter shows.
        terminal.press(filter);
        terminal.wait_for(&format!("vendo {} list {filter}", selectable.group));
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        let shown_once = sent(&server).await;
        terminal.press("back\r");
        terminal.wait_for(&selectable.hint_end());
        terminal.wait_for("\u{1b}[?25h");
        // The same rows, the filter cleared, the cursor on the item just viewed; nothing sent.
        assert_eq!(shown_from(&terminal, &selectable.title()), selectable.list(1), "{}", selectable.group);
        assert_eq!(sent(&server).await, shown_once, "{}", selectable.group);
        // The action menu leaves no line: answered `back`, it read as a command.
        let back = format!("vendo {} back", selectable.group);
        assert!(!shown_lines(&terminal, (40, 120)).iter().any(|line| line.ends_with(&back)), "{}", selectable.group);
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{}: {rest:?}", selectable.group);
        assert_eq!(sent(&server).await, shown_once, "{}", selectable.group);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_platforms_dictionary_entries_metrics_and_models_by_any_column() {
    for (selectable, args, typed, rows) in [
        (CATALOG_ALL_SELECTABLE, &["catalog", "list", "--all"][..], "on request", &[2][..]),
        (CATALOG_SELECTABLE, &["catalog", "list"], "SHOP", &[1]),
        // The description's second line, on the row's one line.
        (DICTIONARY_SELECTABLE, &["dictionary", "list"], "paid orders", &[0]),
        (DICTIONARY_SELECTABLE, &["dictionary", "list"], "synthetic", &[2]),
        (METRICS_SELECTABLE, &["metrics", "list"], "draft", &[1]),
        (METRICS_SELECTABLE, &["metrics", "list"], "percentage", &[2]),
        (MODELS_SELECTABLE, &["models", "list"], "yes", &[0]),
    ] {
        let case = format!("{} {typed}", args.join(" "));
        let server = catalog_dictionary_metrics_and_models_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = selectable.open(&sandbox, args);
        terminal.press(typed);
        terminal.wait_for(&format!("vendo {} list {typed}", selectable.group));
        terminal.wait_for("\u{1b}[?25h");
        let expected: Vec<&str> = rows.iter().map(|at| selectable.rows[*at]).collect();
        assert_eq!(listed_rows(&terminal), marked(&expected), "{case}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_a_platform_dictionary_metric_or_model_list_or_its_actions_leave_quietly() {
    // As at the group menu (VE-3826): exit 0, nothing run, the title and `<canceled>` where the list or
    // the menu was, and the cursor on the next line.
    for selectable in SELECTABLE {
        let list = format!("vendo {} list", selectable.group);
        let actions = format!("vendo {}", selectable.group);
        for (step, key, title, reads) in
            [("list", "\u{1b}", &list, 1), ("actions", "\u{3}", &actions, 2), ("list after back", "\u{4}", &list, 2)]
        {
            let case = format!("{} at the {step}", selectable.group);
            let server = catalog_dictionary_metrics_and_models_stub().await;
            let sandbox = Sandbox::new(&server.uri());
            let mut terminal = if step == "list" {
                selectable.open(&sandbox, &[selectable.group, "list"])
            } else {
                selectable.menu_of(&sandbox, 0)
            };
            if step == "list after back" {
                terminal.press("back\r");
                terminal.wait_for(&selectable.hint_end());
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let rest = plain(&rest).to_lowercase();
            assert!(!rest.contains("error") && !rest.contains("usage"), "{case}: {rest:?}");
            let (shown, cursor) = screen(&terminal.screen, 40, 120);
            let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
            assert_eq!((shown[last].clone(), cursor), (format!("? {title} <canceled>"), (last + 1, 0)), "{case}");
            let asked = sent(&server).await;
            assert_eq!(asked.len(), reads, "{case}: {asked:#?}");
            assert!(asked.iter().all(|request| request.starts_with("GET ")), "{case}: {asked:#?}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_platform_dictionary_entry_metric_or_model_that_cannot_be_read_ends_with_the_error_get_gives() {
    // Deleted since the list loaded (or a 429, or the network): `get`'s error, exit 1.
    for selectable in SELECTABLE {
        let last = selectable.ids.len() - 1;
        let server = MockServer::start().await;
        let request = selectable.item(last);
        let route = request.trim_start_matches("GET ").split('?').next().unwrap().to_string();
        let missing = match selectable.group {
            "metrics" => json!({ "error": "Metric not found" }),
            _ => json!({ "error": { "code": "NOT_FOUND", "message": "Not found" } }),
        };
        Mock::given(path(route))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(missing).insert_header("x-request-id", "req_server_1"),
            )
            .mount(&server)
            .await;
        let stub = catalog_dictionary_metrics_and_models_stub().await;
        // The list as the full stub sends it.
        let listed = Sandbox::new(&stub.uri()).run(&[selectable.group, "list", "--json"]);
        let body: Value = serde_json::from_slice(&listed.stdout).unwrap();
        let body = match selectable.group {
            // `metrics list --json` prints the CLI's envelope; the route sends `metrics` and `total`.
            "metrics" => json!({ "metrics": body["data"], "total": body["meta"]["pagination"]["total"] }),
            _ => body,
        };
        let list = match selectable.group {
            "catalog" => CATALOG.to_string(),
            "dictionary" => format!("{V1}/dictionary"),
            "metrics" => "/api/metrics".to_string(),
            _ => format!("{V1}/models"),
        };
        serve(&server, "GET", &list, 200, body).await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = selectable.open(&sandbox, &[selectable.group, "list"]);
        terminal.press(&"\u{1b}[B".repeat(last));
        terminal.wait_for(&format!("> {}", selectable.rows[last]));
        terminal.press("\r");
        let answered = format!("{} {}", selectable.title(), selectable.answers[last]);
        terminal.wait_for(&answered[2..]);
        let (rest, code) = terminal.finish();
        let typed = sandbox.run(&[selectable.group, "get", selectable.ids[last]]);
        assert_eq!((code, typed.status.code()), (Some(1), Some(1)), "{}: {rest:?}", selectable.group);
        assert_eq!(after_answer(&terminal, &answered), printed_lines(&typed), "{}", selectable.group);
        assert!(printed_lines(&typed)[0].starts_with("Error: "), "{:?}", printed_lines(&typed));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn where_the_list_cannot_open_and_with_json_or_output_catalog_dictionary_metrics_and_models_list_print_what_they_printed()
 {
    // Byte for byte, as `vendo apps list` (above): piped (`output/` records it), with stdout piped from
    // a terminal, and at a terminal with prompts off, on `TERM=dumb`, with stderr redirected or a
    // write-only stdin, where it prints the table a terminal shows. `--json` and `--output` print what
    // they print on a pipe; an empty `--output` the table. `dictionary search` prints its table at a
    // terminal too.
    for (args, footer, field) in [
        (&["catalog", "list"][..], "2 ready · 1 more on request (vendo catalog list --all)", "appType"),
        (&["catalog", "list", "--all"], "3 platforms", "appType"),
        (&["dictionary", "list"], "3 events", "subjectId"),
        (&["dictionary", "search", "order"], "3 events", "subjectId"),
        (&["metrics", "list"], "3 metrics", "id"),
        (&["models", "list"], "2 models", "id"),
    ] {
        let case = args.join(" ");
        let server = catalog_dictionary_metrics_and_models_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let piped = sandbox.run(args);
        let (reference, code) = OnTerminal::start_env(&sandbox, args, &[("CI", "1")]).finish();
        assert_eq!(code, Some(0), "{case}: {reference:?}");
        let reference = plain(&reference);
        let bordered: Vec<String> = reference.lines().map(str::to_string).collect();
        assert_eq!(table_cells(&bordered), cells(&piped.stdout), "{case}");
        assert!(reference.starts_with("┌") && reference.ends_with(&format!("{footer}\n")), "{case}: {reference}");
        for (setting, code, screen) in where_no_list_opens(&sandbox, args) {
            assert_eq!((code, screen), (Some(0), reference.clone()), "{case}: {setting}");
        }
        if args[1] == "search" {
            let (screen, code) = OnTerminal::start(&sandbox, args).finish();
            assert_eq!((code, plain(&screen)), (Some(0), reference.clone()), "{case}: at a terminal");
        }
        let (_controller, terminal) = pseudo_terminal();
        let terminal = std::fs::File::from(terminal);
        let out = sandbox.command(args).stdin(terminal.try_clone().unwrap()).stderr(terminal).output().unwrap();
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), text(&piped.stdout)), "{case}");
        for flags in [&["--json"][..], &["--output", field]] {
            let args: Vec<&str> = args.iter().chain(flags).copied().collect();
            let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
            let piped = sandbox.run(&args);
            assert_eq!((code, plain(&screen)), (Some(0), text(&piped.stdout)), "{case} {flags:?}");
        }
        let args: Vec<&str> = args.iter().chain(&["--output", ""]).copied().collect();
        let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
        assert_eq!((code, plain(&screen)), (Some(0), reference.clone()), "{case} --output ''");
        assert!(sent(&server).await.iter().all(|request| request.starts_with("GET ")), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_empty_platform_dictionary_metric_or_model_list_prints_the_table_and_its_count_at_a_terminal_too() {
    let server = MockServer::start().await;
    let none = json!({ "data": [], "meta": { "total": 0, "selfServeTotal": 0, "requestAccessTotal": 0 } });
    serve(&server, "GET", CATALOG, 200, none).await;
    for list in ["dictionary", "models"] {
        serve(&server, "GET", &format!("{V1}/{list}"), 200, page_of(Vec::new(), 0, false)).await;
    }
    serve(&server, "GET", "/api/metrics", 200, json!({ "metrics": [], "total": 0 })).await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, footer) in [
        (&["catalog", "list"], "0 ready"),
        (&["dictionary", "list"], "0 events"),
        (&["metrics", "list"], "0 metrics"),
        (&["models", "list"], "0 models"),
    ] {
        let (reference, code) = OnTerminal::start_env(&sandbox, args, &[("CI", "1")]).finish();
        assert_eq!(code, Some(0), "{args:?}");
        let (screen, code) = OnTerminal::start(&sandbox, args).finish();
        assert_eq!((code, plain(&screen)), (Some(0), plain(&reference)), "{args:?}");
        assert!(plain(&screen).ends_with(&format!("{footer}\n")), "{args:?}: {screen:?}");
        assert_eq!(text(&sandbox.run(args).stdout).lines().last(), Some(footer), "{args:?}");
    }
}

// ── VE-3894: measurement lists and profile list ──────────────────────────────
// `vendo measurement methodologies list`, `vendo measurement ltv list`, `vendo measurement signals list`
// and `vendo profile list` (its old name `config list` too) at a terminal, as `vendo apps list` above:
// the rows they print, to choose from. Enter on a methodology shows it as `methodologies get` does,
// from the list's row (nothing sent); on a cohort as `ltv cohort` does with the list's granularity and
// segment; on a signal or a profile nothing, as they have no `get`. Then the actions that apply: the
// `click_path` signal `click-path`, a profile `switch` unless it is the saved active profile; anything
// else nothing; then `back`.

/// `vendo profile list`'s lines as its selectable list shows them, for `profiles` named as
/// [`Sandbox::new`] names them (`acct-<name>`) on `base_url`, `*` on `active`: the marker and name,
/// the account ID and the base URL, padded per column.
fn profile_list_rows(base_url: &str, profiles: &[&str], active: &str) -> Vec<String> {
    let first = |name: &str| if name == active { format!("* {name} (active)") } else { format!("  {name}") };
    let width = profiles.iter().map(|name| first(name).len()).max().unwrap();
    let account = profiles.iter().map(|name| name.len() + 5).max().unwrap();
    let row = |name: &&str| format!("{:<width$}  {:<account$}  {base_url}", first(name), format!("acct-{name}"));
    profiles.iter().map(row).collect()
}

/// The signals as `GET /api/measurement/signals` sends them: click-path live, MMM a stub, survey live.
fn signals_to_browse() -> Value {
    json!({ "data": { "signals": [
        { "id": "click_path", "state": "live", "availability": { "available": true } },
        { "id": "mmm", "state": "stub", "availability": { "available": false, "reason": "Not enough spend history" } },
        { "id": "survey", "state": "live", "availability": { "available": true } },
    ] } })
}

/// [`measurement_and_dictionary_stub`] (the methodologies, the monthly and weekly cohorts and each
/// cohort's detail), with [`signals_to_browse`] and the click-path status.
async fn measurement_lists_stub() -> MockServer {
    let server = measurement_and_dictionary_stub().await;
    serve(&server, "GET", "/api/measurement/signals", 200, signals_to_browse()).await;
    let status = json!({ "status": {
        "enabled": true, "lastComputedAt": null, "sampleEstimates": [{ "tier_label": "a" }],
        "readiness": { "available": true, "readiness": [{ "key": "clicks", "label": "Has click data", "ok": true }] },
    } });
    serve(&server, "GET", "/api/measurement/signals/click-path", 200, status).await;
    server
}

/// A measurement list as its selectable list shows [`measurement_lists_stub`]'s items.
#[derive(Clone, Copy, Debug)]
struct Measured {
    /// The list command.
    args: &'static [&'static str],
    /// The group as the tree names it.
    group: &'static str,
    /// The request the list sends, at a terminal as on a pipe.
    listed: &'static str,
    /// The items' IDs as their `get` takes them (a cohort's period), in the list's order.
    ids: &'static [&'static str],
    /// The items as the list shows them: the table's cells as plain text, padded per column.
    rows: &'static [&'static str],
    /// How an answered line names each item.
    answers: &'static [&'static str],
    /// The table's footer, which follows the list's hint.
    footer: &'static str,
    /// The actions each item offers before `back`.
    actions: &'static [&'static [&'static str]],
}

const METHODOLOGIES_MEASURED: Measured = Measured {
    args: &["measurement", "methodologies", "list"],
    group: "measurement methodologies",
    listed: METHODOLOGIES_LISTED,
    ids: &[METHODOLOGY_BLENDED, METHODOLOGY_LAST_CLICK, METHODOLOGY_MIX],
    rows: &[
        "0d0e0f10...  Blended     linear          account  1  —",
        "1e1f2021...  Last Click  last_click      system   1  —",
        "2f303132...  Media Mix   position_based  system   1  —",
    ],
    answers: &["0d0e0f10... (Blended)", "1e1f2021... (Last Click)", "2f303132... (Media Mix)"],
    footer: "3 methodologys",
    actions: &[&[], &[], &[]],
};

const COHORTS_MEASURED: Measured = Measured {
    args: &["measurement", "ltv", "list"],
    group: "measurement ltv",
    listed: "GET /api/measurement/ltv?granularity=monthly&segment_key=all&limit=50",
    ids: &["2026-09-01", "2026-08-01", "2026-07-01"],
    // Every column the table shows, the money ones too.
    rows: &[
        "2026-09-01  all  1,204  $12.50  —  —  —  —",
        "2026-08-01  all  987    $12.50  —  —  —  —",
        "2026-07-01  all  15     $12.50  —  —  —  —",
    ],
    answers: &["2026-09-01", "2026-08-01", "2026-07-01"],
    footer: "3 cohorts",
    actions: &[&[], &[], &[]],
};

/// `vendo measurement ltv list --granularity weekly --segment channel:meta`.
const WEEKLY_MEASURED: Measured = Measured {
    args: &["measurement", "ltv", "list", "--granularity", "weekly", "--segment", "channel:meta"],
    listed: "GET /api/measurement/ltv?granularity=weekly&segment_key=channel%3Ameta&limit=50",
    ids: &["2026-09-28", "2026-09-21"],
    rows: &["2026-09-28  channel:meta  88  $12.50  —  —  —  —", "2026-09-21  channel:meta  90  $12.50  —  —  —  —"],
    answers: &["2026-09-28", "2026-09-21"],
    footer: "2 cohorts",
    actions: &[&[], &[]],
    ..COHORTS_MEASURED
};

const SIGNALS_MEASURED: Measured = Measured {
    args: &["measurement", "signals", "list"],
    group: "measurement signals",
    listed: "GET /api/measurement/signals",
    ids: &["click_path", "mmm", "survey"],
    rows: &["click_path  live  yes  —", "mmm         stub  no   Not enough spend history", "survey      live  yes  —"],
    answers: &["click_path", "mmm", "survey"],
    footer: "3 signals",
    actions: &[&["click-path"], &[], &[]],
};

const MEASURED: [Measured; 4] = [METHODOLOGIES_MEASURED, COHORTS_MEASURED, WEEKLY_MEASURED, SIGNALS_MEASURED];

impl Measured {
    /// The open list's title.
    fn title(self) -> String {
        format!("? vendo {} list", self.group)
    }

    /// The end of the list's hint: the table's footer.
    fn hint_end(self) -> String {
        format!("type to filter · {}]", self.footer)
    }

    /// What the open list shows with the cursor on the row at `at`.
    fn list(self, at: usize) -> Vec<String> {
        let rows = self.rows.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == at { '>' } else { ' ' }));
        let hint = format!("[↑↓ to move, enter to select, {}", self.hint_end());
        [vec![self.title()], rows.collect(), vec![hint]].concat()
    }

    /// The requests Enter sends for the item at `at`: a cohort's, with the list's granularity and
    /// segment; nothing for a methodology (the list's row) or a signal.
    fn item(self, at: usize) -> Vec<String> {
        match self.group {
            "measurement ltv" => {
                let query = self.listed.split_once('?').unwrap().1.trim_end_matches("&limit=50");
                vec![format!("GET /api/measurement/ltv/cohort/{}?{query}", self.ids[at])]
            }
            _ => Vec::new(),
        }
    }

    /// The command typed on a pipe that prints what Enter shows of the item at `at`; none for a signal.
    fn typed(self, at: usize) -> Option<Vec<&'static str>> {
        match self.group {
            "measurement methodologies" => Some(vec!["measurement", "methodologies", "get", self.ids[at]]),
            "measurement ltv" => Some([&["measurement", "ltv", "cohort", self.ids[at]][..], &self.args[3..]].concat()),
            _ => None,
        }
    }

    /// The action menu of the item at `at`: its actions, then `back`, each described as the group's help
    /// lists it.
    fn menu(self, sandbox: &Sandbox, at: usize) -> Vec<String> {
        let words: Vec<&str> = self.group.split(' ').collect();
        let help = command_rows(sandbox, &words);
        let about = |action: &str| match action {
            "back" => "Back to the list".to_string(),
            _ => help
                .iter()
                .find_map(|row| row.strip_prefix(action).filter(|about| about.starts_with(' ')))
                .map(|about| about.trim().to_string())
                .unwrap_or_else(|| panic!("{action}: {help:#?}")),
        };
        let names: Vec<&str> = self.actions[at].iter().copied().chain(["back"]).collect();
        let width = names.iter().map(|name| name.len()).max().unwrap();
        let rows: Vec<String> = names.iter().map(|name| format!("{name:<width$}  {}", about(name))).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        [vec![format!("? vendo {}", self.group)], marked(&rows), vec![HINT.to_string()]].concat()
    }

    /// Opens `vendo <args>`, this list, and waits for it.
    fn open(self, sandbox: &Sandbox) -> OnTerminal {
        let mut terminal = OnTerminal::start(sandbox, self.args);
        terminal.wait_for(&self.hint_end());
        terminal
    }

    /// Opens the list, chooses the row at `at` and waits for its action menu.
    fn menu_of(self, sandbox: &Sandbox, at: usize) -> OnTerminal {
        let mut terminal = self.open(sandbox);
        if at > 0 {
            terminal.press(&"\u{1b}[B".repeat(at));
            terminal.wait_for(&format!("> {}", self.rows[at]));
        }
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        terminal
    }
}

#[cfg(unix)]
#[tokio::test]
async fn measurement_lists_at_a_terminal_list_the_tables_rows_from_the_same_request() {
    // Titled with the command, every row the table shows (its cells, plain, the LTV money columns
    // too), the first marked, the table's footer after the hint. The flags go into the request as typed:
    // the table's request, and nothing more.
    let no_system = Measured {
        args: &["measurement", "methodologies", "list", "--no-system"],
        listed: "GET /api/measurement/methodologies?include_system=false",
        ..METHODOLOGIES_MEASURED
    };
    for measured in MEASURED.into_iter().chain([no_system]) {
        let case = measured.args.join(" ");
        let server = measurement_lists_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let piped = sandbox.run(measured.args);
        assert_eq!(sent(&server).await, [measured.listed], "{case}");
        let mut terminal = measured.open(&sandbox);
        let shown = shown_lines(&terminal, (40, 120));
        assert_eq!(shown, measured.list(0), "{case}");
        let table = table_cells(&printed_lines(&piped));
        assert_eq!(table.last().unwrap(), &[measured.footer], "{case}");
        let listed: Vec<String> = shown[1..shown.len() - 1].iter().map(|row| row[2..].to_string()).collect();
        assert_eq!(table_cells(&listed), table[1..table.len() - 1], "{case}: the table's rows");
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(sent(&server).await, [measured.listed, measured.listed], "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn enter_on_a_methodology_cohort_or_signal_shows_it_as_its_get_does_then_the_actions_that_apply() {
    // A methodology as `methodologies get` shows it, from the list's row: nothing sent. A cohort as `ltv
    // cohort` shows it, with the list's granularity and segment. A signal nothing (no `get`). Then the
    // `click_path` signal `click-path`; anything else nothing but back.
    for measured in MEASURED {
        for at in 0..measured.ids.len() {
            let case = format!("{} {}", measured.args.join(" "), measured.ids[at]);
            let server = measurement_lists_stub().await;
            let sandbox = Sandbox::new(&server.uri());
            let mut terminal = measured.menu_of(&sandbox, at);
            let asked = sent(&server).await;
            assert_eq!(asked, [vec![measured.listed.to_string()], measured.item(at)].concat(), "{case}");
            let shown = shown_from(&terminal, &format!("{} {}", measured.title(), measured.answers[at]));
            let menu = measured.menu(&sandbox, at);
            let details = &shown[1..shown.len() - menu.len()];
            let details: Vec<String> = details.iter().filter(|line| !line.is_empty()).cloned().collect();
            match measured.typed(at) {
                Some(typed) => {
                    let typed = sandbox.run(&typed);
                    assert_eq!(typed.status.code(), Some(0), "{case}");
                    assert_eq!(details, printed_lines(&typed), "{case}");
                    if measured.group == "measurement ltv" {
                        assert_eq!(sent(&server).await[asked.len()..], measured.item(at), "{case}: as typed");
                    }
                }
                None => assert!(details.is_empty(), "{case}: {details:#?}"),
            }
            assert_eq!(shown[shown.len() - menu.len()..], menu, "{case}");
            let before = sent(&server).await.len();
            terminal.press("\u{1b}");
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            assert_eq!(sent(&server).await.len(), before, "{case}: nothing more sent");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_click_path_signals_action_runs_its_command_exactly_as_typed() {
    let measured = SIGNALS_MEASURED;
    let server = measurement_lists_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut terminal = measured.menu_of(&sandbox, 0);
    terminal.press("\r");
    let answered = "? vendo measurement signals click-path";
    terminal.wait_for(&answered[2..]);
    let (rest, code) = terminal.finish();
    let asked = sent(&server).await;
    let typed = sandbox.run(&["measurement", "signals", "click-path"]);
    let typed_sent = sent(&server).await[asked.len()..].to_vec();
    assert_eq!(typed_sent, ["GET /api/measurement/signals/click-path"]);
    assert_eq!(asked, [measured.listed.to_string(), typed_sent[0].clone()]);
    assert_eq!((code, typed.status.code()), (Some(0), Some(0)), "{rest:?}");
    assert_eq!(after_answer(&terminal, answered), printed_lines(&typed));
}

#[cfg(unix)]
#[tokio::test]
async fn back_opens_a_measurement_list_again_on_the_item_with_no_request() {
    for (measured, filter) in [
        (METHODOLOGIES_MEASURED, "last"),
        (COHORTS_MEASURED, "987"),
        (WEEKLY_MEASURED, "09-21"),
        (SIGNALS_MEASURED, "mmm"),
    ] {
        let case = measured.args.join(" ");
        let server = measurement_lists_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = measured.open(&sandbox);
        // Filtered down to the second row, which Enter shows.
        terminal.press(filter);
        terminal.wait_for(&format!("vendo {} list {filter}", measured.group));
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        let shown_once = sent(&server).await;
        terminal.press("back\r");
        terminal.wait_for(&measured.hint_end());
        terminal.wait_for("\u{1b}[?25h");
        // The same rows, the filter cleared, the cursor on the item just viewed; nothing sent.
        assert_eq!(shown_from(&terminal, &measured.title()), measured.list(1), "{case}");
        assert_eq!(sent(&server).await, shown_once, "{case}");
        // The action menu leaves no line: answered `back`, it read as a command.
        let back = format!("vendo {} back", measured.group);
        assert!(!shown_lines(&terminal, (40, 120)).iter().any(|line| line.ends_with(&back)), "{case}");
        terminal.press("\u{1b}");
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(sent(&server).await, shown_once, "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn typing_filters_the_measurement_lists_by_any_column() {
    for (measured, typed, rows) in [
        (METHODOLOGIES_MEASURED, "SYSTEM", &[1, 2][..]),
        (METHODOLOGIES_MEASURED, "position", &[2]),
        (COHORTS_MEASURED, "1,204", &[0]),
        (WEEKLY_MEASURED, "90", &[1]),
        (SIGNALS_MEASURED, "spend history", &[1]),
        (SIGNALS_MEASURED, "live", &[0, 2]),
    ] {
        let case = format!("{} {typed}", measured.args.join(" "));
        let server = measurement_lists_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let mut terminal = measured.open(&sandbox);
        terminal.press(typed);
        terminal.wait_for(&format!("vendo {} list {typed}", measured.group));
        terminal.wait_for("\u{1b}[?25h");
        let expected: Vec<&str> = rows.iter().map(|at| measured.rows[*at]).collect();
        assert_eq!(listed_rows(&terminal), marked(&expected), "{case}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn esc_ctrl_c_or_ctrl_d_at_a_measurement_list_or_its_actions_leave_quietly() {
    // As at the group menu (VE-3826): exit 0, nothing run, the title and `<canceled>` where the list or
    // the menu was, and the cursor on the next line.
    for measured in MEASURED {
        let list = format!("vendo {} list", measured.group);
        let actions = format!("vendo {}", measured.group);
        for (step, key, title) in
            [("list", "\u{1b}", &list), ("actions", "\u{3}", &actions), ("list after back", "\u{4}", &list)]
        {
            let case = format!("{} at the {step}", measured.args.join(" "));
            let server = measurement_lists_stub().await;
            let sandbox = Sandbox::new(&server.uri());
            let mut terminal = if step == "list" { measured.open(&sandbox) } else { measured.menu_of(&sandbox, 0) };
            if step == "list after back" {
                terminal.press("back\r");
                terminal.wait_for(&measured.hint_end());
            }
            terminal.press(key);
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
            let rest = plain(&rest).to_lowercase();
            assert!(!rest.contains("error") && !rest.contains("usage"), "{case}: {rest:?}");
            let (shown, cursor) = screen(&terminal.screen, 40, 120);
            let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
            assert_eq!((shown[last].clone(), cursor), (format!("? {title} <canceled>"), (last + 1, 0)), "{case}");
            let reads = if step == "list" { 1 } else { 1 + measured.item(0).len() };
            let asked = sent(&server).await;
            assert_eq!(asked.len(), reads, "{case}: {asked:#?}");
            assert!(asked.iter().all(|request| request.starts_with("GET ")), "{case}: {asked:#?}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_cohort_that_cannot_be_read_ends_with_the_error_ltv_cohort_gives() {
    // A 404, a 429 or the network: `ltv cohort`'s error, exit 1.
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/measurement/ltv", 200, cohorts("monthly", "all", &MONTHLY)).await;
    let missing = json!({ "error": "Cohort not found" });
    Mock::given(path("/api/measurement/ltv/cohort/2026-07-01"))
        .respond_with(ResponseTemplate::new(404).set_body_json(missing).insert_header("x-request-id", "req_server_1"))
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());
    let measured = COHORTS_MEASURED;
    let mut terminal = measured.open(&sandbox);
    terminal.press("\u{1b}[B\u{1b}[B");
    terminal.wait_for(&format!("> {}", measured.rows[2]));
    terminal.press("\r");
    let answered = format!("{} {}", measured.title(), measured.answers[2]);
    terminal.wait_for(&answered[2..]);
    let (rest, code) = terminal.finish();
    let typed = sandbox.run(&["measurement", "ltv", "cohort", "2026-07-01"]);
    assert_eq!((code, typed.status.code()), (Some(1), Some(1)), "{rest:?}");
    assert_eq!(after_answer(&terminal, &answered), printed_lines(&typed));
    assert!(printed_lines(&typed)[0].starts_with("Error: "), "{:?}", printed_lines(&typed));
}

#[cfg(unix)]
#[tokio::test]
async fn where_the_list_cannot_open_and_with_json_or_output_the_measurement_lists_print_what_they_printed() {
    // Byte for byte, as `vendo apps list` (above): piped (`output/` records it), with stdout piped from
    // a terminal, and at a terminal with prompts off, on `TERM=dumb`, with stderr redirected or a
    // write-only stdin, where it prints the table a terminal shows. `--json` and `--output` print what
    // they print on a pipe; an empty `--output` the table. `signals list` has no `--output`.
    for (measured, field) in [
        (METHODOLOGIES_MEASURED, Some("clickPathModel")),
        (COHORTS_MEASURED, Some("cohortPeriod")),
        (WEEKLY_MEASURED, Some("cohort_size")),
        (SIGNALS_MEASURED, None),
    ] {
        let args = measured.args;
        let case = args.join(" ");
        let server = measurement_lists_stub().await;
        let sandbox = Sandbox::new(&server.uri());
        let piped = sandbox.run(args);
        let (reference, code) = OnTerminal::start_env(&sandbox, args, &[("CI", "1")]).finish();
        assert_eq!(code, Some(0), "{case}: {reference:?}");
        let reference = plain(&reference);
        let bordered: Vec<String> = reference.lines().map(str::to_string).collect();
        assert_eq!(table_cells(&bordered), cells(&piped.stdout), "{case}");
        assert!(reference.starts_with("┌") && reference.ends_with(&format!("{}\n", measured.footer)), "{case}");
        for (setting, code, screen) in where_no_list_opens(&sandbox, args) {
            assert_eq!((code, screen), (Some(0), reference.clone()), "{case}: {setting}");
        }
        let (_controller, terminal) = pseudo_terminal();
        let terminal = std::fs::File::from(terminal);
        let out = sandbox.command(args).stdin(terminal.try_clone().unwrap()).stderr(terminal).output().unwrap();
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), text(&piped.stdout)), "{case}");
        let mut flags = vec![vec!["--json"]];
        flags.extend(field.map(|field| vec!["--output", field]));
        for flags in flags {
            let args: Vec<&str> = args.iter().copied().chain(flags.iter().copied()).collect();
            let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
            let piped = sandbox.run(&args);
            assert_eq!((code, plain(&screen)), (Some(0), text(&piped.stdout)), "{case} {flags:?}");
        }
        if field.is_some() {
            let args: Vec<&str> = args.iter().chain(&["--output", ""]).copied().collect();
            let (screen, code) = OnTerminal::start(&sandbox, &args).finish();
            assert_eq!((code, plain(&screen)), (Some(0), reference.clone()), "{case} --output ''");
        }
        assert!(sent(&server).await.iter().all(|request| request == measured.listed), "{case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_empty_measurement_list_prints_the_table_and_its_count_at_a_terminal_too() {
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/measurement/methodologies", 200, json!({ "data": { "methodologies": [] } })).await;
    serve(&server, "GET", "/api/measurement/ltv", 200, cohorts("monthly", "all", &[])).await;
    serve(&server, "GET", "/api/measurement/signals", 200, json!({ "data": { "signals": [] } })).await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, footer) in [
        (&["measurement", "methodologies", "list"], "0 methodologys"),
        (&["measurement", "ltv", "list"], "0 cohorts"),
        (&["measurement", "signals", "list"], "0 signals"),
    ] {
        let (reference, code) = OnTerminal::start_env(&sandbox, args, &[("CI", "1")]).finish();
        assert_eq!(code, Some(0), "{args:?}");
        let (screen, code) = OnTerminal::start(&sandbox, args).finish();
        assert_eq!((code, plain(&screen)), (Some(0), plain(&reference)), "{args:?}");
        assert!(plain(&screen).ends_with(&format!("{footer}\n")), "{args:?}: {screen:?}");
        assert_eq!(text(&sandbox.run(args).stdout).lines().last(), Some(footer), "{args:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_measurement_list_and_an_items_actions_exit_as_ctrl_d_does_when_their_terminal_hangs_up() {
    let server = measurement_lists_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let mut outcomes = Vec::new();
    for (measured, step) in [(SIGNALS_MEASURED, "list"), (SIGNALS_MEASURED, "actions"), (COHORTS_MEASURED, "actions")] {
        let mut terminal = OnTerminal::start_detached(&sandbox, measured.args);
        terminal.wait_for(&measured.hint_end());
        if step == "actions" {
            terminal.press("\r");
            terminal.wait_for("Back to the list");
            terminal.wait_for("type to filter]");
        }
        let (exited, ended) = hang_up(&mut terminal);
        outcomes.push((measured.group, step, exited, ended));
    }
    assert!(
        outcomes.iter().all(|(_, _, exited, ended)| *exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "each should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {outcomes:#?}"
    );
    let cohort = COHORTS_MEASURED.item(0).remove(0);
    let (signals, ltv) = (SIGNALS_MEASURED.listed, COHORTS_MEASURED.listed);
    assert_eq!(sent(&server).await, [signals, signals, ltv, &cohort]);
}

// `vendo profile list`: no request; the profiles as its lines show them, to choose from. Enter shows
// nothing (there is no `profile get`), then `switch` unless the profile is the saved active one.

/// What the open profile list shows, the cursor on the row at `at`.
fn profile_browse(rows: &[String], at: usize) -> Vec<String> {
    let rows = rows.iter().enumerate().map(|(i, row)| format!("{} {row}", if i == at { '>' } else { ' ' }));
    [vec!["? vendo profile list".to_string()], rows.collect(), vec![HINT.to_string()]].concat()
}

/// The action menu of a profile: `switch` when offered, then `back`, described as `vendo profile
/// --help` lists them.
fn profile_menu(sandbox: &Sandbox, switch: bool) -> Vec<String> {
    let help = command_rows(sandbox, &["profile"]);
    let about = help.iter().find_map(|row| row.strip_prefix("switch ")).unwrap().trim().to_string();
    let rows = if switch {
        vec![format!("switch  {about}"), "back    Back to the list".to_string()]
    } else {
        vec!["back  Back to the list".to_string()]
    };
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    [vec!["? vendo profile".to_string()], marked(&rows), vec![HINT.to_string()]].concat()
}

#[cfg(unix)]
#[tokio::test]
async fn profile_list_at_a_terminal_lists_its_lines_to_choose_from_and_offers_switch_but_on_the_saved_profile() {
    // `config list`, the old name (VE-3827), is titled as the tree names it. Enter shows nothing first
    // and sends nothing; the saved active profile offers back only.
    let server = MockServer::start().await;
    let sandbox = Sandbox::new(&server.uri());
    let rows = profile_list_rows(&server.uri(), &["alpha", "beta"], "alpha");
    for args in [&["profile", "list"][..], &["config", "list"]] {
        let case = args.join(" ");
        let piped = sandbox.run(args);
        assert_eq!(cells(&piped.stdout), table_cells(&rows), "{case}: the lines the list shows");
        for (at, switch) in [(0, false), (1, true)] {
            let mut terminal = OnTerminal::start(&sandbox, args);
            terminal.wait_for("type to filter]");
            assert_eq!(shown_lines(&terminal, (40, 120)), profile_browse(&rows, 0), "{case}");
            if at > 0 {
                terminal.press("\u{1b}[B");
            }
            terminal.press("\r");
            terminal.wait_for("Back to the list");
            terminal.wait_for("type to filter]");
            let name = ["alpha", "beta"][at];
            let shown = shown_from(&terminal, &format!("? vendo profile list {name}"));
            assert_eq!(shown[1..], profile_menu(&sandbox, switch), "{case} {name}: nothing shown first");
            terminal.press("\u{1b}");
            let (rest, code) = terminal.finish();
            assert_eq!(code, Some(0), "{case}: {rest:?}");
        }
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[tokio::test]
async fn a_chosen_profile_switch_runs_exactly_as_typed_with_the_profile_and_environment_the_list_had() {
    // The marker follows `--profile` and VENDO_PROFILE; `switch` follows the saved activeProfile (alpha),
    // so beta offers it however it is marked, and alpha never. What follows is what `profile switch beta`
    // prints, typed with the same flags and environment (its VENDO_PROFILE note too).
    let server = MockServer::start().await;
    for (args, env, marked_on) in [
        (&["profile", "list"][..], &[][..], "alpha"),
        (&["profile", "list"], &[("VENDO_PROFILE", "beta")], "beta"),
        (&["--profile", "beta", "profile", "list"], &[], "beta"),
        (&["config", "list", "--profile=beta"], &[], "beta"),
    ] {
        let case = format!("{env:?} {}", args.join(" "));
        let sandbox = Sandbox::new(&server.uri());
        let rows = profile_list_rows(&server.uri(), &["alpha", "beta"], marked_on);
        // alpha, the saved active profile: back only, whatever the marker.
        let mut terminal = OnTerminal::start_env(&sandbox, args, env);
        terminal.wait_for("type to filter]");
        assert_eq!(shown_lines(&terminal, (40, 120)), profile_browse(&rows, 0), "{case}");
        terminal.press("\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        assert_eq!(shown_from(&terminal, "? vendo profile list alpha")[1..], profile_menu(&sandbox, false), "{case}");
        terminal.press("\u{1b}");
        assert_eq!(terminal.finish().1, Some(0), "{case}");
        // beta: switch, as typed.
        let mut terminal = OnTerminal::start_env(&sandbox, args, env);
        terminal.wait_for("type to filter]");
        terminal.press("bet\r");
        terminal.wait_for("Back to the list");
        terminal.wait_for("type to filter]");
        terminal.press("\r");
        let answered = "? vendo profile switch beta";
        terminal.wait_for(&answered[2..]);
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{case}: {rest:?}");
        assert_eq!(sandbox.config()["activeProfile"], "beta", "{case}");
        let twin = Sandbox::new(&server.uri());
        let globals: Vec<&str> =
            args.iter().copied().filter(|arg| arg.starts_with("--profile") || *arg == "beta").collect();
        let typed_args: Vec<&str> = globals.into_iter().chain(["profile", "switch", "beta"]).collect();
        let typed = twin.command(&typed_args).envs(env.iter().copied()).output().unwrap();
        assert_eq!(typed.status.code(), Some(0), "{case}");
        assert_eq!(after_answer(&terminal, answered), printed_lines(&typed), "{case}");
        assert_eq!(twin.config()["activeProfile"], "beta", "{case}");
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn back_esc_ctrl_c_and_ctrl_d_at_the_profile_list_or_a_profiles_actions() {
    let server = MockServer::start().await;
    let sandbox = Sandbox::new(&server.uri());
    let rows = profile_list_rows(&server.uri(), &["alpha", "beta"], "alpha");
    // Back: the same rows, the filter cleared, the cursor on the profile just viewed.
    let mut terminal = OnTerminal::start(&sandbox, &["profile", "list"]);
    terminal.wait_for("type to filter]");
    terminal.press("acct-beta");
    terminal.wait_for("vendo profile list acct-beta");
    terminal.press("\r");
    terminal.wait_for("Back to the list");
    terminal.wait_for("type to filter]");
    terminal.press("back\r");
    terminal.wait_for("vendo profile back");
    terminal.wait_for("type to filter]");
    terminal.wait_for("\u{1b}[?25h");
    assert_eq!(shown_from(&terminal, "? vendo profile list"), profile_browse(&rows, 1));
    terminal.press("\u{1b}");
    assert_eq!(terminal.finish().1, Some(0));
    // Esc, Ctrl-C and Ctrl-D: exit 0, nothing switched, `<canceled>` where the list or menu was.
    for (step, key, title) in [
        ("list", "\u{1b}", "vendo profile list"),
        ("actions", "\u{3}", "vendo profile"),
        ("list after back", "\u{4}", "vendo profile list"),
    ] {
        let mut terminal = OnTerminal::start(&sandbox, &["profile", "list"]);
        terminal.wait_for("type to filter]");
        if step != "list" {
            terminal.press("\u{1b}[B\r");
            terminal.wait_for("Back to the list");
            terminal.wait_for("type to filter]");
        }
        if step == "list after back" {
            terminal.press("back\r");
            terminal.wait_for("vendo profile back");
            terminal.wait_for("type to filter]");
        }
        terminal.press(key);
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(0), "{step}: {rest:?}");
        let (shown, cursor) = screen(&terminal.screen, 40, 120);
        let last = shown.iter().rposition(|line| !line.is_empty()).unwrap();
        assert_eq!((shown[last].clone(), cursor), (format!("? {title} <canceled>"), (last + 1, 0)), "{step}");
    }
    // A hang-up, at the list and at the actions.
    let mut outcomes = Vec::new();
    for step in ["list", "actions"] {
        let mut terminal = OnTerminal::start_detached(&sandbox, &["profile", "list"]);
        terminal.wait_for("type to filter]");
        if step == "actions" {
            terminal.press("\u{1b}[B\r");
            terminal.wait_for("Back to the list");
            terminal.wait_for("type to filter]");
        }
        let (exited, ended) = hang_up(&mut terminal);
        outcomes.push((step, exited, ended));
    }
    assert!(
        outcomes.iter().all(|(_, exited, ended)| *exited && ended.code == Some(0) && ended.cpu < HUNG_UP_CPU),
        "each should exit 0 within {HUNG_UP_EXIT:?}, using under {HUNG_UP_CPU:?} of processor time: {outcomes:#?}"
    );
    assert_eq!(sent(&server).await, Vec::<String>::new());
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[cfg(unix)]
#[tokio::test]
async fn where_the_list_cannot_open_with_json_or_with_no_profiles_profile_list_prints_what_it_printed() {
    // Byte for byte what a pipe gets: at a terminal with prompts off, on `TERM=dumb`, with stderr
    // redirected or a write-only stdin, with stdout piped, and with `--json`. With no profiles, its line
    // that says so, at a terminal too.
    let server = MockServer::start().await;
    let sandbox = Sandbox::new(&server.uri());
    let none = Sandbox::new(&server.uri());
    std::fs::write(none.home.path().join(".config/vendo/config.json"), r#"{"profiles":{}}"#).unwrap();
    for args in [&["profile", "list"][..], &["config", "list"]] {
        let case = args.join(" ");
        let piped = sandbox.run(args);
        let reference = text(&piped.stdout);
        assert!(reference.contains("* alpha (active)"), "{case}: {reference}");
        for (setting, code, screen) in where_no_list_opens(&sandbox, args) {
            assert_eq!((code, screen), (Some(0), reference.clone()), "{case}: {setting}");
        }
        let (_controller, terminal) = pseudo_terminal();
        let terminal = std::fs::File::from(terminal);
        let out = sandbox.command(args).stdin(terminal.try_clone().unwrap()).stderr(terminal).output().unwrap();
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), reference.clone()), "{case}");
        let args_json: Vec<&str> = args.iter().chain(&["--json"]).copied().collect();
        let (screen, code) = OnTerminal::start(&sandbox, &args_json).finish();
        assert_eq!((code, plain(&screen)), (Some(0), text(&sandbox.run(&args_json).stdout)), "{case} --json");
        let empty = text(&none.run(args).stdout);
        assert_eq!(empty, "No profiles configured. Run `vendo login` to create one.\n", "{case}");
        let (screen, code) = OnTerminal::start(&none, args).finish();
        assert_eq!((code, plain(&screen)), (Some(0), empty), "{case}: no profiles, at a terminal");
    }
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

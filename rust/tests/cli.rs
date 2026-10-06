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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vendo"));
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
            &["ID", "Name", "Type", "Format", "Status", "Updated"],
            &["6f1c2a9e...", "ROAS", "multiplier", "active", "—"],
            &["short-id", "Draft one", "number", "draft", "—"],
            &["abcdefab...", "Archived", "legacy", "currency", "archived", "—"],
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
            "\nROAS (undefined)\n\n  ID:           {M1}\n  Type:         undefined\n  Format:       multiplier\n  Status:       active\n  Updated:      —\n  Description:  Return on ad spend\n  Unit:         x\n  Higher=Better: yes\n  Calculation:  segmentation\n"
        )
    );
    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "get", "short-id"])),
        "\nROAS (ratio)\n\n  ID:           short-id\n  Type:         ratio\n  Format:       multiplier\n  Status:       draft\n  Updated:      —\n  Higher=Better: no\n  Calculation:  unknown\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["metrics", "get", "odd"])),
        "\nnull (undefined)\n\n  ID:           odd\n  Type:         undefined\n  Format:       null\n  Status:       null\n  Updated:      —\n  Description:  Return on ad spend\n  Unit:         x\n  Higher=Better: yes\n  Calculation:  unknown\n"
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

fn model(over: Value) -> Value {
    let mut row = json!({
        "id": "11111111-2222-4333-8444-555555555555", "state": "active", "accountId": "acct-alpha",
        "name": "orders_clean", "description": null, "modelType": "sql", "viewName": "orders_clean",
        "outputConfig": { "dataset_id": "prod", "write_mode": "replace" },
        "schedule": { "frequency_unit": "hours", "frequency_value": 6 },
        "columns": [{ "name": "order_id", "type": "STRING", "is_nullable": false }],
        "primaryKeyColumns": null, "incrementalColumn": null, "isValid": true, "validationError": null,
        "lastValidatedAt": null, "createdAt": null, "updatedAt": null, "managedBy": "customer",
    });
    row.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    row
}

#[tokio::test]
async fn models_list_and_get_like_ts() {
    let server = MockServer::start().await;
    let list = json!({
        "data": [model(json!({})), model(json!({ "id": "m2", "name": "broken", "isValid": false, "dataType": "events" }))],
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
            &["11111111...", "orders_clean", "yes", "—"],
            &["m2", "broken", "events", "no", "—"],
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
        "\norders_clean (undefined)\n\n  ID:            11111111-2222-4333-8444-555555555555\n  Data Type:     undefined\n  Valid:         yes\n  Validated:     —\n  Created:       —\n  Description:   Clean orders\n  Primary Keys:  order_id, line_id\n  Incremental:   updated_at\n\n  Validation Error: Table not found\n\n  SQL Query:\n    SELECT order_id\n    FROM `p.d.orders`\n      WHERE 1 = 1\n"
    );
    assert_eq!(
        ok_output(&sandbox.run(&["models", "get", "mod-2"])),
        "\norders_clean (undefined)\n\n  ID:            11111111-2222-4333-8444-555555555555\n  Data Type:     undefined\n  Valid:         no\n  Validated:     —\n  Created:       —\n"
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
    output: std::sync::mpsc::Receiver<Vec<u8>>,
    /// Everything the terminal showed so far, as `vendo` wrote it.
    screen: String,
    /// The end of what [`OnTerminal::wait_for`] last found.
    seen: usize,
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

    /// `cmd` on a terminal of `lines` × `columns` that already shows `before`; stdin and stderr
    /// elsewhere than the terminal when given.
    fn spawn(
        mut cmd: Command,
        (lines, columns): (u16, u16),
        before: &str,
        stdin: Option<Stdio>,
        stderr: Option<Stdio>,
    ) -> Self {
        use std::{
            io::{Read, Write},
            os::unix::process::CommandExt,
        };
        let (controller, terminal, name) = pseudo_terminal_sized(lines, columns);
        std::fs::File::from(terminal.try_clone().unwrap()).write_all(before.as_bytes()).unwrap();
        let kept = terminal.try_clone().unwrap();
        cmd.env("NO_COLOR", "1")
            .stdin(stdin.unwrap_or_else(|| terminal.try_clone().unwrap().into()))
            .stdout(terminal.try_clone().unwrap())
            .stderr(stderr.unwrap_or_else(|| terminal.into()));
        // The terminal is `vendo`'s own, as in a terminal window: its session's controlling
        // terminal, which `/dev/tty` opens, not the one running the tests (or none, in CI).
        // SAFETY: setsid and ioctl only, between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(libc::STDOUT_FILENO, libc::TIOCSCTTY as _, 0) < 0 {
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
        std::thread::spawn(move || {
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
        OnTerminal { controller, name, terminal: Some(kept), child, output, screen: String::new(), seen: 0 }
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
}

#[cfg(unix)]
impl Drop for OnTerminal {
    /// Stops a `vendo` still running when the test ends early, on a failed assertion, so it reads
    /// nothing from what the test leaves behind (login opens a browser on a line it reads).
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        "\nSetup summary\n  Profile:     {profile}\n  Base URL:    {base_url}\n  Account ID:  {account_id}\n  Auth:        verified as Demo Account (synthetic)\n\nNext steps\n  vendo doctor\n  vendo whoami\n  vendo status\nDone: Vendo CLI setup complete.\n"
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
            "Error: Could not verify the API key: {reason}. Nothing was changed: check your connection (`vendo doctor`) and run `vendo login` again.\n"
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
            "Error: Could not verify the API key: {reason}. The new key is saved in profile demo-account: run `vendo whoami` to check it again.\n"
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
            "Using existing profile alpha. Run `vendo login --force` to sign in again.\n\nSetup summary\n  Profile:     alpha\n  Base URL:    {stub}\n  Account ID:  missing\n  Auth:        incomplete (account ID still required)\n\nNext steps\n  vendo doctor\n  vendo whoami\n  vendo status\n\nSet an account explicitly with `vendo profile set --account <account-id>` if your login flow did not provide one.\nDone: Vendo CLI setup complete.\n"
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
    let message = "Could not verify the API key: HTTP 500 Internal Server Error. Nothing was changed: check your connection (`vendo doctor`) and run `vendo login` again.";
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

fn whoami_config(sandbox: &Sandbox, args: &[&str], vendo_profile: Option<&str>) -> Value {
    let mut cmd = sandbox.command(&[args, &["whoami", "--json"]].concat());
    if let Some(name) = vendo_profile {
        cmd.env("VENDO_PROFILE", name);
    }
    let out: Value = serde_json::from_str(&ok_output(&cmd.output().unwrap())).unwrap();
    json!([out["data"]["accountId"], out["config"]["selectedProfile"]])
}

#[tokio::test]
async fn vendo_profile_selects_the_profile_and_the_flag_overrides_it() {
    let server = me_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    assert_eq!(whoami_config(&sandbox, &[], None), json!(["acct-alpha", "alpha"]));
    assert_eq!(whoami_config(&sandbox, &[], Some("beta")), json!(["acct-beta", "beta"]));
    assert_eq!(whoami_config(&sandbox, &["--profile", "alpha"], Some("beta")), json!(["acct-alpha", "alpha"]));
    // Empty is unset.
    assert_eq!(whoami_config(&sandbox, &[], Some("")), json!(["acct-alpha", "alpha"]));
    // The profile commands act on it, as with --profile; the saved active profile stays.
    let out = sandbox.command(&["profile", "list"]).env("VENDO_PROFILE", "beta").output().unwrap();
    assert_eq!(
        cells(ok_output(&out).as_bytes()),
        rows(&[&["alpha", "acct-alpha", &server.uri()], &["* beta (active)", "acct-beta", &server.uri()]])
    );
    assert_eq!(sandbox.config()["activeProfile"], "alpha");
}

#[tokio::test]
async fn whoami_and_doctor_show_the_profile_vendo_profile_selects_as_they_show_the_flags() {
    // They name the profile as they do for --profile and activeProfile; their hints about switching
    // profiles say that VENDO_PROFILE overrides the active profile (Yalcin, 2026-10-06).
    let server = me_stub().await;
    let sandbox = Sandbox::new(&server.uri());
    let env = sandbox.command(&["whoami"]).env("VENDO_PROFILE", "beta").output().unwrap();
    let printed = ok_output(&env);
    assert!(printed.contains("  Profile:     beta\n"), "{printed}");
    let switch_hint = "  Switch with `vendo profile switch` or target one command with `vendo --profile <name> ...`.\n";
    let note = "  VENDO_PROFILE=beta overrides the active profile in this shell: change or unset VENDO_PROFILE to switch here.\n";
    assert_eq!(
        printed,
        ok_output(&sandbox.run(&["--profile", "beta", "whoami"]))
            .replace(switch_hint, &(switch_hint.to_string() + note))
    );
    // --profile wins over VENDO_PROFILE, so the note goes.
    let flag = sandbox.command(&["--profile", "beta", "whoami"]).env("VENDO_PROFILE", "alpha").output().unwrap();
    assert_eq!(ok_output(&flag), ok_output(&sandbox.run(&["--profile", "beta", "whoami"])));

    let doctor = |vendo_profile: Option<&str>, args: &[&str]| {
        let mut cmd = sandbox.command(&[args, &["doctor", "--json"]].concat());
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
    for args in [&["apps", "list"][..], &["whoami"], &["jobs", "get", "1a2b3c4d"], &["logout"]] {
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
        "Done: Switched to profile beta.\n  Account ID: acct-beta  Base URL: https://example.test\n  VENDO_PROFILE=alpha still overrides it in this shell: unset VENDO_PROFILE to use beta here.\n  Verify with `vendo whoami`.\n"
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
        "Done: Switched to profile beta.\n  Account ID: acct-beta  Base URL: https://example.test\n  Verify with `vendo whoami`.\n"
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
async fn doctors_fixes_for_a_rejected_key_under_vendo_profile_say_it_overrides_the_active_profile() {
    // Following "run `vendo login`, then retry `vendo whoami`" under VENDO_PROFILE checks the old key again:
    // login saves the new one in a profile it does not make active (Yalcin, 2026-10-06).
    let server = MockServer::start().await;
    serve(&server, "GET", "/api/v1/me", 401, json!({ "error": { "message": "Invalid API key" } })).await;
    let sandbox = Sandbox::new(&server.uri());
    let auth_fix = |cmd: &mut Command| {
        let out: Value = serde_json::from_str(&text(&cmd.output().unwrap().stdout)).unwrap();
        out["checks"].as_array().unwrap().iter().find(|c| c["name"] == "API auth").unwrap()["remediation"].clone()
    };
    let fix = "Run `vendo login` to refresh credentials, then retry `vendo --profile <profile> whoami` with the profile it saved (VENDO_PROFILE=beta overrides the active profile in this shell).";
    assert_eq!(auth_fix(sandbox.command(&["doctor", "--json"]).env("VENDO_PROFILE", "beta")), json!(fix));
    let out = sandbox.command(&["doctor"]).env("VENDO_PROFILE", "beta").output().unwrap();
    let printed = text(&out.stdout);
    assert!(printed.contains(&format!("\n       Fix: {fix}\n")), "{printed}");
    assert!(printed.contains(&format!("\n  - {fix}\n")), "{printed}");
    // Without VENDO_PROFILE, or with --profile over it, as before.
    let plain = json!("Run `vendo login` to refresh credentials, then retry `vendo whoami`.");
    assert_eq!(auth_fix(&mut sandbox.command(&["doctor", "--json"])), plain);
    assert_eq!(
        auth_fix(sandbox.command(&["--profile", "beta", "doctor", "--json"]).env("VENDO_PROFILE", "alpha")),
        plain
    );
}

#[tokio::test]
async fn a_login_whose_new_key_cannot_be_checked_under_vendo_profile_says_how_to_check_it() {
    // The new key is checked with `/me`; when that fails, the error says how to check that profile again,
    // which `vendo whoami` alone would not while VENDO_PROFILE names another.
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
        stderr.ends_with(". The new key is saved in profile demo-account: run `vendo --profile demo-account whoami` to check it again (VENDO_PROFILE=beta overrides the active profile in this shell).\n"),
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
// (snapshots.rs checks every group). A chosen command that needs an argument fails as it does when
// typed without it.

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
/// the cursor). Colours and modes change nothing.
fn screen(output: &str, lines: usize, columns: usize) -> (Vec<String>, (usize, usize)) {
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
                if wrap {
                    line_feed(&mut shown, &mut line);
                    (column, wrap) = (0, false);
                }
                shown[line][column] = c;
                if column + 1 == columns {
                    wrap = true;
                } else {
                    column += 1;
                }
            }
        }
    }
    (shown.iter().map(|line| line.iter().collect::<String>().trim_end().to_string()).collect(), (line, column))
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
    // `vendo --profile beta apps list` runs against the stub.
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran}");
    let ran = plain(&ran);
    assert!(ran.contains("a1b2c3d4...") && ran.contains("Menu Shop") && ran.contains("shopify"), "{ran}");
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
    terminal.press("\r");
    let (ran, code) = terminal.finish();
    assert_eq!(code, Some(0), "{ran}");
    let ran = plain(&ran);
    assert!(ran.contains("alpha") && ran.contains("beta"), "{ran}");
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
#[test]
fn a_chosen_command_missing_its_argument_is_the_usage_error_it_is_when_typed() {
    let sandbox = Sandbox::new(CLOSED);
    let typed = sandbox.run(&["apps", "get"]);
    assert_eq!(typed.status.code(), Some(2));
    let mut terminal = OnTerminal::start(&sandbox, &["apps"]);
    terminal.wait_for("Update an app");
    terminal.press("get app\r");
    terminal.wait_for("vendo apps get");
    let (rest, code) = terminal.finish();
    assert_eq!(code, Some(2), "{rest:?}");
    assert!(plain(&rest).ends_with(&text(&typed.stderr)), "{rest:?}");
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
}

#[cfg(unix)]
#[test]
fn on_a_short_terminal_the_menu_scrolls_its_list() {
    // A menu taller than the screen drew over itself: the title went, rows went missing or showed
    // twice, two of them marked `>`. It lists as many commands as fit between the title and the
    // hint, with a line to spare, and scrolls; the one marked `>` is the one Enter runs.
    let sandbox = Sandbox::new(CLOSED);
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
        // Both need an ID, so Enter ends in the usage error for the command shown.
        terminal.press("\r");
        terminal.wait_for(&format!("vendo {group} {chosen}"));
        let (rest, code) = terminal.finish();
        assert_eq!(code, Some(2), "{rest:?}");
        assert!(plain(&rest).contains(&format!("Usage: vendo {group} {chosen} <")), "{rest:?}");
    }
}

// ── VE-3826: CI and VENDO_NO_INPUT turn prompts off; no menu on TERM=dumb ───
// Decided by Yalcin, 2026-10-06, CLI 1.1. `CI` or `VENDO_NO_INPUT` set to anything but empty, `0`
// or `false` (in any case) turns every prompt off, also at a terminal: the y/N questions, the group
// menu and the profile picker do what they do without a terminal, and login reads no "Press ENTER"
// at a terminal (a stdin that is no terminal it reads as before). There is no `--no-input` flag. On `TERM=dumb` a bare group is the usage error, as without a
// terminal; the questions still ask there. The profile picker asks only when stdin and stdout are
// terminals, the rule the questions ask by.

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
    let mut terminal = OnTerminal::start_env(&sandbox, &["profile", "switch"], &[("TERM", "dumb")]);
    terminal.wait_for("Search (ENTER for all, q to cancel) ");
    terminal.press("q\r");
    assert_eq!(terminal.finish().1, Some(0));
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn the_profile_picker_asks_only_when_stdin_and_stdout_are_terminals() {
    use std::io::Write;
    // `echo 1 | vendo profile switch` on a terminal: what the pipe holds is no answer. As without a
    // terminal, "Cancelled." and nothing switched. ("\n2\n" would list every profile and pick beta.)
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
    // Prompts on, at a terminal it asks: ENTER lists every profile, and 2 picks beta.
    for setting in PROMPTS_ON {
        let sandbox = Sandbox::new(CLOSED);
        let mut terminal = OnTerminal::start_env(&sandbox, &["profile", "switch"], &[setting]);
        terminal.wait_for("Search (ENTER for all, q to cancel) ");
        terminal.press("\r");
        terminal.wait_for("Choose an option (ENTER to search again) ");
        terminal.press("2\r");
        let (screen, code) = terminal.finish();
        assert_eq!(code, Some(0), "{setting:?} {screen:?}");
        assert!(plain(&screen).contains("Switched to profile beta."), "{setting:?} {screen:?}");
        assert_eq!(sandbox.config()["activeProfile"], "beta", "{setting:?}");
    }
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

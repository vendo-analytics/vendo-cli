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
        // Dates and numbers follow the locale (VE-3728): pin it, and the time zone.
        cmd.args(args).env("HOME", self.home.path()).env("LANG", "C").env("TZ", "UTC").stdin(Stdio::null());
        for var in ["VENDO_API_KEY", "VENDO_API_URL", "VENDO_ACCOUNT_ID", "VENDO_DEBUG", "LC_ALL", "LC_MESSAGES"] {
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
                "Error: No API key configured. Run `vendo login` or `vendo config set --api-key <key>` or set VENDO_API_KEY."
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
            vec!["integrations", "delete", ID],
            format!("This deletes integration {ID}."),
            format!("DELETE {v1}/connections/{ID}"),
        ),
        (vec!["int", "delete", ID], format!("This deletes integration {ID}."), format!("DELETE {v1}/connections/{ID}")),
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
        // Refused before any request, also with --json (which used to imply --yes).
        assert_needs_yes(&sandbox.run(&args), &what);
        assert_needs_yes(&sandbox.run(&[&args[..], &["--json"]].concat()), &what);
        assert_eq!(sent(&server).await, Vec::<String>::new(), "{args:?} sent a request without --yes");

        for yes in [&["--yes"][..], &["-y"], &["--yes", "--json"]] {
            let out = sandbox.run(&[&args[..], yes].concat());
            assert_eq!(out.status.code(), Some(0), "{args:?} {yes:?}: {}", text(&out.stderr));
        }
        assert_eq!(sent(&server).await, vec![request; 3], "{args:?}");
    }
}

#[tokio::test]
async fn a_dry_run_needs_no_yes() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    for (args, printed) in [
        (["apps", "delete", ID, "--dry-run"], "[dry-run] Would delete app 550e8400...\n"),
        (["sources", "delete", ID, "--dry-run"], "[dry-run] Would delete source 550e8400...\n"),
        (["int", "delete", ID, "--dry-run"], "[dry-run] Would delete integration 550e8400...\n"),
        (["jobs", "cancel", ID, "--dry-run"], "[dry-run] Would cancel job 550e8400...\n"),
    ] {
        assert_eq!(ok_output(&sandbox.run(&args)), printed);
    }
    assert_eq!(sent(&server).await, Vec::<String>::new());
}

#[test]
fn without_a_terminal_config_reset_needs_yes() {
    let sandbox = Sandbox::new(CLOSED);
    let file = sandbox.home.path().join(".config/vendo/config.json");
    assert_needs_yes(&sandbox.run(&["config", "reset"]), "This deletes all CLI configuration.");
    assert!(file.exists(), "the refusal kept the configuration");
    assert_eq!(ok_output(&sandbox.run(&["config", "reset", "--yes"])), "Done: Configuration deleted.\n");
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
        (std::fs::File::from_raw_fd(controller), OwnedFd::from_raw_fd(terminal))
    }
}

/// Run `vendo` with stdin, stdout and stderr on a pseudo-terminal, like a
/// person at a terminal; `answer` is typed once the `(y/N)` question shows.
/// Returns what the terminal showed and the exit code.
#[cfg(unix)]
fn answer_on_terminal(sandbox: &Sandbox, args: &[&str], answer: &str) -> (String, Option<i32>) {
    use std::io::{Read, Write};
    let (controller, terminal) = pseudo_terminal();
    let mut cmd = sandbox.command(args);
    cmd.env("NO_COLOR", "1")
        .stdin(terminal.try_clone().unwrap())
        .stdout(terminal.try_clone().unwrap())
        .stderr(terminal);
    let mut child = cmd.spawn().unwrap();
    // Close the test's copies of the terminal end, so reading ends when `vendo` exits.
    drop(cmd);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut reader = controller.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut screen = String::new();
    let mut typed = false;
    while let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_secs(20)) {
        screen.push_str(&text(&chunk));
        if !typed && screen.contains("(y/N) ") {
            (&controller).write_all(answer.as_bytes()).unwrap();
            typed = true;
        }
    }
    let _ = child.kill();
    (screen, child.wait().unwrap().code())
}

#[cfg(unix)]
#[tokio::test]
async fn on_a_terminal_the_question_is_unchanged() {
    let server = stub_accepting_everything().await;
    let sandbox = Sandbox::new(&server.uri());
    // "n" changes nothing; the questions and "Cancelled" lines are the TS CLI's.
    for (args, question, after) in [
        (&["apps", "delete", ID][..], "Delete app 550e8400...? (y/N) ", ""),
        (&["sources", "delete", ID, "--json"], "Delete source 550e8400...? (y/N) ", ""),
        (&["int", "delete", ID], "Delete integration 550e8400...? (y/N) ", ""),
        (&["jobs", "cancel", ID], "Cancel job 550e8400...? (y/N) ", ""),
        (
            &["metrics", "delete", ID, "--json"],
            "Delete metric 550e8400...? This cannot be undone. (y/N) ",
            "Cancelled\n",
        ),
        (&["config", "reset"], "Delete all CLI configuration? (y/N) ", "Cancelled.\n"),
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

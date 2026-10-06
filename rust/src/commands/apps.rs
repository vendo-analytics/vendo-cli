//! `vendo apps …` (port of `src/commands/apps.ts`).

use std::{collections::HashMap, time::Duration};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{
    client::{Client, payload},
    commands::pipeline_resource::{read_json_file, split_list},
    context::Ctx,
    jobs::Job,
    output::{
        OutputMode, bold, color_status, dim, js_color_status, js_greater_than, js_if, js_join, js_template, js_truthy,
        print_field, print_json, print_label, print_list_count, print_status_label, print_success, red,
        resolve_output_mode, run_action, short_id, table, time_ago, yellow,
    },
    short_ids::{Listing, resolve},
};

// ── roles and permissions ──────────────────────────────────────────────────
// Detection set (to recognise a destination app) vs grant set (what `--role`
// asks for) differ on purpose: `--role` never grants campaign changes.
const SOURCE_PERMISSIONS: &[&str] = &["performance_data"];
const DESTINATION_PERMISSIONS: &[&str] = &[
    "send_conversions",
    "sync_audiences",
    "campaign_changes",
    "campaign_adjust_bids",
    "campaign_adjust_spend",
    "campaign_toggle",
];

fn text(v: &Value, key: &str) -> Option<String> {
    Job(v).text(key)
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn permissions_from_roles(roles: &[String]) -> Vec<String> {
    let has = |r: &str| roles.iter().any(|x| x == r || x == "both");
    let mut perms: Vec<String> = Vec::new();
    if has("source") {
        perms.extend(SOURCE_PERMISSIONS.iter().map(|s| s.to_string()));
    }
    if has("destination") {
        perms.extend(["send_conversions", "sync_audiences"].map(str::to_string));
    }
    if perms.is_empty() { SOURCE_PERMISSIONS.iter().map(|s| s.to_string()).collect() } else { perms }
}

/// The grant for `--role` from the server catalog's `defaultPermissions`
/// (VE-2534), read as the TS CLI read it. Without a client (no API key) or
/// when the lookup fails it falls back to the local map, unless the error
/// says the type "does not support" the role: that stops the command.
async fn resolve_permissions_for_roles(
    client: Option<&Client>,
    app_type: &str,
    roles: &[String],
) -> Result<Vec<String>> {
    let Some(client) = client else { return Ok(permissions_from_roles(roles)) };
    let res = match client.get(&format!("/catalog/{app_type}"), &[]).await {
        Ok(res) => res,
        Err(err) if err.message.contains("does not support") => return Err(err.into()),
        Err(_) => return Ok(permissions_from_roles(roles)),
    };
    // `if (defaults)`: any truthy value, so an array or a string refuses below.
    let Some(defaults) = payload(&res).get("defaultPermissions").filter(|d| js_truthy(d)) else {
        return Ok(permissions_from_roles(roles));
    };
    let mut perms: Vec<String> = Vec::new();
    for (role, key) in [("source", "source"), ("destination", "destination")] {
        if !roles.iter().any(|r| r == role || r == "both") {
            continue;
        }
        // `for (const p of defaults[key] ?? [])`: arrays by element, strings by
        // character; anything else threw in TS and fell back to the local map.
        let granted: Vec<String> = match defaults.get(key) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Some(Value::String(s)) => s.chars().map(String::from).collect(),
            Some(_) => return Ok(permissions_from_roles(roles)),
        };
        for p in granted {
            if !perms.contains(&p) {
                perms.push(p);
            }
        }
    }
    if perms.is_empty() {
        bail!(
            "App type \"{app_type}\" does not support the requested role(s): {}. Check 'vendo catalog get {app_type}' for its supported roles.",
            roles.join(", ")
        );
    }
    Ok(perms)
}

fn capability_label(permissions: &[String]) -> String {
    let source = permissions.iter().any(|p| SOURCE_PERMISSIONS.contains(&p.as_str()));
    let destination = permissions.iter().any(|p| DESTINATION_PERMISSIONS.contains(&p.as_str()));
    match (source, destination) {
        (true, true) => "source, destination".into(),
        (false, true) => "destination".into(),
        (true, false) => "source".into(),
        (false, false) => "—".into(),
    }
}

fn role_label(app: &Value) -> String {
    match app.get("roles") {
        Some(Value::Array(roles)) if roles.is_empty() => "—".into(),
        Some(Value::Array(roles)) => js_join(roles, ", "),
        // Older servers don't send roles: derive from permissions.
        _ => capability_label(&strings(app, "permissions")),
    }
}

fn access_status_label(app: &Value) -> String {
    if app.get("state").and_then(Value::as_str) == Some("inactive") {
        return dim("paused");
    }
    match app.get("accessStatus").filter(|s| js_truthy(s)) {
        None => dim("not checked"),
        Some(Value::String(s)) if s == "auth_expired" => red("reconnect required"),
        status => js_color_status(status),
    }
}

fn role_list(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

// ── list / diagnose / get ──────────────────────────────────────────────────

pub struct ListArgs {
    pub state: Option<String>,
    pub app_type: Option<String>,
    pub role: Option<String>,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action(
        "Fetching apps...",
        client.get(
            "/apps",
            &[
                ("state", args.state),
                ("app_type", args.app_type),
                ("capability", args.role),
                ("limit", Some(args.limit)),
                ("offset", Some(args.offset)),
            ],
        ),
    )
    .await?;
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    match resolve_output_mode(args.json, args.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => print_field(&rows, args.output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            let mut grid = table(&["ID", "Name", "Type", "Role", "State", "Status", "Last Sync"]);
            for app in &rows {
                grid.add_row(vec![
                    dim(&short_id(&text(app, "id").unwrap_or_default())),
                    text(app, "displayName").unwrap_or_default(),
                    text(app, "appType").unwrap_or_default(),
                    role_label(app),
                    color_status(&text(app, "state").unwrap_or_default()),
                    access_status_label(app),
                    time_ago(text(app, "lastSyncAt").as_deref()),
                ]);
            }
            println!("{grid}");
            print_list_count(&res, rows.len(), "app");
        }
    }
    Ok(())
}

pub async fn diagnose(ctx: &Ctx, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let page = [("limit", Some("100".to_string()))];
    let (apps, sources, integrations) = run_action("Diagnosing apps...", async {
        tokio::try_join!(client.get("/apps", &page), client.get("/sources", &page), client.get("/integrations", &page))
    })
    .await?;
    let rows = |v: &Value| payload(v).as_array().cloned().unwrap_or_default();
    let (apps, sources, integrations) = (rows(&apps), rows(&sources), rows(&integrations));

    let mut source_counts: HashMap<String, usize> = HashMap::new();
    for s in &sources {
        if let Some(app_id) = text(s, "appId").filter(|s| !s.is_empty()) {
            *source_counts.entry(app_id).or_default() += 1;
        }
    }
    let mut dest_counts: HashMap<String, usize> = HashMap::new();
    for i in &integrations {
        if let Some(app_id) = text(i, "destinationAppId").filter(|s| !s.is_empty()) {
            *dest_counts.entry(app_id).or_default() += 1;
        }
    }

    let mut broken: Vec<Value> = Vec::new();
    let mut orphaned: Vec<(Value, &'static str)> = Vec::new();
    for app in &apps {
        if text(app, "state").as_deref() == Some("inactive") {
            continue;
        }
        if matches!(text(app, "accessStatus").as_deref(), Some("disconnected" | "auth_expired")) {
            broken.push(app.clone());
        }
        let id = text(app, "id").unwrap_or_default();
        let roles = strings(app, "roles");
        // One entry per missing direction, like MCP apps_diagnose.
        if roles.iter().any(|r| r == "source") && source_counts.get(&id).copied().unwrap_or(0) == 0 {
            orphaned.push((app.clone(), "source"));
        }
        if roles.iter().any(|r| r == "destination") && dest_counts.get(&id).copied().unwrap_or(0) == 0 {
            orphaned.push((app.clone(), "destination"));
        }
    }

    if json {
        let orphaned: Vec<Value> = orphaned
            .iter()
            .map(|(app, missing)| {
                let mut row = app.as_object().cloned().unwrap_or_default();
                row.insert("missing".into(), json!(missing));
                Value::Object(row)
            })
            .collect();
        print_json(&json!({ "broken": broken, "orphaned": orphaned }));
        return Ok(());
    }

    if apps.len() >= 100 {
        println!("{}", dim("Note: diagnosis covers the first 100 apps — larger accounts may have more."));
    }
    if broken.is_empty() && orphaned.is_empty() {
        print_success("No apps need attention.");
        return Ok(());
    }
    let describe = |app: &Value| {
        format!(
            "{} {}",
            js_template(app.get("displayName")),
            dim(&format!("({}, {})", short_id(&text(app, "id").unwrap_or_default()), js_template(app.get("appType"))))
        )
    };
    if !broken.is_empty() {
        println!("{}", bold("\nBroken access:"));
        for app in &broken {
            let reason = js_if(app.get("accessStatusReason")).map(|r| dim(&format!(": {r}"))).unwrap_or_default();
            println!("  {} {} — {}{reason}", red("✗"), describe(app), access_status_label(app));
        }
    }
    if !orphaned.is_empty() {
        println!("{}", bold("\nConnected but unused:"));
        for (app, missing) in &orphaned {
            println!("  {} {} — no {missing} configured", yellow("!"), describe(app));
        }
    }
    println!();
    Ok(())
}

pub async fn get(ctx: &Ctx, app_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let app_id = resolve(&client, Listing::Apps, app_id).await;
    let res = run_action("Fetching app...", client.get(&format!("/apps/{app_id}"), &[])).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    for line in app_lines(payload(&res)) {
        println!("{line}");
    }
    Ok(())
}

/// The `apps get` view, with the TS CLI's `${…}` rendering of missing (`undefined`) and null fields.
fn app_lines(app: &Value) -> Vec<String> {
    let field = |key: &str| app.get(key);
    let t = |key: &str| text(app, key);
    // `(app.permissions ?? []).join(', ') || '—'`
    let permissions = match field("permissions") {
        Some(Value::Array(items)) => js_join(items, ", "),
        _ => String::new(),
    };
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            bold(&js_template(field("displayName"))),
            dim(&format!("({})", js_template(field("appType"))))
        ),
        String::new(),
        format!("  ID:          {}", js_template(field("id"))),
        format!("  Type:        {}", js_template(field("appType"))),
        format!("  Capability:  {}", role_label(app)),
        format!("  Permissions: {}", if permissions.is_empty() { "—".to_string() } else { permissions }),
        format!("  State:       {}", js_color_status(field("state"))),
    ];
    let checked = js_if(field("accessStatusCheckedAt"))
        .map(|at| dim(&format!(" (checked {})", time_ago(Some(&at)))))
        .unwrap_or_default();
    lines.push(format!("  Status:      {}{checked}", access_status_label(app)));
    if let Some(reason) = js_if(field("accessStatusReason")) {
        lines.push(format!("  Reason:      {}", red(&reason)));
    }
    lines.push(format!("  Last Sync:   {}", time_ago(t("lastSyncAt").as_deref())));
    lines.push(format!("  Created:     {}", time_ago(t("createdAt").as_deref())));
    if let Some(error) = js_if(field("errorMessage")) {
        lines.push(format!("  Error:       {}", red(&error)));
    }
    // `typeof count === 'number' && count > 0`: a count sent as a string is not shown.
    if let Some(count) =
        field("consecutiveFailureCount").filter(|v| matches!(v, Value::Number(_)) && js_greater_than(v, 0.0))
    {
        lines.push(format!("  Failures:    {} consecutive", red(&crate::output::js_string(count))));
    }
    lines
}

// ── create / update ────────────────────────────────────────────────────────

pub struct CreateArgs {
    pub app_type: String,
    pub name: String,
    pub role: String,
    pub permissions: Option<String>,
    pub credentials_file: Option<String>,
    pub config_file: Option<String>,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn create(ctx: &Ctx, args: CreateArgs) -> Result<()> {
    // As in the TS CLI, a missing API key only stops the command when a
    // request needs it: the role lookup falls back to the local map and the
    // files are read first, credentials before config.
    let client = ctx.client();
    // Explicit --permissions wins; otherwise resolve --role via the catalog.
    let permissions: Vec<String> = match args.permissions.as_deref().filter(|p| !p.is_empty()) {
        Some(perms) => role_list(perms),
        None => resolve_permissions_for_roles(client.as_ref().ok(), &args.app_type, &role_list(&args.role)).await?,
    };
    let read_config = || match args.config_file.as_deref().filter(|p| !p.is_empty()) {
        Some(path) => read_json_file(path).map(Some),
        None => Ok(None),
    };
    let mode = resolve_output_mode(args.json, args.output.as_deref());

    // No credentials → the browser-assisted OAuth flow.
    let Some(credentials_file) = args.credentials_file.as_deref().filter(|p| !p.is_empty()) else {
        let config = read_config()?;
        let client = client?;
        let app_id = run_browser_oauth(&client, &args.app_type, &args.name, &permissions, config).await?;
        match mode {
            OutputMode::Json => print_json(&json!({ "data": { "id": app_id } })),
            OutputMode::Field => println!("{app_id}"),
            OutputMode::Table => print_success(&format!("App {} created via OAuth.", short_id(&app_id))),
        }
        return Ok(());
    };

    let credentials = read_json_file(credentials_file)?;
    let config = read_config()?;
    let client = client?;
    let mut body = Map::new();
    body.insert("appType".into(), json!(args.app_type));
    body.insert("displayName".into(), json!(args.name));
    body.insert("permissions".into(), json!(permissions));
    body.insert("credentials".into(), credentials);
    if let Some(config) = config {
        body.insert("config".into(), config);
    }
    let res = run_action("Creating app...", client.post("/apps", Some(Value::Object(body)))).await?;
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let app = payload(&res);
    if !js_truthy(app) {
        print_success("App created.");
        return Ok(());
    }
    if mode == OutputMode::Field {
        println!("{}", js_template(app.get("id")));
        return Ok(());
    }
    print_success(&format!(
        "App {} ({}) created.",
        bold(&js_template(app.get("displayName"))),
        short_id(&text(app, "id").unwrap_or_default())
    ));
    print_label("Type", app.get("appType"));
    print_label("Capability", Some(&json!(role_label(app))));
    print_status_label("State", app.get("state"));
    Ok(())
}

pub struct UpdateArgs {
    pub name: Option<String>,
    pub role: Option<String>,
    pub permissions: Option<String>,
    pub credentials_file: Option<String>,
    pub config_file: Option<String>,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn update(ctx: &Ctx, app_id: &str, args: UpdateArgs) -> Result<()> {
    // A short ID is looked up once, with the first request that needs it (VE-3831).
    let mut resolved: Option<String> = None;
    let mut body = Map::new();
    if let Some(name) = args.name.filter(|n| !n.is_empty()) {
        body.insert("displayName".into(), json!(name));
    }
    if let Some(perms) = args.permissions.as_deref().filter(|p| !p.is_empty()) {
        body.insert("permissions".into(), Value::Array(split_list(perms)));
    } else if let Some(role) = args.role.as_deref().filter(|r| !r.is_empty()) {
        // Role defaults are per platform: look up the app's type.
        let client = ctx.client()?;
        let id = resolve(&client, Listing::Apps, app_id).await;
        let current = client.get(&format!("/apps/{id}"), &[]).await?;
        resolved = Some(id);
        let app_type = text(payload(&current), "appType").unwrap_or_default();
        body.insert(
            "permissions".into(),
            json!(resolve_permissions_for_roles(Some(&client), &app_type, &role_list(role)).await?),
        );
    }
    if let Some(path) = args.credentials_file.as_deref().filter(|p| !p.is_empty()) {
        body.insert("credentials".into(), read_json_file(path)?);
    }
    if let Some(path) = args.config_file.as_deref().filter(|p| !p.is_empty()) {
        body.insert("config".into(), read_json_file(path)?);
    }
    if body.is_empty() {
        bail!("Nothing to update — pass at least one flag.");
    }
    let client = ctx.client()?;
    let app_id = match resolved {
        Some(id) => id,
        None => resolve(&client, Listing::Apps, app_id).await,
    };
    let res = run_action("Updating app...", client.patch(&format!("/apps/{app_id}"), Value::Object(body))).await?;
    match resolve_output_mode(args.json, args.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{app_id}"),
        OutputMode::Table => print_success(&format!("App {} updated.", short_id(&app_id))),
    }
    Ok(())
}

// ── browser-assisted OAuth for `apps create` ───────────────────────────────

/// How long a callback connection may take to send its request: Node's HTTP
/// server default (`headersTimeout`), as for `vendo login`.
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(60);
const CALLBACK_METHODS: &str = "POST, GET, OPTIONS";
/// A larger request is not the approval page's: the connection is dropped.
const MAX_CALLBACK_REQUEST: usize = 64 * 1024;

#[derive(Debug, PartialEq)]
pub struct OAuthResult {
    pub status: String,
    pub app_id: Option<String>,
    pub error: Option<String>,
}

/// 1. POST /apps with `oauthAssist`; the API answers with an `authUrl`.
/// 2. Open it; the web drawer POSTs the outcome to our local `/callback`.
/// 3. Whichever comes first wins: the callback or polling the session.
async fn run_browser_oauth(
    client: &Client,
    app_type: &str,
    display_name: &str,
    permissions: &[String],
    config: Option<Value>,
) -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut body = Map::new();
    body.insert("appType".into(), json!(app_type));
    body.insert("displayName".into(), json!(display_name));
    body.insert("permissions".into(), json!(permissions));
    if let Some(config) = config {
        body.insert("config".into(), config);
    }
    body.insert("oauthAssist".into(), json!({ "caller": "cli", "httpCallbackPort": port }));
    let init = run_action("Opening OAuth session...", client.post("/apps", Some(Value::Object(body)))).await?;
    let oauth = payload(&init);
    let (Some(session_id), true) = (text(oauth, "sessionId"), oauth.get("authRequired").is_some()) else {
        bail!("Server returned an unexpected response to the OAuth-assist request.");
    };
    let auth_url = text(oauth, "authUrl").unwrap_or_default();
    println!("\n{}\n{auth_url}\n", bold("Authorize in your browser:"));
    let _ = open::that(&auth_url);

    let settled = run_action("Waiting for authorization...", async {
        Ok::<_, anyhow::Error>(tokio::select! {
            result = wait_for_oauth_callback(&listener) => result,
            result = poll_oauth_session(client, &session_id, Duration::from_secs(2), Duration::from_secs(5 * 60)) => result,
        })
    })
    .await?;
    match settled {
        // `!settled.appId` in TS: an empty ID is not a result.
        OAuthResult { status, app_id: Some(app_id), .. } if status == "completed" && !app_id.is_empty() => Ok(app_id),
        OAuthResult { error, .. } => Err(anyhow!(error.unwrap_or_else(|| "Authorization did not complete".into()))),
    }
}

/// Serve the local `/callback` until the approval page reports the outcome.
/// It POSTs `{status, appId, error}` as JSON from the web app's origin, so the
/// browser first sends a CORS preflight (OPTIONS): that gets its answer and
/// the wait goes on. Only the POST, or a GET that carries `?status=`, settles
/// it; other requests get an error status and the wait goes on. Connections
/// are served side by side, as Node's HTTP server does, so an idle browser
/// preconnect can't hold up the real request.
pub async fn wait_for_oauth_callback(listener: &TcpListener) -> OAuthResult {
    wait_for_oauth_callback_with(listener, CALLBACK_READ_TIMEOUT).await
}

async fn wait_for_oauth_callback_with(listener: &TcpListener, read_timeout: Duration) -> OAuthResult {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((socket, _)) = accepted {
                    connections.spawn(handle_oauth_callback(socket, read_timeout));
                }
            }
            Some(handled) = connections.join_next() => {
                if let Ok(Some(result)) = handled {
                    return result;
                }
            }
        }
    }
}

/// One connection: `Some` only when it carries the outcome.
async fn handle_oauth_callback(mut socket: TcpStream, read_timeout: Duration) -> Option<OAuthResult> {
    let request = tokio::time::timeout(read_timeout, read_request(&mut socket)).await.ok()??;
    let url = match reqwest::Url::parse(&format!("http://127.0.0.1{}", request.target)) {
        Ok(url) if url.path() == "/callback" => url,
        _ => {
            respond(&mut socket, "404 Not Found", &[], "").await;
            return None;
        }
    };
    let query = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    // Any origin is echoed: the listener is one-shot on a random port, and a
    // header-safe value is all we check.
    let mut headers: Vec<(&str, String)> = request
        .origin
        .filter(|origin| !origin.is_empty() && origin.bytes().all(|b| b.is_ascii_graphic()))
        .map(|origin| ("Access-Control-Allow-Origin", origin))
        .into_iter()
        .collect();
    let result = match request.method.as_str() {
        "OPTIONS" => {
            headers.extend([
                ("Access-Control-Allow-Methods", CALLBACK_METHODS.into()),
                ("Access-Control-Allow-Headers", "Content-Type".into()),
                ("Access-Control-Allow-Private-Network", "true".into()),
                ("Access-Control-Max-Age", "600".into()),
            ]);
            respond(&mut socket, "204 No Content", &headers, "").await;
            return None;
        }
        "GET" => match query("status").filter(|status| !status.is_empty()) {
            Some(status) => OAuthResult { status, app_id: query("appId"), error: query("error") },
            None => {
                respond(&mut socket, "400 Bad Request", &headers, "").await;
                return None;
            }
        },
        "POST" => {
            let parsed: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let field = |key: &str| parsed.get(key).and_then(Value::as_str).map(str::to_string);
            OAuthResult {
                status: field("status").or_else(|| query("status")).unwrap_or_else(|| "failed".into()),
                app_id: field("appId"),
                error: field("error"),
            }
        }
        _ => {
            headers.push(("Allow", CALLBACK_METHODS.into()));
            respond(&mut socket, "405 Method Not Allowed", &headers, "").await;
            return None;
        }
    };
    headers.push(("Content-Type", "text/plain".into()));
    respond(&mut socket, "200 OK", &headers, "ok").await;
    Some(result)
}

struct CallbackRequest {
    method: String,
    target: String,
    origin: Option<String>,
    body: Vec<u8>,
}

/// Method, target, `Origin` and body (by Content-Length) of one HTTP request.
async fn read_request(socket: &mut TcpStream) -> Option<CallbackRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > MAX_CALLBACK_REQUEST {
            return None;
        }
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut request_line = head.lines().next()?.split_whitespace();
    let (method, target) = (request_line.next()?.to_string(), request_line.next()?.to_string());
    let header = |name: &str| {
        head.lines()
            .skip(1)
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)))
            .map(|(_, v)| v.trim().to_string())
    };
    let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if length > MAX_CALLBACK_REQUEST {
        return None;
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Some(CallbackRequest { method, target, origin: header("origin"), body })
}

async fn respond(socket: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &str) {
    let mut response = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    // A 204 has no body, so no Content-Length either.
    if !status.starts_with("204") {
        response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    response.push_str("Connection: close\r\n\r\n");
    response.push_str(body);
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// Poll `GET /apps/oauth-session/{id}` (a global route) until it settles.
/// Three 404s in a row fail fast; other errors are treated as transient.
pub async fn poll_oauth_session(client: &Client, session_id: &str, every: Duration, deadline: Duration) -> OAuthResult {
    let started = tokio::time::Instant::now();
    let mut not_found = 0;
    while started.elapsed() < deadline {
        tokio::time::sleep(every).await;
        match client.get(&format!("/apps/oauth-session/{session_id}"), &[]).await {
            Ok(res) => {
                not_found = 0;
                let data = payload(&res);
                match text(data, "status").as_deref() {
                    Some("completed") => {
                        return OAuthResult { status: "completed".into(), app_id: text(data, "appId"), error: None };
                    }
                    Some(status @ ("failed" | "cancelled")) => {
                        return OAuthResult { status: status.into(), app_id: None, error: text(data, "error") };
                    }
                    _ => {}
                }
            }
            Err(err) if err.status == 404 => {
                not_found += 1;
                if not_found >= 3 {
                    return OAuthResult {
                        status: "failed".into(),
                        app_id: None,
                        error: Some(format!(
                            "OAuth session {session_id} was not found on the server (GET /apps/oauth-session/{session_id} returned 404 {not_found} times). The session may have expired — run the command again."
                        )),
                    };
                }
            }
            Err(_) => {}
        }
    }
    OAuthResult { status: "failed".into(), app_id: None, error: Some("Timed out after 5 minutes".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    #[test]
    fn role_label_prefers_server_roles_then_permissions() {
        assert_eq!(role_label(&json!({ "roles": ["source", "destination"] })), "source, destination");
        assert_eq!(role_label(&json!({ "roles": [] })), "—");
        assert_eq!(role_label(&json!({ "permissions": ["campaign_toggle"] })), "destination");
        assert_eq!(role_label(&json!({ "permissions": [] })), "—");
    }

    #[test]
    fn access_status_labels() {
        assert!(access_status_label(&json!({ "state": "inactive", "accessStatus": "connected" })).contains("paused"));
        assert!(access_status_label(&json!({ "state": "active" })).contains("not checked"));
        assert!(
            access_status_label(&json!({ "state": "active", "accessStatus": "auth_expired" }))
                .contains("reconnect required")
        );
        assert_eq!(access_status_label(&json!({ "state": "active", "accessStatus": "connected" })), "connected");
    }

    #[test]
    fn legacy_role_grants_never_include_campaign_changes() {
        assert_eq!(
            permissions_from_roles(&["both".into()]),
            ["performance_data", "send_conversions", "sync_audiences"]
        );
        assert_eq!(permissions_from_roles(&[]), ["performance_data"]);
    }

    #[tokio::test]
    async fn roles_resolve_from_catalog_defaults_and_reject_unsupported() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/catalog/webhook"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "data": { "defaultPermissions": { "source": ["performance_data"], "destination": [] } } }),
            ))
            .mount(&server)
            .await;
        let client = Client::new("k".into(), server.uri(), Some("a".into()), false);
        assert_eq!(
            resolve_permissions_for_roles(Some(&client), "webhook", &["source".into()]).await.unwrap(),
            ["performance_data"]
        );
        let err = resolve_permissions_for_roles(Some(&client), "webhook", &["destination".into()]).await.unwrap_err();
        assert!(err.to_string().contains("does not support the requested role(s): destination"));
        // Older server (no catalog entry): legacy local map.
        assert_eq!(
            resolve_permissions_for_roles(Some(&client), "other", &["destination".into()]).await.unwrap(),
            ["send_conversions", "sync_audiences"]
        );
    }

    #[tokio::test]
    async fn oauth_callback_reads_the_posted_outcome() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let waiter = tokio::spawn(async move { wait_for_oauth_callback(&listener).await });
        let http = reqwest::Client::new();
        assert_eq!(http.get(format!("http://127.0.0.1:{port}/other")).send().await.unwrap().status(), 404);
        let res = http
            .post(format!("http://127.0.0.1:{port}/callback"))
            .json(&json!({ "status": "completed", "appId": "app-9" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.text().await.unwrap(), "ok");
        assert_eq!(
            waiter.await.unwrap(),
            OAuthResult { status: "completed".into(), app_id: Some("app-9".into()), error: None }
        );
    }

    const WEB_APP: &str = "https://stg.vendodata.com";

    async fn oauth_callback_server() -> (u16, tokio::task::JoinHandle<OAuthResult>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (port, tokio::spawn(async move { wait_for_oauth_callback(&listener).await }))
    }

    /// The approval page's request, as the web app sends it.
    fn drawer_post(port: u16, body: Value) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/callback?sessionId=s1&status=completed"))
            .header("Origin", WEB_APP)
            .json(&body)
    }

    async fn settled(waiter: tokio::task::JoinHandle<OAuthResult>) -> OAuthResult {
        tokio::time::timeout(Duration::from_secs(5), waiter).await.expect("the callback settles").unwrap()
    }

    /// Lets a callback the server took as the answer finish before checking.
    async fn still_waiting(waiter: &tokio::task::JoinHandle<OAuthResult>) -> bool {
        tokio::time::sleep(Duration::from_millis(50)).await;
        !waiter.is_finished()
    }

    #[tokio::test]
    async fn oauth_callback_answers_the_browser_preflight_and_waits_for_the_post() {
        let (port, waiter) = oauth_callback_server().await;
        let preflight = reqwest::Client::new()
            .request(
                reqwest::Method::OPTIONS,
                format!("http://127.0.0.1:{port}/callback?sessionId=s1&status=completed"),
            )
            .header("Origin", WEB_APP)
            .header("Access-Control-Request-Method", "POST")
            .header("Access-Control-Request-Headers", "content-type")
            .header("Access-Control-Request-Private-Network", "true")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), 204);
        let header = |name: &str| preflight.headers().get(name).map(|v| v.to_str().unwrap().to_string());
        assert_eq!(header("access-control-allow-origin").as_deref(), Some(WEB_APP));
        assert_eq!(header("access-control-allow-methods").as_deref(), Some("POST, GET, OPTIONS"));
        assert_eq!(header("access-control-allow-headers").as_deref(), Some("Content-Type"));
        assert_eq!(header("access-control-allow-private-network").as_deref(), Some("true"));
        assert_eq!(header("access-control-max-age").as_deref(), Some("600"));
        assert!(still_waiting(&waiter).await, "the preflight is not the answer");

        let res = drawer_post(port, json!({ "status": "completed", "appId": "app-9" })).send().await.unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["access-control-allow-origin"], WEB_APP);
        assert_eq!(res.text().await.unwrap(), "ok");
        assert_eq!(
            settled(waiter).await,
            OAuthResult { status: "completed".into(), app_id: Some("app-9".into()), error: None }
        );
    }

    #[tokio::test]
    async fn an_idle_connection_does_not_delay_the_oauth_callback() {
        let (port, waiter) = oauth_callback_server().await;
        // A browser preconnect: connected, never sends a request.
        let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            drawer_post(port, json!({ "status": "completed", "appId": "app-9" })).send(),
        )
        .await
        .expect("the POST is answered while the idle connection stays open")
        .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(settled(waiter).await.app_id.as_deref(), Some("app-9"));
    }

    #[tokio::test]
    async fn idle_oauth_callback_connections_are_closed_after_the_read_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let waiter =
            tokio::spawn(async move { wait_for_oauth_callback_with(&listener, Duration::from_millis(100)).await });
        let mut idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut buf = [0u8; 16];
        let read = tokio::time::timeout(Duration::from_secs(5), idle.read(&mut buf)).await;
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))), "the server closes the idle connection: {read:?}");
        assert!(!waiter.is_finished());
        waiter.abort();
    }

    #[tokio::test]
    async fn a_get_with_a_status_settles_the_oauth_callback() {
        let (port, waiter) = oauth_callback_server().await;
        let res = reqwest::get(format!("http://127.0.0.1:{port}/callback?status=failed&error=x")).await.unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(
            settled(waiter).await,
            OAuthResult { status: "failed".into(), app_id: None, error: Some("x".into()) }
        );
    }

    #[tokio::test]
    async fn requests_that_are_not_the_outcome_keep_the_oauth_callback_waiting() {
        let (port, waiter) = oauth_callback_server().await;
        let http = reqwest::Client::new();
        let url = |path: &str| format!("http://127.0.0.1:{port}{path}");
        assert_eq!(http.get(url("/favicon.ico")).send().await.unwrap().status(), 404);
        assert_eq!(http.get(url("/callback?sessionId=s1")).send().await.unwrap().status(), 400);
        let put = http.put(url("/callback?status=completed")).send().await.unwrap();
        assert_eq!(put.status(), 405);
        assert_eq!(put.headers()["allow"], "POST, GET, OPTIONS");
        assert!(still_waiting(&waiter).await);

        drawer_post(port, json!({ "status": "cancelled" })).send().await.unwrap();
        assert_eq!(settled(waiter).await, OAuthResult { status: "cancelled".into(), app_id: None, error: None });
    }

    #[tokio::test]
    async fn oauth_polling_settles_and_fails_fast_on_404s() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/apps/oauth-session/s1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": { "status": "completed", "appId": "app-1" } })),
            )
            .mount(&server)
            .await;
        let client = Client::new("k".into(), server.uri(), Some("a".into()), false);
        let done = poll_oauth_session(&client, "s1", Duration::from_millis(1), Duration::from_secs(5)).await;
        assert_eq!(done.app_id.as_deref(), Some("app-1"));
        let gone = poll_oauth_session(&client, "missing", Duration::from_millis(1), Duration::from_secs(5)).await;
        assert_eq!(gone.status, "failed");
        assert!(gone.error.unwrap().contains("returned 404 3 times"));
    }

    async fn catalog_with(app_type: &str, response: ResponseTemplate) -> (MockServer, Client) {
        let server = MockServer::start().await;
        Mock::given(path(format!("/api/v1/catalog/{app_type}"))).respond_with(response).mount(&server).await;
        let client = Client::new("k".into(), server.uri(), Some("a".into()), false);
        (server, client)
    }

    #[tokio::test]
    async fn a_catalog_error_saying_does_not_support_stops_the_command() {
        let refusal = json!({ "error": { "message": "App type x does not support role destination" } });
        let (_server, client) = catalog_with("x", ResponseTemplate::new(400).set_body_json(refusal)).await;
        let err = resolve_permissions_for_roles(Some(&client), "x", &["destination".into()]).await.unwrap_err();
        assert_eq!(err.to_string(), "App type x does not support role destination");
        // Any other failure still falls back to the local map.
        let (_server, client) = catalog_with("y", ResponseTemplate::new(500)).await;
        let perms = resolve_permissions_for_roles(Some(&client), "y", &["destination".into()]).await.unwrap();
        assert_eq!(perms, ["send_conversions", "sync_audiences"]);
    }

    #[tokio::test]
    async fn default_permissions_follow_javascript_truthiness_and_iteration() {
        let defaults =
            |value: Value| ResponseTemplate::new(200).set_body_json(json!({ "data": { "defaultPermissions": value } }));
        for refused in [json!([]), json!("abc"), json!(true), json!(5), json!({})] {
            let (_server, client) = catalog_with("t", defaults(refused.clone())).await;
            let err = resolve_permissions_for_roles(Some(&client), "t", &["source".into()]).await.unwrap_err();
            assert!(err.to_string().contains("does not support the requested role(s): source"), "{refused}: {err}");
        }
        for legacy in [json!(0), json!(""), json!(false), json!(null), json!({ "source": true })] {
            let (_server, client) = catalog_with("t", defaults(legacy.clone())).await;
            let perms = resolve_permissions_for_roles(Some(&client), "t", &["source".into()]).await.unwrap();
            assert_eq!(perms, ["performance_data"], "{legacy}");
        }
        // A string is iterated character by character, as `for…of` does.
        let (_server, client) = catalog_with("t", defaults(json!({ "source": "ab" }))).await;
        assert_eq!(resolve_permissions_for_roles(Some(&client), "t", &["source".into()]).await.unwrap(), ["a", "b"]);
    }

    #[tokio::test]
    async fn no_client_means_the_local_map() {
        assert_eq!(resolve_permissions_for_roles(None, "t", &["source".into()]).await.unwrap(), ["performance_data"]);
    }

    #[test]
    fn get_prints_missing_and_null_fields_like_the_ts_cli() {
        // Expected lines from the TS CLI 0.3.1 run against a local stub with the same body (VE-3728).
        let app = json!({
            "id": "app-1", "displayName": null, "permissions": [null, "x"], "roles": ["source", null], "state": null,
            "accessStatus": 0, "accessStatusReason": 0, "accessStatusCheckedAt": "", "errorMessage": false,
            "consecutiveFailureCount": "3",
        });
        assert_eq!(
            app_lines(&app),
            [
                "",
                "null (undefined)",
                "",
                "  ID:          app-1",
                "  Type:        undefined",
                "  Capability:  source, ",
                "  Permissions: , x",
                "  State:       null",
                "  Status:      not checked",
                "  Last Sync:   —",
                "  Created:     —",
            ]
        );
        let app = json!({
            "id": "app-3", "displayName": 7, "appType": null, "state": 5, "accessStatus": 9, "roles": [],
            "consecutiveFailureCount": 1.5e300, "accessStatusReason": "", "errorMessage": 0,
        });
        assert_eq!(
            app_lines(&app),
            [
                "",
                "7 (null)",
                "",
                "  ID:          app-3",
                "  Type:        null",
                "  Capability:  —",
                "  Permissions: —",
                "  State:       5",
                "  Status:      9",
                "  Last Sync:   —",
                "  Created:     —",
                "  Failures:    1.5e+300 consecutive",
            ]
        );
    }
}

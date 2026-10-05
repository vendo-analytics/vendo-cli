//! `vendo login` (port of `src/commands/login.ts`): headless with
//! `--api-key` + `--account`, or the browser flow against the web app's
//! `/cli-auth` page, which redirects to
//! `http://127.0.0.1:<port>/callback?key=&account=&account_id=&state=`.
//! The server requires `state` to match `^[a-f0-9]{32}$`.

use std::{io::Write, time::Duration};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use uuid::Uuid;

use crate::{
    config::DEFAULT_BASE_URL,
    context::Ctx,
    identity::{IdentityError, fetch_identity},
    output::{bold, dim, green, run_action},
    update_check,
};

const AUTH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a callback connection may take to send its request line and
/// headers: Node's HTTP server default (`headersTimeout`).
const READ_TIMEOUT: Duration = Duration::from_secs(60);

pub struct LoginResult {
    pub account: String,
    pub account_id: Option<String>,
}

pub async fn run(
    ctx: &Ctx,
    api_key: Option<String>,
    account: Option<String>,
    env: Option<String>,
    base_url: Option<String>,
) -> Result<()> {
    // Resolve the instance first so a typo fails before a browser opens at
    // the wrong one (VE-1563).
    let base_url = ctx.store.resolve_login_base_url(env.as_deref(), base_url.as_deref())?;

    let result = if let Some((api_key, account_id)) = headless_credentials(api_key, account)? {
        let identity = run_action("Validating credentials...", async {
            fetch_identity(&api_key, &account_id, &base_url, ctx.debug).await.map_err(|err| match err {
                IdentityError::Http { status, .. } => {
                    anyhow!("Credential validation failed (HTTP {status}). Check your API key and account ID.")
                }
                other => anyhow!(other),
            })
        })
        .await?;
        let name = identity.me.account_slug.or(identity.me.account_name).unwrap_or_else(|| account_id.clone());
        ctx.store.save_profile(&name, profile(&api_key, Some(&account_id), &base_url))?;
        LoginResult { account: name, account_id: Some(account_id) }
    } else {
        run_browser_login(ctx, &base_url).await?
    };

    print_login_success(&result);
    Ok(())
}

/// `--api-key` and `--account` read as the TS CLI did (`opts.apiKey ||
/// opts.account`, so empty is unset): both for a headless login, neither for
/// the browser flow, anything else is an error.
pub fn headless_credentials(api_key: Option<String>, account: Option<String>) -> Result<Option<(String, String)>> {
    match (api_key.filter(|k| !k.is_empty()), account.filter(|a| !a.is_empty())) {
        (None, None) => Ok(None),
        (Some(api_key), Some(account)) => Ok(Some((api_key, account))),
        _ => bail!(
            "Both --api-key and --account are required for headless login.\n{}",
            dim("  Example: vendo login --api-key <key> --account <id>")
        ),
    }
}

/// The browser flow, shared with `vendo init`.
pub async fn run_browser_login(ctx: &Ctx, base_url: &str) -> Result<LoginResult> {
    update_check::check(&ctx.update_cache_path()).await;
    let callback = browser_flow(base_url).await?;
    ctx.store.save_profile(&callback.account, profile(&callback.key, callback.account_id.as_deref(), base_url))?;
    Ok(LoginResult { account: callback.account, account_id: callback.account_id })
}

pub fn print_login_success(result: &LoginResult) {
    for line in login_success_lines(result) {
        println!("{line}");
    }
}

fn login_success_lines(result: &LoginResult) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        format!("{} Authenticated as {}", green("Done:"), bold(&result.account)),
        dim(&format!("Active profile: {}", result.account)),
    ];
    // `if (result.accountId)`: an empty ID is saved but not announced.
    if let Some(id) = result.account_id.as_deref().filter(|id| !id.is_empty()) {
        lines.push(dim(&format!("Account ID saved: {id}")));
    }
    lines.extend([String::new(), bold("Next steps"), "  vendo whoami".into(), "  vendo status".into()]);
    lines.push("  vendo doctor".into());
    lines
}

fn profile(api_key: &str, account_id: Option<&str>, base_url: &str) -> Map<String, Value> {
    let mut profile = Map::new();
    profile.insert("apiKey".into(), Value::String(api_key.to_string()));
    if let Some(account_id) = account_id {
        profile.insert("accountId".into(), Value::String(account_id.to_string()));
    }
    if base_url != DEFAULT_BASE_URL {
        profile.insert("baseUrl".into(), Value::String(base_url.to_string()));
    }
    profile
}

#[derive(Debug, PartialEq)]
pub struct Callback {
    pub key: String,
    pub account: String,
    pub account_id: Option<String>,
}

async fn browser_flow(base_url: &str) -> Result<Callback> {
    let state = Uuid::new_v4().simple().to_string(); // 32 lowercase hex characters
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let auth_url = format!("{base_url}/cli-auth?port={port}&state={state}");

    println!("Login at:");
    println!("{auth_url}");
    print!("Press ENTER to open in the browser...");
    let _ = std::io::stdout().flush();
    let url = auth_url.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        if matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0) && open::that(&url).is_err() {
            println!("{}", dim("Could not open browser automatically. Please visit the URL above."));
        }
    });
    println!();
    println!("{}", dim("Waiting for authorization..."));

    wait_for_callback(&listener, &state, AUTH_TIMEOUT).await
}

pub async fn wait_for_callback(listener: &TcpListener, state: &str, timeout: Duration) -> Result<Callback> {
    tokio::time::timeout(timeout, accept_callback(listener, state))
        .await
        .map_err(|_| anyhow!("Authentication timed out after 5 minutes"))?
}

/// Serve the local callback until a `/callback` request arrives; other paths
/// get a 404 and the flow keeps waiting. Connections are served side by side,
/// as Node's HTTP server does, so an idle browser preconnect can't hold up
/// the real request.
pub async fn accept_callback(listener: &TcpListener, state: &str) -> Result<Callback> {
    accept_callback_with(listener, state, READ_TIMEOUT).await
}

pub async fn accept_callback_with(listener: &TcpListener, state: &str, read_timeout: Duration) -> Result<Callback> {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((socket, _)) = accepted {
                    connections.spawn(handle_connection(socket, state.to_string(), read_timeout));
                }
            }
            Some(handled) = connections.join_next() => {
                if let Ok(Some(outcome)) = handled {
                    return outcome;
                }
            }
        }
    }
}

/// One connection: `None` unless it was a `/callback` request, which ends the
/// flow either way. Like `URLSearchParams.get`, the first value of a repeated
/// parameter counts.
async fn handle_connection(mut socket: TcpStream, state: String, read_timeout: Duration) -> Option<Result<Callback>> {
    let target = tokio::time::timeout(read_timeout, read_request_target(&mut socket)).await.ok()??;
    let url = match reqwest::Url::parse(&format!("http://localhost{target}")) {
        Ok(url) if url.path() == "/callback" => url,
        _ => {
            respond(&mut socket, "404 Not Found", "").await;
            return None;
        }
    };
    let param = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    let given = |key: &str| param(key).filter(|v| !v.is_empty());

    if let Some(error) = given("error") {
        respond_page(&mut socket, "Authentication Failed", &error).await;
        return Some(Err(anyhow!(error)));
    }
    if param("state").as_deref() != Some(state.as_str()) {
        respond_page(&mut socket, "Invalid Request", "State mismatch. Please try again.").await;
        return Some(Err(anyhow!("State mismatch — possible CSRF attack")));
    }
    let (Some(key), Some(account)) = (given("key"), given("account")) else {
        respond_page(&mut socket, "Missing Credentials", "Please try again.").await;
        return Some(Err(anyhow!("Missing key or account in callback")));
    };
    respond_page(&mut socket, "Authenticated", "You can close this window and return to the terminal.").await;
    // `accountId ?? undefined`: an empty account_id is kept (and saved).
    Some(Ok(Callback { key, account, account_id: param("account_id") }))
}

async fn read_request_target(socket: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    head.lines().next()?.split_whitespace().nth(1).map(str::to_string)
}

async fn respond(socket: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

async fn respond_page(socket: &mut TcpStream, title: &str, message: &str) {
    let body = format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Vendo CLI</title>
<style>
  body {{ font-family: system-ui, sans-serif; display: flex; align-items: center;
    justify-content: center; min-height: 100vh; margin: 0; background: #fafafa; }}
  .card {{ background: white; border-radius: 12px; padding: 2rem 3rem;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1); text-align: center; }}
  h1 {{ margin: 0 0 0.5rem; font-size: 1.5rem; }}
  p {{ color: #666; margin: 0; }}
</style>
</head>
<body><div class="card"><h1>{}</h1><p>{}</p></div></body>
</html>"#,
        escape_html(title),
        escape_html(message)
    );
    respond(socket, "200 OK", &body).await;
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn hit(port: u16, path: &str) -> (u16, String) {
        let res = reqwest::get(format!("http://127.0.0.1:{port}{path}")).await.unwrap();
        (res.status().as_u16(), res.text().await.unwrap())
    }

    async fn serve() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[tokio::test]
    async fn success_returns_credentials_after_ignoring_other_paths() {
        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        assert_eq!(hit(port, "/favicon.ico").await.0, 404);
        let (status, body) = hit(port, "/callback?key=k1&account=acme&account_id=a-1&state=abc").await;
        assert_eq!(status, 200);
        assert!(body.contains("<h1>Authenticated</h1>"));
        assert_eq!(
            server.await.unwrap().unwrap(),
            Callback { key: "k1".into(), account: "acme".into(), account_id: Some("a-1".into()) }
        );
    }

    #[tokio::test]
    async fn wrong_state_is_rejected() {
        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        assert!(hit(port, "/callback?key=k&account=a&state=nope").await.1.contains("Invalid Request"));
        assert_eq!(server.await.unwrap().unwrap_err().to_string(), "State mismatch — possible CSRF attack");
    }

    #[tokio::test]
    async fn server_error_is_escaped_and_returned() {
        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        let body = hit(port, "/callback?error=Access%20denied%20%3Cscript%3E").await.1;
        assert!(body.contains("<p>Access denied &lt;script&gt;</p>"));
        assert_eq!(server.await.unwrap().unwrap_err().to_string(), "Access denied <script>");
    }

    #[tokio::test]
    async fn missing_key_or_account_is_rejected() {
        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        assert!(hit(port, "/callback?key=&account=a&state=abc").await.1.contains("Missing Credentials"));
        assert_eq!(server.await.unwrap().unwrap_err().to_string(), "Missing key or account in callback");
    }

    #[tokio::test]
    async fn no_callback_times_out() {
        let (listener, _port) = serve().await;
        let err = wait_for_callback(&listener, "abc", Duration::from_millis(50)).await.unwrap_err();
        assert_eq!(err.to_string(), "Authentication timed out after 5 minutes");
    }

    #[test]
    fn state_is_32_lowercase_hex_characters() {
        let state = Uuid::new_v4().simple().to_string();
        assert_eq!(state.len(), 32);
        assert!(state.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn saved_profile_omits_default_base_url_and_missing_account() {
        let p = profile("k", None, DEFAULT_BASE_URL);
        assert_eq!(Value::Object(p), serde_json::json!({ "apiKey": "k" }));
        let p = profile("k", Some("a"), "https://stg.vendodata.com");
        assert_eq!(
            Value::Object(p),
            serde_json::json!({ "apiKey": "k", "accountId": "a", "baseUrl": "https://stg.vendodata.com" })
        );
    }

    #[test]
    fn headless_login_reads_its_flags_with_javascript_truthiness() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(headless_credentials(s("k"), s("a")).unwrap(), Some(("k".into(), "a".into())));
        for (key, account) in [(s("k"), s("")), (s(""), s("a")), (s("k"), None), (None, s("a"))] {
            let err = headless_credentials(key.clone(), account.clone()).unwrap_err().to_string();
            assert!(err.starts_with("Both --api-key and --account are required"), "{key:?} {account:?}: {err}");
        }
        // Both empty: the browser flow, as in the TS CLI.
        assert_eq!(headless_credentials(s(""), s("")).unwrap(), None);
        assert_eq!(headless_credentials(None, None).unwrap(), None);
    }

    #[tokio::test]
    async fn an_idle_connection_does_not_block_the_callback() {
        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        // A browser preconnect: connected, never sends a request.
        let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (status, _) = hit(port, "/callback?key=k1&account=acme&state=abc").await;
        assert_eq!(status, 200);
        assert_eq!(server.await.unwrap().unwrap().key, "k1");
    }

    #[tokio::test]
    async fn idle_connections_are_closed_after_the_read_timeout() {
        let (listener, port) = serve().await;
        let server =
            tokio::spawn(async move { accept_callback_with(&listener, "abc", Duration::from_millis(100)).await });
        let mut idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut buf = [0u8; 16];
        let read = tokio::time::timeout(Duration::from_secs(5), idle.read(&mut buf)).await;
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))), "the server closes the idle connection: {read:?}");
        server.abort();
    }

    #[tokio::test]
    async fn the_first_value_of_a_parameter_counts_and_an_empty_account_id_is_kept() {
        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        let (status, _) = hit(port, "/callback?key=k1&key=k2&account=acme&account=other&account_id=&state=abc").await;
        assert_eq!(status, 200);
        assert_eq!(
            server.await.unwrap().unwrap(),
            Callback { key: "k1".into(), account: "acme".into(), account_id: Some(String::new()) }
        );

        let (listener, port) = serve().await;
        let server = tokio::spawn(async move { accept_callback(&listener, "abc").await });
        assert!(hit(port, "/callback?key=k&account=a&state=nope&state=abc").await.1.contains("Invalid Request"));
        assert_eq!(server.await.unwrap().unwrap_err().to_string(), "State mismatch — possible CSRF attack");
    }

    #[test]
    fn an_empty_account_id_is_saved_but_not_announced() {
        let p = profile("k", Some(""), DEFAULT_BASE_URL);
        assert_eq!(Value::Object(p), serde_json::json!({ "apiKey": "k", "accountId": "" }));
        let lines = |id: Option<&str>| {
            login_success_lines(&LoginResult { account: "acme".into(), account_id: id.map(Into::into) }).join("\n")
        };
        assert!(!lines(Some("")).contains("Account ID saved"));
        assert!(lines(Some("a-1")).contains("Account ID saved: a-1"));
    }
}

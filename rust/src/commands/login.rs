//! `vendo login` (port of `src/commands/login.ts` and `init.ts`, one command
//! since CLI 1.1, VE-3825; `vendo init` is its hidden alias). It signs in when
//! there is no working key, checks the key with `/me` and prints the setup
//! summary:
//!
//! - `--api-key` + `--account`: checks that key and saves it, never a browser.
//! - A key it already has (the selected profile's, or `VENDO_API_KEY`) is
//!   checked and kept. It signs in through the browser instead when there is
//!   no key, with `--force`, when `--env`/`--base-url` name another instance,
//!   or when `/me` rejects the key (401/403). Any other failed check (no
//!   answer, a server error) creates no key: login fails and changes nothing.
//! - With `VENDO_API_KEY` set it never opens a browser: where it would, it
//!   stops with an error instead.
//!
//! The browser flow goes through the web app's `/cli-auth` page, which creates
//! an API key and redirects to
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
    config::{DEFAULT_BASE_URL, EffectiveConfig, Source},
    context::Ctx,
    identity::{IdentityError, fetch_identity},
    output::{bold, dim, green, print_success, run_action, yellow},
    update_check,
};

const AUTH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a callback connection may take to send its request line and
/// headers: Node's HTTP server default (`headersTimeout`).
const READ_TIMEOUT: Duration = Duration::from_secs(60);

pub struct LoginArgs {
    pub api_key: Option<String>,
    pub account: Option<String>,
    pub env: Option<String>,
    pub base_url: Option<String>,
    /// Sign in through the browser even when the key it has works.
    pub force: bool,
}

pub async fn run(ctx: &Ctx, args: LoginArgs) -> Result<()> {
    // Resolve the instance first so a typo fails before a browser opens at
    // the wrong one (VE-1563).
    let base_url = ctx.store.resolve_login_base_url(args.env.as_deref(), args.base_url.as_deref())?;

    if let Some((api_key, account_id)) = headless_credentials(args.api_key, args.account)? {
        let identity = run_action("Validating credentials...", async {
            fetch_identity(&api_key, &account_id, &base_url, ctx.debug).await.map_err(|err| match err {
                IdentityError::Http { status, .. } => {
                    anyhow!("Credential validation failed (HTTP {status}). Check your API key and account ID.")
                }
                other => anyhow!(other),
            })
        })
        .await?;
        let verified = Check::Verified(identity.me.display_name().to_string());
        let name = identity.me.account_slug.or(identity.me.account_name).unwrap_or_else(|| account_id.clone());
        ctx.store.save_profile(&name, profile(&api_key, Some(&account_id), &base_url))?;
        let summary = Summary { profile: Some(name), base_url, account_id: Some(account_id) };
        return finish(&summary, verified, NOTHING_CHANGED);
    }

    let config = ctx.effective();
    let from_env = config.api_key_source == Source::Env;
    let sign_in = match &config.api_key {
        None => SignIn::NoKey,
        Some(_) if args.force => SignIn::Force,
        Some(_) if !same_instance(&config.base_url, &base_url) => SignIn::OtherInstance,
        Some(key) => {
            let check = check_key(ctx, key, config.account_id.as_deref(), &config.base_url).await;
            match check.rejected() {
                Some(status) => SignIn::Rejected(status),
                None => {
                    println!("{}", dim(&using_existing(&config)));
                    let summary = Summary {
                        profile: config.selected_profile.clone(),
                        base_url: config.base_url.clone(),
                        account_id: config.account_id.clone(),
                    };
                    return finish(&summary, check, NOTHING_CHANGED);
                }
            }
        }
    };
    if from_env {
        bail!(sign_in.refusal(&config, &base_url));
    }
    println!("{}", dim(&sign_in.announcement(&config, &base_url)));
    let signed_in = run_browser_login(ctx, &base_url).await?;
    let account_id = signed_in.account_id.filter(|id| !id.is_empty());
    let check = check_key(ctx, &signed_in.key, account_id.as_deref(), &base_url).await;
    let saved = format!("The new key is saved in profile {}: run `vendo whoami` to check it again.", signed_in.account);
    let summary = Summary { profile: Some(signed_in.account), base_url, account_id };
    finish(&summary, check, &saved)
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

/// Why login signs in through the browser instead of using the key it has.
enum SignIn {
    NoKey,
    Force,
    /// `--env`/`--base-url` name an instance the key is not used with.
    OtherInstance,
    /// `/me` refused the key with this status.
    Rejected(u16),
}

impl SignIn {
    /// What login prints before the browser flow.
    fn announcement(&self, config: &EffectiveConfig, base_url: &str) -> String {
        let profile = config.selected_profile.as_deref().unwrap_or_default();
        match self {
            SignIn::NoKey => "No API key found. Starting browser login...".into(),
            SignIn::Force => "Signing in again (--force). Starting browser login...".into(),
            SignIn::OtherInstance => {
                format!("Profile {profile} is for {}. Starting browser login for {base_url}...", config.base_url)
            }
            SignIn::Rejected(status) => {
                format!("The API key in profile {profile} was rejected (HTTP {status}). Starting browser login...")
            }
        }
    }

    /// The error when the key is in `VENDO_API_KEY`: headless, so no browser.
    fn refusal(&self, config: &EffectiveConfig, base_url: &str) -> String {
        let no_browser = "`vendo login` does not open a browser while VENDO_API_KEY is set";
        match self {
            SignIn::Rejected(status) => format!(
                "The API key in VENDO_API_KEY was rejected (HTTP {status}). {no_browser}: set a working key, or unset it to sign in through the browser."
            ),
            SignIn::OtherInstance => format!(
                "VENDO_API_KEY is used with {}, not {base_url}. {no_browser}: unset it to sign in through the browser.",
                config.base_url
            ),
            SignIn::NoKey | SignIn::Force => format!("{no_browser}: unset it to sign in through the browser."),
        }
    }
}

fn using_existing(config: &EffectiveConfig) -> String {
    match (config.api_key_source, config.selected_profile.as_deref()) {
        (Source::Env, _) => "Using the API key in VENDO_API_KEY.".into(),
        (_, profile) => format!(
            "Using existing profile {}. Run `vendo login --force` to sign in again.",
            profile.unwrap_or_default()
        ),
    }
}

/// The same instance: `--base-url` is saved as its origin, a profile's URL as typed.
fn same_instance(a: &str, b: &str) -> bool {
    let origin = |url: &str| reqwest::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or(url.into());
    origin(a) == origin(b)
}

/// What checking a key with `/me` showed.
#[derive(Debug)]
enum Check {
    /// Accepted; the account's display name.
    Verified(String),
    /// No account ID to check it with (`/me` needs one).
    Incomplete,
    Failed(IdentityError),
}

impl Check {
    /// The status when the API refused the key itself, as opposed to not answering.
    fn rejected(&self) -> Option<u16> {
        match self {
            Check::Failed(IdentityError::Http { status: status @ (401 | 403), .. }) => Some(*status),
            _ => None,
        }
    }
}

async fn check_key(ctx: &Ctx, api_key: &str, account_id: Option<&str>, base_url: &str) -> Check {
    let Some(account_id) = account_id.filter(|id| !id.is_empty()) else { return Check::Incomplete };
    match run_action("Checking your API key...", fetch_identity(api_key, account_id, base_url, ctx.debug)).await {
        Ok(identity) => Check::Verified(identity.me.display_name().to_string()),
        Err(err) => Check::Failed(err),
    }
}

const NOTHING_CHANGED: &str =
    "Nothing was changed: check your connection (`vendo doctor`) and run `vendo login` again.";

/// What the summary shows: the profile the key is in, its instance and account.
struct Summary {
    profile: Option<String>,
    base_url: String,
    account_id: Option<String>,
}

/// Print the setup summary and, when the check did not fail, the next steps.
/// A failed check is an error that ends with `unverified`.
fn finish(summary: &Summary, check: Check, unverified: &str) -> Result<()> {
    for line in summary_lines(summary, &check) {
        println!("{line}");
    }
    if let Check::Failed(err) = check {
        bail!("Could not verify the API key: {}. {unverified}", failure_reason(&err));
    }
    print_success("Vendo CLI setup complete.");
    Ok(())
}

fn summary_lines(summary: &Summary, check: &Check) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        bold("Setup summary"),
        format!("  Profile:     {}", summary.profile.clone().unwrap_or_else(|| dim("none selected"))),
        format!("  Base URL:    {}", summary.base_url),
        format!("  Account ID:  {}", summary.account_id.clone().unwrap_or_else(|| dim("missing"))),
        match check {
            Check::Verified(name) => format!("  Auth:        {} as {name}", green("verified")),
            Check::Failed(_) => format!("  Auth:        {} (API check failed)", yellow("not verified")),
            Check::Incomplete => format!("  Auth:        {} (account ID still required)", yellow("incomplete")),
        },
    ];
    if matches!(check, Check::Failed(_)) {
        return lines;
    }
    lines.extend([String::new(), bold("Next steps")]);
    lines.extend(["  vendo doctor", "  vendo whoami", "  vendo status"].map(String::from));
    if summary.account_id.is_none() {
        lines.push(String::new());
        lines.push(dim(
            "Set an account explicitly with `vendo profile set --account <account-id>` if your login flow did not provide one.",
        ));
    }
    lines
}

/// Why `/me` did not confirm the key: the status the API answered, or why there was no answer.
fn failure_reason(err: &IdentityError) -> String {
    match err {
        IdentityError::Http { status, status_text } if !status_text.is_empty() => {
            format!("HTTP {status} {status_text}")
        }
        IdentityError::Http { status, .. } => format!("HTTP {status}"),
        IdentityError::Transport(err) => err.message.clone(),
        IdentityError::Invalid(message) => message.clone(),
    }
}

/// The browser flow: the key it signed in with, saved as the active profile.
async fn run_browser_login(ctx: &Ctx, base_url: &str) -> Result<Callback> {
    update_check::check(&ctx.update_cache_path()).await;
    let callback = browser_flow(base_url).await?;
    ctx.store.save_profile(&callback.account, profile(&callback.key, callback.account_id.as_deref(), base_url))?;
    Ok(callback)
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
    fn an_empty_account_id_is_saved_but_the_summary_asks_for_one() {
        let p = profile("k", Some(""), DEFAULT_BASE_URL);
        assert_eq!(Value::Object(p), serde_json::json!({ "apiKey": "k", "accountId": "" }));
        // The flow reads an empty ID as none, as `if (result.accountId)` did.
        let summary = |id: Option<&str>| Summary {
            profile: Some("acme".into()),
            base_url: DEFAULT_BASE_URL.into(),
            account_id: id.map(Into::into),
        };
        let lines = summary_lines(&summary(None), &Check::Incomplete).join("\n");
        assert!(
            lines.contains("account ID still required") && lines.contains("vendo profile set --account"),
            "{lines}"
        );
        let lines = summary_lines(&summary(Some("a-1")), &Check::Verified("Acme".into())).join("\n");
        assert!(lines.contains("Account ID:  a-1") && lines.contains("as Acme"), "{lines}");
        assert!(lines.contains("Next steps") && !lines.contains("vendo profile set"), "{lines}");
    }

    #[test]
    fn a_failed_check_ends_the_summary_at_the_auth_line() {
        let summary = Summary { profile: None, base_url: DEFAULT_BASE_URL.into(), account_id: Some("a".into()) };
        let lines = summary_lines(&summary, &Check::Failed(IdentityError::Invalid("x".into())));
        assert!(lines.last().unwrap().contains("API check failed"), "{lines:?}");
        assert!(lines[2].contains("none selected"), "{lines:?}");
    }

    #[test]
    fn only_a_refusal_of_the_key_itself_signs_in_again() {
        let http = |status| Check::Failed(IdentityError::Http { status, status_text: String::new() });
        assert_eq!((http(401).rejected(), http(403).rejected()), (Some(401), Some(403)));
        for status in [400, 404, 408, 429, 500, 502, 503] {
            assert_eq!(http(status).rejected(), None, "{status}");
        }
        let no_answer = crate::client::ApiError {
            message: "fetch failed".into(),
            status: 0,
            code: None,
            request_id: None,
            server_request_id: None,
            details: None,
            status_text: None,
        };
        assert_eq!(failure_reason(&IdentityError::Transport(no_answer.clone())), "fetch failed");
        for check in [
            Check::Failed(IdentityError::Transport(no_answer)),
            Check::Failed(IdentityError::Invalid("Unexpected /me response".into())),
            Check::Verified("Acme".into()),
            Check::Incomplete,
        ] {
            assert_eq!(check.rejected(), None, "{check:?}");
        }
        let reason = |status, text: &str| failure_reason(&IdentityError::Http { status, status_text: text.into() });
        assert_eq!(
            (reason(503, "Service Unavailable"), reason(599, "")),
            ("HTTP 503 Service Unavailable".into(), "HTTP 599".into())
        );
    }

    #[test]
    fn instances_compare_by_origin() {
        assert!(same_instance("https://stg.vendodata.com", "https://stg.vendodata.com/"));
        assert!(same_instance("https://stg.vendodata.com/api", "https://stg.vendodata.com"));
        assert!(!same_instance(DEFAULT_BASE_URL, crate::config::STAGING_BASE_URL));
        assert!(!same_instance("http://127.0.0.1:1", "http://127.0.0.1:2"));
        assert!(same_instance("not a url", "not a url"));
    }
}

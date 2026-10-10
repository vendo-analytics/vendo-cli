//! `vendo login` (port of `src/commands/login.ts` and `init.ts`, one command
//! since CLI 1.1, VE-3825; `vendo init` is its hidden alias). It signs in when
//! there is no working key, checks the key with `/me` and prints the
//! `vendo workspace` screen of the profile it saved or checked, then points to `vendo help`
//! (VE-4109, Yalcin 2026-10-10):
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
//! With `--json` (VE-3831) stdout carries only the summary as JSON (`summary_json`), never the
//! key, with `vendo workspace --json`'s object under `workspace` (VE-4109); what login says on
//! the way, the sign-in URL among it, goes to stderr.
//!
//! The browser flow goes through the web app's `/cli-auth` page, which creates
//! an API key and redirects to
//! `http://127.0.0.1:<port>/callback?key=&account=&account_id=&state=`.
//! The server requires `state` to match `^[a-f0-9]{32}$`.

use std::{
    io::{IsTerminal, Write},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use uuid::Uuid;

use crate::{
    commands::workspace,
    config::{DEFAULT_BASE_URL, EffectiveConfig, Source, vendo_profile_overrides},
    context::Ctx,
    identity::{Identity, IdentityError, fetch_identity},
    output::{bold, dim, print_json, print_success, prompts_off, run_action},
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
    /// The summary as JSON on stdout, the rest on stderr.
    pub json: bool,
}

/// A line login says on the way: on stdout, or on stderr with `--json`, which keeps stdout for the JSON.
fn say(json: bool, line: &str) {
    if json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
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
        let me = &identity.me;
        let name = me.account_slug.clone().or(me.account_name.clone()).unwrap_or_else(|| account_id.clone());
        ctx.store.save_profile(&name, profile(&api_key, Some(&account_id), &base_url))?;
        let note = not_made_active(ctx, &name);
        let summary = Summary {
            profile: Some(name.clone()),
            base_url,
            account_id: Some(account_id),
            note,
            saved: Some(name),
            api_key,
        };
        return finish(ctx, &summary, Check::Verified(identity), NOTHING_CHANGED, args.json).await;
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
                    say(args.json, &dim(&using_existing(&config)));
                    let summary = Summary {
                        profile: config.selected_profile.clone(),
                        base_url: config.base_url.clone(),
                        account_id: config.account_id.clone(),
                        note: None,
                        saved: None,
                        api_key: key.clone(),
                    };
                    return finish(ctx, &summary, check, NOTHING_CHANGED, args.json).await;
                }
            }
        }
    };
    if from_env {
        bail!(sign_in.refusal(&config, &base_url));
    }
    say(args.json, &dim(&sign_in.announcement(&config, &base_url)));
    let signed_in = run_browser_login(ctx, &base_url, args.json).await?;
    let account_id = signed_in.account_id.filter(|id| !id.is_empty());
    let check = check_key(ctx, &signed_in.key, account_id.as_deref(), &base_url).await;
    let saved = match ctx.store.vendo_profile().filter(|name| *name != signed_in.account) {
        // `vendo workspace` would check the profile VENDO_PROFILE names.
        Some(name) => format!(
            "The new key is saved in profile {0}: run `vendo --profile {0} workspace` to check it again ({1}).",
            signed_in.account,
            vendo_profile_overrides(name)
        ),
        None => {
            format!("The new key is saved in profile {}: run `vendo workspace` to check it again.", signed_in.account)
        }
    };
    let note = not_made_active(ctx, &signed_in.account);
    let summary = Summary {
        profile: Some(signed_in.account.clone()),
        base_url,
        account_id,
        note,
        saved: Some(signed_in.account),
        api_key: signed_in.key,
    };
    finish(ctx, &summary, check, &saved, args.json).await
}

/// With `VENDO_PROFILE` in effect, login saves its profile without making it active (Yalcin,
/// 2026-10-06), and says so. A profile that already is the saved active one was not left inactive:
/// the note says VENDO_PROFILE overrides it, and nothing when VENDO_PROFILE names it too.
fn not_made_active(ctx: &Ctx, saved: &str) -> Option<String> {
    let name = ctx.store.vendo_profile()?;
    let overrides = vendo_profile_overrides(name);
    if ctx.store.saved_active_profile().as_deref() == Some(saved) {
        return (name != saved).then(|| {
            format!(
                "Profile {saved} was saved and is the active profile, but {overrides}: unset VENDO_PROFILE to use {saved} here."
            )
        });
    }
    let mut note = format!("Profile {saved} was saved but not made active: {overrides}.");
    if name != saved {
        note.push_str(&format!(" Use it with VENDO_PROFILE={saved}."));
    }
    Some(note)
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
    /// Accepted: `/me`'s answer.
    Verified(Identity),
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

    /// `/me`'s answer as the workspace screen takes it: none when nothing was asked.
    fn into_identity(self) -> Option<Result<Identity, IdentityError>> {
        match self {
            Check::Verified(identity) => Some(Ok(identity)),
            Check::Failed(err) => Some(Err(err)),
            Check::Incomplete => None,
        }
    }
}

async fn check_key(ctx: &Ctx, api_key: &str, account_id: Option<&str>, base_url: &str) -> Check {
    let Some(account_id) = account_id.filter(|id| !id.is_empty()) else { return Check::Incomplete };
    match run_action("Checking your API key...", fetch_identity(api_key, account_id, base_url, ctx.debug)).await {
        Ok(identity) => Check::Verified(identity),
        Err(err) => Check::Failed(err),
    }
}

const NOTHING_CHANGED: &str =
    "Nothing was changed: check your connection (`vendo workspace`) and run `vendo login` again.";

/// What login did: the profile the key is in, its instance and account, why that profile is not
/// the active one when `VENDO_PROFILE` kept it so, the profile it saved (`None` for a key it kept)
/// and the key it checked.
struct Summary {
    profile: Option<String>,
    base_url: String,
    account_id: Option<String>,
    note: Option<String>,
    saved: Option<String>,
    api_key: String,
}

/// Print the workspace screen of the profile login saved, or of the selection whose key it kept
/// (VE-4109), and, when the check did not fail, the next step; with `--json` the summary as JSON
/// with the workspace's under `workspace`. A failed check is an error that ends with `unverified`.
/// `/me` is not asked again when the screen's profile has the key, account and instance login
/// checked; an environment variable that overrides one of them is asked about as the workspace asks.
async fn finish(ctx: &Ctx, summary: &Summary, check: Check, unverified: &str, json: bool) -> Result<()> {
    let store = match &summary.saved {
        Some(name) => ctx.store.with_profile(name),
        None => ctx.store.clone(),
    };
    let auth = auth_json(&check);
    let failure = match &check {
        Check::Failed(err) => Some(failure_reason(err)),
        _ => None,
    };
    let config = store.effective();
    let checked = config.api_key.as_deref() == Some(summary.api_key.as_str())
        && config.account_id.as_deref().filter(|id| !id.is_empty()) == summary.account_id.as_deref()
        && same_instance(&config.base_url, &summary.base_url);
    let identity = if checked { check.into_identity() } else { workspace::check_identity(&config, ctx.debug).await };
    let view = workspace::View::new(ctx, &store, identity, false);
    if json {
        if let Some(note) = &summary.note {
            say(json, &dim(note));
        }
        print_json(&summary_json(summary, auth, view.json()));
    } else {
        for line in summary_lines(summary, &view.lines(), failure.is_none()) {
            println!("{line}");
        }
    }
    if let Some(reason) = failure {
        bail!("Could not verify the API key: {reason}. {unverified}");
    }
    if !json {
        print_success("Vendo CLI setup complete.");
    }
    Ok(())
}

/// `auth` and `accountName` of `login --json`: `verified` with the account the key was verified
/// as, `unverified` (the check failed: the error follows on stderr) or `incomplete` (no account
/// ID to check with).
fn auth_json(check: &Check) -> (&'static str, Option<String>) {
    match check {
        Check::Verified(identity) => ("verified", Some(identity.me.display_name().to_string())),
        Check::Failed(_) => ("unverified", None),
        Check::Incomplete => ("incomplete", None),
    }
}

/// `login --json`: the summary's fields as before VE-4109, then `vendo workspace --json`'s object
/// for the profile login saved or checked (`workspace`), a superset of what it printed.
fn summary_json(summary: &Summary, (auth, account_name): (&str, Option<String>), workspace: Value) -> Value {
    json!({
        "profile": summary.profile,
        "baseUrl": summary.base_url,
        "accountId": summary.account_id,
        "auth": auth,
        "accountName": account_name,
        "workspace": workspace,
    })
}

/// The workspace screen, the note on the profile, and the next step when the check did not fail.
fn summary_lines(summary: &Summary, screen: &[String], verified: bool) -> Vec<String> {
    let mut lines = vec![String::new()];
    lines.extend(screen.iter().cloned());
    if let Some(note) = &summary.note {
        lines.extend([String::new(), dim(note)]);
    }
    if verified {
        lines.extend([String::new(), bold("Next steps"), NEXT_STEP.to_string()]);
    }
    lines
}

/// What login suggests next (VE-4109, Yalcin 2026-10-10: "it should say vendo help to see
/// available functionality").
const NEXT_STEP: &str = "  Run `vendo help` to see everything you can do.";

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
async fn run_browser_login(ctx: &Ctx, base_url: &str, json: bool) -> Result<Callback> {
    update_check::check(&ctx.update_cache_path()).await;
    let callback = browser_flow(base_url, json).await?;
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

async fn browser_flow(base_url: &str, json: bool) -> Result<Callback> {
    let state = Uuid::new_v4().simple().to_string(); // 32 lowercase hex characters
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let auth_url = format!("{base_url}/cli-auth?port={port}&state={state}");

    say(json, "Login at:");
    say(json, &auth_url);
    if json {
        eprint!("Press ENTER to open in the browser...");
        let _ = std::io::stderr().flush();
    } else {
        print!("Press ENTER to open in the browser...");
        let _ = std::io::stdout().flush();
    }
    // Any stdin answers, a terminal or not, as in the TS CLI. With prompts off (`CI`,
    // `VENDO_NO_INPUT`; VE-3826) a terminal is not read: login then does what it does without a
    // terminal, where stdin is closed on a CI runner. The line above still shows, and no browser
    // opens. A stdin that is no terminal (a pipe, a file) is no prompt and is read as before, so
    // `echo | CI=true vendo login` opens the browser as `echo | vendo login` does.
    if !prompts_off() || !std::io::stdin().is_terminal() {
        let url = auth_url.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            if matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0) && open::that(&url).is_err() {
                say(json, &dim("Could not open browser automatically. Please visit the URL above."));
            }
        });
    }
    say(json, "");
    say(json, &dim("Waiting for authorization..."));

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

    fn identity(name: &str) -> Identity {
        let me = crate::identity::Me {
            account_id: "a-1".into(),
            account_name: Some(name.into()),
            account_slug: None,
            api_key_id: None,
            scopes: None,
        };
        Identity { me, raw: json!({}), response: json!({}) }
    }

    fn summary(note: Option<&str>) -> Summary {
        Summary {
            profile: Some("acme".into()),
            base_url: DEFAULT_BASE_URL.into(),
            account_id: Some("a-1".into()),
            note: note.map(Into::into),
            saved: Some("acme".into()),
            api_key: "k".into(),
        }
    }

    #[test]
    fn an_empty_account_id_is_saved() {
        let p = profile("k", Some(""), DEFAULT_BASE_URL);
        assert_eq!(Value::Object(p), serde_json::json!({ "apiKey": "k", "accountId": "" }));
    }

    #[test]
    fn the_json_summary_says_how_far_the_check_got_and_carries_the_workspace() {
        let mut summary = summary(None);
        summary.profile = None;
        summary.account_id = None;
        let shape = |auth: &str, name: Value| {
            json!({
                "profile": null, "baseUrl": DEFAULT_BASE_URL, "accountId": null, "auth": auth, "accountName": name,
                "workspace": { "config": {} },
            })
        };
        let workspace = || json!({ "config": {} });
        let json_of = |check: &Check| summary_json(&summary, auth_json(check), workspace());
        assert_eq!(json_of(&Check::Verified(identity("Acme"))), shape("verified", json!("Acme")));
        assert_eq!(json_of(&Check::Incomplete), shape("incomplete", Value::Null));
        let failed = Check::Failed(IdentityError::Invalid("x".into()));
        assert_eq!(json_of(&failed), shape("unverified", Value::Null));
        // The keys login printed before VE-4109 come first, as they were.
        let keys: Vec<String> = json_of(&Check::Incomplete).as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["profile", "baseUrl", "accountId", "auth", "accountName", "workspace"]);
    }

    #[test]
    fn the_screen_is_the_workspace_and_the_next_step_is_vendo_help() {
        let screen = ["Acme".to_string(), "  Account ID:  a-1".into()];
        assert_eq!(
            summary_lines(&summary(None), &screen, true),
            [
                "",
                "Acme",
                "  Account ID:  a-1",
                "",
                &bold("Next steps"),
                "  Run `vendo help` to see everything you can do."
            ]
        );
        // A failed check ends with the screen; a note on the profile (VE-3831) follows it.
        assert_eq!(summary_lines(&summary(None), &screen, false), ["", "Acme", "  Account ID:  a-1"]);
        let lines = summary_lines(&summary(Some("Profile acme was saved but not made active.")), &screen, false);
        assert_eq!(lines[lines.len() - 2..], ["", "Profile acme was saved but not made active."]);
        let lines = summary_lines(&summary(Some("Profile acme was saved but not made active.")), &screen, true);
        assert_eq!(lines[3..6], ["", "Profile acme was saved but not made active.", ""]);
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
            Check::Verified(identity("Acme")),
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

//! `vendo login` — headless (`--api-key` + `--account`) or the browser flow
//! against the web app's `/cli-auth` page, which redirects to
//! `http://127.0.0.1:<port>/callback?key=&account=&account_id=&state=`.
//! The server requires `state` to match `^[a-f0-9]{32}$`.

use std::{collections::HashMap, io::Write, time::Duration};

use anyhow::{Result, anyhow, bail};
use clap::Args;
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use uuid::Uuid;

use crate::{
    client::{ApiError, Client},
    config::{Config, DEFAULT_BASE_URL, Profile, STAGING_BASE_URL},
    output::{bold, dim, print_success, run_action},
    update_check,
};

#[derive(Args)]
pub struct LoginArgs {
    /// API key for headless/CI login (requires --account)
    #[arg(long, value_name = "key")]
    api_key: Option<String>,
    /// Account ID for headless/CI login (requires --api-key)
    #[arg(long, value_name = "id")]
    account: Option<String>,
    /// Target instance: "staging" or "prod" (default: VENDO_API_URL/profile, else prod)
    #[arg(long, value_name = "environment")]
    env: Option<String>,
    /// Explicit API base URL (overrides --env)
    #[arg(long, value_name = "url")]
    base_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub account_id: String,
    pub account_name: Option<String>,
    pub account_slug: Option<String>,
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

pub async fn run(config: &mut Config, args: LoginArgs, debug: bool) -> Result<()> {
    // Resolve the instance first so a typo fails before a browser opens at the wrong one.
    let base_url = resolve_login_base_url(config, args.env.as_deref(), args.base_url.as_deref())?;

    let (account, account_id) = if args.api_key.is_some() || args.account.is_some() {
        let (Some(api_key), Some(account_id)) = (args.api_key, args.account) else {
            bail!(
                "Both --api-key and --account are required for headless login.\n{}",
                dim("  Example: vendo login --api-key <key> --account <id>")
            );
        };
        let client = Client::new(api_key.clone(), base_url.clone(), Some(account_id.clone()), debug);
        let me = run_action("Validating credentials...", async {
            let res = client.get("/me", &[]).await.map_err(|err| {
                match err.downcast_ref::<ApiError>() {
                    Some(api) if api.status_text.is_some() => anyhow!(
                        "Credential validation failed (HTTP {}). Check your API key and account ID.",
                        api.status
                    ),
                    _ => err,
                }
            })?;
            Ok(serde_json::from_value::<Me>(res["data"].clone())?)
        })
        .await?;
        let name = me.account_slug.or(me.account_name).unwrap_or_else(|| account_id.clone());
        config.save_profile(&name, profile(api_key, Some(account_id.clone()), &base_url))?;
        (name, Some(account_id))
    } else {
        update_check::check(config.path()).await;
        let callback = browser_flow(&base_url).await?;
        config.save_profile(
            &callback.account,
            profile(callback.key, callback.account_id.clone(), &base_url),
        )?;
        (callback.account, callback.account_id)
    };

    println!();
    print_success(&format!("Authenticated as {}", bold(&account)));
    println!("{}", dim(&format!("Active profile: {account}")));
    if let Some(id) = account_id {
        println!("{}", dim(&format!("Account ID saved: {id}")));
    }
    println!();
    println!("{}", bold("Next steps"));
    println!("  vendo whoami\n  vendo status\n  vendo doctor");
    Ok(())
}

fn profile(api_key: String, account_id: Option<String>, base_url: &str) -> Profile {
    Profile {
        api_key: Some(api_key),
        account_id,
        base_url: (base_url != DEFAULT_BASE_URL).then(|| base_url.to_string()),
        ..Profile::default()
    }
}

fn resolve_login_base_url(config: &Config, env: Option<&str>, base_url: Option<&str>) -> Result<String> {
    if let Some(raw) = base_url {
        let parsed = reqwest::Url::parse(raw).map_err(|_| {
            anyhow!("Invalid --base-url: \"{raw}\". Expected a full URL like {STAGING_BASE_URL}.")
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("Invalid --base-url protocol: \"{raw}\". Use http(s).");
        }
        return Ok(parsed.origin().ascii_serialization());
    }
    if let Some(env) = env {
        return match env.to_lowercase().as_str() {
            "staging" | "stg" => Ok(STAGING_BASE_URL.into()),
            "prod" | "production" => Ok(DEFAULT_BASE_URL.into()),
            _ => bail!("Unknown --env \"{env}\". Use \"staging\" or \"prod\"."),
        };
    }
    Ok(config.effective().base_url)
}

struct Callback {
    key: String,
    account: String,
    account_id: Option<String>,
}

async fn browser_flow(base_url: &str) -> Result<Callback> {
    let state = Uuid::new_v4().simple().to_string(); // 32 lowercase hex chars
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let auth_url = format!("{base_url}/cli-auth?port={port}&state={state}");

    println!("Login at:\n{auth_url}");
    print!("Press ENTER to open in the browser...");
    let _ = std::io::stdout().flush();
    let url = auth_url.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        if matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0) && open::that(&url).is_err() {
            println!("{}", dim("Could not open browser automatically. Please visit the URL above."));
        }
    });
    println!("\n{}", dim("Waiting for authorization..."));

    tokio::time::timeout(Duration::from_secs(5 * 60), accept_callback(&listener, &state))
        .await
        .map_err(|_| anyhow!("Authentication timed out after 5 minutes"))?
}

async fn accept_callback(listener: &TcpListener, state: &str) -> Result<Callback> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let Some(target) = read_request_target(&mut socket).await else { continue };
        let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
            respond(&mut socket, "404 Not Found", "").await;
            continue;
        };
        if url.path() != "/callback" {
            respond(&mut socket, "404 Not Found", "").await;
            continue;
        }
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let get = |k: &str| query.get(k).filter(|v| !v.is_empty()).cloned();

        if let Some(error) = get("error") {
            respond_page(&mut socket, "Authentication Failed", &error).await;
            bail!(error);
        }
        if get("state").as_deref() != Some(state) {
            respond_page(&mut socket, "Invalid Request", "State mismatch. Please try again.").await;
            bail!("State mismatch — possible CSRF attack");
        }
        let (Some(key), Some(account)) = (get("key"), get("account")) else {
            respond_page(&mut socket, "Missing Credentials", "Please try again.").await;
            bail!("Missing key or account in callback");
        };
        respond_page(&mut socket, "Authenticated", "You can close this window and return to the terminal.")
            .await;
        return Ok(Callback { key, account, account_id: get("account_id") });
    }
}

/// Read up to the end of the request headers and return the request target.
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
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

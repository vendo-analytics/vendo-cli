//! HTTP client for the web gateway `/api/v1`.
//!
//! Cleanup vs the TS client: responses come back verbatim. There is no
//! snake→camel rewrite, no envelope synthesis and no `fixFieldTypes` aliasing —
//! the gateway already serves the canonical `{ data, meta }` camelCase envelope,
//! so `--json` prints exactly what the API returned.

use std::{fmt, time::Duration};

use anyhow::{Result, anyhow};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

/// Paths that get an `/accounts/{accountId}` prefix (unchanged from TS).
const ACCOUNT_SCOPED_PREFIXES: &[&str] = &[
    "/apps", "/connections", "/sources", "/jobs", "/models", "/pipeline", "/triggers", "/costs",
    "/events", "/specs", "/pulse", "/bigquery",
];
/// Share an account-scoped prefix but are global routes.
const GLOBAL_PATH_EXCEPTIONS: &[&str] = &["/apps/oauth-session"];

#[derive(Debug)]
pub struct ApiError {
    pub message: String,
    pub status: u16,
    pub code: Option<String>,
    pub request_id: Option<String>,
    pub server_request_id: Option<String>,
    /// Set only when an HTTP response came back (absent for timeouts/network errors).
    pub status_text: Option<String>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

pub struct Client {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    account_id: Option<String>,
    debug: bool,
}

impl Client {
    pub fn new(api_key: String, base_url: String, account_id: Option<String>, debug: bool) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("vendo-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client builds with static config");
        Client { http, api_key, base_url, account_id, debug }
    }

    pub async fn get(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<Value> {
        let query: Vec<(&str, String)> = query
            .iter()
            .filter_map(|(k, v)| v.clone().map(|v| (*k, v)))
            .collect();
        self.request(Method::GET, path, &query).await
    }

    fn route(&self, path: &str) -> Result<(String, Option<String>)> {
        let mut path = match path.strip_prefix("/integrations") {
            Some(rest) => format!("/connections{rest}"),
            None => path.to_string(),
        };
        let global = GLOBAL_PATH_EXCEPTIONS.iter().any(|p| path.starts_with(p));
        let scoped = !global && ACCOUNT_SCOPED_PREFIXES.iter().any(|p| path.starts_with(p));
        let header_scoped = !global && (path == "/me" || path.starts_with("/me/"));
        let account = if scoped || header_scoped {
            Some(self.account_id.clone().ok_or_else(|| {
                anyhow!(
                    "No account configured. Run `vendo config set --account <account-id>` or set VENDO_ACCOUNT_ID."
                )
            })?)
        } else {
            None
        };
        if scoped {
            path = format!("/accounts/{}{path}", account.as_deref().unwrap_or_default());
        }
        Ok((path, account))
    }

    async fn request(&self, method: Method, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let (path, account_id) = self.route(path)?;
        let mut url = reqwest::Url::parse(&self.base_url)?.join(&format!("/api/v1{path}"))?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        let request_id = format!("cli-{}", Uuid::new_v4());
        self.debug_line("request", &[
            ("method", method.as_str()),
            ("url", url.as_str()),
            ("requestId", &request_id),
            ("accountId", account_id.as_deref().unwrap_or("")),
        ]);

        let mut req = self
            .http
            .request(method.clone(), url.clone())
            .bearer_auth(&self.api_key)
            .header("X-API-Secret", &self.api_key)
            .header("X-Request-Id", &request_id)
            .header("X-Actor", "vendo-cli");
        if let Some(account) = &account_id {
            req = req.header("X-Account-Id", account);
        }

        let started = std::time::Instant::now();
        let res = req.send().await.map_err(|err| {
            let message = if err.is_timeout() {
                "Request timed out".to_string()
            } else {
                error_chain(&err)
            };
            ApiError {
                status: if err.is_timeout() { 408 } else { 0 },
                message,
                code: None,
                request_id: Some(request_id.clone()),
                server_request_id: None,
                status_text: None,
            }
        })?;

        let status = res.status();
        let server_request_id = res
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        self.debug_line("response", &[
            ("method", method.as_str()),
            ("url", url.as_str()),
            ("requestId", &request_id),
            ("serverRequestId", server_request_id.as_deref().unwrap_or("")),
            ("status", status.as_str()),
            ("durationMs", &started.elapsed().as_millis().to_string()),
        ]);
        warn_on_low_rate_limit(res.headers());

        if status == StatusCode::NO_CONTENT {
            return Ok(json!({ "data": {} }));
        }
        let body = res.bytes().await?;
        if !status.is_success() {
            let parsed: Option<Value> = serde_json::from_slice(&body).ok();
            let error = parsed.as_ref().and_then(|v| v.get("error"));
            let message = error
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| friendly_http_error(status));
            return Err(ApiError {
                message,
                status: status.as_u16(),
                code: error
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                request_id: Some(request_id),
                server_request_id,
                status_text: Some(status.canonical_reason().unwrap_or("").to_string()),
            }
            .into());
        }
        serde_json::from_slice(&body).map_err(|err| {
            anyhow!("Expected JSON from {url} (HTTP {status}) but could not parse it: {err}")
        })
    }

    fn debug_line(&self, message: &str, fields: &[(&str, &str)]) {
        if !self.debug {
            return;
        }
        let rendered: Vec<String> = fields
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| {
                if v.contains(char::is_whitespace) {
                    format!("{k}={v:?}")
                } else {
                    format!("{k}={v}")
                }
            })
            .collect();
        eprintln!("[debug] {message} {}", rendered.join(" "));
    }
}

fn warn_on_low_rate_limit(headers: &reqwest::header::HeaderMap) {
    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    if let Some(remaining) = remaining.filter(|r| *r < 5) {
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok())
            .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
            .map(|ts| ts.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M:%S").to_string())
            .unwrap_or_else(|| "soon".into());
        eprintln!("Warning: Rate limit low ({remaining} remaining, resets {reset})");
    }
}

fn friendly_http_error(status: StatusCode) -> String {
    match status.as_u16() {
        401 => "Authentication failed. Run `vendo login` to re-authenticate or check your API key.".into(),
        403 => "Permission denied. Your API key may not have access to this resource.".into(),
        404 => "Resource not found. Check the ID and try again.".into(),
        429 => "Rate limit exceeded. Wait a moment and try again.".into(),
        code => match status.canonical_reason() {
            Some(reason) => format!("HTTP {code}: {reason}"),
            None => format!("HTTP {code}"),
        },
    }
}

fn error_chain(err: &dyn std::error::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        parts.push(next.to_string());
        source = next.source();
    }
    parts.join(": ")
}

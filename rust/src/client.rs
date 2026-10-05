//! HTTP client for the Vendo API (port of `src/client.ts`).
//!
//! Routing, headers, error messages, the rate-limit warning and `--debug`
//! lines match the TypeScript client. Responses are returned verbatim: no
//! snake→camel rewriting and no field aliasing (decided 2026-10-04/05), so
//! `--json` prints exactly what the API sent.

// `ApiError` carries request IDs and error details for the one error a
// command reports; its size doesn't matter on that path.
#![allow(clippy::result_large_err)]

use std::time::{Duration, Instant};

use crate::output::{fetch_body_text, js_parse_int, parse_json, to_locale_time_string};

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

/// Paths that get an `/accounts/{accountId}` prefix.
const ACCOUNT_SCOPED_PREFIXES: &[&str] = &[
    "/apps",
    "/connections",
    "/sources",
    "/jobs",
    "/models",
    "/pipeline",
    "/triggers",
    "/costs",
    "/events",
    "/dictionary",
    "/pulse",
    "/bigquery",
];
/// Share an account-scoped prefix but are global routes (scoped by the key).
const GLOBAL_PATH_EXCEPTIONS: &[&str] = &["/apps/oauth-session"];
/// Flat routes that take the account as the `X-Account-Id` header.
const ACCOUNT_HEADER_PATHS: &[&str] = &["/me"];

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub message: String,
    /// HTTP status; 408 for a client timeout, 0 for a network failure.
    pub status: u16,
    pub code: Option<String>,
    pub request_id: Option<String>,
    pub server_request_id: Option<String>,
    pub details: Option<Value>,
    /// Set only when an HTTP response came back (absent for timeouts and
    /// network errors), which is how callers tell the two apart.
    pub status_text: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Default)]
pub struct RequestOptions {
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
    /// Absolute API path outside `/api/v1` (e.g. `/api/measurement/...`):
    /// no path mapping and no account prefix.
    pub raw_path: bool,
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    account_id: Option<String>,
    debug: bool,
    /// How long to wait for the response headers; reading the body is not
    /// limited (the TS client cleared its abort timer once fetch resolved).
    timeout: Duration,
}

impl Client {
    pub fn new(api_key: String, base_url: String, account_id: Option<String>, debug: bool) -> Self {
        Self::with_timeout(api_key, base_url, account_id, debug, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(
        api_key: String,
        base_url: String,
        account_id: Option<String>,
        debug: bool,
        timeout: Duration,
    ) -> Self {
        // No proxies: Node's fetch ignores HTTP(S)_PROXY / ALL_PROXY.
        let http = reqwest::Client::builder()
            .user_agent(concat!("vendo-cli/", env!("CARGO_PKG_VERSION")))
            .no_proxy()
            .build()
            .expect("reqwest client builds with static config");
        Client { http, api_key, base_url, account_id, debug, timeout }
    }

    pub async fn get(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<Value, ApiError> {
        self.request(Method::GET, path, RequestOptions { query: defined(query), ..Default::default() }).await
    }

    pub async fn post(&self, path: &str, body: Option<Value>) -> Result<Value, ApiError> {
        self.request(Method::POST, path, RequestOptions { body, ..Default::default() }).await
    }

    // PATCH/DELETE serve the resource commands ported in VE-3667.
    #[allow(dead_code)]
    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.request(Method::PATCH, path, RequestOptions { body: Some(body), ..Default::default() }).await
    }

    #[allow(dead_code)]
    pub async fn delete(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<Value, ApiError> {
        self.request(Method::DELETE, path, RequestOptions { query: defined(query), ..Default::default() }).await
    }

    pub async fn request(&self, method: Method, path: &str, opts: RequestOptions) -> Result<Value, ApiError> {
        let (path, account_id) = if opts.raw_path { (path.to_string(), None) } else { self.route(path)? };
        let api_path = if opts.raw_path { path } else { format!("/api/v1{path}") };
        let mut url = reqwest::Url::parse(&self.base_url)
            .and_then(|base| base.join(&api_path))
            .map_err(|err| client_error(format!("Invalid URL {}{api_path}: {err}", self.base_url), 0, None))?;
        if !opts.query.is_empty() {
            url.query_pairs_mut().extend_pairs(opts.query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }

        let request_id = format!("cli-{}", Uuid::new_v4());
        self.debug_line(
            "request",
            &[
                ("method", Field::Str(method.as_str())),
                ("url", Field::Str(url.as_str())),
                ("requestId", Field::Str(&request_id)),
                ("accountId", account_id.as_deref().map(Field::Str).unwrap_or(Field::None)),
            ],
        );

        // fetch refuses these (Basic auth would replace the Bearer token).
        if !url.username().is_empty() || url.password().is_some() {
            let message = format!("Request cannot be constructed from a URL that includes credentials: {url}");
            self.debug_line(
                "request_failed",
                &[
                    ("method", Field::Str(method.as_str())),
                    ("url", Field::Str(url.as_str())),
                    ("requestId", Field::Str(&request_id)),
                    ("durationMs", Field::Num(0)),
                    ("error", Field::Str(&message)),
                ],
            );
            return Err(client_error(message, 0, None));
        }

        let mut req = self
            .http
            .request(method.clone(), url.clone())
            .bearer_auth(&self.api_key)
            .header("X-API-Secret", &self.api_key)
            .header("Content-Type", "application/json")
            .header("X-Request-Id", &request_id)
            .header("X-Actor", "vendo-cli");
        if let Some(account) = &account_id {
            req = req.header("X-Account-Id", account);
        }
        match &opts.body {
            Some(body) => req = req.body(crate::output::js_stringify(body)),
            // fetch sends `Content-Length: 0` for a bodiless POST or PUT.
            None if matches!(method, Method::POST | Method::PUT) => req = req.header("Content-Length", "0"),
            None => {}
        }

        let started = Instant::now();
        let res = match tokio::time::timeout(self.timeout, req.send()).await {
            Ok(Ok(res)) => res,
            failed => {
                // The TS client (undici fetch) said only "fetch failed"; the full cause goes to --debug.
                let (message, status, cause) = match failed {
                    Ok(Err(err)) if err.is_timeout() => ("Request timed out", 408, error_chain(&err)),
                    Ok(Err(err)) => ("fetch failed", 0, error_chain(&err)),
                    _ => ("Request timed out", 408, "Request timed out".to_string()),
                };
                self.debug_line(
                    "request_failed",
                    &[
                        ("method", Field::Str(method.as_str())),
                        ("url", Field::Str(url.as_str())),
                        ("requestId", Field::Str(&request_id)),
                        ("durationMs", Field::Num(started.elapsed().as_millis() as i64)),
                        ("error", Field::Str(&cause)),
                    ],
                );
                // Like the TS client, network failures carry no request ID: the server never saw one.
                return Err(client_error(message.to_string(), status, None));
            }
        };

        let status = res.status();
        // fetch's statusText is the server's reason phrase; hyper keeps it only when it isn't
        // the canonical one.
        let status_text = match res.extensions().get::<hyper::ext::ReasonPhrase>() {
            Some(reason) => String::from_utf8_lossy(reason.as_bytes()).into_owned(),
            None => status.canonical_reason().unwrap_or("").to_string(),
        };
        let server_request_id = res.headers().get("x-request-id").and_then(|v| v.to_str().ok()).map(str::to_string);
        let duration_ms = started.elapsed().as_millis() as i64;
        self.debug_line(
            "response",
            &[
                ("method", Field::Str(method.as_str())),
                ("url", Field::Str(url.as_str())),
                ("requestId", Field::Str(&request_id)),
                ("serverRequestId", server_request_id.as_deref().map(Field::Str).unwrap_or(Field::None)),
                ("status", Field::Num(status.as_u16() as i64)),
                ("durationMs", Field::Num(duration_ms)),
            ],
        );
        if let Some(warning) = rate_limit_warning(res.headers()) {
            eprintln!("{warning}");
        }

        if status == StatusCode::NO_CONTENT {
            // The TS client answered `{ data: {} }`; raw-path callers read the body from that `data`.
            return Ok(if opts.raw_path { json!({}) } else { json!({ "data": {} }) });
        }
        let body = res.bytes().await.map_err(|err| client_error(error_chain(&err), 0, None))?;

        if !status.is_success() {
            let parsed: Option<Value> = parse_json(&fetch_body_text(&body)).ok();
            let Some(parsed) = parsed else {
                self.debug_line(
                    "response_error",
                    &[
                        ("method", Field::Str(method.as_str())),
                        ("url", Field::Str(url.as_str())),
                        ("requestId", Field::Str(&request_id)),
                        ("serverRequestId", server_request_id.as_deref().map(Field::Str).unwrap_or(Field::None)),
                        ("status", Field::Num(status.as_u16() as i64)),
                        ("statusText", Field::Str(&status_text)),
                        ("durationMs", Field::Num(duration_ms)),
                    ],
                );
                return Err(ApiError {
                    message: friendly_http_error(status.as_u16(), Some(&status_text)),
                    status: status.as_u16(),
                    code: None,
                    request_id: Some(request_id),
                    server_request_id,
                    details: None,
                    status_text: Some(status_text),
                });
            };
            let error = parsed.get("error");
            let error_message = error.and_then(|e| e.get("message")).and_then(Value::as_str);
            let code = error.and_then(|e| e.get("code")).and_then(Value::as_str).map(str::to_string);
            let details = error.and_then(|e| e.get("details")).cloned();
            self.debug_line(
                "response_error",
                &[
                    ("method", Field::Str(method.as_str())),
                    ("url", Field::Str(url.as_str())),
                    ("requestId", Field::Str(&request_id)),
                    ("serverRequestId", server_request_id.as_deref().map(Field::Str).unwrap_or(Field::None)),
                    ("status", Field::Num(status.as_u16() as i64)),
                    ("statusText", Field::Str(&status_text)),
                    ("errorMessage", error_message.map(Field::Str).unwrap_or(Field::None)),
                    ("durationMs", Field::Num(duration_ms)),
                    ("code", code.as_deref().map(Field::Str).unwrap_or(Field::None)),
                    ("details", details.as_ref().map(Field::Json).unwrap_or(Field::None)),
                ],
            );
            return Err(ApiError {
                message: error_message
                    .filter(|m| !m.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| friendly_http_error(status.as_u16(), None)),
                status: status.as_u16(),
                code,
                request_id: Some(request_id),
                server_request_id,
                details,
                status_text: Some(status_text),
            });
        }

        // A 2xx body that isn't JSON: `res.json()` threw V8's SyntaxError, which the TS client
        // logged as request_failed and reported as is.
        parse_json(&fetch_body_text(&body)).map_err(|message| {
            self.debug_line(
                "request_failed",
                &[
                    ("method", Field::Str(method.as_str())),
                    ("url", Field::Str(url.as_str())),
                    ("requestId", Field::Str(&request_id)),
                    ("durationMs", Field::Num(started.elapsed().as_millis() as i64)),
                    ("error", Field::Str(&message)),
                ],
            );
            client_error(message, 0, None)
        })
    }

    fn route(&self, path: &str) -> Result<(String, Option<String>), ApiError> {
        let mut path = match path.strip_prefix("/integrations") {
            Some(rest) => format!("/connections{rest}"),
            None => path.to_string(),
        };
        let global = GLOBAL_PATH_EXCEPTIONS.iter().any(|p| path.starts_with(p));
        let scoped = !global && ACCOUNT_SCOPED_PREFIXES.iter().any(|p| path.starts_with(p));
        let header_scoped =
            !global && ACCOUNT_HEADER_PATHS.iter().any(|p| path == *p || path.starts_with(&format!("{p}/")));
        let account = if scoped || header_scoped {
            Some(self.account_id.clone().ok_or_else(|| {
                client_error(
                    "No account configured. Run `vendo config set --account <account-id>` or set VENDO_ACCOUNT_ID."
                        .to_string(),
                    0,
                    None,
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

    fn debug_line(&self, message: &str, fields: &[(&str, Field)]) {
        if self.debug {
            eprintln!("{}", format_debug_line(message, fields));
        }
    }
}

/// The payload of a response: `data` when the body is the `{ data, meta }`
/// envelope, otherwise the body itself (bare responses).
pub fn payload(body: &Value) -> &Value {
    body.get("data").unwrap_or(body)
}

fn defined(query: &[(&str, Option<String>)]) -> Vec<(String, String)> {
    query.iter().filter_map(|(k, v)| v.as_ref().map(|v| (k.to_string(), v.clone()))).collect()
}

fn client_error(message: String, status: u16, request_id: Option<String>) -> ApiError {
    ApiError { message, status, code: None, request_id, server_request_id: None, details: None, status_text: None }
}

pub fn friendly_http_error(status: u16, fallback: Option<&str>) -> String {
    match status {
        401 => "Authentication failed. Run `vendo login` to re-authenticate or check your API key.".into(),
        403 => "Permission denied. Your API key may not have access to this resource.".into(),
        404 => "Resource not found. Check the ID and try again.".into(),
        429 => "Rate limit exceeded. Wait a moment and try again.".into(),
        _ => match fallback.filter(|f| !f.is_empty()) {
            Some(text) => format!("HTTP {status}: {text}"),
            None => format!("HTTP {status}"),
        },
    }
}

/// The TS client's warning when `X-RateLimit-Remaining` drops below 5: both headers read with
/// `parseInt`, the reset time printed with `toLocaleTimeString()`.
pub fn rate_limit_warning(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let header = |name: &str| {
        let values: Vec<String> =
            headers.get_all(name).iter().map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).collect();
        (!values.is_empty()).then(|| values.join(", ")).filter(|v| !v.is_empty())
    };
    let remaining = header("x-ratelimit-remaining")?;
    if !js_parse_int(&remaining).is_some_and(|n| n < 5.0) {
        return None;
    }
    let reset = match header("x-ratelimit-reset") {
        None => "soon".to_string(),
        // `new Date(NaN)`, or past ±8.64e15 ms, is an Invalid Date.
        Some(raw) => match js_parse_int(&raw).map(|secs| secs * 1000.0) {
            Some(ms) if ms.abs() <= 8.64e15 => to_locale_time_string(ms as i64),
            _ => "Invalid Date".to_string(),
        },
    };
    Some(format!("Warning: Rate limit low ({remaining} remaining, resets {reset})"))
}

pub enum Field<'a> {
    Str(&'a str),
    Num(i64),
    Json(&'a Value),
    None,
}

/// `[debug] <message> key=value ...`, matching `src/debug.ts`: absent fields
/// are dropped and strings containing whitespace are JSON-quoted.
pub fn format_debug_line(message: &str, fields: &[(&str, Field)]) -> String {
    let rendered: Vec<String> = fields
        .iter()
        .filter_map(|(key, value)| {
            let value = match value {
                Field::None => return None,
                Field::Str(s) if s.contains(char::is_whitespace) => Value::String(s.to_string()).to_string(),
                Field::Str(s) => s.to_string(),
                Field::Num(n) => n.to_string(),
                Field::Json(v) => v.to_string(),
            };
            Some(format!("{key}={value}"))
        })
        .collect();
    if rendered.is_empty() { format!("[debug] {message}") } else { format!("[debug] {message} {}", rendered.join(" ")) }
}

fn error_chain(err: &dyn std::error::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        let text = next.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        source = next.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, header_exists, method, path, query_param},
    };

    fn client(server: &MockServer) -> Client {
        Client::new("test-api-key".into(), server.uri(), Some("acct-123".into()), false)
    }

    async fn fail(server: &MockServer, status: u16, body: Option<Value>) -> ApiError {
        let mut template = ResponseTemplate::new(status);
        if let Some(body) = body {
            template = template.set_body_json(body);
        }
        Mock::given(path("/api/v1/me")).respond_with(template).mount(server).await;
        client(server).get("/me", &[]).await.unwrap_err()
    }

    #[tokio::test]
    async fn friendly_messages_for_common_statuses() {
        for (status, expected) in [
            (401, "Authentication failed. Run `vendo login` to re-authenticate or check your API key."),
            (403, "Permission denied. Your API key may not have access to this resource."),
            (404, "Resource not found. Check the ID and try again."),
            (429, "Rate limit exceeded. Wait a moment and try again."),
        ] {
            let server = MockServer::start().await;
            let err = fail(&server, status, None).await;
            assert_eq!((err.message.as_str(), err.status), (expected, status));
            assert!(err.status_text.is_some());
        }
    }

    #[tokio::test]
    async fn structured_error_message_and_code_win() {
        let server = MockServer::start().await;
        let err = fail(
            &server,
            422,
            Some(json!({ "error": { "code": "VALIDATION_ERROR", "message": "Invalid field: name is required" } })),
        )
        .await;
        assert_eq!(err.message, "Invalid field: name is required");
        assert_eq!(err.code.as_deref(), Some("VALIDATION_ERROR"));
    }

    #[tokio::test]
    async fn error_body_without_message_falls_back_to_the_status() {
        let server = MockServer::start().await;
        assert_eq!(fail(&server, 500, Some(json!({ "error": {} }))).await.message, "HTTP 500");
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(502).set_body_string("<html>"))
            .mount(&server)
            .await;
        assert_eq!(client(&server).get("/me", &[]).await.unwrap_err().message, "HTTP 502: Bad Gateway");
    }

    #[tokio::test]
    async fn responses_pass_through_verbatim() {
        let server = MockServer::start().await;
        let body = json!({ "data": [{ "id": "c1", "config": { "custom_source": true, "identity_mapping": {} } }], "meta": { "pagination": { "total": 1 } } });
        Mock::given(path("/api/v1/accounts/acct-123/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;
        assert_eq!(client(&server).get("/integrations", &[]).await.unwrap(), body);

        let bare = json!({ "accountId": "acct-123", "account_name": "Bare" });
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&bare))
            .mount(&server)
            .await;
        let res = client(&server).get("/me", &[]).await.unwrap();
        assert_eq!(res, bare);
        assert_eq!(payload(&res), &bare);
    }

    #[tokio::test]
    async fn no_content_returns_an_empty_data_envelope() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/accounts/acct-123/apps/a1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        assert_eq!(client(&server).delete("/apps/a1", &[]).await.unwrap(), json!({ "data": {} }));
    }

    #[tokio::test]
    async fn routes_account_scoped_global_and_header_paths() {
        let server = MockServer::start().await;
        let ok = || ResponseTemplate::new(200).set_body_json(json!({ "data": {} }));
        Mock::given(method("GET"))
            .and(path("/api/v1/accounts/acct-123/connections/i1"))
            .respond_with(ok())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/api/v1/catalog")).respond_with(ok()).expect(1).mount(&server).await;
        Mock::given(path("/api/v1/catalog/stripe")).respond_with(ok()).expect(1).mount(&server).await;
        Mock::given(path("/api/v1/apps/oauth-session/s1")).respond_with(ok()).expect(1).mount(&server).await;
        Mock::given(path("/api/v1/accounts/acct-123/apps")).respond_with(ok()).expect(1).mount(&server).await;
        Mock::given(path("/api/v1/accounts/acct-123/jobs")).respond_with(ok()).expect(1).mount(&server).await;
        Mock::given(path("/api/v1/accounts/acct-123/dictionary/lookup"))
            .and(header("X-Account-Id", "acct-123"))
            .respond_with(ok())
            .expect(1)
            .mount(&server)
            .await;
        // `specs` is gone (VE-2545): no longer an account-scoped route.
        Mock::given(path("/api/v1/specs")).respond_with(ok()).expect(1).mount(&server).await;
        Mock::given(path("/api/v1/me"))
            .and(header("X-Account-Id", "acct-123"))
            .respond_with(ok())
            .expect(1)
            .mount(&server)
            .await;

        let c = client(&server);
        for p in [
            "/integrations/i1",
            "/catalog",
            "/catalog/stripe",
            "/apps/oauth-session/s1",
            "/apps",
            "/jobs",
            "/dictionary/lookup",
            "/specs",
            "/me",
        ] {
            c.get(p, &[]).await.unwrap_or_else(|e| panic!("{p}: {e}"));
        }
    }

    #[tokio::test]
    async fn global_routes_do_not_need_an_account() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/catalog"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .mount(&server)
            .await;
        let c = Client::new("k".into(), server.uri(), None, false);
        assert!(c.get("/catalog", &[]).await.is_ok());
        let err = c.get("/apps", &[]).await.unwrap_err();
        assert!(err.message.starts_with("No account configured."));
    }

    #[tokio::test]
    async fn sends_auth_actor_request_id_and_user_agent_headers() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me"))
            .and(header("Authorization", "Bearer test-api-key"))
            .and(header("X-API-Secret", "test-api-key"))
            .and(header("X-Actor", "vendo-cli"))
            .and(header("Content-Type", "application/json"))
            .and(header("User-Agent", concat!("vendo-cli/", env!("CARGO_PKG_VERSION"))))
            .and(header_exists("X-Request-Id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
            .expect(1)
            .mount(&server)
            .await;
        client(&server).get("/me", &[]).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let id = requests[0].headers.get("X-Request-Id").unwrap().to_str().unwrap();
        assert!(id.starts_with("cli-") && id.len() == 40, "{id}");
    }

    #[tokio::test]
    async fn query_skips_undefined_values_and_post_keeps_post() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/accounts/acct-123/apps"))
            .and(query_param("limit", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/accounts/acct-123/apps/a1/pause"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server);
        c.get("/apps", &[("limit", Some("20".into())), ("state", None)]).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].url.query(), Some("limit=20"));
        c.post("/apps/a1/pause", None).await.unwrap();
    }

    #[tokio::test]
    async fn timeout_is_408_and_network_failure_is_0() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .mount(&server)
            .await;
        let c = Client::with_timeout("k".into(), server.uri(), Some("a".into()), false, Duration::from_millis(50));
        let err = c.get("/me", &[]).await.unwrap_err();
        assert_eq!((err.message.as_str(), err.status, err.status_text.is_none()), ("Request timed out", 408, true));
        assert_eq!(err.request_id, None, "the TS CLI prints no Request ID line for a timeout");

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let c = Client::new("k".into(), format!("http://{closed}"), Some("a".into()), false);
        let err = c.get("/me", &[]).await.unwrap_err();
        assert_eq!(
            (err.message.as_str(), err.status, err.status_text.is_none(), err.request_id),
            ("fetch failed", 0, true, None)
        );
    }

    #[test]
    fn rate_limit_headers_are_read_with_parse_int() {
        let warning = |remaining: &str, reset: Option<&str>| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("x-ratelimit-remaining", remaining.parse().unwrap());
            if let Some(reset) = reset {
                headers.insert("x-ratelimit-reset", reset.parse().unwrap());
            }
            rate_limit_warning(&headers)
        };
        // `parseInt(remaining, 10) < 5`, and the header is printed as sent.
        assert_eq!(warning("  3abc", None).as_deref(), Some("Warning: Rate limit low (  3abc remaining, resets soon)"));
        assert_eq!(warning("-1", Some("")).as_deref(), Some("Warning: Rate limit low (-1 remaining, resets soon)"));
        assert_eq!(warning("4.9", None).as_deref(), Some("Warning: Rate limit low (4.9 remaining, resets soon)"));
        assert_eq!(warning("x3", None), None);
        assert_eq!(warning("1e3", None).as_deref(), Some("Warning: Rate limit low (1e3 remaining, resets soon)"));
        // The reset time through `new Date(parseInt(reset, 10) * 1000).toLocaleTimeString()`.
        assert_eq!(
            warning("0", Some("abc")).as_deref(),
            Some("Warning: Rate limit low (0 remaining, resets Invalid Date)")
        );
        assert_eq!(
            warning("0", Some("9999999999999")).as_deref(),
            Some("Warning: Rate limit low (0 remaining, resets Invalid Date)")
        );
        let expected = crate::output::to_locale_time_string(1_700_000_000_000);
        assert_eq!(
            warning("0", Some("1700000000.9")),
            Some(format!("Warning: Rate limit low (0 remaining, resets {expected})"))
        );
    }

    #[test]
    fn rate_limit_warning_only_below_five() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "3".parse().unwrap());
        assert!(rate_limit_warning(&headers).unwrap().starts_with("Warning: Rate limit low (3 remaining, resets "));
        headers.insert("x-ratelimit-remaining", "5".parse().unwrap());
        assert_eq!(rate_limit_warning(&headers), None);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        assert_eq!(rate_limit_warning(&headers).unwrap(), "Warning: Rate limit low (0 remaining, resets soon)");
    }

    #[test]
    fn debug_lines_match_the_ts_format() {
        let details = json!({ "field": "account_id" });
        let line = format_debug_line(
            "response_error",
            &[
                ("method", Field::Str("GET")),
                ("serverRequestId", Field::Str("req_server_422")),
                ("status", Field::Num(422)),
                ("statusText", Field::Str("Unprocessable Entity")),
                ("errorMessage", Field::Str("Invalid payload")),
                ("code", Field::Str("VALIDATION_ERROR")),
                ("details", Field::Json(&details)),
                ("accountId", Field::None),
            ],
        );
        assert_eq!(
            line,
            r#"[debug] response_error method=GET serverRequestId=req_server_422 status=422 statusText="Unprocessable Entity" errorMessage="Invalid payload" code=VALIDATION_ERROR details={"field":"account_id"}"#
        );
        assert_eq!(format_debug_line("request", &[]), "[debug] request");
    }

    #[tokio::test]
    async fn a_raw_path_with_no_content_is_an_empty_body() {
        // The TS client answered 204 with `{ data: {} }`, and the web-app
        // commands read the body from `data`: an empty object (VE-3668).
        let server = MockServer::start().await;
        Mock::given(path("/api/metrics/m1")).respond_with(ResponseTemplate::new(204)).mount(&server).await;
        let c = Client::new("k".into(), server.uri(), None, false);
        let res = c
            .request(Method::DELETE, "/api/metrics/m1", RequestOptions { raw_path: true, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(res, json!({}));
    }

    #[tokio::test]
    async fn raw_paths_skip_the_api_prefix_and_account_routing() {
        let server = MockServer::start().await;
        Mock::given(path("/api/measurement/signals"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "signals": [] } })))
            .expect(1)
            .mount(&server)
            .await;
        let c = Client::new("k".into(), server.uri(), None, false);
        let res = c
            .request(Method::GET, "/api/measurement/signals", RequestOptions { raw_path: true, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(res, json!({ "data": { "signals": [] } }));
    }

    #[tokio::test]
    async fn the_timeout_covers_only_the_wait_for_headers() {
        // Headers at once, then the body after the timeout: like the TS client
        // (it cleared its abort timer once fetch resolved), this succeeds.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let body = br#"{"data":{"ok":true}}"#;
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len());
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body[..5]).await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            socket.write_all(&body[5..]).await.unwrap();
        });
        let c = Client::with_timeout(
            "k".into(),
            format!("http://{addr}"),
            Some("a".into()),
            false,
            Duration::from_millis(100),
        );
        assert_eq!(c.get("/me", &[]).await.unwrap(), json!({ "data": { "ok": true } }));
    }

    #[tokio::test]
    async fn bodiless_posts_send_content_length_zero() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/accounts/acct-123/apps/a1/pause"))
            .and(header("Content-Length", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
            .mount(&server)
            .await;
        let c = client(&server);
        c.post("/apps/a1/pause", None).await.unwrap();
        c.delete("/apps/a1", &[]).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].headers.get_all("content-length").iter().count(), 1);
        assert!(requests[1].headers.get("content-length").is_none(), "fetch sends none for a bodiless DELETE");
    }

    #[tokio::test]
    async fn credentials_in_the_base_url_are_refused() {
        let server = MockServer::start().await;
        let port = server.address().port();
        for (base, shown) in [
            (format!("http://u:p@127.0.0.1:{port}"), format!("http://u:p@127.0.0.1:{port}")),
            (format!("http://u@127.0.0.1:{port}"), format!("http://u@127.0.0.1:{port}")),
            (format!("http://:p@127.0.0.1:{port}"), format!("http://:p@127.0.0.1:{port}")),
        ] {
            let c = Client::new("k".into(), base, Some("a".into()), false);
            let err = c.get("/apps", &[("limit", Some("2".into()))]).await.unwrap_err();
            assert_eq!(
                err.message,
                format!(
                    "Request cannot be constructed from a URL that includes credentials: {shown}/api/v1/accounts/a/apps?limit=2"
                )
            );
            assert_eq!((err.status, err.request_id, err.status_text), (0, None, None));
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        // `http://@host` has no credentials once parsed.
        Mock::given(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
            .mount(&server)
            .await;
        Client::new("k".into(), format!("http://@127.0.0.1:{port}"), Some("a".into()), false)
            .get("/me", &[])
            .await
            .unwrap();
    }

    async fn ok_body(body: &'static [u8]) -> Result<Value, ApiError> {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html"))
            .mount(&server)
            .await;
        client(&server).get("/me", &[]).await
    }

    #[tokio::test]
    async fn a_2xx_body_that_is_not_json_fails_with_v8s_message() {
        let err = ok_body(b"<html>").await.unwrap_err();
        assert_eq!(err.message, r#"Unexpected token '<', "<html>" is not valid JSON"#);
        assert_eq!((err.status, err.request_id, err.status_text), (0, None, None));
        assert_eq!(ok_body(b"").await.unwrap_err().message, "Unexpected end of JSON input");
        assert_eq!(
            ok_body(b"{\"data\": [1, 2}").await.unwrap_err().message,
            "Expected ',' or ']' after array element in JSON at position 14 (line 1 column 15)"
        );
        // fetch's res.json() drops a byte order mark and decodes invalid UTF-8 as U+FFFD.
        assert_eq!(ok_body(b"\xef\xbb\xbf{\"data\":1}").await.unwrap(), json!({ "data": 1 }));
        assert_eq!(ok_body(b"{\"data\":\"\xff\"}").await.unwrap(), json!({ "data": "\u{fffd}" }));
    }

    /// A one-shot HTTP server that answers with `head` (status line and headers) and `body`.
    async fn raw_server(head: &'static str, body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let response = format!("{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn the_servers_reason_phrase_is_the_status_text() {
        for (head, message, status_text) in [
            ("HTTP/1.1 502 Proxy Error", "HTTP 502: Proxy Error", "Proxy Error"),
            ("HTTP/1.1 502 Bad Gateway", "HTTP 502: Bad Gateway", "Bad Gateway"),
            ("HTTP/1.1 503 ", "HTTP 503", ""),
            ("HTTP/1.1 599 Custom", "HTTP 599: Custom", "Custom"),
        ] {
            let base = raw_server(head, "<html>").await;
            let err = Client::new("k".into(), base, Some("a".into()), false).get("/me", &[]).await.unwrap_err();
            assert_eq!((err.message.as_str(), err.status_text.as_deref()), (message, Some(status_text)), "{head}");
        }
    }

    #[tokio::test]
    async fn an_empty_server_request_id_falls_back_to_the_cli_id() {
        let base = raw_server(
            "HTTP/1.1 400 Bad Request\r\nX-Request-Id: \r\nContent-Type: application/json",
            r#"{"error":{"message":"nope"}}"#,
        )
        .await;
        let err = Client::new("k".into(), base, Some("a".into()), false).get("/me", &[]).await.unwrap_err();
        let shown = crate::output::format_error(&anyhow::Error::new(err.clone()));
        assert_eq!(shown, format!("nope\nRequest ID: {}", err.request_id.unwrap()));
    }
}

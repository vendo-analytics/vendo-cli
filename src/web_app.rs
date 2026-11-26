//! The web-app routes behind `vendo metrics` and `vendo measurement`
//! (`/api/metrics`, `/api/measurement/*`; port of `src/api/web-app.ts`).
//!
//! They live outside `/api/v1`, so the client sends them as raw paths: no
//! account prefix or `X-Account-Id` header (the API key scopes them), and the
//! JSON comes back exactly as the route sent it, snake_case included. Routes
//! answering through the web app's `apiResponse.success` wrap their body in
//! `{ data: … }`; the others don't. This module is the one place that knows
//! their paths.

// `ApiError` carries request IDs and error details for the one error a
// command reports; its size doesn't matter on that path.
#![allow(clippy::result_large_err)]

use reqwest::Method;
use serde_json::Value;

use crate::client::{ApiError, Client, RequestOptions};

/// Query parameters in order; `None` is left out, as the TS client skipped
/// `undefined`.
pub type Query = Vec<(&'static str, Option<String>)>;

async fn send(
    client: &Client,
    method: Method,
    path: &str,
    query: Query,
    body: Option<Value>,
) -> Result<Value, ApiError> {
    let query = query.into_iter().filter_map(|(key, value)| value.map(|v| (key.to_string(), v))).collect();
    client.request(method, path, RequestOptions { query, body, raw_path: true }).await
}

// Metric IDs go into the path as given, as the TS template strings did.

pub async fn metrics_list(client: &Client, query: Query) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/metrics", query, None).await
}

pub async fn metrics_get(client: &Client, id: &str) -> Result<Value, ApiError> {
    send(client, Method::GET, &format!("/api/metrics/{id}"), Vec::new(), None).await
}

pub async fn metrics_create(client: &Client, body: Value) -> Result<Value, ApiError> {
    send(client, Method::POST, "/api/metrics", Vec::new(), Some(body)).await
}

pub async fn metrics_update(client: &Client, id: &str, body: Value) -> Result<Value, ApiError> {
    send(client, Method::PATCH, &format!("/api/metrics/{id}"), Vec::new(), Some(body)).await
}

pub async fn metrics_remove(client: &Client, id: &str) -> Result<Value, ApiError> {
    send(client, Method::DELETE, &format!("/api/metrics/{id}"), Vec::new(), None).await
}

/// `{ data: { methodologies } }`.
pub async fn methodologies(client: &Client, query: Query) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/measurement/methodologies", query, None).await
}

/// `{ previews, total_distinct_contexts }`.
pub async fn preview_rules(client: &Client, body: Value) -> Result<Value, ApiError> {
    send(client, Method::POST, "/api/measurement/methodologies/rules/preview", Vec::new(), Some(body)).await
}

/// `{ data: { signals } }`.
pub async fn signals(client: &Client) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/measurement/signals", Vec::new(), None).await
}

/// `{ status }`.
pub async fn click_path(client: &Client, query: Query) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/measurement/signals/click-path", query, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    async fn routes_are_raw_paths_without_an_account() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/measurement/signals/click-path"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "x" })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "metrics": [], "total": 0 })))
            .expect(1)
            .mount(&server)
            .await;
        // No account configured: raw paths never need one.
        let client = Client::new("k".into(), server.uri(), None, false);
        let query = vec![("sampleLimit", Some("5".to_string())), ("unset", None)];
        assert_eq!(click_path(&client, query).await.unwrap(), json!({ "status": "x" }));
        let query = vec![("status", Some(String::new())), ("limit", Some("20".to_string()))];
        assert_eq!(metrics_list(&client, query).await.unwrap(), json!({ "metrics": [], "total": 0 }));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].url.query(), Some("sampleLimit=5"));
        assert_eq!(requests[1].url.query(), Some("status=&limit=20"));
        assert!(requests.iter().all(|r| r.headers.get("x-account-id").is_none()));
    }
}

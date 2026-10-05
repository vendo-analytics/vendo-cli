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

/// `{ data: { granularity, segment_key, cohorts, total_returned } }`.
pub async fn ltv(client: &Client, query: Query) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/measurement/ltv", query, None).await
}

/// One cohort's retention matrix, cumulative curve and prediction (bare).
pub async fn cohort(client: &Client, period: &str, query: Query) -> Result<Value, ApiError> {
    let path = format!("/api/measurement/ltv/cohort/{}", encode_uri_component(period));
    send(client, Method::GET, &path, query, None).await
}

/// One customer's cohort row, revenue timeline and realised LTV (bare).
pub async fn customer(client: &Client, customer_id: &str) -> Result<Value, ApiError> {
    let path = format!("/api/measurement/ltv/customer/{}", encode_uri_component(customer_id));
    send(client, Method::GET, &path, Vec::new(), None).await
}

/// `{ data: { signals } }`.
pub async fn signals(client: &Client) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/measurement/signals", Vec::new(), None).await
}

/// `{ status }`.
pub async fn click_path(client: &Client, query: Query) -> Result<Value, ApiError> {
    send(client, Method::GET, "/api/measurement/signals/click-path", query, None).await
}

/// JavaScript's `encodeURIComponent`: every UTF-8 byte except
/// `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is percent-encoded.
pub fn encode_uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[test]
    fn encode_uri_component_matches_javascript() {
        assert_eq!(encode_uri_component("2026-08-01"), "2026-08-01");
        assert_eq!(encode_uri_component("cust 1/2"), "cust%201%2F2");
        assert_eq!(encode_uri_component("cust_é?#"), "cust_%C3%A9%3F%23");
        assert_eq!(encode_uri_component("a-_.!~*'()b"), "a-_.!~*'()b");
        assert_eq!(encode_uri_component("&=+:;,@$%"), "%26%3D%2B%3A%3B%2C%40%24%25");
        assert_eq!(encode_uri_component("😀"), "%F0%9F%98%80");
    }

    #[tokio::test]
    async fn routes_are_raw_paths_without_an_account() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/measurement/ltv/cohort/2026%2F08"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "cohort_period": "x" })))
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
        let query = vec![("granularity", Some("weekly".to_string())), ("segment_key", None)];
        assert_eq!(cohort(&client, "2026/08", query).await.unwrap(), json!({ "cohort_period": "x" }));
        let query = vec![("status", Some(String::new())), ("limit", Some("20".to_string()))];
        assert_eq!(metrics_list(&client, query).await.unwrap(), json!({ "metrics": [], "total": 0 }));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].url.query(), Some("granularity=weekly"));
        assert_eq!(requests[1].url.query(), Some("status=&limit=20"));
        assert!(requests.iter().all(|r| r.headers.get("x-account-id").is_none()));
    }
}

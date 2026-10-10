//! The account-scoped data dictionary routes (port of `src/api/dictionary.ts`):
//! `GET /dictionary` lists one subject type, `GET /dictionary/lookup` reads
//! one entry by subject ID or alias. Responses print verbatim, so the field
//! and parameter names here are the wire contract with vendo-web-v2
//! (`apps/web/app/api/v1/_lib/route-handlers/dictionary/{collection,lookup,serialize}.ts`).
//! The tests pin them, like `src/__tests__/dictionary-contract.test.ts`.

// `ApiError` carries request IDs and error details for the one error a
// command reports; its size doesn't matter on that path.
#![allow(clippy::result_large_err)]

use serde_json::Value;

use crate::client::{ApiError, Client};

/// The `type` values the server accepts (`isDictionarySubjectType`).
pub const SUBJECT_TYPES: [&str; 7] = ["event", "prop", "group", "column", "metric", "model", "audience"];

/// The `--type` help, which names every type the server accepts.
pub fn type_help() -> String {
    format!("Filter by subject type ({})", SUBJECT_TYPES.join(", "))
}

/// Query parameters of `GET /dictionary`, in the order the CLI sends them.
pub const LIST_PARAMS: [&str; 4] = ["type", "q", "limit", "offset"];
/// The one query parameter of `GET /dictionary/lookup`.
pub const LOOKUP_PARAM: &str = "subject_id";

/// One catalog definition, as `toDictionaryItem` serializes it.
///
/// For event, prop, group, metric and audience, `subjectId` is the published
/// semantic registry ID (32 hex characters). Columns and models keep their
/// path IDs (`source:…/table:…/col:…`, `model:…`).
#[derive(Clone, Copy)]
pub struct Item<'a>(pub &'a Value);

impl<'a> Item<'a> {
    pub fn subject_id(self) -> Option<&'a Value> {
        self.0.get("subjectId")
    }
    pub fn subject_type(self) -> Option<&'a Value> {
        self.0.get("subjectType")
    }
    pub fn display_name(self) -> Option<&'a Value> {
        self.0.get("displayName")
    }
    pub fn description(self) -> Option<&'a Value> {
        self.0.get("description")
    }
    pub fn data_type(self) -> Option<&'a Value> {
        self.0.get("dataType")
    }
    pub fn semantic_type(self) -> Option<&'a Value> {
        self.0.get("semanticType")
    }
    pub fn tags(self) -> Option<&'a Value> {
        self.0.get("tags")
    }
    pub fn origin(self) -> Option<&'a Value> {
        self.0.get("origin")
    }
    pub fn last_seen_at(self) -> Option<&'a Value> {
        self.0.get("lastSeenAt")
    }
    pub fn status(self) -> Option<&'a Value> {
        self.0.get("status")
    }
}

/// The `data` of `GET /dictionary/lookup`: `definition` is present only when
/// `found`; `subjectId` echoes the (trimmed) ID that was asked for.
#[derive(Clone, Copy)]
pub struct Lookup<'a>(pub &'a Value);

impl<'a> Lookup<'a> {
    pub fn found(self) -> Option<&'a Value> {
        self.0.get("found")
    }
    pub fn definition(self) -> Option<&'a Value> {
        self.0.get("definition")
    }
}

/// `GET /dictionary?type=&q=&limit=&offset=` (`q` only when given).
pub async fn list(
    client: &Client,
    subject_type: String,
    query: Option<String>,
    limit: String,
    offset: String,
) -> Result<Value, ApiError> {
    let values = [Some(subject_type), query, Some(limit), Some(offset)];
    let params: Vec<(&str, Option<String>)> = LIST_PARAMS.into_iter().zip(values).collect();
    client.get("/dictionary", &params).await
}

/// `GET /dictionary/lookup?subject_id=`: a subject ID from `list`, or an
/// alias such as `event:<name>`.
pub async fn get(client: &Client, subject_id: &str) -> Result<Value, ApiError> {
    client.get("/dictionary/lookup", &[(LOOKUP_PARAM, Some(subject_id.to_string()))]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    /// The dictionary wire contract, as vendo-web-v2 serves it
    /// (`route-handlers/dictionary/{collection,lookup,serialize}.ts` on
    /// origin/staging e867fac6d, 2026-10-05; the same lists pin the TS CLI in
    /// `dictionary-contract.test.ts`). Change them only together with the server.
    const SERVER_ITEM_FIELDS: [&str; 10] = [
        "subjectId",
        "subjectType",
        "displayName",
        "description",
        "dataType",
        "semanticType",
        "tags",
        "origin",
        "lastSeenAt",
        "status",
    ];
    const SERVER_LOOKUP_FIELDS: [&str; 3] = ["subjectId", "found", "definition"];
    const SERVER_LIST_PARAMS: [&str; 4] = ["type", "q", "limit", "offset"];
    const SERVER_LOOKUP_PARAM: &str = "subject_id";

    /// A server-shaped object whose every value names its key.
    fn server_shaped(fields: &[&str]) -> Value {
        Value::Object(fields.iter().map(|key| (key.to_string(), json!(format!("v:{key}")))).collect())
    }

    fn named(fields: &[&str]) -> Vec<Value> {
        fields.iter().map(|key| json!(format!("v:{key}"))).collect()
    }

    #[test]
    fn every_item_accessor_reads_its_own_server_field() {
        let item = server_shaped(&SERVER_ITEM_FIELDS);
        let view = Item(&item);
        let read = [
            view.subject_id(),
            view.subject_type(),
            view.display_name(),
            view.description(),
            view.data_type(),
            view.semantic_type(),
            view.tags(),
            view.origin(),
            view.last_seen_at(),
            view.status(),
        ];
        assert_eq!(
            read.map(|v| v.cloned()).to_vec(),
            named(&SERVER_ITEM_FIELDS).into_iter().map(Some).collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_lookup_accessor_reads_its_own_server_field() {
        // `subjectId` is the server's echo; the CLI names the argument as typed instead.
        let lookup = server_shaped(&SERVER_LOOKUP_FIELDS);
        let view = Lookup(&lookup);
        assert_eq!(
            [view.found().cloned(), view.definition().cloned()],
            [Some(json!("v:found")), Some(json!("v:definition"))]
        );
    }

    #[test]
    fn the_cli_sends_exactly_the_server_params() {
        assert_eq!(LIST_PARAMS, SERVER_LIST_PARAMS);
        assert_eq!(LOOKUP_PARAM, SERVER_LOOKUP_PARAM);
    }

    #[tokio::test]
    async fn list_and_lookup_send_the_server_params_to_the_account_routes() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/accounts/acct-123/dictionary"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .mount(&server)
            .await;
        Mock::given(path("/api/v1/accounts/acct-123/dictionary/lookup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "found": false } })))
            .mount(&server)
            .await;
        let client = Client::new("k".into(), server.uri(), Some("acct-123".into()), false);
        list(&client, "event".into(), Some("checkout".into()), "20".into(), "0".into()).await.unwrap();
        list(&client, "prop".into(), None, "5".into(), "10".into()).await.unwrap();
        get(&client, "source/table with spaces").await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let keys = |i: usize| -> Vec<String> { requests[i].url.query_pairs().map(|(k, _)| k.into_owned()).collect() };
        assert_eq!(keys(0), SERVER_LIST_PARAMS);
        assert_eq!(requests[0].url.query(), Some("type=event&q=checkout&limit=20&offset=0"));
        assert_eq!(requests[1].url.query(), Some("type=prop&limit=5&offset=10"), "an absent query is left out");
        assert_eq!(keys(2), [SERVER_LOOKUP_PARAM]);
        assert_eq!(requests[2].url.query(), Some("subject_id=source%2Ftable+with+spaces"));
    }
}

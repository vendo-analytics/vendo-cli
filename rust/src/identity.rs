//! `/api/v1/me` lookups with explicit, not-yet-saved credentials (port of
//! `src/identity.ts`), used by login, init and doctor.

use serde::{Deserialize, Serialize};

use crate::client::{ApiError, Client, payload};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub account_id: String,
    pub account_name: Option<String>,
    pub account_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
}

impl Me {
    /// Account name, else slug, else ID: how every command names the account.
    pub fn display_name(&self) -> &str {
        self.account_name.as_deref().or(self.account_slug.as_deref()).unwrap_or(&self.account_id)
    }
}

/// The parsed identity plus the `/me` payload as sent (doctor prints it whole).
#[derive(Debug, Clone)]
pub struct Identity {
    pub me: Me,
    pub raw: serde_json::Value,
}

#[derive(Debug)]
pub enum IdentityError {
    /// The API answered with an HTTP error (credentials rejected, …).
    Http { status: u16, status_text: String },
    /// No HTTP answer: timeout or network failure.
    Transport(ApiError),
    /// The API answered 200 with something that isn't an identity.
    Invalid(String),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::Http { status, .. } => write!(f, "Credential validation failed (HTTP {status})"),
            IdentityError::Transport(err) => write!(f, "{err}"),
            IdentityError::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for IdentityError {}

pub async fn fetch_identity(
    api_key: &str,
    account_id: &str,
    base_url: &str,
    debug: bool,
) -> Result<Identity, IdentityError> {
    let client = Client::new(api_key.to_string(), base_url.to_string(), Some(account_id.to_string()), debug);
    match client.get("/me", &[]).await {
        Ok(body) => {
            let raw = payload(&body).clone();
            let me = serde_json::from_value(raw.clone())
                .map_err(|err| IdentityError::Invalid(format!("Unexpected /me response: {err}")))?;
            Ok(Identity { me, raw })
        }
        Err(err) => match err.status_text.clone() {
            Some(status_text) => Err(IdentityError::Http { status: err.status, status_text }),
            None => Err(IdentityError::Transport(err)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    #[tokio::test]
    async fn returns_the_identity_on_success() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "accountId": "acct-1", "accountName": "Acme", "accountSlug": "acme" } })))
            .mount(&server)
            .await;
        let identity = fetch_identity("k", "acct-1", &server.uri(), false).await.unwrap();
        assert_eq!(identity.me.account_id, "acct-1");
        assert_eq!(identity.me.display_name(), "Acme");
        assert_eq!(identity.raw["accountSlug"], "acme");
    }

    #[tokio::test]
    async fn maps_an_http_error_to_a_credential_failure() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/me")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        match fetch_identity("bad", "acct-1", &server.uri(), false).await.unwrap_err() {
            IdentityError::Http { status, status_text } => assert_eq!((status, status_text.as_str()), (401, "Unauthorized")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_network_failure_is_not_a_credential_failure() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let err = fetch_identity("k", "a", &format!("http://{closed}"), false).await.unwrap_err();
        assert!(matches!(err, IdentityError::Transport(ApiError { status: 0, .. })), "{err:?}");
    }
}

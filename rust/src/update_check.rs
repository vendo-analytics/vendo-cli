//! Once-a-day GitHub release check (port of `src/update-check.ts`), sharing
//! its `~/.config/vendo/.update-check` cache. Agreed clean-up: the notice
//! goes to stderr so `--json` stdout stays parseable.

use std::{fs, path::Path, time::Duration};

use owo_colors::{OwoColorize, Stream};

use crate::output::paint;
use serde::{Deserialize, Serialize};

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
const RELEASES_URL: &str = "https://api.github.com/repos/vendo-analytics/vendo-cli/releases/latest";
pub const INSTALL_COMMAND: &str = "curl -fsSL https://app2.vendodata.com/install.sh | bash";

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    pub last_check: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Checked within 24h: show a notice only if the cached version differs.
    Cached(Option<String>),
    Fetch,
}

pub fn decide(cache: &Cache, now_ms: i64, current: &str) -> Decision {
    if now_ms - cache.last_check < CHECK_INTERVAL_MS {
        Decision::Cached(cache.latest_version.clone().filter(|latest| latest != current))
    } else {
        Decision::Fetch
    }
}

pub fn normalize_release_version(tag: &str) -> String {
    tag.strip_prefix("cli-v").unwrap_or(tag).to_string()
}

pub fn notice(latest: &str) -> String {
    format!(
        "{} {}\n",
        paint(&format!("Update available: {CURRENT_VERSION} → {latest}"), Stream::Stderr, |t| t.yellow().to_string()),
        paint(&format!("— run `{INSTALL_COMMAND}` to update"), Stream::Stderr, |t| t.dimmed().to_string()),
    )
}

/// Check and print the notice. Never fails and never blocks for more than 3s.
pub async fn check(cache_path: &Path) {
    let now = jiff::Timestamp::now().as_millisecond();
    if let Some(latest) = check_with(cache_path, now, CURRENT_VERSION, fetch_latest()).await {
        eprintln!("{}", notice(&latest));
    }
}

/// The testable core: returns the version to announce, if any.
pub async fn check_with(
    cache_path: &Path,
    now_ms: i64,
    current: &str,
    fetch: impl Future<Output = Option<String>>,
) -> Option<String> {
    let cache: Cache =
        fs::read_to_string(cache_path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    match decide(&cache, now_ms, current) {
        Decision::Cached(notice) => notice,
        Decision::Fetch => {
            let latest = fetch.await?;
            let fresh = Cache { last_check: now_ms, latest_version: Some(latest.clone()) };
            if let Ok(body) = serde_json::to_string(&fresh) {
                let _ = fs::write(cache_path, body);
            }
            (latest != current).then_some(latest)
        }
    }
}

async fn fetch_latest() -> Option<String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: Option<String>,
    }
    let release: Release = reqwest::Client::builder()
        .user_agent(concat!("vendo-cli/", env!("CARGO_PKG_VERSION")))
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    release.tag_name.as_deref().map(normalize_release_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn cache_file(contents: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".update-check");
        if let Some(contents) = contents {
            fs::write(&path, contents).unwrap();
        }
        (dir, path)
    }

    async fn never() -> Option<String> {
        panic!("must not fetch")
    }

    #[tokio::test]
    async fn fresh_cache_skips_the_network() {
        let (_d, path) = cache_file(Some(&format!(r#"{{"lastCheck":{},"latestVersion":"0.3.1"}}"#, 10 * DAY)));
        assert_eq!(check_with(&path, 10 * DAY + 1000, "0.3.1", never()).await, None);
    }

    #[tokio::test]
    async fn fresh_cache_announces_a_different_version() {
        let (_d, path) = cache_file(Some(&format!(r#"{{"lastCheck":{},"latestVersion":"9.9.9"}}"#, 10 * DAY)));
        assert_eq!(check_with(&path, 10 * DAY + 1000, "0.3.1", never()).await.as_deref(), Some("9.9.9"));
    }

    #[tokio::test]
    async fn stale_or_missing_cache_fetches_and_saves() {
        let (_d, path) = cache_file(Some(r#"{"lastCheck":0,"latestVersion":"0.3.1"}"#));
        assert_eq!(
            check_with(&path, 2 * DAY, "0.3.1", async { Some("1.0.0".to_string()) }).await.as_deref(),
            Some("1.0.0")
        );
        let saved: Cache = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, Cache { last_check: 2 * DAY, latest_version: Some("1.0.0".into()) });

        let (_d, path) = cache_file(None);
        assert_eq!(check_with(&path, DAY, "0.3.1", async { Some("0.3.1".to_string()) }).await, None);
    }

    #[tokio::test]
    async fn fetch_failures_and_unwritable_caches_are_silent() {
        let (_d, path) = cache_file(None);
        assert_eq!(check_with(&path, DAY, "0.3.1", async { None }).await, None);
        let missing_dir = Path::new("/nonexistent-vendo-dir/.update-check");
        assert_eq!(
            check_with(missing_dir, DAY, "0.3.1", async { Some("2.0.0".to_string()) }).await.as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn release_tags_drop_the_cli_prefix() {
        assert_eq!(normalize_release_version("cli-v1.0.0"), "1.0.0");
        assert_eq!(normalize_release_version("1.0.0"), "1.0.0");
    }

    #[test]
    fn notice_text_matches_today() {
        assert_eq!(
            notice("1.0.0"),
            format!("Update available: {CURRENT_VERSION} → 1.0.0 — run `{INSTALL_COMMAND}` to update\n")
        );
    }
}

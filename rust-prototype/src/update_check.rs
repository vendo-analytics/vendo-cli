//! Once-a-day GitHub release check, sharing the TS CLI's cache file.
//! Cleanup vs TS: the notice goes to stderr so `--json` stdout stays parseable.

use std::{fs, path::Path, time::Duration};

use owo_colors::{OwoColorize, Stream};
use serde::{Deserialize, Serialize};

const CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
const RELEASES_URL: &str = "https://api.github.com/repos/vendo-analytics/vendo-cli/releases/latest";
const INSTALL_COMMAND: &str = "curl -fsSL https://app2.vendodata.com/install.sh | bash";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    last_check: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_version: Option<String>,
}

pub async fn check(config_path: &Path) {
    let cache_path = config_path.with_file_name(".update-check");
    let cache: Cache = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let now = jiff::Timestamp::now().as_millisecond();

    if now - cache.last_check < CHECK_INTERVAL_MS {
        if let Some(latest) = cache.latest_version.filter(|v| v != CURRENT) {
            print_notice(&latest);
        }
        return;
    }

    let Some(latest) = fetch_latest().await else { return };
    let fresh = Cache { last_check: now, latest_version: Some(latest.clone()) };
    if let Ok(body) = serde_json::to_string(&fresh) {
        let _ = fs::write(&cache_path, body);
    }
    if latest != CURRENT {
        print_notice(&latest);
    }
}

async fn fetch_latest() -> Option<String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: Option<String>,
    }
    let release: Release = reqwest::Client::builder()
        .user_agent(concat!("vendo-cli/", env!("CARGO_PKG_VERSION")))
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
    release
        .tag_name
        .map(|tag| tag.strip_prefix("cli-v").unwrap_or(&tag).to_string())
}

fn print_notice(latest: &str) {
    eprintln!(
        "{} {}\n",
        format!("Update available: {CURRENT} → {latest}").if_supports_color(Stream::Stderr, |t| t.yellow()),
        format!("— run `{INSTALL_COMMAND}` to update").if_supports_color(Stream::Stderr, |t| t.dimmed()),
    );
}

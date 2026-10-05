//! `vendo integrations refresh-source` helpers (port of `src/source-refresh.ts`,
//! VE-1565): the availability window and the summary of the API's answer.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::output::js_date_parse;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const DEFAULT_LOOKBACK_DAYS: i64 = 7;

#[derive(Debug, PartialEq)]
pub struct RefreshWindow {
    pub requested_start: String,
    pub requested_end: String,
}

/// A datetime already carrying a timezone: trailing Z or ±hh[:]mm.
fn has_timezone(value: &str) -> bool {
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    let digits = |range: std::ops::Range<usize>| chars[range].iter().all(char::is_ascii_digit);
    let sign = |i: usize| matches!(chars[i], '+' | '-');
    value.ends_with(['z', 'Z'])
        || (n >= 6 && sign(n - 6) && digits(n - 5..n - 3) && chars[n - 3] == ':' && digits(n - 2..n))
        || (n >= 5 && sign(n - 5) && digits(n - 4..n))
}

/// Parse `--from`/`--to` as UTC: a full ISO datetime or bare `YYYY-MM-DD`.
/// Offset-less datetimes are treated as UTC too, so the window doesn't
/// depend on the machine's timezone (VE-1603).
fn parse_window_bound(flag: &str, value: &str) -> Result<i64> {
    let normalized = if value.contains('T') && !has_timezone(value) { format!("{value}Z") } else { value.to_string() };
    match js_date_parse(&normalized) {
        Some(ms) => Ok(ms),
        None => {
            bail!("Invalid {flag} value: \"{value}\". Use an ISO datetime (2026-07-01T00:00:00Z) or date (2026-07-01).")
        }
    }
}

fn iso(ms: i64) -> String {
    crate::output::js_iso_string(ms)
}

pub fn resolve_refresh_window(from: Option<&str>, to: Option<&str>, now_ms: i64) -> Result<RefreshWindow> {
    let end = match to {
        Some(to) => parse_window_bound("--to", to)?,
        None => now_ms,
    };
    let start = match from {
        Some(from) => parse_window_bound("--from", from)?,
        None => end - DEFAULT_LOOKBACK_DAYS * DAY_MS,
    };
    if start >= end {
        bail!("--from ({}) must be earlier than --to ({}).", iso(start), iso(end));
    }
    Ok(RefreshWindow { requested_start: iso(start), requested_end: iso(end) })
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Tone {
    Success,
    Error,
    Info,
}

#[derive(Debug, PartialEq)]
pub struct Summary {
    pub tone: Tone,
    pub headline: String,
    pub job_ids: Vec<String>,
}

/// `ready` and `importing` are successes, `unavailable` is an error, anything
/// else is informational. Reads the payload as the API sends it.
pub fn summarize(result: &Value) -> Summary {
    let text = |key: &str| result.get(key).and_then(Value::as_str).map(str::to_string);
    let job_ids: Vec<String> = result
        .get("importJobIds")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().map(crate::output::js_string).collect())
        .unwrap_or_default();
    let status = text("status");
    let (tone, fallback) = match status.as_deref() {
        Some("ready") => (Tone::Success, "Source data already available — nothing to import.".to_string()),
        Some("importing") => (Tone::Success, format!("Triggered {} import job(s) for missing data.", job_ids.len())),
        Some("unavailable") => (Tone::Error, "Could not trigger imports — check the source configuration.".to_string()),
        _ => (Tone::Info, format!("Availability status: {}", status.as_deref().unwrap_or("unknown"))),
    };
    Summary { tone, headline: text("message").unwrap_or(fallback), job_ids }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> i64 {
        js_date_parse("2026-07-02T12:00:00.000Z").unwrap()
    }

    fn window(from: Option<&str>, to: Option<&str>) -> RefreshWindow {
        resolve_refresh_window(from, to, now()).unwrap()
    }

    #[test]
    fn defaults_to_seven_days_ending_now() {
        let w = window(None, None);
        assert_eq!(
            (w.requested_start.as_str(), w.requested_end.as_str()),
            ("2026-06-25T12:00:00.000Z", "2026-07-02T12:00:00.000Z")
        );
    }

    #[test]
    fn accepts_iso_datetimes_and_bare_dates() {
        let w = window(Some("2026-06-29"), Some("2026-07-02T23:59:59Z"));
        assert_eq!(
            (w.requested_start.as_str(), w.requested_end.as_str()),
            ("2026-06-29T00:00:00.000Z", "2026-07-02T23:59:59.000Z")
        );
    }

    #[test]
    fn default_from_is_relative_to_an_explicit_to() {
        let w = window(None, Some("2026-06-10"));
        assert_eq!(
            (w.requested_start.as_str(), w.requested_end.as_str()),
            ("2026-06-03T00:00:00.000Z", "2026-06-10T00:00:00.000Z")
        );
    }

    #[test]
    fn offset_less_datetimes_are_utc() {
        let w = window(Some("2026-06-29T06:30:00"), Some("2026-07-01T10:00:00"));
        assert_eq!(
            (w.requested_start.as_str(), w.requested_end.as_str()),
            ("2026-06-29T06:30:00.000Z", "2026-07-01T10:00:00.000Z")
        );
    }

    #[test]
    fn explicit_offsets_are_preserved() {
        let w = window(Some("2026-06-29T00:00:00+02:00"), Some("2026-07-01T00:00:00-0500"));
        assert_eq!(
            (w.requested_start.as_str(), w.requested_end.as_str()),
            ("2026-06-28T22:00:00.000Z", "2026-07-01T05:00:00.000Z")
        );
    }

    #[test]
    fn invalid_dates_name_the_flag() {
        assert!(resolve_refresh_window(Some("not-a-date"), None, now()).unwrap_err().to_string().contains("--from"));
        assert!(resolve_refresh_window(None, Some("nope"), now()).unwrap_err().to_string().contains("--to"));
    }

    #[test]
    fn empty_or_reversed_windows_are_rejected() {
        for (from, to) in [("2026-07-03", "2026-07-01"), ("2026-07-01", "2026-07-01")] {
            let err = resolve_refresh_window(Some(from), Some(to), now()).unwrap_err().to_string();
            assert!(err.contains("earlier than"), "{err}");
        }
    }

    #[test]
    fn summaries_by_status() {
        let ready =
            summarize(&json!({ "status": "ready", "importJobIds": [], "message": "Source data is already available" }));
        assert_eq!((ready.tone, ready.job_ids.len()), (Tone::Success, 0));
        let importing = summarize(&json!({ "status": "importing", "importJobIds": ["job_1", "job_2"] }));
        assert_eq!(importing.tone, Tone::Success);
        assert_eq!(importing.job_ids, ["job_1", "job_2"]);
        assert!(importing.headline.contains("2 import job"));
        assert_eq!(summarize(&json!({ "status": "unavailable" })).tone, Tone::Error);
        let other = summarize(&json!({ "status": "partial" }));
        assert_eq!(other.tone, Tone::Info);
        assert!(other.headline.contains("partial"));
    }
}

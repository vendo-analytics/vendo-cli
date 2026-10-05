//! `vendo integrations refresh-source` helpers (port of `src/source-refresh.ts`,
//! VE-1565): the availability window and the summary of the API's answer.

use anyhow::{Result, bail};
use serde_json::Value;

use jiff::tz::TimeZone;

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

/// Parse `--from`/`--to` as the TS CLI did: a datetime with a `T` and no
/// zone gets a `Z` (UTC, not the machine's zone; VE-1603), then `new Date()`
/// reads it, legacy forms such as `July 1, 2026` included.
fn parse_window_bound(flag: &str, value: &str, tz: &TimeZone) -> Result<i64> {
    let normalized = if value.contains('T') && !has_timezone(value) { format!("{value}Z") } else { value.to_string() };
    match crate::js_date::parse_in(&normalized, tz) {
        Some(ms) => Ok(ms),
        None => {
            bail!("Invalid {flag} value: \"{value}\". Use an ISO datetime (2026-07-01T00:00:00Z) or date (2026-07-01).")
        }
    }
}

fn iso(ms: i64) -> String {
    crate::output::js_iso_string(ms)
}

/// `--to` defaults to now and `--from` to 7 days before `--to`; empty values
/// count as unset, as in the TS CLI.
pub fn resolve_refresh_window(from: Option<&str>, to: Option<&str>, now_ms: i64) -> Result<RefreshWindow> {
    resolve_refresh_window_in(from, to, now_ms, &TimeZone::system())
}

/// [`resolve_refresh_window`] with an explicit zone for local-time forms.
pub fn resolve_refresh_window_in(
    from: Option<&str>,
    to: Option<&str>,
    now_ms: i64,
    tz: &TimeZone,
) -> Result<RefreshWindow> {
    let (from, to) = (from.filter(|f| !f.is_empty()), to.filter(|t| !t.is_empty()));
    let end = match to {
        Some(to) => parse_window_bound("--to", to, tz)?,
        None => now_ms,
    };
    let start = match from {
        Some(from) => parse_window_bound("--from", from, tz)?,
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
    // `result[key] ?? …`, printed as JavaScript prints the value.
    let text = |key: &str| result.get(key).filter(|v| !v.is_null()).map(crate::output::js_string);
    let job_ids: Vec<String> = result
        .get("importJobIds")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().map(crate::output::js_string).collect())
        .unwrap_or_default();
    let (tone, fallback) = match result.get("status").and_then(Value::as_str) {
        Some("ready") => (Tone::Success, "Source data already available — nothing to import.".to_string()),
        Some("importing") => (Tone::Success, format!("Triggered {} import job(s) for missing data.", job_ids.len())),
        Some("unavailable") => (Tone::Error, "Could not trigger imports — check the source configuration.".to_string()),
        _ => (Tone::Info, format!("Availability status: {}", text("status").as_deref().unwrap_or("unknown"))),
    };
    Summary { tone, headline: text("message").unwrap_or(fallback), job_ids }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> i64 {
        crate::output::js_date_parse("2026-07-02T12:00:00.000Z").unwrap()
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

    #[test]
    fn summaries_print_odd_values_like_javascript() {
        // Expected values from Node running the TS `summarizeEnsureSourceData`.
        let headline = |result: Value| summarize(&result).headline;
        assert_eq!(headline(json!({ "status": 5 })), "Availability status: 5");
        assert_eq!(headline(json!({ "status": null })), "Availability status: unknown");
        assert_eq!(headline(json!({ "status": false })), "Availability status: false");
        assert_eq!(headline(json!({ "status": "ready", "message": 7 })), "7");
        let importing = summarize(&json!({ "status": "importing", "importJobIds": [null, 3] }));
        assert_eq!(importing.headline, "Triggered 2 import job(s) for missing data.");
        assert_eq!(importing.job_ids, ["null", "3"]);
    }

    #[test]
    fn empty_flags_mean_the_default_window() {
        assert_eq!(window(Some(""), Some("")), window(None, None));
        let w = window(Some(""), Some("2026-06-10"));
        assert_eq!(w.requested_start, "2026-06-03T00:00:00.000Z");
    }

    #[test]
    fn bounds_parse_as_new_date_does_in_node() {
        // Expected values: the TS parseWindowBound under TZ=Australia/Sydney.
        let sydney = jiff::tz::TimeZone::get("Australia/Sydney").unwrap();
        let start = |from: &str| {
            resolve_refresh_window_in(Some(from), Some("+010000-01-02"), now(), &sydney)
                .map(|w| w.requested_start)
                .map_err(|e| e.to_string())
        };
        for (from, expected) in [
            ("2026-07", "2026-07-01T00:00:00.000Z"),
            ("2026", "2026-01-01T00:00:00.000Z"),
            ("2026-07-01Z", "2026-07-01T00:00:00.000Z"),
            ("2026/07/01", "2026-06-30T14:00:00.000Z"),
            ("2026-7-1", "2026-06-30T14:00:00.000Z"),
            ("07/01/2026", "2026-06-30T14:00:00.000Z"),
            ("July 1, 2026", "2026-06-30T14:00:00.000Z"),
            ("1 Jul 2026", "2026-06-30T14:00:00.000Z"),
            (" 2026-07-01", "2026-06-30T14:00:00.000Z"),
            ("2026-07-01 ", "2026-06-30T14:00:00.000Z"),
            ("2026-02-30", "2026-03-02T00:00:00.000Z"),
            ("2026-07-01T24:00:00Z", "2026-07-02T00:00:00.000Z"),
            ("Tue Jul 01 2026", "2026-07-01T00:00:00.000Z"),
            ("2026-07-01T10:00", "2026-07-01T10:00:00.000Z"),
            ("+010000-01-01", "+010000-01-01T00:00:00.000Z"),
        ] {
            assert_eq!(start(from).as_deref(), Ok(expected), "{from:?}");
        }
        for invalid in ["20260701", "2026-07-01T10", "2026-07-01T23:59:60Z", "2026-07-01T10:00:00 "] {
            assert!(start(invalid).unwrap_err().starts_with("Invalid --from value"), "{invalid:?}");
        }
    }
}

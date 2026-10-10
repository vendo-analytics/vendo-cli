//! Short IDs (VE-3831, decided by Yalcin 2026-10-05): tables show an ID by its first 8
//! characters and `...` (`output::short_id`), and wherever a command takes that resource's full ID
//! it takes those 8 too, with or without the dots (Yalcin, 2026-10-06). An argument of exactly 8 hex
//! digits, alone or followed by `...`, is looked up in the resource's list; when exactly one ID
//! starts with it, the command uses that ID. When several do, the command stops before its request
//! with an error that lists them (Yalcin, 2026-10-06): sent as typed, the API would only say "not
//! found". When none does, or the list cannot be read, the argument goes out as typed, so the API
//! answers as it did before. A full ID, or anything else, is never looked up.
//!
//! Commands look an ID up just before the request it goes into, so nothing is sent where nothing
//! was before: not on a dry run that sends nothing, and only after the consent a delete or cancel
//! asks for (VE-3823).
//!
//! The metrics route leaves archived metrics out of its list unless asked for them, though
//! `metrics list --status archived` shows their short IDs: a short ID that the default list
//! does not have is looked up in the archived list too, so that read happens only on a miss.

// `ApiError` carries request IDs and error details for the one error a
// command reports; its size doesn't matter on that path.
#![allow(clippy::result_large_err)]

use anyhow::bail;
use serde_json::Value;

use crate::{
    client::{ApiError, Client, payload},
    web_app,
};

/// The lists whose tables show short IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    Apps,
    Sources,
    /// The API's integrations, which customers call destinations.
    Destinations,
    Jobs,
    Models,
    /// A web-app route (`/api/metrics`): its own page shape.
    Metrics,
}

impl Listing {
    /// What a refusal calls several of them.
    fn plural(self) -> &'static str {
        match self {
            Listing::Apps => "apps",
            Listing::Sources => "sources",
            Listing::Destinations => "destinations",
            Listing::Jobs => "jobs",
            Listing::Models => "models",
            Listing::Metrics => "metrics",
        }
    }

    /// What the list's table shows of `row` to tell it apart, besides its ID: the name and type
    /// columns (the two apps and the data type for a destination; type, platform and status for a
    /// job, which has no name).
    fn label(self, row: &Value) -> String {
        let text = |key: &str| row.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
        let parts = match self {
            Listing::Apps => vec![text("displayName"), text("appType")],
            Listing::Sources => vec![text("appName"), text("syncType")],
            Listing::Destinations => {
                let apps = match (text("sourceAppName"), text("destinationAppName")) {
                    (None, None) => None,
                    (source, destination) => {
                        Some(format!("{} → {}", source.unwrap_or("—"), destination.unwrap_or("—")))
                    }
                };
                return [apps.as_deref(), text("dataType")].into_iter().flatten().collect::<Vec<_>>().join("  ");
            }
            Listing::Jobs => vec![text("jobType"), text("connectorType"), text("status")],
            Listing::Models | Listing::Metrics => return name_of(row),
        };
        parts.into_iter().flatten().collect::<Vec<_>>().join("  ")
    }
}

/// A row's `name`, how the lists that have one show it.
pub fn name_of(row: &Value) -> String {
    row.get("name").and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The API's largest page (`MAX_LIMIT` of the v1 routes, the metrics route's cap).
pub(crate) const PAGE: usize = 100;
/// At most this many pages per lookup, 500 rows in the API's default order (newest first for
/// jobs): one lookup stays a few requests of the API key's 60 a minute. A command missing an ID lists as
/// many to choose from (VE-3881, `crate::ask`).
pub(crate) const MAX_PAGES: usize = 5;
/// At most this many matches in a refusal, which then says how many more there are.
const LISTED: usize = 10;

/// The digits of a short ID: exactly 8 hex digits (`1a2b3c4d`), alone or followed by the `...` a
/// table prints after them (`1a2b3c4d...`).
pub fn short_id_digits(arg: &str) -> Option<&str> {
    let digits = arg.strip_suffix("...").unwrap_or(arg);
    (digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digits)
}

/// A listed item whose ID starts with a short ID's digits, and what its table shows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub id: String,
    pub label: String,
}

/// The full ID `arg` stands for, `arg` as typed, or the refusal of a short ID that several IDs
/// start with (see the module docs).
pub async fn resolve(client: &Client, listing: Listing, arg: &str) -> anyhow::Result<String> {
    let Some(digits) = short_id_digits(arg) else { return Ok(arg.to_string()) };
    match matches_of(client, listing, digits).await {
        Ok(found) => pick(arg, listing.plural(), found),
        Err(_) => Ok(arg.to_string()),
    }
}

/// [`resolve`] for an optional flag value (`--source <sourceId>`).
pub async fn resolve_opt(client: &Client, listing: Listing, arg: Option<String>) -> anyhow::Result<Option<String>> {
    match arg {
        Some(arg) => Ok(Some(resolve(client, listing, &arg).await?)),
        None => Ok(None),
    }
}

/// The rows in `rows` whose `id` starts with `digits`, ignoring case, each with its `label`.
pub fn matching<'a>(
    rows: impl IntoIterator<Item = &'a Value>,
    digits: &str,
    label: impl Fn(&Value) -> String,
) -> Vec<Match> {
    let starts = |id: &str| id.get(..digits.len()).is_some_and(|start| start.eq_ignore_ascii_case(digits));
    rows.into_iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str).filter(|id| starts(id))?;
            Some(Match { id: id.to_string(), label: label(row) })
        })
        .collect()
}

/// What the short ID `arg` stands for among the `found` items it matched: the one match's ID, or
/// `arg` as typed when none matched. When several did, the error that stops the command (Yalcin,
/// 2026-10-06): the short ID, how many `plural` match, each by full ID and label (the first
/// [`LISTED`], then how many more), and to use the full ID.
pub fn pick(arg: &str, plural: &str, found: Vec<Match>) -> anyhow::Result<String> {
    match found.as_slice() {
        [] => Ok(arg.to_string()),
        [only] => Ok(only.id.clone()),
        several => {
            let mut message = format!("Short ID {arg} matches {} {plural}:", several.len());
            for item in several.iter().take(LISTED) {
                message.push_str(&format!("\n  {}", item.id));
                if !item.label.is_empty() {
                    message.push_str(&format!("  {}", item.label));
                }
            }
            if several.len() > LISTED {
                message.push_str(&format!("\n  and {} more", several.len() - LISTED));
            }
            message.push_str("\nUse the full ID.");
            bail!(message)
        }
    }
}

/// The listed items whose IDs start with `digits`. For metrics, the archived ones when no other
/// matches: the route lists them only when asked (`listMetrics`' `excludeArchivedWhenNoStatus` in
/// vendo-web-v2), and `metrics list --status archived` shows their short IDs.
async fn matches_of(client: &Client, listing: Listing, digits: &str) -> Result<Vec<Match>, ApiError> {
    let found = matches_in_list(client, listing, None, digits).await?;
    if found.is_empty() && listing == Listing::Metrics {
        return matches_in_list(client, listing, Some("archived"), digits).await;
    }
    Ok(found)
}

/// The items in one list (`status` filters the metrics list) whose IDs start with `digits`,
/// reading up to [`MAX_PAGES`] pages. Each ID counts once: a row inserted between two page reads
/// moves the next page down by one, so the same row can come back at the top of it.
async fn matches_in_list(
    client: &Client,
    listing: Listing,
    status: Option<&str>,
    digits: &str,
) -> Result<Vec<Match>, ApiError> {
    let mut found: Vec<Match> = Vec::new();
    for page in 0..MAX_PAGES {
        let offset = page * PAGE;
        let (rows, more) = list_page(client, listing, status, offset).await?;
        for item in matching(&rows, digits, |row| listing.label(row)) {
            if !found.iter().any(|seen| seen.id == item.id) {
                found.push(item);
            }
        }
        if !more || rows.is_empty() {
            break;
        }
    }
    Ok(found)
}

/// One page of the list, and whether there is another: also what a command missing an ID lists to choose
/// from (VE-3881, `crate::ask`).
pub(crate) async fn list_page(
    client: &Client,
    listing: Listing,
    status: Option<&str>,
    offset: usize,
) -> Result<(Vec<Value>, bool), ApiError> {
    let path = match listing {
        Listing::Apps => "/apps",
        Listing::Sources => "/sources",
        Listing::Destinations => "/integrations",
        Listing::Jobs => "/jobs",
        Listing::Models => "/models",
        Listing::Metrics => {
            // `{ metrics, total, limit, offset }`; the query in `metrics list`'s order.
            let query = vec![
                ("status", status.map(str::to_string)),
                ("limit", Some(PAGE.to_string())),
                ("offset", Some(offset.to_string())),
            ];
            let res = web_app::metrics_list(client, query).await?;
            let rows = res.get("metrics").and_then(Value::as_array).cloned().unwrap_or_default();
            let total = res.get("total").and_then(Value::as_u64).unwrap_or(0);
            let more = ((offset + rows.len()) as u64) < total;
            return Ok((rows, more));
        }
    };
    // `{ data: [...], meta: { pagination: { hasMore } } }`.
    let res = client.get(path, &[("limit", Some(PAGE.to_string())), ("offset", Some(offset.to_string()))]).await?;
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    let more = res.pointer("/meta/pagination/hasMore").and_then(Value::as_bool).unwrap_or(false);
    Ok((rows, more))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eight_hex_digits_are_short_ids_alone_or_with_the_tables_dots() {
        for (arg, digits) in [("1a2b3c4d", "1a2b3c4d"), ("ABCDEF01", "ABCDEF01"), ("00000000", "00000000")] {
            assert_eq!(short_id_digits(arg), Some(digits), "{arg}");
            assert_eq!(short_id_digits(&format!("{arg}...")), Some(digits), "{arg}...");
        }
        let uuid = "1a2b3c4d-0000-4000-8000-000000000001";
        for other in [
            uuid,
            "1a2b3c4",
            "1a2b3c4d5",
            "1a2b3c4g",
            "app-1234",
            "1a2b3c4d..",
            "1a2b3c4d....",
            "1a2b3c4...",
            "...",
            "1a2b3c4d\u{2026}",
            "",
            "1a2b3c4\u{e9}",
        ] {
            assert_eq!(short_id_digits(other), None, "{other}");
        }
    }

    #[test]
    fn matching_compares_the_start_of_each_id_ignoring_case() {
        let rows = [
            json!({ "id": "1a2b3c4d-0000-4000-8000-000000000001", "name": "One" }),
            json!({ "id": "1A2B3C4E-0000-4000-8000-000000000002" }),
            json!({ "id": "1a2b3c4d" }),
            json!({ "id": "1a2b" }),
            json!({ "id": 7 }),
            json!({ "name": "no id" }),
            json!({ "id": "é1a2b3c4d" }),
        ];
        let ids =
            |digits: &str| -> Vec<String> { matching(&rows, digits, name_of).into_iter().map(|m| m.id).collect() };
        assert_eq!(ids("1A2B3C4D"), ["1a2b3c4d-0000-4000-8000-000000000001", "1a2b3c4d"]);
        assert_eq!(ids("1a2b3c4e"), ["1A2B3C4E-0000-4000-8000-000000000002"]);
        assert!(ids("ffffffff").is_empty());
        let first = &matching(&rows, "1a2b3c4d", name_of)[0];
        assert_eq!((first.id.as_str(), first.label.as_str()), ("1a2b3c4d-0000-4000-8000-000000000001", "One"));
    }

    fn item(id: &str, label: &str) -> Match {
        Match { id: id.into(), label: label.into() }
    }

    #[test]
    fn one_match_is_used_none_is_sent_as_typed_and_several_are_refused() {
        assert_eq!(pick("1a2b3c4d", "apps", vec![]).unwrap(), "1a2b3c4d");
        assert_eq!(pick("1a2b3c4d...", "apps", vec![item("1a2b3c4d-1", "Shop")]).unwrap(), "1a2b3c4d-1");
        let refused = pick("1a2b3c4d", "apps", vec![item("1a2b3c4d-1", "Shop  shopify"), item("1a2b3c4d-2", "")]);
        assert_eq!(
            refused.unwrap_err().to_string(),
            "Short ID 1a2b3c4d matches 2 apps:\n  1a2b3c4d-1  Shop  shopify\n  1a2b3c4d-2\nUse the full ID."
        );
        let many: Vec<Match> = (1..=13).map(|n| item(&format!("1a2b3c4d-{n}"), "")).collect();
        let message = pick("1a2b3c4d", "jobs", many).unwrap_err().to_string();
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(lines.len(), 13, "{message}");
        assert_eq!(
            (lines[0], lines[10], lines[11], lines[12]),
            ("Short ID 1a2b3c4d matches 13 jobs:", "  1a2b3c4d-10", "  and 3 more", "Use the full ID.")
        );
    }

    #[test]
    fn each_list_labels_a_match_by_the_columns_that_name_it() {
        let row = json!({
            "displayName": "Demo Shop", "appType": "shopify", "appName": "Demo Shop", "syncType": "shopify_orders",
            "sourceAppName": "Demo Shop", "destinationAppName": "Demo Warehouse", "dataType": "orders",
            "jobType": "import", "connectorType": "shopify", "status": "failed", "name": "Revenue",
        });
        for (listing, label) in [
            (Listing::Apps, "Demo Shop  shopify"),
            (Listing::Sources, "Demo Shop  shopify_orders"),
            (Listing::Destinations, "Demo Shop → Demo Warehouse  orders"),
            (Listing::Jobs, "import  shopify  failed"),
            (Listing::Models, "Revenue"),
            (Listing::Metrics, "Revenue"),
        ] {
            assert_eq!(listing.label(&row), label, "{listing:?}");
        }
        // What a row leaves out is left out; a destination missing one app shows the table's dash.
        assert_eq!(Listing::Apps.label(&json!({ "appType": "shopify", "displayName": "" })), "shopify");
        assert_eq!(Listing::Jobs.label(&json!({})), "");
        assert_eq!(Listing::Destinations.label(&json!({ "destinationAppName": "W" })), "— → W");
    }
}

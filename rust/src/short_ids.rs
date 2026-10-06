//! Short IDs (VE-3831, decided by Yalcin 2026-10-05): tables show an ID by its first 8
//! characters (`output::short_id`), and wherever a command takes that resource's full ID it
//! takes those 8 too. An argument of exactly 8 hex digits is looked up in the resource's list;
//! when exactly one ID starts with it, the command uses that ID. When none or several do, or the
//! list cannot be read, the argument goes out as typed, so the API answers as it did before
//! (refusing an ambiguous one with its candidates was not approved). A full ID, or anything but
//! 8 hex digits, is never looked up.
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

/// The API's largest page (`MAX_LIMIT` of the v1 routes, the metrics route's cap).
const PAGE: usize = 100;
/// At most this many pages per lookup, 500 rows in the API's default order (newest first for
/// jobs): one lookup stays a few requests of the API key's 60 a minute.
const MAX_PAGES: usize = 5;

/// `1a2b3c4d`: exactly 8 hex digits, what a table shows of an ID.
pub fn is_short_id(arg: &str) -> bool {
    arg.len() == 8 && arg.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The full ID `arg` stands for, or `arg` as typed (see the module docs).
pub async fn resolve(client: &Client, listing: Listing, arg: &str) -> String {
    if !is_short_id(arg) {
        return arg.to_string();
    }
    match ids_starting_with(client, listing, arg).await {
        Ok(ids) if ids.len() == 1 => ids.into_iter().next().unwrap_or_default(),
        _ => arg.to_string(),
    }
}

/// [`resolve`] for an optional flag value (`--source <sourceId>`).
pub async fn resolve_opt(client: &Client, listing: Listing, arg: Option<String>) -> Option<String> {
    match arg {
        Some(arg) => Some(resolve(client, listing, &arg).await),
        None => None,
    }
}

/// The IDs in `rows` (each row's `id`) that start with `prefix`, ignoring case.
pub fn matching<'a>(rows: impl IntoIterator<Item = &'a Value>, prefix: &str) -> Vec<String> {
    let starts = |id: &&str| id.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix));
    rows.into_iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .filter(starts)
        .map(str::to_string)
        .collect()
}

/// The listed IDs that start with `prefix`. For metrics, the archived ones when no other matches:
/// the route lists them only when asked (`listMetrics`' `excludeArchivedWhenNoStatus` in
/// vendo-web-v2), and `metrics list --status archived` shows their short IDs.
async fn ids_starting_with(client: &Client, listing: Listing, prefix: &str) -> Result<Vec<String>, ApiError> {
    let found = ids_in_list(client, listing, None, prefix).await?;
    if found.is_empty() && listing == Listing::Metrics {
        return ids_in_list(client, listing, Some("archived"), prefix).await;
    }
    Ok(found)
}

/// The IDs in one list (`status` filters the metrics list) that start with `prefix`, reading up to
/// [`MAX_PAGES`] pages. Each ID counts once: a row inserted between two page reads moves the next
/// page down by one, so the same row can come back at the top of it.
async fn ids_in_list(
    client: &Client,
    listing: Listing,
    status: Option<&str>,
    prefix: &str,
) -> Result<Vec<String>, ApiError> {
    let mut found: Vec<String> = Vec::new();
    for page in 0..MAX_PAGES {
        let offset = page * PAGE;
        let (rows, more) = list_page(client, listing, status, offset).await?;
        for id in matching(&rows, prefix) {
            if !found.contains(&id) {
                found.push(id);
            }
        }
        if !more || rows.is_empty() {
            break;
        }
    }
    Ok(found)
}

/// One page of the list, and whether there is another.
async fn list_page(
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
    fn only_eight_hex_digits_are_short_ids() {
        for short in ["1a2b3c4d", "ABCDEF01", "00000000"] {
            assert!(is_short_id(short), "{short}");
        }
        let uuid = "1a2b3c4d-0000-4000-8000-000000000001";
        for other in [uuid, "1a2b3c4", "1a2b3c4d5", "1a2b3c4g", "app-1234", "1a2b3c4d...", "", "1a2b3c4\u{e9}"] {
            assert!(!is_short_id(other), "{other}");
        }
    }

    #[test]
    fn matching_compares_the_start_of_each_id_ignoring_case() {
        let rows = [
            json!({ "id": "1a2b3c4d-0000-4000-8000-000000000001" }),
            json!({ "id": "1A2B3C4E-0000-4000-8000-000000000002" }),
            json!({ "id": "1a2b3c4d" }),
            json!({ "id": "1a2b" }),
            json!({ "id": 7 }),
            json!({ "name": "no id" }),
            json!({ "id": "é1a2b3c4d" }),
        ];
        assert_eq!(matching(&rows, "1A2B3C4D"), ["1a2b3c4d-0000-4000-8000-000000000001", "1a2b3c4d"]);
        assert_eq!(matching(&rows, "1a2b3c4e"), ["1A2B3C4E-0000-4000-8000-000000000002"]);
        assert!(matching(&rows, "ffffffff").is_empty());
    }
}

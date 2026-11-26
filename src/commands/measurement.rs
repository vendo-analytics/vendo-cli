//! `vendo measurement …` (port of `src/commands/measurement.ts`):
//! methodologies, segmentation-rule previews and signal
//! availability. Every route is a web-app route (`web_app`); `--json` prints
//! its body unchanged, as the TS CLI already did.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    browse,
    commands::pipeline_resource::js_number_value,
    context::Ctx,
    js_text::{cell, cell_or, length_of, time_ago_of},
    output::{
        OutputMode, arg_error, bold, count_text, cyan, dim, format_number, green, js_number, js_number_string,
        js_string, js_template, js_truthy, print_count_of, print_field, print_json, red, resolve_output_mode,
        run_action, short_id, table, to_locale_number,
    },
    short_ids, web_app,
};

fn dash() -> String {
    dim("—")
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(js_truthy)
}

fn array_at<'a>(value: &'a Value, pointer: &str) -> &'a [Value] {
    value.pointer(pointer).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

/// `String.prototype.padEnd(width)`, counting UTF-16 units as JavaScript does.
fn pad_end(text: &str, width: usize) -> String {
    let len = text.encode_utf16().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(len)))
}

/// `Object.keys(value)` for an object or array, in V8's order: array-index
/// keys ascending, then the rest in insertion order.
fn object_keys(value: &Value) -> Vec<String> {
    let index = crate::output::array_index;
    match value {
        Value::Object(map) => {
            let mut indexed: Vec<(u32, &String)> = map.keys().filter_map(|k| index(k).map(|n| (n, k))).collect();
            indexed.sort();
            let named = map.keys().filter(|k| index(k).is_none());
            indexed.into_iter().map(|(_, k)| k).chain(named).cloned().collect()
        }
        Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn key_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
        other => other.get(key),
    }
}

// ── methodologies ──────────────────────────────────────────────────────────

/// A row with camelCase aliases, so `--output` takes either form
/// (`clickPathModel` or `click_path_model`).
fn methodology_as_field(row: &Value) -> Value {
    let mut out = row.as_object().cloned().unwrap_or_default();
    for (key, source) in [
        ("id", "id"),
        ("name", "name"),
        ("description", "description"),
        ("isSystem", "is_system"),
        ("is_system", "is_system"),
        ("clickPathModel", "click_path_model"),
        ("click_path_model", "click_path_model"),
    ] {
        out.insert(key.into(), row.get(source).cloned().unwrap_or(Value::Null));
    }
    Value::Object(out)
}

pub async fn methodologies_list(ctx: &Ctx, no_system: bool, json: bool, output: Option<String>) -> Result<()> {
    let mode = resolve_output_mode(json, output.as_deref());
    let client = ctx.client()?;
    let query = vec![("include_system", no_system.then(|| "false".to_string()))];
    let res = run_action("Fetching methodologies...", web_app::methodologies(&client, query)).await?;
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let rows = array_at(&res, "/data/methodologies");
    if mode == OutputMode::Field {
        let rows: Vec<Value> = rows.iter().map(methodology_as_field).collect();
        print_field(&rows, output.as_deref().unwrap_or_default());
        return Ok(());
    }
    // The table, or at a terminal the same rows to choose from (VE-3894).
    let table = browse::Table {
        header: &["ID", "Name", "Click Path", "Scope", "Version", "Updated"],
        cells: rows.iter().map(methodology_cells).collect(),
        footer: count_text(Some(&Value::from(rows.len())), "methodology"),
    };
    browse::shown(&client, browse::Group::Methodologies, rows, table, output.as_deref()).await
}

/// A row of the `methodologies list` table, its cells styled as the table shows them; a selectable
/// list shows them plain (VE-3894).
fn methodology_cells(row: &Value) -> Vec<String> {
    vec![
        dim(&short_id(&cell(row.get("id")))),
        cell(row.get("name")),
        cell(row.get("click_path_model")),
        if truthy(row.get("is_system")) { cyan("system") } else { "account".to_string() },
        js_template(row.get("version")),
        time_ago_of(row.get("updated_at")),
    ]
}

pub async fn methodologies_get(ctx: &Ctx, methodology_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    // No GET-by-id route: find the row in the list, system rows included. The list is read anyway,
    // so the short ID the list's table shows is matched in it, with no request of its own, and one
    // that several IDs start with is refused like any other (VE-3831).
    let res = run_action("Fetching methodology...", web_app::methodologies(&client, Vec::new())).await?;
    let rows = array_at(&res, "/data/methodologies");
    let methodology_id = match short_ids::short_id_digits(methodology_id) {
        Some(digits) => {
            let found = short_ids::matching(rows, digits, short_ids::name_of);
            short_ids::pick(methodology_id, "methodologies", found)?
        }
        None => methodology_id.to_string(),
    };
    let Some(row) = rows.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(methodology_id.as_str())) else {
        bail!("Methodology {methodology_id} not found");
    };
    if json {
        print_json(row);
        return Ok(());
    }
    print!("{}", render_methodology(row));
    Ok(())
}

/// The `methodologies get` text view.
pub fn render_methodology(row: &Value) -> String {
    let t = |key: &str| js_template(row.get(key));
    let mut lines = vec![
        String::new(),
        format!("{} {}", bold(&t("name")), dim(&format!("({})", t("click_path_model")))),
        String::new(),
        format!("  ID:             {}", t("id")),
        format!("  Scope:          {}", if truthy(row.get("is_system")) { cyan("system") } else { "account".into() }),
        format!("  Version:        {}", t("version")),
        format!("  Updated:        {}", time_ago_of(row.get("updated_at"))),
    ];
    if truthy(row.get("description")) {
        lines.push(format!("  Description:   {}", t("description")));
    }
    // `row.ensemble_weights ?? {}`
    let weights = row.get("ensemble_weights").filter(|w| !w.is_null()).cloned().unwrap_or_else(|| json!({}));
    let keys = object_keys(&weights);
    if !keys.is_empty() {
        lines.push(String::new());
        lines.push(bold("  Ensemble weights:"));
        for key in keys {
            let weight = key_value(&weights, &key).filter(|w| !w.is_null()).cloned().unwrap_or(json!(0));
            lines.push(format!("    {}  {}", pad_end(&key, 12), format_number(Some(&weight))));
        }
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

// ── rules ──────────────────────────────────────────────────────────────────

pub async fn rules_preview(ctx: &Ctx, from: String, to: String, limit: &str, json: bool) -> Result<()> {
    let n = js_number(limit);
    if !n.is_finite() || n <= 0.0 || n > 200.0 {
        arg_error(
            "--limit must be 1..200",
            &["vendo measurement rules preview --from 2025-01-01 --to 2025-01-31 --limit 50"],
        );
    }
    let client = ctx.client()?;
    let body = json!({ "from_date": from, "to_date": to, "limit": js_number_value(limit) });
    let res = run_action("Previewing segmentation rules...", web_app::preview_rules(&client, body)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    let mut grid = table(&["Objective", "Channel", "Custom Label", "Methodology", "Via", "Sample"]);
    for row in array_at(&res, "/previews") {
        let context = |key: &str| row.get("context").and_then(|c| c.get(key));
        grid.add_row(vec![
            cell_or(context("campaign_objective"), dash),
            cell_or(context("channel_grouping"), dash),
            cell_or(context("custom_label"), dash),
            cell(row.pointer("/resolved_methodology/name")),
            if row.get("via").and_then(Value::as_str) == Some("rule") { "rule".to_string() } else { dim("default") },
            js_template(row.get("sample_count")),
        ]);
    }
    println!("{grid}");
    print_count_of(res.get("total_distinct_contexts"), "distinct context");
    Ok(())
}

// ── signals ────────────────────────────────────────────────────────────────

pub async fn signals_list(ctx: &Ctx, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching signal availability...", web_app::signals(&client)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    let rows = array_at(&res, "/data/signals");
    // The table, or at a terminal the same rows to choose from (VE-3894): the `click_path` signal
    // offers its `click-path` command.
    let table = browse::Table {
        header: &["Signal", "State", "Available", "Reason / Notes"],
        cells: rows.iter().map(signal_cells).collect(),
        footer: count_text(Some(&Value::from(rows.len())), "signal"),
    };
    browse::shown(&client, browse::Group::Signals, rows, table, None).await
}

/// A row of the `signals list` table, its cells styled as the table shows them; a selectable list
/// shows them plain (VE-3894).
fn signal_cells(row: &Value) -> Vec<String> {
    let availability = |key: &str| row.get("availability").and_then(|a| a.get(key));
    let available = match availability("available") {
        None | Some(Value::Null) => dash(),
        Some(v) if js_truthy(v) => green("yes"),
        Some(_) => red("no"),
    };
    vec![
        cell(row.get("id")),
        if row.get("state").and_then(Value::as_str) == Some("live") { green("live") } else { dim("stub") },
        available,
        cell_or(availability("reason"), dash),
    ]
}

pub async fn click_path(ctx: &Ctx, sample_limit: Option<String>, json: bool) -> Result<()> {
    let mut query = Vec::new();
    if let Some(raw) = sample_limit {
        let n = js_number(&raw);
        if !n.is_finite() || !(1.0..=500.0).contains(&n) {
            arg_error("--sample-limit must be 1..500", &["vendo measurement signals click-path --sample-limit 100"]);
        }
        query.push(("sampleLimit", Some(js_number_string(n))));
    }
    let client = ctx.client()?;
    let res = run_action("Fetching click-path status...", web_app::click_path(&client, query)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    print!("{}", render_click_path(res.get("status").unwrap_or(&Value::Null)));
    Ok(())
}

/// The `signals click-path` text view.
pub fn render_click_path(status: &Value) -> String {
    let mut lines = vec![
        String::new(),
        bold("Click-path signal"),
        String::new(),
        format!("  Enabled:        {}", if truthy(status.get("enabled")) { green("yes") } else { red("no") }),
        format!("  Last computed:  {}", time_ago_of(status.get("lastComputedAt"))),
        format!("  Sample rows:    {}", to_locale_number(length_of(status.get("sampleEstimates")))),
    ];
    let readiness = status.get("readiness");
    let items = readiness.and_then(|r| r.get("readiness")).and_then(Value::as_array).filter(|items| !items.is_empty());
    if let Some(items) = items {
        lines.push(String::new());
        lines.push(bold("  Readiness:"));
        for item in items {
            let ok = truthy(item.get("ok"));
            lines.push(format!("    {} {}", if ok { green("✓") } else { red("✗") }, js_template(item.get("label"))));
            if !ok && truthy(item.get("detail")) {
                lines.push(format!("      {}", dim(&js_template(item.get("detail")))));
            }
        }
    } else if let Some(reason) = readiness.and_then(|r| r.get("reason")).filter(|r| js_truthy(r)) {
        lines.push(format!("  Note:           {}", dim(&js_string(reason))));
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_keys_follow_v8_property_order_and_pad_like_pad_end() {
        assert_eq!(object_keys(&json!({ "b": 1, "10": 2, "a": 3, "2": 4, "02": 5 })), ["2", "10", "b", "a", "02"]);
        assert_eq!(object_keys(&json!([5, 6])), ["0", "1"]);
        assert!(object_keys(&json!(3)).is_empty());
        assert_eq!(pad_end("click_path", 12), "click_path  ");
        assert_eq!(pad_end("survey_response_share", 12), "survey_response_share");
        assert_eq!(pad_end("é", 3), "é  ");
    }

    #[test]
    fn field_rows_offer_both_casings() {
        let row = methodology_as_field(&json!({ "id": "m", "is_system": true, "click_path_model": "linear" }));
        assert_eq!((row["isSystem"].clone(), row["clickPathModel"].clone()), (json!(true), json!("linear")));
        assert_eq!(row["description"], Value::Null);
    }
}

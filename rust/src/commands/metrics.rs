//! `vendo metrics …` (port of `src/commands/metrics.ts`): custom metrics in
//! the Metrics Library, defined by native QuerySpec v2 files (VE-2637).
//!
//! `--json` prints the TS command's own envelope (`{ data: metric }`, and
//! `{ data, meta: { pagination: { total } } }` for the list) around the
//! route's snake_case rows, unchanged (Yalcin, 2026-10-05). The routes send no
//! type, so the list has no Type column and get no type (VE-3856, Yalcin
//! 2026-10-06), where the TS CLI showed the `metric_type` the API dropped.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    commands::pipeline_resource::read_json,
    context::Ctx,
    js_text::{cell, time_ago_of},
    output::{
        OutputMode, bold, confirm, cyan, dim, green, js_template, js_truthy, print_count_of, print_field, print_json,
        red, resolve_output_mode, run_action, short_id, table, yellow,
    },
    short_ids::{Listing, resolve},
    web_app,
};

/// The QuerySpec v2 file, sent as written.
pub fn read_metric_definition(path: &str) -> Result<Value> {
    read_json(path).map_err(|reason| anyhow!("Failed to read Metric definition {path}: {reason}"))
}

/// An object of the entries whose value is present: `JSON.stringify` drops
/// `undefined` (`{ data: res.metric }` with no `metric` prints `{}`).
fn object(entries: Vec<(&str, Option<&Value>)>) -> Value {
    Value::Object(entries.into_iter().filter_map(|(key, value)| Some((key.to_string(), value?.clone()))).collect())
}

/// `{ data: res.metric }`.
fn data_of_metric(res: &Value) -> Value {
    object(vec![("data", res.get("metric"))])
}

fn metric_report_type(definition: Option<&Value>) -> String {
    match definition {
        Some(Value::Object(spec)) => spec.get("reportType").and_then(Value::as_str).unwrap_or("unknown").to_string(),
        _ => "unknown".to_string(),
    }
}

fn status_cell(status: Option<&Value>) -> String {
    match status.and_then(Value::as_str) {
        Some("active") => green("active"),
        Some("draft") => yellow("draft"),
        _ => dim(&cell(status)),
    }
}

pub struct ListArgs {
    pub status: Option<String>,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    let client = ctx.client()?;
    let query = vec![("status", args.status), ("limit", Some(args.limit)), ("offset", Some(args.offset))];
    let res = run_action("Fetching metrics...", web_app::metrics_list(&client, query)).await?;
    let metrics = res.get("metrics");
    let rows = metrics.and_then(Value::as_array).cloned().unwrap_or_default();
    match mode {
        OutputMode::Json => {
            let pagination = object(vec![("total", res.get("total"))]);
            print_json(&object(vec![("data", metrics), ("meta", Some(&json!({ "pagination": pagination })))]));
        }
        OutputMode::Field => print_field(&rows, args.output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            let mut grid = table(&["ID", "Name", "Format", "Status", "Updated"]);
            for metric in &rows {
                grid.add_row(vec![
                    dim(&short_id(&cell(metric.get("id")))),
                    cell(metric.get("name")),
                    cell(metric.get("format")),
                    status_cell(metric.get("status")),
                    time_ago_of(metric.get("updated_at")),
                ]);
            }
            println!("{grid}");
            print_count_of(res.get("total"), "metric");
        }
    }
    Ok(())
}

pub async fn get(ctx: &Ctx, metric_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let metric_id = resolve(&client, Listing::Metrics, metric_id).await?;
    let res = run_action("Fetching metric...", web_app::metrics_get(&client, &metric_id)).await?;
    if json {
        print_json(&data_of_metric(&res));
        return Ok(());
    }
    print!("{}", render_metric(res.get("metric").unwrap_or(&Value::Null)));
    Ok(())
}

/// The `metrics get` text view (the TS action's `console.log` lines).
pub fn render_metric(metric: &Value) -> String {
    let t = |key: &str| js_template(metric.get(key));
    let status = match metric.get("status").and_then(Value::as_str) {
        Some("active") => green("active"),
        _ => t("status"),
    };
    let mut lines = vec![
        String::new(),
        bold(&t("name")),
        String::new(),
        format!("  ID:           {}", t("id")),
        format!("  Format:       {}", t("format")),
        format!("  Status:       {status}"),
        format!("  Updated:      {}", time_ago_of(metric.get("updated_at"))),
    ];
    let truthy = |key: &str| metric.get(key).is_some_and(js_truthy);
    if truthy("description") {
        lines.push(format!("  Description:  {}", t("description")));
    }
    if truthy("unit") {
        lines.push(format!("  Unit:         {}", t("unit")));
    }
    lines.push(format!("  Higher=Better: {}", if truthy("higher_is_better") { green("yes") } else { red("no") }));
    lines.push(format!("  Calculation:  {}", cyan(&metric_report_type(metric.get("definition")))));
    lines.iter().map(|line| format!("{line}\n")).collect()
}

pub struct CreateArgs {
    pub name: String,
    pub definition: String,
    pub description: Option<String>,
    pub format: String,
    pub unit: Option<String>,
    pub json: bool,
}

pub async fn create(ctx: &Ctx, args: CreateArgs) -> Result<()> {
    let mut body = Map::new();
    body.insert("name".into(), json!(args.name));
    body.insert("definition".into(), read_metric_definition(&args.definition)?);
    body.insert("format".into(), json!(args.format));
    if let Some(description) = args.description.filter(|d| !d.is_empty()) {
        body.insert("description".into(), json!(description));
    }
    if let Some(unit) = args.unit.filter(|u| !u.is_empty()) {
        body.insert("unit".into(), json!(unit));
    }
    let client = ctx.client()?;
    let res = run_action("Creating metric...", web_app::metrics_create(&client, Value::Object(body))).await?;
    if args.json {
        print_json(&data_of_metric(&res));
        return Ok(());
    }
    let metric = res.get("metric").unwrap_or(&Value::Null);
    println!();
    println!("{} Metric \"{}\" created", green("✓"), js_template(metric.get("name")));
    println!("  ID:     {}", js_template(metric.get("id")));
    println!("  Status: {}", js_template(metric.get("status")));
    if metric.get("status").and_then(Value::as_str) == Some("draft") {
        println!();
        println!("{}", dim("  The calculation could not compile. Update its definition before activating it."));
    }
    Ok(())
}

pub struct UpdateArgs {
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<String>,
    pub format: Option<String>,
    pub unit: Option<String>,
    pub status: Option<String>,
    pub json: bool,
}

pub async fn update(ctx: &Ctx, metric_id: &str, args: UpdateArgs) -> Result<()> {
    // Only the flags given a non-empty value, in the TS order.
    let mut body = Map::new();
    let given = |value: Option<String>| value.filter(|v| !v.is_empty());
    if let Some(name) = given(args.name) {
        body.insert("name".into(), json!(name));
    }
    if let Some(description) = given(args.description) {
        body.insert("description".into(), json!(description));
    }
    if let Some(path) = given(args.definition) {
        body.insert("definition".into(), read_metric_definition(&path)?);
    }
    if let Some(format) = given(args.format) {
        body.insert("format".into(), json!(format));
    }
    if let Some(unit) = given(args.unit) {
        body.insert("unit".into(), json!(unit));
    }
    if let Some(status) = given(args.status) {
        body.insert("status".into(), json!(status));
    }
    if body.is_empty() {
        bail!("No updates provided");
    }
    let client = ctx.client()?;
    let metric_id = resolve(&client, Listing::Metrics, metric_id).await?;
    let res =
        run_action("Updating metric...", web_app::metrics_update(&client, &metric_id, Value::Object(body))).await?;
    if args.json {
        print_json(&data_of_metric(&res));
        return Ok(());
    }
    println!();
    println!("{} Metric \"{}\" updated", green("✓"), js_template(res.pointer("/metric/name")));
    Ok(())
}

pub async fn activate(ctx: &Ctx, metric_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let metric_id = resolve(&client, Listing::Metrics, metric_id).await?;
    let body = json!({ "status": "active" });
    let res = run_action("Activating metric...", web_app::metrics_update(&client, &metric_id, body)).await?;
    if json {
        print_json(&data_of_metric(&res));
        return Ok(());
    }
    println!();
    println!("{} Metric \"{}\" is now active", green("✓"), js_template(res.pointer("/metric/name")));
    Ok(())
}

/// Permanent delete, after a confirmation (see [`confirm`]).
pub async fn delete(ctx: &Ctx, metric_id: &str, yes: bool, json: bool) -> Result<()> {
    let question = format!("Delete metric {}? This cannot be undone.", short_id(metric_id));
    if !confirm(yes, &question, &format!("This deletes metric {metric_id} and cannot be undone."))? {
        println!("Cancelled");
        return Ok(());
    }
    let client = ctx.client()?;
    // After the consent, which names the ID as typed (VE-3823).
    let metric_id = resolve(&client, Listing::Metrics, metric_id).await?;
    let res = run_action("Deleting metric...", web_app::metrics_remove(&client, &metric_id)).await?;
    if json {
        print_json(&json!({ "data": res }));
        return Ok(());
    }
    println!();
    println!("{} Metric deleted", green("✓"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_spec_definition_is_read_without_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metric.query.json");
        let definition = json!({ "version": 2, "reportType": "segmentation", "metricOutput": { "kind": "measure", "measureId": "m_a" } });
        std::fs::write(&path, definition.to_string()).unwrap();
        assert_eq!(read_metric_definition(path.to_str().unwrap()).unwrap(), definition);
    }

    #[test]
    fn an_unreadable_definition_names_the_file() {
        let err = read_metric_definition("/missing/metric.query.json").unwrap_err().to_string();
        assert_eq!(
            err,
            "Failed to read Metric definition /missing/metric.query.json: ENOENT: no such file or directory, open '/missing/metric.query.json'"
        );
    }

    #[test]
    fn report_types_come_only_from_a_definition_object() {
        assert_eq!(metric_report_type(Some(&json!({ "reportType": "funnel" }))), "funnel");
        for unknown in [None, Some(json!(null)), Some(json!({ "reportType": 7 })), Some(json!([{ "reportType": "x" }]))]
        {
            assert_eq!(metric_report_type(unknown.as_ref()), "unknown", "{unknown:?}");
        }
    }

    #[test]
    fn missing_values_are_left_out_of_the_envelope() {
        assert_eq!(
            data_of_metric(&json!({ "metric": { "id": "m" }, "registryWarning": "x" })),
            json!({ "data": { "id": "m" } })
        );
        assert_eq!(data_of_metric(&json!({ "other": 1 })), json!({}));
    }

    #[test]
    fn the_detail_view_prints_undefined_and_null_like_template_literals() {
        let out = render_metric(
            &json!({ "id": "m1", "name": null, "format": "number", "status": "draft", "higher_is_better": 0, "unit": "" }),
        );
        assert_eq!(
            out,
            "\nnull\n\n  ID:           m1\n  Format:       number\n  Status:       draft\n  Updated:      —\n  Higher=Better: no\n  Calculation:  unknown\n"
        );
    }
}

//! `vendo sources …` (port of `src/commands/sources.ts`).

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::{
    browse,
    client::{Client, payload},
    commands::pipeline_resource::{js_number_value, read_json_file, split_list},
    context::Ctx,
    jobs::{Job, format_job_progress},
    output::{
        OutputMode, bold, color_status, dim, js_color_status, js_date_parse, js_if, js_iso_string, js_join, js_nullish,
        js_positive, js_template, js_truthy, list_count, print_field, print_json, print_label, print_status_label,
        print_success, red, resolve_output_mode, run_action, short_id, time_ago,
    },
    short_ids::{Listing, resolve, resolve_opt},
    watch::{self, ResourceKind},
};

fn text(v: &Value, key: &str) -> Option<String> {
    Job(v).text(key)
}

/// `consecutiveFailures && consecutiveFailures > 0`.
fn failures(v: &Value) -> Option<String> {
    js_positive(v.get("consecutiveFailures"))
}

pub struct ListArgs {
    pub state: Option<String>,
    pub sync_type: Option<String>,
    pub app: Option<String>,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let client = ctx.client()?;
    let query = [
        ("state", args.state),
        ("sync_type", args.sync_type),
        ("app_id", resolve_opt(&client, Listing::Apps, args.app).await?),
        ("limit", Some(args.limit)),
        ("offset", Some(args.offset)),
    ];
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    if mode != OutputMode::Table {
        let res = run_action("Fetching sources...", client.get("/sources", &query)).await?;
        match mode {
            OutputMode::Json => print_json(&res),
            _ => print_field(
                payload(&res).as_array().map(Vec::as_slice).unwrap_or_default(),
                args.output.as_deref().unwrap_or_default(),
            ),
        }
        return Ok(());
    }

    let (res, active) = run_action("Fetching sources...", async {
        tokio::try_join!(client.get("/sources", &query), watch::active_jobs(&client, 100, None, None))
    })
    .await?;
    // `new Map(entries)`: for a source with several active jobs the last one wins.
    let mut active_by_source = HashMap::new();
    for job in &active {
        if let Some(source_id) = text(job, "sourceId").filter(|s| !s.is_empty()) {
            active_by_source.insert(source_id, job.clone());
        }
    }
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    // The table, or at a terminal the same rows to choose from (VE-3894).
    let table = browse::Table {
        header: &["ID", "Name", "Type", "Status", "Progress", "Failures", "Last Sync"],
        cells: rows.iter().map(|source| source_cells(source, &active_by_source)).collect(),
        footer: list_count(&res, rows.len(), "source"),
    };
    browse::shown(&client, browse::Group::Sources, &rows, table, args.output.as_deref()).await
}

/// A row of the `sources list` table, its cells styled as the table shows them; `active` holds the
/// active job of each source by its ID (Progress). A selectable list shows them plain (VE-3894).
fn source_cells(source: &Value, active: &HashMap<String, Value>) -> Vec<String> {
    let id = text(source, "id").unwrap_or_default();
    vec![
        dim(&short_id(&id)),
        text(source, "appName").unwrap_or_else(|| dim("—")),
        text(source, "syncType").unwrap_or_default(),
        color_status(&text(source, "integrationStatus").unwrap_or_default()),
        format_job_progress(active.get(&id).map(Job)),
        failures(source).map(|n| red(&n)).unwrap_or_else(|| dim("0")),
        time_ago(text(source, "lastSyncAt").as_deref()),
    ]
}

pub async fn get(ctx: &Ctx, source_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let source_id = &resolve(&client, Listing::Sources, source_id).await?;
    if json {
        let res = run_action("Fetching source...", client.get(&format!("/sources/{source_id}"), &[])).await?;
        print_json(&res);
        return Ok(());
    }
    show(&client, source_id).await.map(|_| ())
}

/// What `sources get` shows of the source `id`, a full ID (no short-ID lookup), from its requests
/// (the source and its active job) behind its spinner; the source as the API sent it. A selectable
/// list shows it for the source chosen and reads the actions that apply from it (VE-3894).
pub(crate) async fn show(client: &Client, id: &str) -> Result<Value> {
    let path = format!("/sources/{id}");
    let (res, active) = run_action("Fetching source...", async {
        tokio::try_join!(client.get(&path, &[]), watch::active_job_for_resource(client, ResourceKind::Source, id))
    })
    .await?;
    print!("{}", render_source(payload(&res), active.as_ref()));
    Ok(payload(&res).clone())
}

/// The `sources get` text view (the TS action's `console.log` lines).
pub fn render_source(src: &Value, active: Option<&Value>) -> String {
    let t = |key: &str| text(src, key);
    let field = |key: &str| src.get(key);
    let sync_type = js_template(field("syncType"));
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            bold(&js_template(js_nullish(field("appName"), field("syncType")))),
            dim(&format!("({sync_type})"))
        ),
        String::new(),
        format!("  ID:          {}", js_template(field("id"))),
        format!("  App:         {} {}", t("appName").unwrap_or_else(|| dim("—")), dim(&js_template(field("appId")))),
        format!("  Type:        {sync_type}"),
        format!("  State:       {}", js_color_status(field("state"))),
        format!("  Status:      {}", js_color_status(field("integrationStatus"))),
        format!("  Progress:    {}", format_job_progress(active.map(Job))),
        format!("  Frequency:   {}", t("syncFrequency").unwrap_or_else(|| dim("—"))),
        format!(
            "  Anchor:      {} {}",
            t("syncAnchorTime").unwrap_or_else(|| dim("—")),
            t("syncAnchorTimezone").unwrap_or_default()
        ),
        format!("  Last successful sync: {}", time_ago(t("lastSyncAt").as_deref())),
    ];
    match src.get("importProgress") {
        // `importProgress !== null`: missing (older server) or an object.
        Some(Value::Null) => lines.push("  Data access: Read in place; no import checkpoint".to_string()),
        progress => {
            let checkpoint = progress
                .and_then(|p| p.get("latestCheckpointAt"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .and_then(js_date_parse)
                .map(js_iso_string);
            lines.push(format!("  Latest import checkpoint: {}", checkpoint.unwrap_or_else(|| "Not available".into())));
            if let Some(p) = progress.filter(|p| js_truthy(p)) {
                let n = |key: &str| p.get(key).map(crate::output::js_string).unwrap_or_else(|| "undefined".into());
                lines.push(format!(
                    "  Enabled streams: {}/{} with checkpoints; {} need attention",
                    n("checkpointStreamCount"),
                    n("enabledStreamCount"),
                    n("attentionStreamCount")
                ));
            }
            lines.push("  Checkpoints describe import progress, not record dates or complete history.".to_string());
        }
    }
    lines.push("  Record date range: Not measured".to_string());
    lines.push(format!("  Dataset:     {}", t("datasetId").unwrap_or_else(|| dim("—"))));
    lines.push(format!("  Created:     {}", time_ago(t("createdAt").as_deref())));
    if let Some(error) = js_if(field("lastError")) {
        lines.push(format!("  Error:       {}", red(&error)));
    }
    if let Some(n) = failures(src) {
        lines.push(format!("  Failures:    {} consecutive", red(&n)));
    }
    if let Some(job) = js_if(field("latestJobId")) {
        lines.push(format!("  Latest Job:  {}", dim(&job)));
    }
    if let Some(Value::Array(tasks)) = field("importTasks").filter(|t| matches!(t, Value::Array(a) if !a.is_empty())) {
        lines.push(format!("  Tasks:       {}", js_join(tasks, ", ")));
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

pub fn dry_run_fields(src: &Value) -> Vec<(&'static str, String)> {
    vec![("Name", text(src, "appName").unwrap_or_else(|| "—".into())), ("State", js_color_status(src.get("state")))]
}

pub struct CreateArgs {
    pub app: String,
    pub sync_type: String,
    pub import_tasks: Option<String>,
    pub frequency: String,
    pub unit: String,
    pub config_file: Option<String>,
    pub run_now: bool,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn create(ctx: &Ctx, args: CreateArgs) -> Result<()> {
    let mut body = Map::new();
    // A short app ID is looked up below, once the files are read (VE-3831).
    body.insert("appId".into(), json!(args.app));
    body.insert("syncType".into(), json!(args.sync_type));
    body.insert("syncFrequencyValue".into(), js_number_value(&args.frequency));
    body.insert("syncFrequencyUnit".into(), json!(args.unit));
    body.insert("runNow".into(), json!(args.run_now));
    if let Some(tasks) = args.import_tasks.filter(|t| !t.is_empty()) {
        body.insert("importTasks".into(), Value::Array(split_list(&tasks)));
    }
    if let Some(path) = args.config_file.filter(|p| !p.is_empty()) {
        body.insert("config".into(), read_json_file(&path)?);
    }
    let client = ctx.client()?;
    body.insert("appId".into(), json!(resolve(&client, Listing::Apps, &args.app).await?));
    let res = run_action("Creating source...", client.post("/sources", Some(Value::Object(body)))).await?;
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let src = payload(&res);
    if !js_truthy(src) {
        print_success("Source created.");
        return Ok(());
    }
    if mode == OutputMode::Field {
        println!("{}", js_template(src.get("id")));
        return Ok(());
    }
    print_success(&format!(
        "Source {} ({}) created.",
        bold(&js_template(src.get("syncType"))),
        short_id(&text(src, "id").unwrap_or_default())
    ));
    print_label("App", src.get("appId"));
    print_status_label("State", src.get("state"));
    Ok(())
}

pub struct UpdateArgs {
    pub import_tasks: Option<String>,
    pub frequency: Option<String>,
    pub unit: Option<String>,
    pub config_file: Option<String>,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn update(ctx: &Ctx, source_id: &str, args: UpdateArgs) -> Result<()> {
    let mut body = Map::new();
    if let Some(tasks) = &args.import_tasks {
        body.insert("importTasks".into(), Value::Array(split_list(tasks)));
    }
    if let Some(frequency) = &args.frequency {
        body.insert("syncFrequencyValue".into(), js_number_value(frequency));
    }
    if let Some(unit) = &args.unit {
        body.insert("syncFrequencyUnit".into(), json!(unit));
    }
    if let Some(path) = args.config_file.as_deref().filter(|p| !p.is_empty()) {
        body.insert("config".into(), read_json_file(path)?);
    }
    if body.is_empty() {
        bail!("Nothing to update — pass at least one flag.");
    }
    let client = ctx.client()?;
    let source_id = &resolve(&client, Listing::Sources, source_id).await?;
    let res =
        run_action("Updating source...", client.patch(&format!("/sources/{source_id}"), Value::Object(body))).await?;
    match resolve_output_mode(args.json, args.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{source_id}"),
        OutputMode::Table => print_success(&format!("Source {} updated.", short_id(source_id))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(progress: Option<Value>) -> Value {
        let mut v = json!({
            "id": "source-1", "appId": "app-1", "syncType": "mongodb", "state": "active",
            "integrationStatus": "warning", "createdAt": "2026-01-01T00:00:00Z", "lastSyncAt": "2026-09-30T01:00:00Z",
            "earliestDataAt": "2020-01-01T00:00:00Z", "latestDataAt": "2025-01-01T00:00:00Z",
        });
        if let Some(progress) = progress {
            v["importProgress"] = progress;
        }
        v
    }

    #[test]
    fn renders_the_server_checkpoint_not_legacy_dates() {
        let out = render_source(
            &source(Some(
                json!({ "latestCheckpointAt": "2026-09-30T00:00:00Z", "enabledStreamCount": 3, "checkpointStreamCount": 2, "attentionStreamCount": 1 }),
            )),
            None,
        );
        assert!(out.contains("Latest import checkpoint: 2026-09-30T00:00:00.000Z"));
        assert!(out.contains("2/3 with checkpoints; 1 need attention"));
        assert!(out.contains("Last successful sync:"));
        assert!(out.contains("Record date range: Not measured"));
        assert!(!out.contains("2020") && !out.contains("2025"));
    }

    #[test]
    fn older_servers_show_not_available() {
        let out = render_source(&source(None), None);
        assert!(out.contains("Latest import checkpoint: Not available"));
        assert!(!out.contains("Data Range:") && !out.contains("undefined/undefined"));
    }

    #[test]
    fn read_in_place_sources_are_not_labelled_as_imports() {
        let out = render_source(&source(Some(Value::Null)), None);
        assert!(out.contains("Data access: Read in place"));
        assert!(!out.contains("Latest import checkpoint:"));
    }

    #[test]
    fn invalid_checkpoint_dates_are_not_printed() {
        let out = render_source(
            &source(Some(
                json!({ "latestCheckpointAt": "invalid", "enabledStreamCount": 0, "checkpointStreamCount": 0, "attentionStreamCount": 0 }),
            )),
            None,
        );
        assert!(out.contains("Latest import checkpoint: Not available"));
        assert!(!out.contains("Invalid Date"));
    }

    #[test]
    fn get_prints_missing_and_null_fields_like_the_ts_cli() {
        // Expected output from the TS CLI 0.3.1 run against a local stub with the same body (VE-3728).
        let src = json!({
            "id": "src-1", "appName": null, "syncType": null, "appId": null, "state": 5, "integrationStatus": null,
            "importProgress": "x", "lastError": 0, "consecutiveFailures": "2", "latestJobId": "",
            "importTasks": [null, "orders"],
        });
        assert_eq!(
            render_source(&src, None),
            "
null (null)

  ID:          src-1
  App:         — null
  Type:        null
  State:       5
  Status:      null
  Progress:    —
  Frequency:   —
  Anchor:      — 
  Last successful sync: —
  Latest import checkpoint: Not available
  Enabled streams: undefined/undefined with checkpoints; undefined need attention
  Checkpoints describe import progress, not record dates or complete history.
  Record date range: Not measured
  Dataset:     —
  Created:     —
  Failures:    2 consecutive
  Tasks:       , orders
"
        );
        let src = json!({
            "id": "src-2", "importProgress": null, "lastError": "bad", "latestJobId": "job-12345678901234",
            "consecutiveFailures": 0, "importTasks": [],
        });
        assert_eq!(
            render_source(&src, None),
            "
undefined (undefined)

  ID:          src-2
  App:         — undefined
  Type:        undefined
  State:       undefined
  Status:      undefined
  Progress:    —
  Frequency:   —
  Anchor:      — 
  Last successful sync: —
  Data access: Read in place; no import checkpoint
  Record date range: Not measured
  Dataset:     —
  Created:     —
  Error:       bad
  Latest Job:  job-12345678901234
"
        );
    }

    #[test]
    fn dry_run_rows_follow_javascript() {
        assert_eq!(
            dry_run_fields(&json!({ "appName": null, "state": 5 })),
            [("Name", "—".into()), ("State", "5".into())]
        );
        assert_eq!(dry_run_fields(&json!({})), [("Name", "—".into()), ("State", "undefined".to_string())]);
    }
}

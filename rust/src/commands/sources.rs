//! `vendo sources …` (port of `src/commands/sources.ts`).

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    commands::pipeline_resource::{js_number_value, read_json_file, split_list},
    context::Ctx,
    jobs::{Job, format_job_progress},
    output::{
        OutputMode, bold, color_status, dim, js_date_parse, js_iso_string, print_count, print_field, print_json,
        print_label, print_success, red, resolve_output_mode, run_action, short_id, table, time_ago,
    },
    watch::{self, ResourceKind},
};

fn text(v: &Value, key: &str) -> Option<String> {
    Job(v).text(key)
}

fn failures(v: &Value) -> Option<f64> {
    v.get("consecutiveFailures").and_then(Value::as_f64).filter(|n| *n > 0.0)
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
        ("app_id", args.app),
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
    let mut active_by_source = std::collections::HashMap::new();
    for job in &active {
        if let Some(source_id) = text(job, "sourceId").filter(|s| !s.is_empty()) {
            active_by_source.insert(source_id, job.clone());
        }
    }
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    let mut grid = table(&["ID", "Name", "Type", "Status", "Progress", "Failures", "Last Sync"]);
    for source in &rows {
        let id = text(source, "id").unwrap_or_default();
        grid.add_row(vec![
            dim(&short_id(&id)),
            text(source, "appName").unwrap_or_else(|| dim("—")),
            text(source, "syncType").unwrap_or_default(),
            color_status(&text(source, "integrationStatus").unwrap_or_default()),
            format_job_progress(active_by_source.get(&id).map(Job)),
            failures(source).map(|n| red(&crate::output::js_number_string(n))).unwrap_or_else(|| dim("0")),
            time_ago(text(source, "lastSyncAt").as_deref()),
        ]);
    }
    println!("{grid}");
    let total = res.pointer("/meta/pagination/total").and_then(Value::as_u64).unwrap_or(rows.len() as u64);
    print_count(total, "source");
    Ok(())
}

pub async fn get(ctx: &Ctx, source_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let path = format!("/sources/{source_id}");
    if json {
        let res = run_action("Fetching source...", client.get(&path, &[])).await?;
        print_json(&res);
        return Ok(());
    }
    let (res, active) = run_action("Fetching source...", async {
        tokio::try_join!(
            client.get(&path, &[]),
            watch::active_job_for_resource(&client, ResourceKind::Source, source_id)
        )
    })
    .await?;
    print!("{}", render_source(payload(&res), active.as_ref()));
    Ok(())
}

/// The `sources get` text view (the TS action's `console.log` lines).
pub fn render_source(src: &Value, active: Option<&Value>) -> String {
    let t = |key: &str| text(src, key);
    let sync_type = t("syncType").unwrap_or_default();
    let mut lines = vec![
        String::new(),
        format!("{} {}", bold(&t("appName").unwrap_or_else(|| sync_type.clone())), dim(&format!("({sync_type})"))),
        String::new(),
        format!("  ID:          {}", t("id").unwrap_or_default()),
        format!("  App:         {} {}", t("appName").unwrap_or_else(|| dim("—")), dim(&t("appId").unwrap_or_default())),
        format!("  Type:        {sync_type}"),
        format!("  State:       {}", color_status(&t("state").unwrap_or_default())),
        format!("  Status:      {}", color_status(&t("integrationStatus").unwrap_or_default())),
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
            if let Some(p) = progress.filter(|p| p.is_object()) {
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
    if let Some(error) = t("lastError").filter(|s| !s.is_empty()) {
        lines.push(format!("  Error:       {}", red(&error)));
    }
    if let Some(n) = failures(src) {
        lines.push(format!("  Failures:    {} consecutive", red(&crate::output::js_number_string(n))));
    }
    if let Some(job) = t("latestJobId").filter(|s| !s.is_empty()) {
        lines.push(format!("  Latest Job:  {}", dim(&job)));
    }
    let tasks: Vec<String> = src
        .get("importTasks")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(crate::output::js_string).collect())
        .unwrap_or_default();
    if !tasks.is_empty() {
        lines.push(format!("  Tasks:       {}", tasks.join(", ")));
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

pub fn dry_run_fields(src: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("Name", text(src, "appName").unwrap_or_else(|| "—".into())),
        ("State", color_status(&text(src, "state").unwrap_or_default())),
    ]
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
    let res = run_action("Creating source...", client.post("/sources", Some(Value::Object(body)))).await?;
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let src = payload(&res);
    if src.is_null() {
        print_success("Source created.");
        return Ok(());
    }
    if mode == OutputMode::Field {
        println!("{}", text(src, "id").unwrap_or_default());
        return Ok(());
    }
    print_success(&format!(
        "Source {} ({}) created.",
        bold(&text(src, "syncType").unwrap_or_default()),
        short_id(&text(src, "id").unwrap_or_default())
    ));
    print_label("App", src.get("appId"));
    print_label("State", Some(&json!(color_status(&text(src, "state").unwrap_or_default()))));
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
}

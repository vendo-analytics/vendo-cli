//! `vendo integrations …` / `vendo int …` (port of `src/commands/integrations.ts`).

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    commands::pipeline_resource::{js_number_value, read_json_file},
    context::Ctx,
    jobs::{Job, format_job_progress},
    output::{
        OutputMode, bold, camel_case_keys_deep, color_status, dim, js_color_status, js_if, js_positive, js_stringify,
        js_template, js_truthy, print_field, print_json, print_label, print_list_count, print_status_label,
        print_success, red, resolve_output_mode, run_action, short_id, table, time_ago,
    },
    source_refresh::{Tone, resolve_refresh_window, summarize},
    watch::{self, ResourceKind},
};

fn text(v: &Value, key: &str) -> Option<String> {
    Job(v).text(key)
}

pub struct ListArgs {
    pub state: Option<String>,
    pub status: Option<String>,
    pub data_type: Option<String>,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let client = ctx.client()?;
    let query = [
        ("state", args.state),
        ("status", args.status),
        ("data_type", args.data_type),
        ("limit", Some(args.limit)),
        ("offset", Some(args.offset)),
    ];
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    if mode != OutputMode::Table {
        let res = run_action("Fetching integrations...", client.get("/integrations", &query)).await?;
        match mode {
            OutputMode::Json => print_json(&res),
            _ => print_field(
                payload(&res).as_array().map(Vec::as_slice).unwrap_or_default(),
                args.output.as_deref().unwrap_or_default(),
            ),
        }
        return Ok(());
    }
    let (res, active) = run_action("Fetching integrations...", async {
        tokio::try_join!(client.get("/integrations", &query), watch::active_jobs(&client, 100, None, None))
    })
    .await?;
    let mut active_by_integration = std::collections::HashMap::new();
    for job in &active {
        if let Some(id) = text(job, "integrationId").filter(|s| !s.is_empty()) {
            active_by_integration.insert(id, job.clone());
        }
    }
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    let mut grid = table(&["ID", "Source", "Destination", "Data Type", "Status", "Progress", "Last Sync"]);
    for int in &rows {
        let id = text(int, "id").unwrap_or_default();
        grid.add_row(vec![
            dim(&short_id(&id)),
            text(int, "sourceAppName").unwrap_or_else(|| dim("—")),
            text(int, "destinationAppName").unwrap_or_else(|| dim("—")),
            text(int, "dataType").unwrap_or_default(),
            color_status(&text(int, "status").unwrap_or_default()),
            format_job_progress(active_by_integration.get(&id).map(Job)),
            time_ago(text(int, "lastSyncAt").as_deref()),
        ]);
    }
    println!("{grid}");
    print_list_count(&res, rows.len(), "integration");
    Ok(())
}

pub async fn get(ctx: &Ctx, integration_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let path = format!("/integrations/{integration_id}");
    if json {
        let res = run_action("Fetching integration...", client.get(&path, &[])).await?;
        print_json(&res);
        return Ok(());
    }
    let (res, active) = run_action("Fetching integration...", async {
        tokio::try_join!(
            client.get(&path, &[]),
            watch::active_job_for_resource(&client, ResourceKind::Integration, integration_id)
        )
    })
    .await?;
    for line in integration_lines(payload(&res), active.as_ref()) {
        println!("{line}");
    }
    Ok(())
}

/// The `integrations get` view, with the TS CLI's `${…}` rendering of missing (`undefined`) and null fields.
fn integration_lines(int: &Value, active: Option<&Value>) -> Vec<String> {
    let t = |key: &str| text(int, key);
    let field = |key: &str| int.get(key);
    let data_type = js_template(field("dataType"));
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            bold(&format!(
                "{} → {}",
                t("sourceAppName").unwrap_or_else(|| "—".into()),
                t("destinationAppName").unwrap_or_else(|| "—".into())
            )),
            dim(&format!("({data_type})"))
        ),
        String::new(),
        format!("  ID:            {}", js_template(field("id"))),
        format!(
            "  Source App:    {} {}",
            t("sourceAppName").unwrap_or_else(|| dim("—")),
            dim(&t("sourceAppId").unwrap_or_default())
        ),
        format!(
            "  Dest App:     {} {}",
            t("destinationAppName").unwrap_or_else(|| dim("—")),
            dim(&js_template(field("destinationAppId")))
        ),
        format!("  Data Type:    {data_type}"),
        format!("  State:        {}", js_color_status(field("state"))),
        format!("  Status:       {}", js_color_status(field("status"))),
        format!("  Progress:     {}", format_job_progress(active.map(Job))),
        format!("  Last Sync:    {}", time_ago(t("lastSyncAt").as_deref())),
        format!("  Created:      {}", time_ago(t("createdAt").as_deref())),
    ];
    lines.extend(schedule_line(int));
    if let Some(error) = js_if(field("lastError")) {
        lines.push(format!("  Error:        {}", red(&error)));
    }
    if let Some(n) = js_positive(field("consecutiveFailures")) {
        lines.push(format!("  Failures:     {} consecutive", red(&n)));
    }
    if let Some(job) = js_if(field("latestJobId")) {
        lines.push(format!("  Latest Job:   {}", dim(&job)));
    }
    lines
}

/// `JSON.stringify(int.schedule)` after the TS client camelCased the response, when truthy.
fn schedule_line(int: &Value) -> Option<String> {
    let schedule = int.get("schedule").filter(|s| js_truthy(s))?;
    Some(format!("  Schedule:     {}", js_stringify(&camel_case_keys_deep(schedule))))
}

pub fn dry_run_fields(int: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("Source", text(int, "sourceAppName").unwrap_or_else(|| "—".into())),
        ("Destination", text(int, "destinationAppName").unwrap_or_else(|| "—".into())),
        ("Data Type", js_template(int.get("dataType"))),
    ]
}

pub async fn refresh_source(
    ctx: &Ctx,
    integration_id: &str,
    from: Option<String>,
    to: Option<String>,
    json: bool,
) -> Result<std::process::ExitCode> {
    let window = resolve_refresh_window(from.as_deref(), to.as_deref(), jiff::Timestamp::now().as_millisecond())?;
    let client = ctx.client()?;
    let body = json!({ "requestedStart": window.requested_start, "requestedEnd": window.requested_end });
    let res = run_action(
        "Checking source data availability...",
        client.post(&format!("/integrations/{integration_id}/refresh-source"), Some(body)),
    )
    .await?;
    let summary = summarize(payload(&res));
    if json {
        // stdout stays pure JSON, but `unavailable` still fails so
        // `refresh-source --json && sync` stops (VE-1603).
        print_json(&res);
        return Ok(if summary.tone == Tone::Error {
            std::process::ExitCode::from(1)
        } else {
            std::process::ExitCode::SUCCESS
        });
    }
    if summary.tone == Tone::Error {
        return Err(anyhow!(summary.headline));
    }
    print_success(&summary.headline);
    println!("{}", dim(&format!("Window: {} → {}", window.requested_start, window.requested_end)));
    for job_id in &summary.job_ids {
        println!("  Import job: {job_id}");
    }
    if let Some(first) = summary.job_ids.first() {
        println!();
        println!("{}", dim(&format!("Follow progress: vendo jobs tail {first}")));
    }
    Ok(std::process::ExitCode::SUCCESS)
}

pub struct CreateArgs {
    pub dest_app: String,
    pub source_app: Option<String>,
    pub data_type: String,
    pub config_file: String,
    pub schedule_file: Option<String>,
    pub frequency: String,
    pub unit: String,
    pub run_now: bool,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn create(ctx: &Ctx, args: CreateArgs) -> Result<()> {
    let config = read_json_file(&args.config_file)?;
    let schedule = match args.schedule_file.as_deref().filter(|p| !p.is_empty()) {
        Some(path) => read_json_file(path)?,
        None => json!({
            "frequencyValue": js_number_value(&args.frequency),
            "frequencyUnit": args.unit,
            "runNow": args.run_now,
        }),
    };
    // `sourceAppId: undefined` is dropped by JSON.stringify.
    let mut body = Map::new();
    body.insert("destinationAppId".into(), json!(args.dest_app));
    if let Some(source_app) = args.source_app {
        body.insert("sourceAppId".into(), json!(source_app));
    }
    body.insert("dataType".into(), json!(args.data_type));
    body.insert("config".into(), config);
    body.insert("schedule".into(), schedule);

    let client = ctx.client()?;
    let res = run_action("Creating integration...", client.post("/integrations", Some(Value::Object(body)))).await?;
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let int = payload(&res);
    if !js_truthy(int) {
        print_success("Integration created.");
        return Ok(());
    }
    if mode == OutputMode::Field {
        println!("{}", js_template(int.get("id")));
        return Ok(());
    }
    print_success(&format!("Integration {} created.", short_id(&text(int, "id").unwrap_or_default())));
    print_label("Data type", int.get("dataType"));
    print_status_label("State", int.get("state"));
    Ok(())
}

pub struct UpdateArgs {
    pub config_file: Option<String>,
    pub schedule_file: Option<String>,
    pub frequency: Option<String>,
    pub unit: Option<String>,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn update(ctx: &Ctx, integration_id: &str, args: UpdateArgs) -> Result<()> {
    let mut body = Map::new();
    if let Some(path) = args.config_file.as_deref().filter(|p| !p.is_empty()) {
        body.insert("config".into(), read_json_file(path)?);
    }
    if let Some(path) = args.schedule_file.as_deref().filter(|p| !p.is_empty()) {
        body.insert("schedule".into(), read_json_file(path)?);
    } else if args.frequency.is_some() || args.unit.is_some() {
        let mut schedule = Map::new();
        if let Some(frequency) = &args.frequency {
            schedule.insert("frequencyValue".into(), js_number_value(frequency));
        }
        if let Some(unit) = &args.unit {
            schedule.insert("frequencyUnit".into(), json!(unit));
        }
        body.insert("schedule".into(), Value::Object(schedule));
    }
    if body.is_empty() {
        bail!("Nothing to update — pass at least one flag.");
    }
    let client = ctx.client()?;
    let res = run_action(
        "Updating integration...",
        client.patch(&format!("/integrations/{integration_id}"), Value::Object(body)),
    )
    .await?;
    match resolve_output_mode(args.json, args.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{integration_id}"),
        OutputMode::Table => print_success(&format!("Integration {} updated.", short_id(integration_id))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schedule_row_prints_camel_case_keys_like_ts() {
        let int = json!({ "schedule": { "frequency_value": 1.0, "frequency_unit": "days", "run_now": false } });
        assert_eq!(
            schedule_line(&int).as_deref(),
            Some(r#"  Schedule:     {"frequencyValue":1,"frequencyUnit":"days","runNow":false}"#)
        );
        assert_eq!(schedule_line(&json!({ "schedule": null })), None);
        assert_eq!(schedule_line(&json!({ "schedule": "" })), None);
        assert_eq!(schedule_line(&json!({})), None);
    }

    #[test]
    fn get_prints_missing_and_null_fields_like_the_ts_cli() {
        // Expected lines from the TS CLI 0.3.1 run against a local stub with the same body (VE-3728).
        let int = json!({
            "id": "int-1", "dataType": null, "destinationAppId": null, "state": null, "status": 7, "lastError": "",
            "consecutiveFailures": true, "latestJobId": 0, "schedule": { "frequency_value": 3 },
        });
        assert_eq!(
            integration_lines(&int, None),
            [
                "",
                "— → — (null)",
                "",
                "  ID:            int-1",
                "  Source App:    — ",
                "  Dest App:     — null",
                "  Data Type:    null",
                "  State:        null",
                "  Status:       7",
                "  Progress:     —",
                "  Last Sync:    —",
                "  Created:      —",
                "  Schedule:     {\"frequencyValue\":3}",
                "  Failures:     true consecutive",
            ]
        );
        let int = json!({ "id": "int-2", "lastError": "x", "consecutiveFailures": "4", "latestJobId": "j-1", "sourceAppName": "S" });
        assert_eq!(
            integration_lines(&int, None),
            [
                "",
                "S → — (undefined)",
                "",
                "  ID:            int-2",
                "  Source App:    S ",
                "  Dest App:     — undefined",
                "  Data Type:    undefined",
                "  State:        undefined",
                "  Status:       undefined",
                "  Progress:     —",
                "  Last Sync:    —",
                "  Created:      —",
                "  Error:        x",
                "  Failures:     4 consecutive",
                "  Latest Job:   j-1",
            ]
        );
        assert_eq!(dry_run_fields(&json!({ "dataType": null }))[2], ("Data Type", "null".to_string()));
    }
}

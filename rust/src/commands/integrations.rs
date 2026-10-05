//! `vendo integrations …` / `vendo int …` (port of `src/commands/integrations.ts`).

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    commands::pipeline_resource::{js_number_value, read_json_file},
    context::Ctx,
    jobs::{Job, format_job_progress},
    output::{
        OutputMode, bold, color_status, dim, js_truthy, print_count, print_field, print_json, print_label,
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
    let total = res.pointer("/meta/pagination/total").and_then(Value::as_u64).unwrap_or(rows.len() as u64);
    print_count(total, "integration");
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
    let int = payload(&res);
    let t = |key: &str| text(int, key);
    let data_type = t("dataType").unwrap_or_default();
    println!();
    println!(
        "{} {}",
        bold(&format!(
            "{} → {}",
            t("sourceAppName").unwrap_or_else(|| "—".into()),
            t("destinationAppName").unwrap_or_else(|| "—".into())
        )),
        dim(&format!("({data_type})"))
    );
    println!();
    println!("  ID:            {}", t("id").unwrap_or_default());
    println!(
        "  Source App:    {} {}",
        t("sourceAppName").unwrap_or_else(|| dim("—")),
        dim(&t("sourceAppId").unwrap_or_default())
    );
    println!(
        "  Dest App:     {} {}",
        t("destinationAppName").unwrap_or_else(|| dim("—")),
        dim(&t("destinationAppId").unwrap_or_default())
    );
    println!("  Data Type:    {data_type}");
    println!("  State:        {}", color_status(&t("state").unwrap_or_default()));
    println!("  Status:       {}", color_status(&t("status").unwrap_or_default()));
    println!("  Progress:     {}", format_job_progress(active.as_ref().map(Job)));
    println!("  Last Sync:    {}", time_ago(t("lastSyncAt").as_deref()));
    println!("  Created:      {}", time_ago(t("createdAt").as_deref()));
    if let Some(schedule) = int.get("schedule").filter(|s| js_truthy(s)) {
        println!("  Schedule:     {schedule}");
    }
    if let Some(error) = t("lastError").filter(|s| !s.is_empty()) {
        println!("  Error:        {}", red(&error));
    }
    if let Some(n) = int.get("consecutiveFailures").and_then(Value::as_f64).filter(|n| *n > 0.0) {
        println!("  Failures:     {} consecutive", red(&crate::output::js_number_string(n)));
    }
    if let Some(job) = t("latestJobId").filter(|s| !s.is_empty()) {
        println!("  Latest Job:   {}", dim(&job));
    }
    Ok(())
}

pub fn dry_run_fields(int: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("Source", text(int, "sourceAppName").unwrap_or_else(|| "—".into())),
        ("Destination", text(int, "destinationAppName").unwrap_or_else(|| "—".into())),
        ("Data Type", text(int, "dataType").unwrap_or_else(|| "undefined".into())),
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
    if int.is_null() {
        print_success("Integration created.");
        return Ok(());
    }
    if mode == OutputMode::Field {
        println!("{}", text(int, "id").unwrap_or_default());
        return Ok(());
    }
    print_success(&format!("Integration {} created.", short_id(&text(int, "id").unwrap_or_default())));
    print_label("Data type", int.get("dataType"));
    print_label("State", Some(&json!(color_status(&text(int, "state").unwrap_or_default()))));
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

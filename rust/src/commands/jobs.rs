//! `vendo jobs list|get|cancel|watch|tail` (port of `src/commands/jobs.ts`).

use std::{process::ExitCode, time::Duration};

use anyhow::Result;
use serde_json::Value;

use crate::{
    client::payload,
    context::Ctx,
    jobs::{Job, format_job_duration, format_job_progress, job_detail_lines, job_error_lines},
    output::{
        OutputMode, arg_error, bold, color_status, confirm, dim, print_count, print_dry_run, print_field, print_json,
        print_success, resolve_output_mode, run_action, short_id, table, time_ago,
    },
    watch::{self, MAX_WAIT, NextJob, ResourceKind, Terminal, WatchScope},
};

pub struct ListArgs {
    pub status: Option<String>,
    pub job_type: Option<String>,
    pub source: Option<String>,
    pub integration: Option<String>,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action(
        "Fetching jobs...",
        client.get(
            "/jobs",
            &[
                ("status", args.status),
                ("job_type", args.job_type),
                ("source_id", args.source),
                ("integration_id", args.integration),
                ("limit", Some(args.limit)),
                ("offset", Some(args.offset)),
            ],
        ),
    )
    .await?;

    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    match resolve_output_mode(args.json, args.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => print_field(&rows, args.output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            let mut grid = table(&["ID", "Type", "Connector", "Status", "Progress", "Started", "Duration"]);
            for value in &rows {
                let job = Job(value);
                grid.add_row(vec![
                    dim(&short_id(&job.id())),
                    job.text("jobType").unwrap_or_default(),
                    job.text("connectorType").unwrap_or_else(|| dim("—")),
                    color_status(&job.status()),
                    format_job_progress(Some(job)),
                    time_ago(job.started_or_created().as_deref()),
                    format_job_duration(job.text("startedAt").as_deref(), job.text("finishedAt").as_deref()),
                ]);
            }
            println!("{grid}");
            let total = res.pointer("/meta/pagination/total").and_then(Value::as_u64).unwrap_or(rows.len() as u64);
            print_count(total, "job");
        }
    }
    Ok(())
}

pub async fn get(ctx: &Ctx, job_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching job...", client.get(&format!("/jobs/{job_id}"), &[])).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    let job = Job(payload(&res));
    println!();
    println!(
        "{} {}",
        bold(&format!("{} job", job.text("jobType").unwrap_or_else(|| "undefined".into()))),
        dim(&format!("({})", job.text("connectorType").unwrap_or_else(|| "unknown".into())))
    );
    println!();
    for line in job_detail_lines(job) {
        println!("{line}");
    }
    println!("  Finished:      {}", time_ago(job.text("finishedAt").as_deref()));
    let errors = job_error_lines(job);
    if !errors.is_empty() {
        println!();
        for line in errors {
            println!("{line}");
        }
    }
    Ok(())
}

pub async fn cancel(
    ctx: &Ctx,
    job_id: &str,
    json: bool,
    yes: bool,
    dry_run: bool,
    output: Option<String>,
) -> Result<()> {
    if dry_run {
        print_dry_run("cancel", "job", job_id, &[]);
        return Ok(());
    }
    if !yes && !json && !confirm(&format!("Cancel job {}?", short_id(job_id))) {
        return Ok(());
    }
    let client = ctx.client()?;
    let res = run_action("Cancelling job...", client.post(&format!("/jobs/{job_id}/cancel"), None)).await?;
    match resolve_output_mode(json, output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{job_id}"),
        OutputMode::Table => print_success(&format!("Job {} cancelled.", short_id(job_id))),
    }
    Ok(())
}

/// `Number(value)` from JavaScript, for `--interval`: whitespace-trimmed,
/// empty is 0. Returns the interval only when finite and positive.
pub fn parse_interval_seconds(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    let n = if trimmed.is_empty() { 0.0 } else { trimmed.parse::<f64>().ok()? };
    (n.is_finite() && n > 0.0).then_some(n)
}

fn interval_ms(seconds: f64) -> Duration {
    Duration::from_millis((seconds * 1000.0).floor() as u64)
}

pub async fn watch(ctx: &Ctx, interval: &str, source: Option<String>, integration: Option<String>) -> Result<()> {
    let Some(seconds) = parse_interval_seconds(interval) else {
        arg_error(
            "Polling interval must be a positive number of seconds.",
            &["vendo jobs watch", "vendo jobs watch --interval 10"],
        );
    };
    let client = ctx.client()?;
    let scope = WatchScope { source_id: source, integration_id: integration };
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    watch::watch_active_jobs(&client, &mut Terminal::default(), interval_ms(seconds), &scope, stop).await?;
    Ok(())
}

pub struct TailArgs {
    pub job_id: Option<String>,
    pub source: Option<String>,
    pub integration: Option<String>,
    pub next: bool,
    pub interval: String,
}

pub async fn tail(ctx: &Ctx, args: TailArgs) -> Result<ExitCode> {
    let targets = [&args.job_id, &args.source, &args.integration].iter().filter(|t| t.is_some()).count();
    if targets != 1 {
        arg_error(
            "Specify exactly one target: <jobId>, --source <sourceId>, or --integration <integrationId>.",
            &[
                "vendo jobs tail <jobId>",
                "vendo jobs tail --source <sourceId>",
                "vendo jobs tail --integration <integrationId>",
                "vendo jobs tail --source <sourceId> --next",
            ],
        );
    }
    let Some(seconds) = parse_interval_seconds(&args.interval) else {
        arg_error(
            "Polling interval must be a positive number of seconds.",
            &["vendo jobs tail <jobId>", "vendo jobs tail --source <sourceId> --interval 5"],
        );
    };
    let interval = interval_ms(seconds);
    let mut screen = Terminal::default();

    if let Some(job_id) = &args.job_id {
        if args.next {
            arg_error(
                "`--next` can only be used with `--source` or `--integration`.",
                &["vendo jobs tail --source <sourceId> --next", "vendo jobs tail --integration <integrationId> --next"],
            );
        }
        let client = ctx.client()?;
        watch::tail_job(&client, &mut screen, job_id, interval, MAX_WAIT).await?;
        return Ok(ExitCode::SUCCESS);
    }

    let (resource_id, kind) = match (&args.source, &args.integration) {
        (Some(source), _) => (source.clone(), ResourceKind::Source),
        (None, Some(integration)) => (integration.clone(), ResourceKind::Integration),
        (None, None) => unreachable!("exactly one target was checked above"),
    };
    let client = ctx.client()?;
    let next = if args.next {
        let baseline = watch::latest_job_for_resource(&client, &resource_id, kind).await?;
        NextJob {
            after_created_at: baseline.as_ref().and_then(|j| Job(j).text("createdAt")),
            skip_job_id: baseline.as_ref().map(|j| Job(j).id()),
        }
    } else {
        NextJob { after_created_at: None, skip_job_id: None }
    };
    watch::watch_job(&client, &mut screen, &resource_id, kind, interval, next, MAX_WAIT).await?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_follows_javascript_number() {
        assert_eq!(parse_interval_seconds("5"), Some(5.0));
        assert_eq!(parse_interval_seconds(" 2.5 "), Some(2.5));
        assert_eq!(parse_interval_seconds("1e1"), Some(10.0));
        for bad in ["0", "-1", "", "abc", "inf", "NaN"] {
            assert_eq!(parse_interval_seconds(bad), None, "{bad}");
        }
    }
}

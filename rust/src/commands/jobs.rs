//! `vendo jobs list|get|cancel|watch|tail` (port of `src/commands/jobs.ts`).

use std::{process::ExitCode, time::Duration};

use anyhow::Result;
use serde_json::Value;

use crate::{
    browse,
    client::{Client, payload},
    context::Ctx,
    jobs::{Job, format_job_duration, format_job_progress, job_detail_lines, job_error_lines},
    output::{
        OutputMode, arg_error, bold, color_status, confirm, dim, list_count, print_dry_run, print_field, print_json,
        print_success, resolve_output_mode, run_action, short_id, time_ago,
    },
    short_ids::{Listing, resolve, resolve_opt},
    watch::{self, JsonScreen, MAX_WAIT, NextJob, ResourceKind, Screen, Terminal, WatchScope},
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
    let source = resolve_opt(&client, Listing::Sources, args.source).await?;
    let integration = resolve_opt(&client, Listing::Destinations, args.integration).await?;
    let res = run_action(
        "Fetching jobs...",
        client.get(
            "/jobs",
            &[
                ("status", args.status),
                ("job_type", args.job_type),
                ("source_id", source),
                ("integration_id", integration),
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
            // The table, or at a terminal the same rows to choose from (VE-3894).
            let table = browse::Table {
                header: &["ID", "Type", "Connector", "Status", "Progress", "Started", "Duration"],
                cells: rows.iter().map(|job| job_cells(Job(job))).collect(),
                footer: list_count(&res, rows.len(), "job"),
            };
            browse::shown(&client, browse::Group::Jobs, &rows, table, args.output.as_deref()).await?;
        }
    }
    Ok(())
}

/// A row of the `jobs list` table, its cells styled as the table shows them; a selectable list shows
/// them plain (VE-3894).
fn job_cells(job: Job) -> Vec<String> {
    vec![
        dim(&short_id(&job.id())),
        job.text("jobType").unwrap_or_default(),
        job.text("connectorType").unwrap_or_else(|| dim("—")),
        color_status(&job.status()),
        format_job_progress(Some(job)),
        time_ago(job.started_or_created().as_deref()),
        format_job_duration(job.text("startedAt").as_deref(), job.text("finishedAt").as_deref()),
    ]
}

pub async fn get(ctx: &Ctx, job_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let job_id = resolve(&client, Listing::Jobs, job_id).await?;
    if json {
        print_json(&fetch(&client, &job_id).await?);
        return Ok(());
    }
    show(&client, &job_id).await.map(|_| ())
}

/// `GET /jobs/<id>`, behind `jobs get`'s spinner.
async fn fetch(client: &Client, id: &str) -> Result<Value> {
    Ok(run_action("Fetching job...", client.get(&format!("/jobs/{id}"), &[])).await?)
}

/// What `jobs get` shows of the job `id`, a full ID (no short-ID lookup), from its request behind its
/// spinner; the job as the API sent it. A selectable list shows it for the job chosen and reads the
/// actions that apply from it (VE-3894).
pub(crate) async fn show(client: &Client, id: &str) -> Result<Value> {
    let res = fetch(client, id).await?;
    let job = Job(payload(&res));
    println!();
    println!(
        "{} {}",
        bold(&format!("{} job", job.template("jobType"))),
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
    Ok(job.0.clone())
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
    if !confirm(yes, &format!("Cancel job {}?", short_id(job_id)), &format!("This cancels job {job_id}."))? {
        return Ok(());
    }
    let client = ctx.client()?;
    // After the consent, which names the ID as typed (VE-3823).
    let job_id = &resolve(&client, Listing::Jobs, job_id).await?;
    let res = run_action("Cancelling job...", client.post(&format!("/jobs/{job_id}/cancel"), None)).await?;
    match resolve_output_mode(json, output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{job_id}"),
        OutputMode::Table => print_success(&format!("Job {} cancelled.", short_id(job_id))),
    }
    Ok(())
}

/// `--interval` read with JavaScript's `Number()` (`0x10` is 16 seconds).
/// Returns the interval only when finite and positive.
pub fn parse_interval_seconds(raw: &str) -> Option<f64> {
    let n = crate::output::js_number(raw);
    (n.is_finite() && n > 0.0).then_some(n)
}

fn interval_ms(seconds: f64) -> Duration {
    Duration::from_millis((seconds * 1000.0).floor() as u64)
}

/// `jobs watch`; with `--json` each poll that changed as one line of JSON (VE-3831, [`JsonScreen::stream`]).
pub async fn watch(
    ctx: &Ctx,
    interval: &str,
    source: Option<String>,
    integration: Option<String>,
    json: bool,
) -> Result<()> {
    let Some(seconds) = parse_interval_seconds(interval) else {
        arg_error(
            "Polling interval must be a positive number of seconds.",
            &["vendo jobs watch", "vendo jobs watch --interval 10"],
        );
    };
    let client = ctx.client()?;
    let scope = WatchScope {
        source_id: resolve_opt(&client, Listing::Sources, source).await?,
        integration_id: resolve_opt(&client, Listing::Destinations, integration).await?,
    };
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let interval = interval_ms(seconds);
    if json {
        watch::watch_active_jobs(&client, &mut JsonScreen::stream(), interval, &scope, stop).await?;
    } else {
        watch::watch_active_jobs(&client, &mut Terminal::default(), interval, &scope, stop).await?;
    }
    Ok(())
}

pub struct TailArgs {
    pub job_id: Option<String>,
    pub source: Option<String>,
    pub integration: Option<String>,
    pub next: bool,
    pub interval: String,
    /// The tailed job as `jobs get --json` prints it, once tailing ends (VE-3831, [`JsonScreen::last`]).
    pub json: bool,
}

#[derive(Debug, PartialEq)]
pub enum TailTarget {
    Job(String),
    Resource(String, ResourceKind),
}

/// The one thing `jobs tail` follows. Empty values count as missing, as the
/// TS CLI's `filter(Boolean)` did; `None` unless exactly one is given.
pub fn tail_target(job_id: Option<String>, source: Option<String>, integration: Option<String>) -> Option<TailTarget> {
    let given = |value: Option<String>| value.filter(|v| !v.is_empty());
    match (given(job_id), given(source), given(integration)) {
        (Some(job_id), None, None) => Some(TailTarget::Job(job_id)),
        (None, Some(source), None) => Some(TailTarget::Resource(source, ResourceKind::Source)),
        (None, None, Some(integration)) => Some(TailTarget::Resource(integration, ResourceKind::Integration)),
        _ => None,
    }
}

pub async fn tail(ctx: &Ctx, args: TailArgs) -> Result<ExitCode> {
    let Some(target) = tail_target(args.job_id, args.source, args.integration) else {
        arg_error(
            "Specify exactly one target: <jobId>, --source <sourceId>, or --integration <integrationId>.",
            &[
                "vendo jobs tail <jobId>",
                "vendo jobs tail --source <sourceId>",
                "vendo jobs tail --integration <integrationId>",
                "vendo jobs tail --source <sourceId> --next",
            ],
        );
    };
    let Some(seconds) = parse_interval_seconds(&args.interval) else {
        arg_error(
            "Polling interval must be a positive number of seconds.",
            &["vendo jobs tail <jobId>", "vendo jobs tail --source <sourceId> --interval 5"],
        );
    };
    let interval = interval_ms(seconds);
    if matches!(target, TailTarget::Job(_)) && args.next {
        arg_error(
            "`--next` can only be used with `--source` or `--integration`.",
            &["vendo jobs tail --source <sourceId> --next", "vendo jobs tail --integration <integrationId> --next"],
        );
    }
    let client = ctx.client()?;
    if args.json {
        let mut screen = JsonScreen::last();
        follow(&client, &mut screen, target, args.next, interval).await?;
        screen.finish();
    } else {
        follow(&client, &mut Terminal::default(), target, args.next, interval).await?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Tail the job, or wait for the latest (`next`: the next) job of the source or destination and
/// tail that.
async fn follow(
    client: &Client,
    screen: &mut impl Screen,
    target: TailTarget,
    next: bool,
    interval: Duration,
) -> Result<()> {
    let (resource_id, kind) = match target {
        TailTarget::Job(job_id) => {
            let job_id = resolve(client, Listing::Jobs, &job_id).await?;
            watch::tail_job(client, screen, &job_id, interval, MAX_WAIT).await?;
            return Ok(());
        }
        TailTarget::Resource(resource_id, kind) => (resource_id, kind),
    };
    let listing = if kind == ResourceKind::Source { Listing::Sources } else { Listing::Destinations };
    let resource_id = resolve(client, listing, &resource_id).await?;
    let next = if next {
        let baseline = watch::latest_job_for_resource(client, &resource_id, kind).await?;
        NextJob {
            after_created_at: baseline.as_ref().and_then(|j| Job(j).text("createdAt")),
            skip_job_id: baseline.as_ref().map(|j| Job(j).id()),
        }
    } else {
        NextJob { after_created_at: None, skip_job_id: None }
    };
    watch::watch_job(client, screen, &resource_id, kind, interval, next, MAX_WAIT).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_follows_javascript_number() {
        assert_eq!(parse_interval_seconds("5"), Some(5.0));
        assert_eq!(parse_interval_seconds(" 2.5 "), Some(2.5));
        assert_eq!(parse_interval_seconds("1e1"), Some(10.0));
        assert_eq!(parse_interval_seconds("0x10"), Some(16.0));
        assert_eq!(parse_interval_seconds("0b11"), Some(3.0));
        assert_eq!(parse_interval_seconds("0o7"), Some(7.0));
        for bad in ["0", "-1", "", "abc", "inf", "NaN", "Infinity", "-0x10", "0x"] {
            assert_eq!(parse_interval_seconds(bad), None, "{bad}");
        }
    }

    #[test]
    fn tail_targets_follow_javascript_truthiness() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(tail_target(s("j1"), None, None), Some(TailTarget::Job("j1".into())));
        for (job, source, integration) in [
            (None, s(""), None),
            (None, None, s("")),
            (s(""), None, None),
            (None, None, None),
            (s("j1"), s("s1"), None),
        ] {
            assert_eq!(
                tail_target(job.clone(), source.clone(), integration.clone()),
                None,
                "{job:?} {source:?} {integration:?}"
            );
        }
        assert_eq!(
            tail_target(None, s(""), s("i1")),
            Some(TailTarget::Resource("i1".into(), ResourceKind::Integration))
        );
        assert_eq!(tail_target(s(""), s("s1"), s("")), Some(TailTarget::Resource("s1".into(), ResourceKind::Source)));
    }
}

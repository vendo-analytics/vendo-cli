//! Live job polling (port of `src/watch-job.ts`): `jobs watch`, `jobs tail`
//! and the latest-job lookup behind `tail --source/--integration` and the
//! resource `--watch` flags.
//!
//! The loops talk to a [`JobApi`] and draw through a [`Screen`], so tests run
//! them against scripted job sequences on tokio's paused clock.
#![allow(clippy::result_large_err)] // returns ApiError; see client.rs

use std::{future::Future, time::Duration};

use serde_json::Value;
use tokio::time::Instant;

use crate::{
    client::{ApiError, Client, payload},
    jobs::{
        Job, completed_job_summary, format_job_duration, format_job_progress, is_terminal, job_detail_lines,
        job_error_lines,
    },
    output::{color_status, dim, short_id, stdout_is_tty, table, time_ago, yellow},
};

pub const MAX_WAIT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Source,
    Integration,
}

impl ResourceKind {
    pub fn label(self) -> &'static str {
        match self {
            ResourceKind::Source => "source",
            ResourceKind::Integration => "integration",
        }
    }

    fn query_key(self) -> &'static str {
        match self {
            ResourceKind::Source => "source_id",
            ResourceKind::Integration => "integration_id",
        }
    }
}

/// The two job reads the watcher needs.
pub trait JobApi {
    fn job(&self, id: &str) -> impl Future<Output = Result<Value, ApiError>>;
    fn jobs(&self, query: Vec<(&'static str, Option<String>)>) -> impl Future<Output = Result<Vec<Value>, ApiError>>;
}

impl JobApi for Client {
    async fn job(&self, id: &str) -> Result<Value, ApiError> {
        let body = self.get(&format!("/jobs/{id}"), &[]).await?;
        Ok(payload(&body).clone())
    }

    async fn jobs(&self, query: Vec<(&'static str, Option<String>)>) -> Result<Vec<Value>, ApiError> {
        let body = self.get("/jobs", &query).await?;
        Ok(payload(&body).as_array().cloned().unwrap_or_default())
    }
}

/// Where snapshots and result lines go.
pub trait Screen {
    /// Draw a snapshot: redraw every poll on a terminal, print only changes
    /// otherwise (the TS `renderSnapshot`).
    fn snapshot(&mut self, snapshot: &str);
    fn out(&mut self, line: &str);
    fn err(&mut self, line: &str);
}

pub struct Terminal {
    tty: bool,
    last: String,
}

impl Default for Terminal {
    fn default() -> Self {
        Terminal { tty: stdout_is_tty(), last: String::new() }
    }
}

impl Screen for Terminal {
    fn snapshot(&mut self, snapshot: &str) {
        if self.tty {
            print!("\x1B[2J\x1B[0f");
            println!("{snapshot}");
        } else if snapshot != self.last {
            println!("{snapshot}");
            println!();
        }
        self.last = snapshot.to_string();
    }

    fn out(&mut self, line: &str) {
        println!("{line}");
    }

    fn err(&mut self, line: &str) {
        crate::output::print_error(line);
    }
}

/// Only 401/403 stop polling; anything else is shown as "Last poll failed".
fn is_fatal(err: &ApiError) -> bool {
    err.status == 401 || err.status == 403
}

pub async fn active_jobs(
    api: &impl JobApi,
    limit: u32,
    source_id: Option<&str>,
    integration_id: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    api.jobs(vec![
        ("status", Some("running,pending".into())),
        ("limit", Some(limit.to_string())),
        ("sort", Some("created_at:desc".into())),
        ("source_id", source_id.map(str::to_string)),
        ("integration_id", integration_id.map(str::to_string)),
    ])
    .await
}

pub async fn latest_job_for_resource(
    api: &impl JobApi,
    resource_id: &str,
    kind: ResourceKind,
) -> Result<Option<Value>, ApiError> {
    let jobs = api
        .jobs(vec![
            (kind.query_key(), Some(resource_id.to_string())),
            ("limit", Some("1".into())),
            ("sort", Some("created_at:desc".into())),
        ])
        .await?;
    Ok(jobs.into_iter().next())
}

pub struct WatchScope {
    pub source_id: Option<String>,
    pub integration_id: Option<String>,
}

/// `jobs watch`: poll running/pending jobs until `stop` resolves (Ctrl-C).
pub async fn watch_active_jobs(
    api: &impl JobApi,
    screen: &mut impl Screen,
    interval: Duration,
    scope: &WatchScope,
    stop: impl Future<Output = ()>,
) -> Result<(), ApiError> {
    let interval_seconds = (interval.as_millis() / 1000).max(1) as u64;
    tokio::pin!(stop);
    loop {
        let poll = active_jobs(api, 20, scope.source_id.as_deref(), scope.integration_id.as_deref());
        let result = tokio::select! {
            result = poll => result,
            () = &mut stop => break,
        };
        let snapshot = match result {
            Ok(jobs) => render_active_jobs_snapshot(Some(&jobs), interval_seconds, scope, None),
            Err(err) if is_fatal(&err) => return Err(err),
            Err(err) => render_active_jobs_snapshot(None, interval_seconds, scope, Some(&err.message)),
        };
        screen.snapshot(&snapshot);
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = &mut stop => break,
        }
    }
    screen.out("");
    screen.out(&dim("Stopped watching."));
    Ok(())
}

/// `jobs tail <id>`: poll one job until it reaches a terminal state.
pub async fn tail_job(
    api: &impl JobApi,
    screen: &mut impl Screen,
    job_id: &str,
    interval: Duration,
    max_wait: Duration,
) -> Result<(), ApiError> {
    let started = Instant::now();
    let mut last_known: Option<Value> = None;
    while started.elapsed() < max_wait {
        match api.job(job_id).await {
            Ok(job) => {
                screen.snapshot(&render_tail_snapshot(Some(&job), interval, None));
                if is_terminal(&Job(&job).status()) {
                    print_tail_result(screen, Job(&job));
                    return Ok(());
                }
                last_known = Some(job);
            }
            Err(err) if is_fatal(&err) => return Err(err),
            Err(err) => screen.snapshot(&render_tail_snapshot(last_known.as_ref(), interval, Some(&err.message))),
        }
        tokio::time::sleep(interval).await;
    }
    screen.err(&format!(
        "Timed out waiting for job {}. Use `vendo jobs get {job_id}` to check status.",
        short_id(job_id)
    ));
    Ok(())
}

pub struct NextJob {
    pub after_created_at: Option<String>,
    pub skip_job_id: Option<String>,
}

/// Wait for the latest (or next) job of a source/integration, then tail it.
pub async fn watch_job(
    api: &impl JobApi,
    screen: &mut impl Screen,
    resource_id: &str,
    kind: ResourceKind,
    interval: Duration,
    next: NextJob,
    max_wait: Duration,
) -> Result<(), ApiError> {
    let label = kind.label();
    let waiting_for_next = next.skip_job_id.is_some() || next.after_created_at.is_some();
    let spinner = crate::output::spinner(&if waiting_for_next {
        format!("Waiting for next {label} job...")
    } else {
        format!("Waiting for latest {label} job...")
    });
    let started = Instant::now();
    while started.elapsed() < max_wait {
        match latest_job_for_resource(api, resource_id, kind).await {
            Ok(Some(job)) if should_tail_job(Job(&job), &next) => {
                spinner.finish_and_clear();
                return tail_job(api, screen, &Job(&job).id(), interval, MAX_WAIT).await;
            }
            Ok(_) => {}
            Err(err) if is_fatal(&err) => {
                spinner.finish_and_clear();
                return Err(err);
            }
            Err(_) => {}
        }
        tokio::time::sleep(interval).await;
    }
    spinner.finish_and_clear();
    screen.out(&dim(&format!(
        "Timed out waiting for {} {label} job. Use `vendo jobs list --{label} {resource_id}` to check status.",
        if waiting_for_next { "the next" } else { "a" }
    )));
    Ok(())
}

pub fn should_tail_job(job: Job, next: &NextJob) -> bool {
    if next.skip_job_id.as_deref().is_some_and(|skip| job.id() == skip) {
        return false;
    }
    let Some(threshold) = next.after_created_at.as_deref() else { return true };
    let parse = |s: &str| s.parse::<jiff::Timestamp>().ok();
    match (parse(threshold), job.text("createdAt").as_deref().and_then(parse)) {
        (Some(threshold), Some(created)) => created > threshold,
        _ => true,
    }
}

fn print_tail_result(screen: &mut impl Screen, job: Job) {
    screen.out("");
    match job.status().as_str() {
        "completed" => screen.out(&format!("{} {}", crate::output::green("Done:"), completed_job_summary(job))),
        status @ ("failed" | "errored") => screen.err(&format!(
            "Job {} {status}: {}",
            short_id(&job.id()),
            job.text("errorMessage").unwrap_or_else(|| "Unknown error".into())
        )),
        status => screen.out(&format!("Job {} {}.", short_id(&job.id()), color_status(status))),
    }
}

pub fn render_tail_snapshot(job: Option<&Value>, interval: Duration, poll_error: Option<&str>) -> String {
    let seconds = (interval.as_millis() / 1000).max(1);
    let mut lines = vec![
        dim(&format!(
            "Tailing job {} (refreshing every {seconds}s)",
            job.map(|j| short_id(&Job(j).id())).unwrap_or_else(|| "...".into())
        )),
        String::new(),
    ];
    match job {
        None => lines.push(dim("Waiting for job details...")),
        Some(job) => {
            lines.extend(job_detail_lines(Job(job)));
            let errors = job_error_lines(Job(job));
            if !errors.is_empty() {
                lines.push(String::new());
                lines.extend(errors);
            }
        }
    }
    if let Some(message) = poll_error {
        lines.push(String::new());
        lines.push(format!("{} {message}", yellow("Last poll failed:")));
    }
    lines.join("\n")
}

pub fn render_active_jobs_snapshot(
    jobs: Option<&[Value]>,
    interval_seconds: u64,
    scope: &WatchScope,
    poll_error: Option<&str>,
) -> String {
    let scope_label = match (&scope.source_id, &scope.integration_id) {
        (Some(source), _) => format!("source {source} "),
        (None, Some(integration)) => format!("integration {integration} "),
        (None, None) => String::new(),
    };
    let mut lines = vec![
        dim(&format!("Watching {scope_label}jobs... (Ctrl+C to stop, refreshing every {interval_seconds}s)")),
        String::new(),
    ];
    match jobs {
        None => lines.push(dim("Waiting for jobs...")),
        Some([]) => lines.push(dim("No running or pending jobs.")),
        Some(jobs) => {
            let mut grid = table(&["ID", "Type", "Connector", "Status", "Progress", "Started", "Duration"]);
            for value in jobs {
                let job = Job(value);
                grid.add_row(vec![
                    dim(&short_id(&job.id())),
                    job.text("jobType").unwrap_or_else(|| dim("—")),
                    job.text("connectorType").unwrap_or_else(|| dim("—")),
                    color_status(&job.status()),
                    format_job_progress(Some(job)),
                    time_ago(job.started_or_created().as_deref()),
                    format_job_duration(job.text("startedAt").as_deref(), job.text("finishedAt").as_deref()),
                ]);
            }
            lines.push(grid.to_string());
            let count = |status: &str| jobs.iter().filter(|j| Job(j).status() == status).count();
            lines.push(dim(&format!("{} running, {} pending", count("running"), count("pending"))));
        }
    }
    if let Some(message) = poll_error {
        lines.push(String::new());
        lines.push(format!("{} {message}", yellow("Last poll failed:")));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::RefCell, collections::VecDeque};

    type Query = Vec<(&'static str, Option<String>)>;

    /// Scripted API: each call pops the next response; the last one repeats.
    struct FakeApi {
        jobs: RefCell<VecDeque<Result<Vec<Value>, ApiError>>>,
        job: RefCell<VecDeque<Result<Value, ApiError>>>,
        queries: RefCell<Vec<Query>>,
    }

    impl FakeApi {
        fn new(jobs: Vec<Result<Vec<Value>, ApiError>>, job: Vec<Result<Value, ApiError>>) -> Self {
            FakeApi { jobs: RefCell::new(jobs.into()), job: RefCell::new(job.into()), queries: RefCell::default() }
        }
    }

    fn pop<T: Clone>(queue: &RefCell<VecDeque<T>>) -> T {
        let mut queue = queue.borrow_mut();
        if queue.len() > 1 { queue.pop_front().unwrap() } else { queue.front().cloned().expect("scripted response") }
    }

    impl JobApi for FakeApi {
        async fn job(&self, _id: &str) -> Result<Value, ApiError> {
            pop(&self.job)
        }
        async fn jobs(&self, query: Vec<(&'static str, Option<String>)>) -> Result<Vec<Value>, ApiError> {
            self.queries.borrow_mut().push(query);
            pop(&self.jobs)
        }
    }

    /// Records what a piped terminal would print.
    #[derive(Default)]
    struct Recorder {
        snapshots: Vec<String>,
        last: String,
        out: Vec<String>,
        err: Vec<String>,
    }

    impl Screen for Recorder {
        fn snapshot(&mut self, snapshot: &str) {
            if snapshot != self.last {
                self.snapshots.push(snapshot.to_string());
            }
            self.last = snapshot.to_string();
        }
        fn out(&mut self, line: &str) {
            self.out.push(line.to_string());
        }
        fn err(&mut self, line: &str) {
            self.err.push(line.to_string());
        }
    }

    fn job(id: &str, status: &str) -> Value {
        json!({ "id": id, "status": status, "jobType": "import", "rowsProcessed": 1200, "rowsWritten": 1200 })
    }

    fn api_error(status: u16, message: &str) -> ApiError {
        ApiError {
            message: message.into(),
            status,
            code: None,
            request_id: None,
            server_request_id: None,
            details: None,
            status_text: None,
        }
    }

    const TICK: Duration = Duration::from_secs(3);

    #[tokio::test(start_paused = true)]
    async fn tail_redraws_only_on_change_and_reports_completion() {
        let api =
            FakeApi::new(vec![], vec![Ok(job("j1", "running")), Ok(job("j1", "running")), Ok(job("j1", "completed"))]);
        let mut screen = Recorder::default();
        tail_job(&api, &mut screen, "j1", TICK, MAX_WAIT).await.unwrap();
        assert_eq!(screen.snapshots.len(), 2, "{:#?}", screen.snapshots);
        assert!(screen.snapshots[0].starts_with("Tailing job j1 (refreshing every 3s)"));
        assert!(screen.snapshots[0].contains("  Status:        running"));
        assert!(screen.snapshots[1].contains("  Status:        completed"));
        assert_eq!(screen.out, ["", "Done: Job j1 completed. 1,200 rows processed, 1,200 written."]);
    }

    #[tokio::test(start_paused = true)]
    async fn tail_survives_a_transient_error() {
        let api = FakeApi::new(
            vec![],
            vec![Err(api_error(500, "HTTP 500")), Ok(job("j1", "running")), Ok(job("j1", "completed"))],
        );
        let mut screen = Recorder::default();
        tail_job(&api, &mut screen, "j1", TICK, MAX_WAIT).await.unwrap();
        assert!(screen.snapshots[0].contains("Waiting for job details...\n\nLast poll failed: HTTP 500"));
        assert!(screen.snapshots.last().unwrap().contains("completed"));
    }

    #[tokio::test(start_paused = true)]
    async fn tail_stops_on_401_or_403() {
        for status in [401, 403] {
            let api = FakeApi::new(vec![], vec![Err(api_error(status, "nope"))]);
            let err = tail_job(&api, &mut Recorder::default(), "j1", TICK, MAX_WAIT).await.unwrap_err();
            assert_eq!(err.status, status);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tail_reports_every_terminal_state() {
        for (status, out, err) in [
            ("failed", None, Some("Job j1 failed: boom")),
            ("errored", None, Some("Job j1 errored: boom")),
            ("cancelled", Some("Job j1 cancelled."), None),
        ] {
            let mut finished = job("j1", status);
            finished["errorMessage"] = json!("boom");
            let api = FakeApi::new(vec![], vec![Ok(finished)]);
            let mut screen = Recorder::default();
            tail_job(&api, &mut screen, "j1", TICK, MAX_WAIT).await.unwrap();
            assert_eq!(screen.out.last().map(String::as_str), out.or(Some("")), "{status}");
            assert_eq!(screen.err.first().map(String::as_str), err, "{status}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tail_times_out_after_the_ceiling() {
        let api = FakeApi::new(vec![], vec![Ok(job("550e8400-e29b-41d4-a716-446655440000", "running"))]);
        let mut screen = Recorder::default();
        tail_job(&api, &mut screen, "550e8400-e29b-41d4-a716-446655440000", TICK, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(
            screen.err,
            [
                "Timed out waiting for job 550e8400.... Use `vendo jobs get 550e8400-e29b-41d4-a716-446655440000` to check status."
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watch_polls_until_stopped_and_prints_changes_only() {
        let api = FakeApi::new(
            vec![
                Ok(vec![job("j1", "running")]),
                Ok(vec![job("j1", "running")]),
                Err(api_error(0, "offline")),
                Ok(vec![]),
            ],
            vec![],
        );
        let mut screen = Recorder::default();
        let scope = WatchScope { source_id: Some("src-1".into()), integration_id: None };
        let stop = tokio::time::sleep(Duration::from_secs(5 * 5 + 1));
        watch_active_jobs(&api, &mut screen, Duration::from_secs(5), &scope, stop).await.unwrap();
        assert_eq!(screen.snapshots.len(), 3, "{:#?}", screen.snapshots);
        assert!(screen.snapshots[0].starts_with("Watching source src-1 jobs... (Ctrl+C to stop, refreshing every 5s)"));
        assert!(screen.snapshots[0].ends_with("1 running, 0 pending"));
        assert!(screen.snapshots[1].contains("Waiting for jobs...\n\nLast poll failed: offline"));
        assert!(screen.snapshots[2].contains("No running or pending jobs."));
        assert_eq!(screen.out, ["", "Stopped watching."]);
        let first = &api.queries.borrow()[0];
        assert!(first.contains(&("status", Some("running,pending".into()))));
        assert!(first.contains(&("limit", Some("20".into()))));
        assert!(first.contains(&("source_id", Some("src-1".into()))));
    }

    #[tokio::test(start_paused = true)]
    async fn watch_stops_on_a_fatal_error() {
        let api = FakeApi::new(vec![Err(api_error(401, "Authentication failed."))], vec![]);
        let scope = WatchScope { source_id: None, integration_id: None };
        let err =
            watch_active_jobs(&api, &mut Recorder::default(), TICK, &scope, std::future::pending()).await.unwrap_err();
        assert_eq!(err.status, 401);
    }

    #[tokio::test(start_paused = true)]
    async fn watch_job_next_skips_the_baseline_job() {
        let mut baseline = job("old", "completed");
        baseline["createdAt"] = json!("2026-01-01T00:00:00Z");
        let mut fresh = job("new", "running");
        fresh["createdAt"] = json!("2026-01-01T00:05:00Z");
        let api = FakeApi::new(vec![Ok(vec![baseline]), Ok(vec![fresh])], vec![Ok(job("new", "completed"))]);
        let mut screen = Recorder::default();
        let next = NextJob { after_created_at: Some("2026-01-01T00:00:00Z".into()), skip_job_id: Some("old".into()) };
        watch_job(&api, &mut screen, "src-1", ResourceKind::Source, TICK, next, MAX_WAIT).await.unwrap();
        assert!(screen.out.last().unwrap().contains("Job new completed."));
        assert_eq!(api.queries.borrow().len(), 2);
        assert!(api.queries.borrow()[0].contains(&("source_id", Some("src-1".into()))));
    }

    #[tokio::test(start_paused = true)]
    async fn watch_job_times_out_waiting() {
        let api = FakeApi::new(vec![Ok(vec![])], vec![]);
        let mut screen = Recorder::default();
        let next = NextJob { after_created_at: None, skip_job_id: None };
        watch_job(&api, &mut screen, "i-1", ResourceKind::Integration, TICK, next, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(
            screen.out,
            ["Timed out waiting for a integration job. Use `vendo jobs list --integration i-1` to check status."]
        );
    }

    #[test]
    fn should_tail_job_rules() {
        let created = |id: &str, at: &str| json!({ "id": id, "createdAt": at });
        let none = NextJob { after_created_at: None, skip_job_id: None };
        assert!(should_tail_job(Job(&created("a", "2026-01-01T00:00:00Z")), &none));
        let skip = NextJob { after_created_at: None, skip_job_id: Some("a".into()) };
        assert!(!should_tail_job(Job(&created("a", "2026-01-01T00:00:00Z")), &skip));
        let after = NextJob { after_created_at: Some("2026-01-01T00:00:00Z".into()), skip_job_id: None };
        assert!(!should_tail_job(Job(&created("b", "2026-01-01T00:00:00Z")), &after));
        assert!(should_tail_job(Job(&created("b", "2026-01-01T00:00:01Z")), &after));
        assert!(should_tail_job(Job(&created("b", "not a date")), &after));
    }
}

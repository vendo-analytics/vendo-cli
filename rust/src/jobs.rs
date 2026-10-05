//! Job formatting shared by `jobs` commands and the watcher (port of
//! `src/job-progress.ts` and `src/job-output.ts`). Jobs stay JSON values so
//! `--json` prints them verbatim; these helpers read fields the way the TS
//! code did (`??` for missing/null, truthiness for optional lines).

use serde_json::Value;

use crate::output::{
    dim, format_number, js_color_status, js_greater_than, js_if, js_plus_one, js_strict_equals, js_string, js_template,
    red, short_id, time_ago,
};

/// Read-only view over a job object from the API.
#[derive(Clone, Copy)]
pub struct Job<'a>(pub &'a Value);

impl<'a> Job<'a> {
    /// The field as JavaScript would print it, `None` when missing or null.
    pub fn text(&self, key: &str) -> Option<String> {
        match self.0.get(key) {
            None | Some(Value::Null) => None,
            Some(value) => Some(js_string(value)),
        }
    }

    /// `if (job[key])`: the field as JavaScript prints it, when truthy.
    fn truthy(&self, key: &str) -> Option<String> {
        js_if(self.0.get(key))
    }

    /// The field as sent, `None` when missing or null (`!= null`).
    pub fn present(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    /// `${job[key]}`: `undefined` when missing, `null` when null.
    pub fn template(&self, key: &str) -> String {
        js_template(self.0.get(key))
    }

    pub fn id(&self) -> String {
        self.text("id").unwrap_or_default()
    }

    pub fn status(&self) -> String {
        self.text("status").unwrap_or_default()
    }

    /// `startedAt ?? createdAt`.
    pub fn started_or_created(&self) -> Option<String> {
        self.text("startedAt").or_else(|| self.text("createdAt"))
    }
}

/// The API's finished states: `canceled` and `warning` are current,
/// `cancelled` is legacy (VE-3695).
pub const TERMINAL_STATUSES: [&str; 6] = ["completed", "warning", "failed", "canceled", "cancelled", "errored"];

/// Jobs still waiting or running. The API calls waiting jobs `queued`.
pub const ACTIVE_JOB_STATUSES: &str = "running,pending,queued";

pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// `chunkIndex + 1` and `totalChunks` when both are set and `totalChunks > 1`, with JavaScript's
/// coercions (a chunk index sent as a string concatenates: "1" + 1 is "11").
fn chunk(job: Job) -> Option<(String, String)> {
    let (index, total) = (job.present("chunkIndex")?, job.present("totalChunks")?);
    js_greater_than(total, 1.0).then(|| (js_plus_one(index), js_string(total)))
}

/// `formatJobProgress`. Fields are used as sent: numbers sent as strings print unformatted, and
/// read equals written only when both have the same type and value (`===`).
pub fn format_job_progress(job: Option<Job>) -> String {
    let Some(job) = job else { return dim("—") };
    let mut parts = Vec::new();

    if let Some(pct) = job.present("progressPct") {
        parts.push(format!("{}%", js_string(pct)));
    }
    if let Some((index, total)) = chunk(job) {
        parts.push(format!("chunk {index}/{total}"));
    }
    match (job.present("rowsProcessed"), job.present("rowsWritten")) {
        (Some(read), Some(written)) if js_strict_equals(read, written) => {
            parts.push(format!("{} rows", format_number(Some(read))))
        }
        (Some(read), Some(written)) => {
            parts.push(format!("{} read", format_number(Some(read))));
            parts.push(format!("{} written", format_number(Some(written))));
        }
        (Some(read), None) => parts.push(format!("{} rows", format_number(Some(read)))),
        (None, Some(written)) => parts.push(format!("{} written", format_number(Some(written)))),
        (None, None) => {}
    }

    if !parts.is_empty() {
        return parts.join(" · ");
    }
    match job.status().as_str() {
        "pending" | "queued" => dim("queued"),
        "running" => dim("starting"),
        _ => dim("—"),
    }
}

pub fn format_job_duration(started_at: Option<&str>, finished_at: Option<&str>) -> String {
    format_job_duration_at(started_at, finished_at, jiff::Timestamp::now())
}

pub fn format_job_duration_at(started_at: Option<&str>, finished_at: Option<&str>, now: jiff::Timestamp) -> String {
    let Some(started) = started_at.filter(|s| !s.is_empty()) else { return dim("—") };
    let parse = crate::output::js_date_parse;
    let end = match finished_at.filter(|s| !s.is_empty()) {
        Some(finished) => parse(finished),
        None => Some(now.as_millisecond()),
    };
    // An unparseable date is NaN in JS, which falls through to "NaNh".
    let (Some(start), Some(end)) = (parse(started), end) else { return "NaNh".to_string() };
    let diff_ms = end - start;
    if diff_ms < 1000 {
        return "0s".to_string();
    }
    let seconds = diff_ms.div_euclid(1000);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let (minutes, rest_seconds) = (seconds / 60, seconds % 60);
    if minutes < 60 {
        return if rest_seconds > 0 { format!("{minutes}m {rest_seconds}s") } else { format!("{minutes}m") };
    }
    let (hours, rest_minutes) = (minutes / 60, minutes % 60);
    if rest_minutes > 0 { format!("{hours}h {rest_minutes}m") } else { format!("{hours}h") }
}

pub fn job_detail_lines(job: Job) -> Vec<String> {
    let or_dash = |v: Option<String>| v.unwrap_or_else(|| dim("—"));
    let mut lines = vec![
        format!("  ID:            {}", job.template("id")),
        format!("  Status:        {}", js_color_status(job.0.get("status"))),
        format!("  Progress:      {}", format_job_progress(Some(job))),
        format!("  Type:          {}", or_dash(job.text("jobType"))),
        format!("  Connector:     {}", or_dash(job.text("connectorType"))),
        format!("  Started:       {}", time_ago(job.started_or_created().as_deref())),
        format!(
            "  Duration:      {}",
            format_job_duration(job.text("startedAt").as_deref(), job.text("finishedAt").as_deref())
        ),
        format!("  Rows Read:     {}", format_number(job.present("rowsProcessed"))),
        format!("  Rows Written:  {}", format_number(job.present("rowsWritten"))),
    ];
    if let Some(source) = job.truthy("sourceId") {
        lines.push(format!("  Source:        {}", dim(&source)));
    }
    if let Some(integration) = job.truthy("integrationId") {
        lines.push(format!("  Integration:   {}", dim(&integration)));
    }
    if let Some(execution) = job.truthy("executionType") {
        lines.push(format!("  Execution:     {execution}"));
    }
    if let Some(trigger) = job.truthy("trigger") {
        lines.push(format!("  Trigger:       {trigger}"));
    }
    if let Some(parent) = job.truthy("parentJobId") {
        lines.push(format!("  Parent Job:    {}", dim(&parent)));
    }
    if let Some((index, total)) = chunk(job) {
        lines.push(format!("  Chunk:         {index} of {total}"));
    }
    lines
}

pub fn job_error_lines(job: Job) -> Vec<String> {
    let Some(message) = job.truthy("errorMessage") else { return Vec::new() };
    let mut lines = vec![format!("  {}  {message}", red("Error:"))];
    if let Some(code) = job.truthy("errorCode") {
        lines.push(format!("  {}   {code}", red("Code:")));
    }
    if let Some(category) = job.truthy("errorCategory") {
        lines.push(format!("  {} {category}", red("Category:")));
    }
    lines
}

pub fn completed_job_summary(job: Job) -> String {
    format!(
        "Job {} completed. {} rows processed, {} written.",
        short_id(&job.id()),
        format_number(job.present("rowsProcessed")),
        format_number(job.present("rowsWritten"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn progress(v: Value) -> String {
        format_job_progress(Some(Job(&v)))
    }

    #[test]
    fn progress_without_a_job_is_a_dash() {
        assert!(format_job_progress(None).contains('—'));
    }

    #[test]
    fn progress_matching_counts_read_as_rows() {
        assert_eq!(
            progress(
                json!({ "id": "j1", "status": "running", "progressPct": 50, "rowsProcessed": 1200, "rowsWritten": 1200 })
            ),
            "50% · 1,200 rows"
        );
    }

    #[test]
    fn progress_separate_read_and_written() {
        assert_eq!(
            progress(json!({ "id": "j2", "status": "running", "rowsProcessed": 1200, "rowsWritten": 900 })),
            "1,200 read · 900 written"
        );
    }

    #[test]
    fn progress_percentage_only() {
        assert_eq!(progress(json!({ "id": "j", "status": "running", "progressPct": 38 })), "38%");
        assert_eq!(progress(json!({ "id": "j", "status": "running", "progressPct": 12.5 })), "12.5%");
    }

    #[test]
    fn progress_includes_chunks() {
        assert_eq!(
            progress(
                json!({ "id": "j3", "status": "running", "chunkIndex": 1, "totalChunks": 4, "rowsProcessed": 500 })
            ),
            "chunk 2/4 · 500 rows"
        );
        assert_eq!(progress(json!({ "id": "j", "status": "running", "chunkIndex": 0, "totalChunks": 1 })), "starting");
    }

    #[test]
    fn progress_states_without_counts() {
        assert!(progress(json!({ "id": "j4", "status": "pending" })).contains("queued"));
        assert!(progress(json!({ "id": "j5", "status": "queued" })).contains("queued"));
        assert!(progress(json!({ "id": "j", "status": "running" })).contains("starting"));
        assert!(progress(json!({ "id": "j", "status": "failed" })).contains('—'));
    }

    #[test]
    fn duration_buckets() {
        let now: jiff::Timestamp = "2026-01-15T12:00:00Z".parse().unwrap();
        let d = |start: Option<&str>, end: Option<&str>| format_job_duration_at(start, end, now);
        assert!(d(None, None).contains('—'));
        assert_eq!(d(Some("2026-01-15T11:59:59.500Z"), None), "0s");
        assert_eq!(d(Some("2026-01-15T11:59:15Z"), None), "45s");
        assert_eq!(d(Some("2026-01-15T11:55:00Z"), None), "5m");
        assert_eq!(d(Some("2026-01-15T11:54:30Z"), None), "5m 30s");
        assert_eq!(d(Some("2026-01-15T09:00:00Z"), None), "3h");
        assert_eq!(d(Some("2026-01-15T09:00:00Z"), Some("2026-01-15T10:15:00Z")), "1h 15m");
        assert_eq!(d(Some("garbage"), None), "NaNh");
    }

    #[test]
    fn detail_lines_add_optional_fields_only_when_present() {
        let job = json!({
            "id": "job-1", "status": "running", "jobType": "import", "connectorType": "stripe",
            "sourceId": "src-1", "integrationId": "", "trigger": "schedule", "chunkIndex": 2, "totalChunks": 3,
        });
        let lines = job_detail_lines(Job(&job));
        assert_eq!(lines[0], "  ID:            job-1");
        assert_eq!(lines[3], "  Type:          import");
        assert!(lines.iter().any(|l| l == "  Source:        src-1"));
        assert!(!lines.iter().any(|l| l.contains("Integration:")));
        assert!(lines.iter().any(|l| l == "  Trigger:       schedule"));
        assert_eq!(lines.last().unwrap(), "  Chunk:         3 of 3");
    }

    #[test]
    fn error_lines_and_summary() {
        let job = json!({ "id": "550e8400-e29b-41d4-a716-446655440000", "errorMessage": "boom", "errorCode": "E1", "rowsProcessed": 1200, "rowsWritten": 900 });
        assert_eq!(job_error_lines(Job(&job)), ["  Error:  boom", "  Code:   E1"]);
        assert!(job_error_lines(Job(&json!({ "id": "x" }))).is_empty());
        assert_eq!(completed_job_summary(Job(&job)), "Job 550e8400... completed. 1,200 rows processed, 900 written.");
    }

    /// Values from the TS `formatJobProgress` and `createJobDetailLines` run in Node.
    #[test]
    fn progress_coerces_like_javascript() {
        assert_eq!(
            progress(
                json!({ "status": "running", "progressPct": "45.5", "chunkIndex": "1", "totalChunks": "4", "rowsProcessed": "1200", "rowsWritten": "1200" })
            ),
            "45.5% · chunk 11/4 · 1200 rows"
        );
        assert_eq!(
            progress(json!({ "status": "running", "rowsProcessed": 1200, "rowsWritten": "1200" })),
            "1,200 read · 1200 written"
        );
        assert_eq!(
            progress(json!({ "status": "running", "chunkIndex": 1, "totalChunks": "1", "rowsProcessed": [1234, 5] })),
            "1,234,5 rows"
        );
        assert_eq!(
            progress(
                json!({ "status": "running", "progressPct": null, "chunkIndex": true, "totalChunks": 3, "rowsWritten": 2.5 })
            ),
            "chunk 2/3 · 2.5 written"
        );
    }

    #[test]
    fn detail_lines_print_undefined_and_null_like_javascript() {
        let lines = job_detail_lines(Job(&json!({ "chunkIndex": "2", "totalChunks": 3 })));
        assert_eq!(
            (lines[0].as_str(), lines[1].as_str()),
            ("  ID:            undefined", "  Status:        undefined")
        );
        assert_eq!(lines.last().unwrap(), "  Chunk:         21 of 3");
        let lines = job_detail_lines(Job(&json!({ "id": null, "status": null, "chunkIndex": 2, "totalChunks": 3 })));
        assert_eq!((lines[0].as_str(), lines[1].as_str()), ("  ID:            null", "  Status:        null"));
        assert_eq!(lines.last().unwrap(), "  Chunk:         3 of 3");
    }

    #[test]
    fn optional_rows_use_javascript_truthiness() {
        // `if (job.errorCode)` and friends: 0 and false are falsy, as in the TS CLI.
        let job = json!({ "errorMessage": "e", "errorCode": 0, "errorCategory": "c" });
        assert_eq!(job_error_lines(Job(&job)), ["  Error:  e", "  Category: c"]);
        assert!(job_error_lines(Job(&json!({ "errorMessage": 0, "errorCode": "E1" }))).is_empty());
        let job = json!({ "id": "j-1", "sourceId": 0, "integrationId": false, "trigger": 5, "parentJobId": "" });
        let lines = job_detail_lines(Job(&job));
        assert!(lines.iter().any(|l| l == "  Trigger:       5"), "{lines:?}");
        assert!(
            !lines.iter().any(|l| l.starts_with("  Source:")
                || l.starts_with("  Integration:")
                || l.starts_with("  Parent Job:"))
        );
    }
}

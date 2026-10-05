//! The lifecycle sources and integrations (and apps, for pause/resume/delete)
//! share: pause, resume, soft delete and the idempotent `sync` with `--watch`
//! (port of `src/commands/pipeline-resource.ts`).

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    context::Ctx,
    jobs::Job,
    output::{
        OutputMode, color_status, confirm, dim, js_iso_string, print_dry_run, print_json, print_single_field,
        print_success, resolve_output_mode, run_action, short_id, yellow,
    },
    watch::{self, ResourceKind, Terminal},
};

#[derive(Clone, Copy)]
pub struct Resource {
    /// Lowercase singular noun used in messages.
    pub singular: &'static str,
    /// API base path, e.g. `/sources`.
    pub api_path: &'static str,
}

pub const APP: Resource = Resource { singular: "app", api_path: "/apps" };
pub const SOURCE: Resource = Resource { singular: "source", api_path: "/sources" };
pub const INTEGRATION: Resource = Resource { singular: "integration", api_path: "/integrations" };

impl Resource {
    fn kind(self) -> ResourceKind {
        if self.singular == "source" { ResourceKind::Source } else { ResourceKind::Integration }
    }

    fn title(self) -> String {
        let mut chars = self.singular.chars();
        chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    }
}

/// Read and parse a JSON file passed with `--config-file` and friends. Errors
/// use Node's wording for the common cases, as the TS CLI printed them.
pub fn read_json_file(path: &str) -> Result<Value> {
    let raw =
        std::fs::read_to_string(path).map_err(|err| anyhow!("Failed to read {path}: {}", node_fs_error(&err, path)))?;
    serde_json::from_str(&raw).map_err(|err| {
        let reason = if err.is_eof() { "Unexpected end of JSON input".to_string() } else { err.to_string() };
        anyhow!("Failed to read {path}: {reason}")
    })
}

/// Node's `readFileSync` message for the errors people actually hit.
fn node_fs_error(err: &std::io::Error, path: &str) -> String {
    match err.kind() {
        std::io::ErrorKind::NotFound => format!("ENOENT: no such file or directory, open '{path}'"),
        std::io::ErrorKind::PermissionDenied => format!("EACCES: permission denied, open '{path}'"),
        std::io::ErrorKind::IsADirectory => "EISDIR: illegal operation on a directory, read".to_string(),
        _ => err.to_string(),
    }
}

/// Comma-separated flag values, trimmed, empties dropped.
pub fn split_list(value: &str) -> Vec<Value> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| Value::String(s.to_string())).collect()
}

/// `Number(value)` for numeric flags sent in a body: NaN becomes JSON `null`,
/// as `JSON.stringify` does.
pub fn js_number_value(value: &str) -> Value {
    let trimmed = value.trim();
    let n = if trimmed.is_empty() { Some(0.0) } else { trimmed.parse::<f64>().ok().filter(|n| n.is_finite()) };
    match n {
        Some(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => json!(n as i64),
        Some(n) => json!(n),
        None => Value::Null,
    }
}

pub struct ActionOpts {
    pub json: bool,
    pub dry_run: bool,
    pub output: Option<String>,
}

/// `pause` / `resume`: one POST, then the shared output modes.
pub async fn state_action(ctx: &Ctx, resource: Resource, id: &str, action: &str, opts: ActionOpts) -> Result<()> {
    let (gerund, past) = match action {
        "pause" => ("Pausing", "paused"),
        _ => ("Resuming", "resumed"),
    };
    if opts.dry_run {
        print_dry_run(action, resource.singular, id, &[]);
        return Ok(());
    }
    let client = ctx.client()?;
    let res = run_action(
        &format!("{gerund} {}...", resource.singular),
        client.post(&format!("{}/{id}/{action}", resource.api_path), None),
    )
    .await?;
    match resolve_output_mode(opts.json, opts.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{id}"),
        OutputMode::Table => print_success(&format!("{} {} {past}.", resource.title(), short_id(id))),
    }
    Ok(())
}

/// Soft delete, with a confirmation prompt unless `--yes` or `--json`.
pub async fn delete(ctx: &Ctx, resource: Resource, id: &str, yes: bool, opts: ActionOpts) -> Result<()> {
    if opts.dry_run {
        print_dry_run("delete", resource.singular, id, &[]);
        return Ok(());
    }
    if !yes && !opts.json && !confirm(&format!("Delete {} {}?", resource.singular, short_id(id))) {
        return Ok(());
    }
    let client = ctx.client()?;
    let res = run_action(
        &format!("Deleting {}...", resource.singular),
        client.delete(&format!("{}/{id}", resource.api_path), &[]),
    )
    .await?;
    match resolve_output_mode(opts.json, opts.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => println!("{id}"),
        OutputMode::Table => print_success(&format!("{} {} deleted.", resource.title(), short_id(id))),
    }
    Ok(())
}

/// `sync`: reuse an active job if there is one, otherwise trigger a new
/// sync; `--watch` follows the job. `dry_run_fields` renders the
/// resource-specific rows of `--dry-run`.
pub async fn sync(
    ctx: &Ctx,
    resource: Resource,
    id: &str,
    watch: bool,
    opts: ActionOpts,
    dry_run_fields: fn(&Value) -> Vec<(&'static str, String)>,
) -> Result<()> {
    let client = ctx.client()?;
    let kind = resource.kind();
    let field = opts.output.clone().unwrap_or_else(|| "id".to_string());

    if opts.dry_run {
        let path = format!("{}/{id}", resource.api_path);
        let (detail, active) = run_action(&format!("Checking {}...", resource.singular), async {
            tokio::try_join!(client.get(&path, &[]), watch::active_job_for_resource(&client, kind, id),)
        })
        .await?;
        let mut details = dry_run_fields(payload(&detail));
        details.push((
            "Active Job",
            match &active {
                Some(job) => format!("{} ({})", short_id(&Job(job).id()), Job(job).status()),
                None => "none".to_string(),
            },
        ));
        print_dry_run("trigger sync for", resource.singular, id, &details);
        return Ok(());
    }

    let existing = run_action("Checking for active jobs...", watch::active_job_for_resource(&client, kind, id)).await?;
    if let Some(job) = existing {
        let (job_id, status) = (Job(&job).id(), Job(&job).status());
        match resolve_output_mode(opts.json, opts.output.as_deref()) {
            OutputMode::Json => print_json(
                &json!({ "data": { "jobId": job_id, "status": status, "message": "Sync already in progress" } }),
            ),
            OutputMode::Field => print_single_field(&job, &field),
            OutputMode::Table => {
                println!("{} for {} {}.", yellow("Sync already in progress"), resource.singular, short_id(id));
                println!("  Job: {job_id} ({})", color_status(&status));
                if watch {
                    let now = js_iso_string(jiff::Timestamp::now().as_millisecond());
                    watch::watch_triggered_resource_job(
                        &client,
                        &mut Terminal::default(),
                        id,
                        kind,
                        Some(&job_id),
                        now,
                    )
                    .await?;
                }
            }
        }
        return Ok(());
    }

    let requested_at = js_iso_string(jiff::Timestamp::now().as_millisecond());
    let res = run_action("Triggering sync...", client.post(&format!("{}/{id}/sync", resource.api_path), None)).await?;
    let data = payload(&res);
    let job_id = data.get("jobId").and_then(Value::as_str).map(str::to_string);
    match resolve_output_mode(opts.json, opts.output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => {
            // `{ id: jobId ?? resourceId, ...data }`: fields in the response win.
            let mut row = Map::new();
            row.insert("id".into(), json!(job_id.clone().unwrap_or_else(|| id.to_string())));
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    row.insert(k.clone(), v.clone());
                }
            }
            print_single_field(&Value::Object(row), &field);
        }
        OutputMode::Table => {
            print_success(&format!("Sync triggered for {} {}.", resource.singular, short_id(id)));
            if watch {
                watch::watch_triggered_resource_job(
                    &client,
                    &mut Terminal::default(),
                    id,
                    kind,
                    job_id.as_deref(),
                    requested_at,
                )
                .await?;
            } else {
                println!("{}", dim(&format!("Use 'vendo jobs list --{} {id}' to monitor.", resource.singular)));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_javascript_number() {
        assert_eq!(js_number_value("24"), json!(24));
        assert_eq!(js_number_value("1.5"), json!(1.5));
        assert_eq!(js_number_value(""), json!(0));
        assert_eq!(js_number_value("abc"), Value::Null);
    }

    #[test]
    fn json_file_errors_read_like_node() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        let missing = missing.to_str().unwrap();
        assert_eq!(
            read_json_file(missing).unwrap_err().to_string(),
            format!("Failed to read {missing}: ENOENT: no such file or directory, open '{missing}'")
        );
        let dir_path = dir.path().to_str().unwrap();
        assert_eq!(
            read_json_file(dir_path).unwrap_err().to_string(),
            format!("Failed to read {dir_path}: EISDIR: illegal operation on a directory, read")
        );
        let empty = dir.path().join("empty.json");
        std::fs::write(&empty, "").unwrap();
        let empty = empty.to_str().unwrap();
        assert_eq!(
            read_json_file(empty).unwrap_err().to_string(),
            format!("Failed to read {empty}: Unexpected end of JSON input")
        );
        let ok = dir.path().join("ok.json");
        std::fs::write(&ok, r#"{"tasks":[1]}"#).unwrap();
        assert_eq!(read_json_file(ok.to_str().unwrap()).unwrap(), json!({ "tasks": [1] }));
    }

    #[test]
    fn lists_are_trimmed_and_drop_empties() {
        assert_eq!(split_list(" orders, customers ,,"), vec![json!("orders"), json!("customers")]);
    }

    #[test]
    fn titles_capitalise_the_noun() {
        assert_eq!(SOURCE.title(), "Source");
        assert_eq!(INTEGRATION.title(), "Integration");
        assert_eq!(APP.title(), "App");
    }
}

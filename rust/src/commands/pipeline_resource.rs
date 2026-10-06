//! The lifecycle sources and destinations (and apps, for pause/resume/delete)
//! share: pause, resume, soft delete and the idempotent `sync` with `--watch`
//! (port of `src/commands/pipeline-resource.ts`).

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    context::Ctx,
    jobs::Job,
    output::{
        OutputMode, confirm, dim, js_color_status, js_iso_string, print_dry_run, print_json, print_single_field,
        print_success, resolve_output_mode, run_action, short_id, yellow,
    },
    short_ids::{Listing, resolve},
    watch::{self, ResourceKind, Terminal},
};

#[derive(Clone, Copy)]
pub struct Resource {
    /// Lowercase singular noun used in messages.
    pub singular: &'static str,
    /// API base path, e.g. `/sources`.
    pub api_path: &'static str,
    /// Where its short IDs are looked up (VE-3831).
    pub listing: Listing,
}

pub const APP: Resource = Resource { singular: "app", api_path: "/apps", listing: Listing::Apps };
pub const SOURCE: Resource = Resource { singular: "source", api_path: "/sources", listing: Listing::Sources };
/// The API's integrations, which customers call destinations (vendo-web-v2 glossary, VE-3828).
pub const INTEGRATION: Resource =
    Resource { singular: "destination", api_path: "/integrations", listing: Listing::Destinations };

impl Resource {
    fn kind(self) -> ResourceKind {
        if self.singular == "source" { ResourceKind::Source } else { ResourceKind::Integration }
    }

    fn title(self) -> String {
        let mut chars = self.singular.chars();
        chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    }
}

/// Read and parse a JSON file passed with `--config-file` and friends, as the TS CLI's
/// `JSON.parse(readFileSync(path, 'utf-8'))` did: invalid UTF-8 becomes U+FFFD, a byte order mark
/// stays, and errors use Node's and V8's wording.
pub fn read_json_file(path: &str) -> Result<Value> {
    read_json(path).map_err(|reason| anyhow!("Failed to read {path}: {reason}"))
}

/// `JSON.parse(readFileSync(path, 'utf-8'))`; the error is the reason Node
/// gave (V8's `JSON.parse` wording), which each caller puts in its own message.
pub fn read_json(path: &str) -> std::result::Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|err| node_fs_error(&err, path))?;
    crate::output::parse_json(&String::from_utf8_lossy(&bytes))
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

/// `Number(value)` for numeric flags sent in a body (`0x10` is 16). NaN and
/// ±Infinity become JSON `null`, as `JSON.stringify` writes them; the client
/// prints the rest the JavaScript way (see `output::js_stringify`).
pub fn js_number_value(value: &str) -> Value {
    let n = crate::output::js_number(value);
    if !n.is_finite() {
        Value::Null
    } else if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 {
        json!(n as i64)
    } else {
        json!(n)
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
    let id = &resolve(&client, resource.listing, id).await;
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

/// Soft delete, after a confirmation (see [`confirm`]).
pub async fn delete(ctx: &Ctx, resource: Resource, id: &str, yes: bool, opts: ActionOpts) -> Result<()> {
    if opts.dry_run {
        print_dry_run("delete", resource.singular, id, &[]);
        return Ok(());
    }
    let question = format!("Delete {} {}?", resource.singular, short_id(id));
    if !confirm(yes, &question, &format!("This deletes {} {id}.", resource.singular))? {
        return Ok(());
    }
    let client = ctx.client()?;
    // After the consent, which names the ID as typed (VE-3823).
    let id = &resolve(&client, resource.listing, id).await;
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
    let id = &resolve(&client, resource.listing, id).await;
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
                Some(job) => format!("{} ({})", short_id(&Job(job).id()), Job(job).template("status")),
                None => "none".to_string(),
            },
        ));
        print_dry_run("trigger sync for", resource.singular, id, &details);
        return Ok(());
    }

    let existing = run_action("Checking for active jobs...", watch::active_job_for_resource(&client, kind, id)).await?;
    if let Some(job) = existing {
        let job_id = Job(&job).id();
        match resolve_output_mode(opts.json, opts.output.as_deref()) {
            OutputMode::Json => print_json(&already_in_progress(&job)),
            OutputMode::Field => print_single_field(&job, &field),
            OutputMode::Table => {
                println!("{} for {} {}.", yellow("Sync already in progress"), resource.singular, short_id(id));
                println!("  Job: {} ({})", Job(&job).template("id"), js_color_status(job.get("status")));
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
            row.insert("id".into(), data.get("jobId").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!(id)));
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
                println!("{}", dim(&format!("Use 'vendo jobs list --{} {id}' to monitor.", kind.jobs_flag())));
            }
        }
    }
    Ok(())
}

/// `printJson({ data: { jobId: job.id, status: job.status, message } })`: the job's own values,
/// left out when missing as `JSON.stringify` does with `undefined`.
fn already_in_progress(job: &Value) -> Value {
    let mut data = Map::new();
    for (key, field) in [("jobId", "id"), ("status", "status")] {
        if let Some(value) = job.get(field) {
            data.insert(key.into(), value.clone());
        }
    }
    data.insert("message".into(), json!("Sync already in progress"));
    json!({ "data": data })
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
        assert_eq!(js_number_value("0x10"), json!(16));
        assert_eq!(js_number_value(" 0b11 "), json!(3));
        assert_eq!(js_number_value("Infinity"), Value::Null);
        assert_eq!(js_number_value("-0"), json!(0));
        let big = json!({ "frequencyValue": js_number_value("12345678901234567890") });
        assert_eq!(crate::output::js_stringify(&big), r#"{"frequencyValue":12345678901234567000}"#);
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
        // Other bad JSON reads like V8's JSON.parse, and readFileSync keeps a byte order mark.
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{a:1}").unwrap();
        let bad = bad.to_str().unwrap();
        assert_eq!(
            read_json_file(bad).unwrap_err().to_string(),
            format!("Failed to read {bad}: Expected property name or '}}' in JSON at position 1 (line 1 column 2)")
        );
        let bom = dir.path().join("bom.json");
        std::fs::write(&bom, "\u{feff}{}").unwrap();
        let bom = bom.to_str().unwrap();
        assert_eq!(
            read_json_file(bom).unwrap_err().to_string(),
            format!("Failed to read {bom}: Unexpected token '\u{feff}', \"\u{feff}{{}}\" is not valid JSON")
        );
        let latin1 = dir.path().join("latin1.json");
        std::fs::write(&latin1, b"{\"name\":\"caf\xe9\"}").unwrap();
        assert_eq!(read_json_file(latin1.to_str().unwrap()).unwrap(), json!({ "name": "caf\u{fffd}" }));
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
        assert_eq!(INTEGRATION.title(), "Destination");
        assert_eq!(APP.title(), "App");
    }

    #[test]
    fn an_active_job_is_reported_with_its_own_values() {
        // `JSON.stringify` drops `undefined`: a job without a status has no `status` key.
        assert_eq!(
            crate::output::json_pretty(&already_in_progress(&json!({ "id": "job-77" }))),
            "{\n  \"data\": {\n    \"jobId\": \"job-77\",\n    \"message\": \"Sync already in progress\"\n  }\n}"
        );
        let job: Value = serde_json::from_str(r#"{"id":null,"status":"running"}"#).unwrap();
        assert_eq!(
            already_in_progress(&job),
            json!({ "data": { "jobId": null, "status": "running", "message": "Sync already in progress" } })
        );
    }
}

//! `vendo models …` (port of `src/commands/models.ts`). The `/api/v1` model
//! routes; `--json` prints the response verbatim, so the nested
//! `outputConfig`, `schedule` and `columns` keys keep the API's snake_case
//! (decided 2026-10-05; the TS client camelCased them). The type shown is the
//! API's `modelType` (`sql`, `python`, `bqml`, …), where the TS CLI read a
//! `dataType` the API does not send (VE-3840, Yalcin 2026-10-06).

use anyhow::Result;
use serde_json::Value;

use crate::{
    browse,
    client::{Client, payload},
    context::Ctx,
    js_text::{cell, time_ago_of},
    output::{
        OutputMode, bold, dim, green, js_string, js_template, js_truthy, list_count, print_field, print_json, red,
        resolve_output_mode, run_action, short_id,
    },
    short_ids::{Listing, resolve},
};

pub struct ListArgs {
    pub data_type: Option<String>,
    pub valid: bool,
    pub invalid: bool,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    let client = ctx.client()?;
    // `--invalid` wins when both are given, as the TS action set it last.
    let is_valid = if args.invalid {
        Some("false")
    } else if args.valid {
        Some("true")
    } else {
        None
    };
    let query = [
        ("data_type", args.data_type),
        ("limit", Some(args.limit)),
        ("offset", Some(args.offset)),
        ("is_valid", is_valid.map(str::to_string)),
    ];
    let res = run_action("Fetching models...", client.get("/models", &query)).await?;
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    match mode {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => print_field(&rows, args.output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            // The table, or at a terminal the same rows to choose from (VE-3894).
            let table = browse::Table {
                header: &["ID", "Name", "Type", "Valid", "Last Validated"],
                cells: rows.iter().map(model_cells).collect(),
                footer: list_count(&res, rows.len(), "model"),
            };
            browse::shown(&client, browse::Group::Models, &rows, table, args.output.as_deref()).await?;
        }
    }
    Ok(())
}

/// A row of the `models list` table, its cells styled as the table shows them; a selectable list
/// shows them plain (VE-3894).
fn model_cells(model: &Value) -> Vec<String> {
    vec![
        dim(&short_id(&cell(model.get("id")))),
        cell(model.get("name")),
        cell(model.get("modelType")),
        if model.get("isValid").is_some_and(js_truthy) { green("yes") } else { red("no") },
        time_ago_of(model.get("lastValidatedAt")),
    ]
}

pub async fn get(ctx: &Ctx, model_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let model_id = resolve(&client, Listing::Models, model_id).await?;
    if json {
        print_json(&fetch(&client, &model_id).await?);
        return Ok(());
    }
    show(&client, &model_id).await.map(|_| ())
}

/// `GET /models/<id>`, behind `models get`'s spinner.
async fn fetch(client: &Client, id: &str) -> Result<Value> {
    Ok(run_action("Fetching model...", client.get(&format!("/models/{id}"), &[])).await?)
}

/// What `models get` shows of the model `id`, a full ID (no short-ID lookup), from its request behind
/// its spinner; the model as the API sent it. A selectable list shows it for the model chosen
/// (VE-3894).
pub(crate) async fn show(client: &Client, id: &str) -> Result<Value> {
    let res = fetch(client, id).await?;
    print!("{}", render_model(payload(&res)));
    Ok(payload(&res).clone())
}

/// The `models get` text view (the TS action's `console.log` lines).
pub fn render_model(model: &Value) -> String {
    let t = |key: &str| js_template(model.get(key));
    let truthy = |key: &str| model.get(key).is_some_and(js_truthy);
    let mut lines = vec![
        String::new(),
        format!("{} {}", bold(&t("name")), dim(&format!("({})", t("modelType")))),
        String::new(),
        format!("  ID:            {}", t("id")),
        format!("  Type:          {}", t("modelType")),
        format!("  Valid:         {}", if truthy("isValid") { green("yes") } else { red("no") }),
        format!("  Validated:     {}", time_ago_of(model.get("lastValidatedAt"))),
        format!("  Created:       {}", time_ago_of(model.get("createdAt"))),
    ];
    if truthy("description") {
        lines.push(format!("  Description:   {}", t("description")));
    }
    if let Some(keys) = model.get("primaryKeyColumns").and_then(Value::as_array).filter(|keys| !keys.is_empty()) {
        // `Array.prototype.join`: null and undefined elements are empty.
        let joined: Vec<String> =
            keys.iter().map(|key| if key.is_null() { String::new() } else { js_string(key) }).collect();
        lines.push(format!("  Primary Keys:  {}", joined.join(", ")));
    }
    if truthy("incrementalColumn") {
        lines.push(format!("  Incremental:   {}", t("incrementalColumn")));
    }
    if truthy("validationError") {
        lines.push(String::new());
        lines.push(format!("  {} {}", red("Validation Error:"), t("validationError")));
    }
    if truthy("sqlQuery") {
        lines.push(String::new());
        lines.push(bold("  SQL Query:"));
        for line in t("sqlQuery").split('\n') {
            lines.push(format!("    {}", dim(line)));
        }
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_detail_view_skips_empty_optional_rows() {
        let out = render_model(&json!({
            "id": "m1", "name": "orders", "isValid": false, "primaryKeyColumns": [], "sqlQuery": "",
            "description": "", "incrementalColumn": null, "validationError": null,
        }));
        assert_eq!(
            out,
            "\norders (undefined)\n\n  ID:            m1\n  Type:          undefined\n  Valid:         no\n  Validated:     —\n  Created:       —\n"
        );
    }

    #[test]
    fn primary_keys_join_like_javascript() {
        let out = render_model(&json!({ "primaryKeyColumns": ["a", null, 3] }));
        assert!(out.contains("  Primary Keys:  a, , 3\n"), "{out}");
    }
}

//! `vendo catalog …` (port of `src/commands/catalog.ts`).

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    client::payload,
    context::Ctx,
    jobs::Job,
    output::{
        OutputMode, bold, cyan, dim, green, js_if, js_join, js_template, js_truthy, print_count, print_field,
        print_json, resolve_output_mode, run_action, table,
    },
};

/// `item.supportedRoles.join(', ')`.
fn roles(item: &Value) -> String {
    item.get("supportedRoles").and_then(Value::as_array).map(|r| js_join(r, ", ")).unwrap_or_default()
}

/// `item.selfServe ? 'yes' : 'no'`.
fn self_serve(item: &Value) -> String {
    if item.get("selfServe").is_some_and(js_truthy) { green("yes") } else { dim("no") }
}

pub async fn list(
    ctx: &Ctx,
    category: Option<String>,
    role: Option<String>,
    json: bool,
    output: Option<String>,
) -> Result<()> {
    let client = ctx.client()?;
    let res =
        run_action("Fetching catalog...", client.get("/catalog", &[("category", category), ("role", role)])).await?;
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    match resolve_output_mode(json, output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => print_field(&rows, output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            let mut grid = table(&["App Type", "Name", "Category", "Roles", "Self-Serve"]);
            for item in &rows {
                let t = |key: &str| Job(item).text(key).unwrap_or_default();
                grid.add_row(vec![cyan(&t("appType")), t("displayName"), t("category"), roles(item), self_serve(item)]);
            }
            println!("{grid}");
            print_count(rows.len() as u64, "integration type");
        }
    }
    Ok(())
}

pub async fn get(ctx: &Ctx, app_type: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching catalog entry...", client.get(&format!("/catalog/{app_type}"), &[])).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    for line in catalog_lines(payload(&res)) {
        println!("{line}");
    }
    Ok(())
}

/// The `catalog get` view, with the TS CLI's `${…}` rendering and truthiness checks.
fn catalog_lines(item: &Value) -> Vec<String> {
    let field = |key: &str| item.get(key);
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            bold(&js_template(field("displayName"))),
            dim(&format!("({})", js_template(field("appType"))))
        ),
        String::new(),
        format!("  Category:    {}", js_template(field("category"))),
        format!("  Roles:       {}", roles(item)),
        format!("  Self-Serve:  {}", self_serve(item)),
    ];
    if let Some(lifecycle) = js_if(field("lifecycle")) {
        lines.push(format!("  Lifecycle:   {lifecycle}"));
    }
    if let Some(provider) = js_if(field("provider")) {
        lines.push(format!("  Provider:    {provider}"));
    }
    if let Some(description) = js_if(field("description")) {
        lines.extend([String::new(), format!("  {description}")]);
    }
    if let Some(docs) = js_if(field("documentationUrl")) {
        lines.extend([String::new(), format!("  Docs: {}", cyan(&docs))]);
    }
    let fields = credential_fields(item);
    if !fields.is_empty() {
        lines.extend([String::new(), bold("  Credential Fields:")]);
        for field in &fields {
            lines.extend(credential_field_lines(field, "    ", "      "));
        }
    }
    lines
}

fn credential_fields(item: &Value) -> Vec<Value> {
    item.get("credentialFields").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// `${field.label} (${field.name}) — ${field.type}`, then the description when truthy.
fn credential_field_lines(field: &Value, indent: &str, description_indent: &str) -> Vec<String> {
    let t = |key: &str| js_template(field.get(key));
    let mut lines = vec![format!("{indent}{} ({}) — {}", t("label"), dim(&t("name")), t("type"))];
    if let Some(description) = js_if(field.get("description")) {
        lines.push(format!("{description_indent}{}", dim(&description)));
    }
    lines
}

pub async fn credential_schema(ctx: &Ctx, app_type: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching credential schema...", client.get(&format!("/catalog/{app_type}"), &[])).await?;
    if json {
        // `res.data?.credentialFields ?? []`, as the API sent it.
        let fields = payload(&res).get("credentialFields").filter(|v| !v.is_null()).cloned().unwrap_or(json!([]));
        print_json(&json!({ "appType": app_type, "credentialFields": fields }));
        return Ok(());
    }
    let fields = credential_fields(payload(&res));
    if fields.is_empty() {
        println!("{}", dim(&format!("No credential fields declared for {app_type}.")));
        return Ok(());
    }
    println!("{}", bold(&format!("Credential fields for {app_type}:")));
    for field in &fields {
        for line in credential_field_lines(field, "  ", "    ") {
            println!("{line}");
        }
    }
    println!();
    println!("{}", dim("Use these as keys in the JSON file you pass to --credentials-file for `vendo apps create`."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_prints_missing_null_and_falsy_fields_like_the_ts_cli() {
        // Expected lines from the TS CLI 0.3.1 run against a local stub with the same body (VE-3728).
        let item = json!({
            "appType": "x", "displayName": null, "supportedRoles": ["source", null], "selfServe": "yes", "lifecycle": 0,
            "provider": "", "description": "d", "documentationUrl": null,
            "credentialFields": [{ "name": null, "label": "L", "type": 5, "description": 0 }, { "label": "M" }],
        });
        assert_eq!(
            catalog_lines(&item),
            [
                "",
                "null (x)",
                "",
                "  Category:    undefined",
                "  Roles:       source, ",
                "  Self-Serve:  yes",
                "",
                "  d",
                "",
                "  Credential Fields:",
                "    L (null) — 5",
                "    M (undefined) — undefined",
            ]
        );
    }

    #[test]
    fn self_serve_is_javascript_truthiness() {
        for (value, want) in [
            (json!(true), "yes"),
            (json!(1), "yes"),
            (json!("no"), "yes"),
            (json!(0), "no"),
            (json!(""), "no"),
            (json!(null), "no"),
        ] {
            assert_eq!(self_serve(&json!({ "selfServe": value })), want, "{value}");
        }
        assert_eq!(self_serve(&json!({})), "no");
    }
}

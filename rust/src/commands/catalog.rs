//! `vendo catalog …` (port of `src/commands/catalog.ts`).

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    client::payload,
    context::Ctx,
    jobs::Job,
    output::{
        OutputMode, bold, cyan, dim, green, js_string, print_count, print_field, print_json, resolve_output_mode,
        run_action, table,
    },
};

fn roles(item: &Value) -> String {
    item.get("supportedRoles")
        .and_then(Value::as_array)
        .map(|r| r.iter().map(js_string).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

fn self_serve(item: &Value) -> String {
    if item.get("selfServe").and_then(Value::as_bool).unwrap_or(false) { green("yes") } else { dim("no") }
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
    let item = payload(&res);
    let t = |key: &str| Job(item).text(key).filter(|s| !s.is_empty());
    println!();
    println!(
        "{} {}",
        bold(&t("displayName").unwrap_or_default()),
        dim(&format!("({})", t("appType").unwrap_or_default()))
    );
    println!();
    println!("  Category:    {}", t("category").unwrap_or_default());
    println!("  Roles:       {}", roles(item));
    println!("  Self-Serve:  {}", self_serve(item));
    if let Some(lifecycle) = t("lifecycle") {
        println!("  Lifecycle:   {lifecycle}");
    }
    if let Some(provider) = t("provider") {
        println!("  Provider:    {provider}");
    }
    if let Some(description) = t("description") {
        println!();
        println!("  {description}");
    }
    if let Some(docs) = t("documentationUrl") {
        println!();
        println!("  Docs: {}", cyan(&docs));
    }
    let fields = credential_fields(item);
    if !fields.is_empty() {
        println!();
        println!("{}", bold("  Credential Fields:"));
        for field in &fields {
            print_credential_field(field, "    ", "      ");
        }
    }
    Ok(())
}

fn credential_fields(item: &Value) -> Vec<Value> {
    item.get("credentialFields").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn print_credential_field(field: &Value, indent: &str, description_indent: &str) {
    let t = |key: &str| Job(field).text(key).unwrap_or_else(|| "undefined".into());
    println!("{indent}{} ({}) — {}", t("label"), dim(&t("name")), t("type"));
    if let Some(description) = Job(field).text("description").filter(|s| !s.is_empty()) {
        println!("{description_indent}{}", dim(&description));
    }
}

pub async fn credential_schema(ctx: &Ctx, app_type: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching credential schema...", client.get(&format!("/catalog/{app_type}"), &[])).await?;
    let fields = credential_fields(payload(&res));
    if json {
        print_json(&json!({ "appType": app_type, "credentialFields": fields }));
        return Ok(());
    }
    if fields.is_empty() {
        println!("{}", dim(&format!("No credential fields declared for {app_type}.")));
        return Ok(());
    }
    println!("{}", bold(&format!("Credential fields for {app_type}:")));
    for field in &fields {
        print_credential_field(field, "  ", "    ");
    }
    println!();
    println!("{}", dim("Use these as keys in the JSON file you pass to --credentials-file for `vendo apps create`."));
    Ok(())
}

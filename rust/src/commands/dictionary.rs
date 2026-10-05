//! `vendo dictionary …` (port of `src/commands/dictionary.ts`): list, search
//! and look up data dictionary definitions. `list` and `search` read the same
//! route and print the same page; `--json` prints the response verbatim.

use anyhow::Result;
use serde_json::Value;

use crate::{
    client::payload,
    context::Ctx,
    dictionary::{self, Item, Lookup},
    js_text::{cell, time_ago_of},
    output::{
        OutputMode, bold, cyan, dim, js_color_status, js_join, js_string, js_template, js_truthy, print_field,
        print_json, print_list_count, resolve_output_mode, run_action, table,
    },
};

/// The TS `dash(value)`: the value when it has a non-zero `length`, else a
/// dimmed dash. In the detail view the value prints as `${value}`.
fn dash(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(v @ Value::Array(items)) if !items.is_empty() => js_string(v),
        _ => dim("—"),
    }
}

/// `dash(value)` as a table cell: cli-table3 shows a non-empty array as an
/// empty cell.
fn dash_cell(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(items)) if !items.is_empty() => String::new(),
        other => dash(other),
    }
}

/// The TS `formatTags`: a dash for no tags, else `tags.join(', ')`.
fn format_tags(tags: Option<&Value>) -> String {
    match tags {
        Some(Value::Array(items)) if !items.is_empty() => js_join(items, ", "),
        Some(other) if js_truthy(other) && !other.is_array() => js_string(other),
        _ => dim("—"),
    }
}

pub struct PageArgs {
    pub subject_type: String,
    pub query: Option<String>,
    pub limit: String,
    pub offset: String,
    pub json: bool,
    pub output: Option<String>,
}

/// `list` and `search`: one page of one subject type.
pub async fn page(ctx: &Ctx, label: &str, args: PageArgs) -> Result<()> {
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    let client = ctx.client()?;
    let res =
        run_action(label, dictionary::list(&client, args.subject_type.clone(), args.query, args.limit, args.offset))
            .await?;
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    match mode {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => print_field(&rows, args.output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            // Events, properties, groups, metrics and audiences are identified by their
            // semantic registry ID, so the first column is the ID `get` takes, not a name.
            let mut grid = table(&["Subject ID", "Display", "Description"]);
            for row in &rows {
                let item = Item(row);
                grid.add_row(vec![
                    cyan(&cell(item.subject_id())),
                    dash_cell(item.display_name()),
                    dash_cell(item.description()),
                ]);
            }
            println!("{grid}");
            print_list_count(&res, rows.len(), &args.subject_type);
        }
    }
    Ok(())
}

pub async fn get(ctx: &Ctx, subject_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching dictionary entry...", dictionary::get(&client, subject_id)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    let lookup = Lookup(res.get("data").unwrap_or(&Value::Null));
    match lookup.definition().filter(|d| lookup.found().is_some_and(js_truthy) && js_truthy(d)) {
        Some(definition) => print!("{}", render_definition(Item(definition))),
        None => {
            println!();
            println!("{}", dim(&format!("No dictionary entry for {subject_id}")));
        }
    }
    Ok(())
}

/// The `dictionary get` text view (the TS `printDefinition`).
pub fn render_definition(item: Item) -> String {
    // `item.displayName || item.subjectId`
    let title = item.display_name().filter(|name| js_truthy(name)).or(item.subject_id());
    let mut lines = vec![
        String::new(),
        format!("{} {}", bold(&js_template(title)), dim(&format!("({})", js_template(item.subject_type())))),
        String::new(),
        format!("  Subject:      {}", js_template(item.subject_id())),
        format!("  Type:         {}", js_template(item.subject_type())),
        format!("  Display:      {}", dash(item.display_name())),
        format!("  Data type:    {}", dash(item.data_type())),
        format!("  Semantic:     {}", dash(item.semantic_type())),
        format!("  Origin:       {}", dash(item.origin())),
        format!("  Status:       {}", js_color_status(item.status())),
        format!("  Last seen:    {}", time_ago_of(item.last_seen_at())),
        format!("  Tags:         {}", format_tags(item.tags())),
    ];
    if item.description().is_some_and(js_truthy) {
        lines.push(String::new());
        lines.push(format!("  {}", js_template(item.description())));
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dashes_follow_javascript_length_checks() {
        assert_eq!(dash(Some(&json!("x"))), "x");
        assert_eq!(dash(Some(&json!(["a", "b"]))), "a,b");
        for empty in [None, Some(json!(null)), Some(json!("")), Some(json!([])), Some(json!(7)), Some(json!({}))] {
            assert_eq!(dash(empty.as_ref()), "—", "{empty:?}");
        }
        assert_eq!(dash_cell(Some(&json!(["a"]))), "", "cli-table3 shows an array as an empty cell");
    }

    #[test]
    fn tags_join_like_array_join() {
        assert_eq!(format_tags(Some(&json!(["pii", "crm"]))), "pii, crm");
        assert_eq!(format_tags(Some(&json!(["a", null, 3]))), "a, , 3");
        for none in [None, Some(json!(null)), Some(json!([]))] {
            assert_eq!(format_tags(none.as_ref()), "—", "{none:?}");
        }
    }

    #[test]
    fn the_title_falls_back_to_the_subject_id() {
        let out = render_definition(Item(&json!({ "subjectId": "s1", "displayName": "", "subjectType": "event" })));
        assert!(out.starts_with("\ns1 (event)\n"), "{out}");
        let out = render_definition(Item(&json!({})));
        assert!(out.starts_with("\nundefined (undefined)\n"), "{out}");
        assert!(out.contains("  Status:       undefined\n"), "{out}");
    }
}

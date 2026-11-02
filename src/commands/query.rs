//! `vendo query`: one governed query (saved metrics or event aggregates) or one read-only SQL
//! SELECT, through `POST /accounts/{id}/query`, the route the MCP `query` tool shares (vendo-web-v2
//! `lib/vendo/query-api/run-query.ts`). The body is that route's input; `--json` prints the
//! response as sent.

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value, json};

use crate::{
    client::payload,
    context::Ctx,
    output::{dim, js_string, print_json, run_action, table},
};

/// The route's filter operators, longest first so `not_contains` is found before `contains`.
pub const FILTER_OPS: [&str; 12] =
    ["not_contains", "is_not_set", "contains", "not_in", "is_set", "neq", "gte", "lte", "eq", "in", "gt", "lt"];

/// Aggregations that count rows and so take no `--property`.
const COUNTING_AGGS: [&str; 2] = ["count", "unique_users"];

pub struct QueryArgs {
    pub metrics: Vec<String>,
    pub events: Vec<String>,
    pub aggs: Vec<String>,
    pub properties: Vec<String>,
    pub breakdowns: Vec<String>,
    pub granularity: Option<String>,
    pub filters: Vec<String>,
    pub range: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub compare: Option<String>,
    pub limit: Option<u32>,
    pub sql: Option<String>,
    pub sql_file: Option<String>,
    pub include_sql: bool,
    pub json: bool,
}

/// `--event` measures: the i-th `--agg` (default `count`) applies to the i-th event, and the
/// `--property` values go, in order, to the events whose aggregation needs one.
fn event_measures(args: &QueryArgs) -> Result<Vec<Value>> {
    if args.aggs.len() > args.events.len() {
        bail!("--agg was given {} times for {} --event", args.aggs.len(), args.events.len());
    }
    let mut properties = args.properties.iter();
    let measures = args
        .events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let agg = args.aggs.get(index).map_or("count", String::as_str);
            let mut measure = Map::from_iter([("event".into(), json!(event)), ("agg".into(), json!(agg))]);
            if !COUNTING_AGGS.contains(&agg) {
                let property = properties.next().with_context(|| format!("--agg {agg} on {event} needs --property"))?;
                measure.insert("property".into(), json!(property));
            }
            Ok(Value::Object(measure))
        })
        .collect::<Result<Vec<_>>>()?;
    if properties.next().is_some() {
        bail!("--property was given more times than there are --event with sum, avg, min or max");
    }
    Ok(measures)
}

/// A `--filter`'s value as the route takes it: a list for `in`/`not_in` (comma-separated), a
/// number or boolean when it reads as one, else the text.
fn filter_value(op: &str, value: &str) -> Value {
    if op == "in" || op == "not_in" {
        return Value::Array(value.split(',').map(|v| json!(v.trim())).collect());
    }
    match serde_json::from_str::<Value>(value) {
        Ok(parsed @ (Value::Number(_) | Value::Bool(_))) => parsed,
        _ => json!(value),
    }
}

/// `field:op:value` (or `field:is_set`). The field may itself hold colons (`user:<source>:<key>`),
/// so the operator is found by name.
pub fn parse_filter(text: &str) -> Result<Value> {
    for op in FILTER_OPS {
        let unary = op == "is_set" || op == "is_not_set";
        if let Some(field) = text.strip_suffix(&format!(":{op}")).filter(|f| unary && !f.is_empty()) {
            return Ok(json!({ "field": field, "op": op }));
        }
        if let Some((field, value)) = text.split_once(&format!(":{op}:")).filter(|(f, _)| !f.is_empty()) {
            return Ok(json!({ "field": field, "op": op, "value": filter_value(op, value) }));
        }
    }
    bail!("--filter {text:?} is not field:op:value; op is one of {}", FILTER_OPS.join(", "))
}

pub fn body(args: &QueryArgs) -> Result<Value> {
    let mut body = Map::new();
    let sql = match (&args.sql, &args.sql_file) {
        (Some(sql), _) => Some(sql.clone()),
        (None, Some(path)) => Some(std::fs::read_to_string(path).with_context(|| format!("Could not read {path}"))?),
        (None, None) => None,
    };
    if let Some(sql) = sql {
        body.insert("sql".into(), json!(sql));
    } else {
        let mut measures: Vec<Value> = args.metrics.iter().map(|m| json!({ "metric": m })).collect();
        measures.extend(event_measures(args)?);
        if measures.is_empty() {
            bail!("Pass --metric or --event for a governed query, or --sql");
        }
        body.insert("measures".into(), Value::Array(measures));
        if !args.breakdowns.is_empty() {
            body.insert("breakdowns".into(), json!(args.breakdowns));
        }
        if let Some(granularity) = &args.granularity {
            body.insert("granularity".into(), json!(granularity));
        }
        if !args.filters.is_empty() {
            let filters = args.filters.iter().map(|f| parse_filter(f)).collect::<Result<Vec<_>>>()?;
            body.insert("filters".into(), Value::Array(filters));
        }
        match (&args.range, &args.from, &args.to) {
            (Some(range), _, _) => {
                body.insert("time_range".into(), json!(range));
            }
            (None, Some(from), Some(to)) => {
                body.insert("time_range".into(), json!({ "from": from, "to": to }));
            }
            (None, Some(_), None) | (None, None, Some(_)) => bail!("Pass both --from and --to"),
            (None, None, None) => {}
        }
        if let Some(compare) = &args.compare {
            body.insert("compare".into(), json!(compare));
        }
    }
    if let Some(limit) = args.limit {
        body.insert("limit".into(), json!(limit));
    }
    if args.include_sql {
        body.insert("include_sql".into(), json!(true));
    }
    Ok(Value::Object(body))
}

/// A cell as the table shows it: text as is, null as a dimmed dash.
fn cell(value: &Value) -> String {
    match value {
        Value::Null => dim("—"),
        Value::String(text) => text.clone(),
        other => js_string(other),
    }
}

pub async fn run(ctx: &Ctx, args: QueryArgs) -> Result<()> {
    let body = body(&args)?;
    let client = ctx.client()?;
    let res = run_action("Running query...", client.post("/query", Some(body))).await?;
    if args.json {
        print_json(&res);
        return Ok(());
    }
    let result = payload(&res);
    let headers: Vec<String> = result["columns"]
        .as_array()
        .map(|columns| {
            columns
                .iter()
                .map(|column| match column["unit"].as_str() {
                    Some(unit) => format!("{} ({unit})", js_string(&column["name"])),
                    None => js_string(&column["name"]),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut grid = table(&headers.iter().map(String::as_str).collect::<Vec<_>>());
    let rows = result["rows"].as_array().cloned().unwrap_or_default();
    for row in &rows {
        grid.add_row(row.as_array().map(|cells| cells.iter().map(cell).collect::<Vec<_>>()).unwrap_or_default());
    }
    println!("{grid}");

    let plural = |n: u64| if n == 1 { "row" } else { "rows" };
    let shown = rows.len() as u64;
    let mut summary = vec![match result["row_count"].as_u64() {
        Some(total) if result["truncated"] == json!(true) => format!("{shown} of {total} {}", plural(total)),
        _ => format!("{shown} {}", plural(shown)),
    }];
    if let (Some(from), Some(to)) = (result["time_range"]["from"].as_str(), result["time_range"]["to"].as_str()) {
        summary.push(format!("{from} to {to}"));
    }
    summary.push(if result["source"] == json!("sql") { "custom SQL".into() } else { "governed".into() });
    println!("{}", dim(&summary.join(" · ")));
    if let Some(sql) = result["sql"].as_str() {
        println!();
        println!("{sql}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> QueryArgs {
        QueryArgs {
            metrics: vec![],
            events: vec![],
            aggs: vec![],
            properties: vec![],
            breakdowns: vec![],
            granularity: None,
            filters: vec![],
            range: None,
            from: None,
            to: None,
            compare: None,
            limit: None,
            sql: None,
            sql_file: None,
            include_sql: false,
            json: false,
        }
    }

    #[test]
    fn events_take_their_agg_and_properties_go_to_the_aggregations_that_need_one() {
        let mut a = args();
        a.events = vec!["Purchase".into(), "Purchase".into()];
        a.aggs = vec!["count".into(), "sum".into()];
        a.properties = vec!["revenue".into()];
        a.range = Some("last_7_days".into());
        assert_eq!(
            body(&a).unwrap()["measures"],
            json!([
                { "event": "Purchase", "agg": "count" },
                { "event": "Purchase", "agg": "sum", "property": "revenue" },
            ])
        );
    }

    #[test]
    fn a_sum_without_a_property_is_refused_before_any_request() {
        let mut a = args();
        a.events = vec!["Purchase".into()];
        a.aggs = vec!["sum".into()];
        assert!(body(&a).unwrap_err().to_string().contains("needs --property"));
    }

    #[test]
    fn filters_find_the_operator_by_name_when_the_field_holds_colons() {
        assert_eq!(
            parse_filter("user:hubspot:plan:in:pro,team").unwrap(),
            json!({ "field": "user:hubspot:plan", "op": "in", "value": ["pro", "team"] })
        );
        assert_eq!(parse_filter("revenue:gte:100").unwrap(), json!({ "field": "revenue", "op": "gte", "value": 100 }));
        assert_eq!(parse_filter("country:is_set").unwrap(), json!({ "field": "country", "op": "is_set" }));
        assert_eq!(
            parse_filter("name:not_contains:test").unwrap(),
            json!({ "field": "name", "op": "not_contains", "value": "test" })
        );
        assert!(parse_filter("country=AU").is_err());
    }

    #[test]
    fn sql_mode_sends_only_the_sql_and_the_shared_options() {
        let mut a = args();
        a.sql = Some("SELECT 1".into());
        a.limit = Some(5);
        a.metrics = vec!["ignored".into()];
        assert_eq!(body(&a).unwrap(), json!({ "sql": "SELECT 1", "limit": 5 }));
    }

    #[test]
    fn dates_need_both_ends() {
        let mut a = args();
        a.metrics = vec!["New MRR".into()];
        a.from = Some("2026-10-01".into());
        assert!(body(&a).is_err());
        a.to = Some("2026-10-31".into());
        assert_eq!(body(&a).unwrap()["time_range"], json!({ "from": "2026-10-01", "to": "2026-10-31" }));
    }
}

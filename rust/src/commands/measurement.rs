//! `vendo measurement …` (port of `src/commands/measurement.ts`):
//! methodologies, segmentation-rule previews, cohort LTV and signal
//! availability. Every route is a web-app route (`web_app`); `--json` prints
//! its body unchanged, as the TS CLI already did.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::{
    commands::pipeline_resource::js_number_value,
    context::Ctx,
    js_text::{cell, cell_or, format_number_of, length_of, template, template_or, time_ago_of},
    output::{
        OutputMode, arg_error, bold, cyan, dim, format_number, green, js_number, js_number_string, js_string,
        js_truthy, print_count, print_field, print_json, red, resolve_output_mode, run_action, short_id, table, yellow,
    },
    web_app,
};

fn dash() -> String {
    dim("—")
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(js_truthy)
}

fn array_at<'a>(value: &'a Value, pointer: &str) -> &'a [Value] {
    value.pointer(pointer).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

// ── Number formats only measurement uses ───────────────────────────────────

/// `n.toLocaleString(undefined, { style: 'currency', currency: 'USD',
/// maximumFractionDigits: 2 })` in en-US: Intl rounds the shortest decimal
/// form of the number half away from zero, so 1.005 is $1.01.
/// ❓ en-US only: the TS output follows the user's locale (VE-3728).
fn format_usd(n: f64) -> String {
    let scientific = format!("{:e}", n.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let point = exponent.parse::<i64>().expect("{:e} exponent is an integer") + 1;
    let (int, frac) = if point <= 0 {
        ("0".to_string(), format!("{}{digits}", "0".repeat(point.unsigned_abs() as usize)))
    } else if point as usize >= digits.len() {
        (format!("{digits}{}", "0".repeat(point as usize - digits.len())), String::new())
    } else {
        (digits[..point as usize].to_string(), digits[point as usize..].to_string())
    };
    let round_up = frac.as_bytes().get(2).is_some_and(|d| *d >= b'5');
    let cents = format!("{int}{:0<2}", &frac[..frac.len().min(2)]);
    let cents = if round_up { increment(&cents) } else { cents };
    let (int, frac) = cents.split_at(cents.len() - 2);
    let sign = if n.is_sign_negative() { "-" } else { "" };
    format!("{sign}${}.{frac}", group_thousands(int.trim_start_matches('0')))
}

/// Add one to a string of decimal digits.
fn increment(digits: &str) -> String {
    let mut out: Vec<u8> = digits.bytes().collect();
    for d in out.iter_mut().rev() {
        if *d == b'9' {
            *d = b'0';
        } else {
            *d += 1;
            return String::from_utf8(out).expect("ASCII digits");
        }
    }
    format!("1{}", String::from_utf8(out).expect("ASCII digits"))
}

fn group_thousands(int: &str) -> String {
    if int.is_empty() {
        return "0".to_string();
    }
    let mut grouped = String::new();
    for (i, digit) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// `Number.prototype.toFixed(2)`: the exact binary value rounded half up, so
/// 1.005 is 1.00 and 0.125 is 0.13; `String(n)` from 1e21 up.
fn to_fixed_2(n: f64) -> String {
    if n.abs() >= 1e21 {
        return js_number_string(n);
    }
    // 1074 fraction digits hold any double exactly.
    let exact = format!("{:.1074}", n.abs());
    let (int, frac) = exact.split_once('.').expect("fixed precision has a point");
    let cents = format!("{int}{}", &frac[..2]);
    let cents = if frac.as_bytes()[2] >= b'5' { increment(&cents) } else { cents };
    let (int, frac) = cents.split_at(cents.len() - 2);
    let int = int.trim_start_matches('0');
    let sign = if n < 0.0 { "-" } else { "" };
    format!("{sign}{}.{frac}", if int.is_empty() { "0" } else { int })
}

/// The TS `fmtMoney`.
fn fmt_money(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => dash(),
        Some(Value::Number(n)) => format_usd(n.as_f64().unwrap_or_default()),
        Some(other) => js_string(other),
    }
}

/// The TS `fmtRatio`.
fn fmt_ratio(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => dash(),
        Some(Value::Number(n)) => to_fixed_2(n.as_f64().unwrap_or_default()),
        Some(other) => js_string(other),
    }
}

/// `String.prototype.padEnd(width)`, counting UTF-16 units as JavaScript does.
fn pad_end(text: &str, width: usize) -> String {
    let len = text.encode_utf16().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(len)))
}

/// `Object.keys(value)` for an object or array, in V8's order: array-index
/// keys ascending, then the rest in insertion order.
fn object_keys(value: &Value) -> Vec<String> {
    let index = |key: &str| key.parse::<u32>().ok().filter(|n| *n < u32::MAX && n.to_string() == key);
    match value {
        Value::Object(map) => {
            let mut indexed: Vec<(u32, &String)> = map.keys().filter_map(|k| index(k).map(|n| (n, k))).collect();
            indexed.sort();
            let named = map.keys().filter(|k| index(k).is_none());
            indexed.into_iter().map(|(_, k)| k).chain(named).cloned().collect()
        }
        Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn key_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
        other => other.get(key),
    }
}

/// `/^\d{4}-\d{2}-\d{2}$/` (ASCII digits).
fn is_iso_date(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

// ── methodologies ──────────────────────────────────────────────────────────

/// A row with camelCase aliases, so `--output` takes either form
/// (`clickPathModel` or `click_path_model`).
fn methodology_as_field(row: &Value) -> Value {
    let mut out = row.as_object().cloned().unwrap_or_default();
    for (key, source) in [
        ("id", "id"),
        ("name", "name"),
        ("description", "description"),
        ("isSystem", "is_system"),
        ("is_system", "is_system"),
        ("clickPathModel", "click_path_model"),
        ("click_path_model", "click_path_model"),
    ] {
        out.insert(key.into(), row.get(source).cloned().unwrap_or(Value::Null));
    }
    Value::Object(out)
}

pub async fn methodologies_list(ctx: &Ctx, no_system: bool, json: bool, output: Option<String>) -> Result<()> {
    let mode = resolve_output_mode(json, output.as_deref());
    let client = ctx.client()?;
    let query = vec![("include_system", no_system.then(|| "false".to_string()))];
    let res = run_action("Fetching methodologies...", web_app::methodologies(&client, query)).await?;
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let rows = array_at(&res, "/data/methodologies");
    if mode == OutputMode::Field {
        let rows: Vec<Value> = rows.iter().map(methodology_as_field).collect();
        print_field(&rows, output.as_deref().unwrap_or_default());
        return Ok(());
    }
    let mut grid = table(&["ID", "Name", "Click Path", "Scope", "Version", "Updated"]);
    for row in rows {
        grid.add_row(vec![
            dim(&short_id(&cell(row.get("id")))),
            cell(row.get("name")),
            cell(row.get("click_path_model")),
            if truthy(row.get("is_system")) { cyan("system") } else { "account".to_string() },
            template(row.get("version")),
            time_ago_of(row.get("updated_at")),
        ]);
    }
    println!("{grid}");
    print_count(rows.len() as u64, "methodology");
    Ok(())
}

pub async fn methodologies_get(ctx: &Ctx, methodology_id: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    // No GET-by-id route: find the row in the list, system rows included.
    let res = run_action("Fetching methodology...", web_app::methodologies(&client, Vec::new())).await?;
    let Some(row) = array_at(&res, "/data/methodologies")
        .iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(methodology_id))
    else {
        bail!("Methodology {methodology_id} not found");
    };
    if json {
        print_json(row);
        return Ok(());
    }
    print!("{}", render_methodology(row));
    Ok(())
}

/// The `methodologies get` text view.
pub fn render_methodology(row: &Value) -> String {
    let t = |key: &str| template(row.get(key));
    let mut lines = vec![
        String::new(),
        format!("{} {}", bold(&t("name")), dim(&format!("({})", t("click_path_model")))),
        String::new(),
        format!("  ID:             {}", t("id")),
        format!("  Scope:          {}", if truthy(row.get("is_system")) { cyan("system") } else { "account".into() }),
        format!("  Version:        {}", t("version")),
        format!("  Updated:        {}", time_ago_of(row.get("updated_at"))),
    ];
    if truthy(row.get("description")) {
        lines.push(format!("  Description:   {}", t("description")));
    }
    // `row.ensemble_weights ?? {}`
    let weights = row.get("ensemble_weights").filter(|w| !w.is_null()).cloned().unwrap_or_else(|| json!({}));
    let keys = object_keys(&weights);
    if !keys.is_empty() {
        lines.push(String::new());
        lines.push(bold("  Ensemble weights:"));
        for key in keys {
            let weight = key_value(&weights, &key).filter(|w| !w.is_null()).cloned().unwrap_or(json!(0));
            lines.push(format!("    {}  {}", pad_end(&key, 12), format_number_of(Some(&weight))));
        }
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

// ── rules ──────────────────────────────────────────────────────────────────

pub async fn rules_preview(ctx: &Ctx, from: String, to: String, limit: &str, json: bool) -> Result<()> {
    let n = js_number(limit);
    if !n.is_finite() || n <= 0.0 || n > 200.0 {
        arg_error(
            "--limit must be 1..200",
            &["vendo measurement rules preview --from 2025-01-01 --to 2025-01-31 --limit 50"],
        );
    }
    let client = ctx.client()?;
    let body = json!({ "from_date": from, "to_date": to, "limit": js_number_value(limit) });
    let res = run_action("Previewing segmentation rules...", web_app::preview_rules(&client, body)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    let mut grid = table(&["Objective", "Channel", "Custom Label", "Methodology", "Via", "Sample"]);
    for row in array_at(&res, "/previews") {
        let context = |key: &str| row.get("context").and_then(|c| c.get(key));
        grid.add_row(vec![
            cell_or(context("campaign_objective"), dash),
            cell_or(context("channel_grouping"), dash),
            cell_or(context("custom_label"), dash),
            cell(row.pointer("/resolved_methodology/name")),
            if row.get("via").and_then(Value::as_str) == Some("rule") { "rule".to_string() } else { dim("default") },
            template(row.get("sample_count")),
        ]);
    }
    println!("{grid}");
    print_count(res.get("total_distinct_contexts").and_then(Value::as_u64).unwrap_or_default(), "distinct context");
    Ok(())
}

// ── ltv ────────────────────────────────────────────────────────────────────

/// A cohort row with camelCase aliases for `--output`.
fn ltv_cohort_as_field(row: &Value) -> Value {
    let mut out: Map<String, Value> = row.as_object().cloned().unwrap_or_default();
    for (key, source) in [
        ("cohortPeriod", "cohort_period"),
        ("cohort_period", "cohort_period"),
        ("segmentKey", "segment_key"),
        ("segment_key", "segment_key"),
        ("cohortSize", "cohort_size"),
        ("cohort_size", "cohort_size"),
    ] {
        out.insert(key.into(), row.get(source).cloned().unwrap_or(Value::Null));
    }
    Value::Object(out)
}

pub struct LtvListArgs {
    pub granularity: String,
    pub segment: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: String,
    pub no_predicted: bool,
    pub json: bool,
    pub output: Option<String>,
}

pub async fn ltv_list(ctx: &Ctx, args: LtvListArgs) -> Result<()> {
    let mode = resolve_output_mode(args.json, args.output.as_deref());
    let client = ctx.client()?;
    let query = vec![
        ("granularity", Some(args.granularity)),
        ("segment_key", Some(args.segment)),
        ("from_period", args.from),
        ("to_period", args.to),
        ("limit", Some(args.limit)),
        ("include_predicted", args.no_predicted.then(|| "false".to_string())),
    ];
    let res = run_action("Fetching LTV cohorts...", web_app::ltv(&client, query)).await?;
    if mode == OutputMode::Json {
        print_json(&res);
        return Ok(());
    }
    let rows = array_at(&res, "/data/cohorts");
    if mode == OutputMode::Field {
        let rows: Vec<Value> = rows.iter().map(ltv_cohort_as_field).collect();
        print_field(&rows, args.output.as_deref().unwrap_or_default());
        return Ok(());
    }
    let mut grid = table(&["Cohort", "Segment", "Size", "LTV 30d", "LTV 90d", "LTV 12m", "CAC", "CAC:LTV"]);
    for row in rows {
        let realised = |key: &str| row.get("realised").and_then(|r| r.get(key));
        grid.add_row(vec![
            cell(row.get("cohort_period")),
            cell(row.get("segment_key")),
            format_number_of(row.get("cohort_size")),
            fmt_money(realised("ltv_30d")),
            fmt_money(realised("ltv_90d")),
            fmt_money(realised("ltv_12m")),
            fmt_money(realised("cac")),
            fmt_ratio(realised("cac_ltv_ratio")),
        ]);
    }
    println!("{grid}");
    print_count(res.pointer("/data/total_returned").and_then(Value::as_u64).unwrap_or_default(), "cohort");
    Ok(())
}

pub async fn ltv_cohort(ctx: &Ctx, period: &str, granularity: String, segment: String, json: bool) -> Result<()> {
    if !is_iso_date(period) {
        arg_error("cohort period must be YYYY-MM-DD", &["vendo measurement ltv cohort 2025-01-01"]);
    }
    let client = ctx.client()?;
    let query = vec![("granularity", Some(granularity)), ("segment_key", Some(segment))];
    let res = run_action("Fetching cohort...", web_app::cohort(&client, period, query)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    print!("{}", render_cohort(&res));
    Ok(())
}

/// The `ltv cohort` text view.
pub fn render_cohort(row: &Value) -> String {
    let t = |key: &str| template(row.get(key));
    let curve = array_at(row, "/cumulative_curve");
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            bold(&format!("Cohort {}", t("cohort_period"))),
            dim(&format!("({}, segment={})", t("cohort_granularity"), t("segment_key")))
        ),
        String::new(),
        format!("  Size:           {}", format_number_of(row.get("cohort_size"))),
        format!("  Retention pts:  {}", format_number(Some(length_of(row.get("retention_matrix"))))),
        format!("  Curve points:   {}", format_number(Some(length_of(row.get("cumulative_curve"))))),
    ];
    if let Some(last) = curve.last() {
        let or_zero = |key: &str| last.get(key).filter(|v| !v.is_null()).cloned().unwrap_or(json!(0));
        lines.push(format!(
            "  Cum revenue:    {} {}",
            fmt_money(Some(&or_zero("cumulative_gross_revenue"))),
            dim(&format!("(t+{}d)", template(Some(&or_zero("period_offset_days")))))
        ));
        lines.push(format!("  After COGS:     {}", fmt_money(Some(&or_zero("cumulative_revenue_after_cogs")))));
    }
    match row.get("prediction").filter(|p| js_truthy(p)) {
        Some(prediction) => {
            lines.push(String::new());
            lines.push(bold("  Prediction:"));
            lines.push(format!("    Method:       {}", template(prediction.get("method"))));
            lines.push(format!("    LTV 30d:      {}", fmt_money(prediction.get("ltv_30d_predicted"))));
            lines.push(format!("    LTV 90d:      {}", fmt_money(prediction.get("ltv_90d_predicted"))));
            lines.push(format!("    LTV 12m:      {}", fmt_money(prediction.get("ltv_12m_predicted"))));
            lines.push(format!("    Computed:     {}", time_ago_of(prediction.get("computed_at"))));
        }
        None => lines.push(dim("  Prediction:     (none)")),
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

pub async fn ltv_customer(ctx: &Ctx, customer_id: &str, json: bool) -> Result<()> {
    if customer_id.is_empty() {
        arg_error("customerId is required", &["vendo measurement ltv customer cust_abc123"]);
    }
    let client = ctx.client()?;
    let res = run_action("Fetching customer LTV...", web_app::customer(&client, customer_id)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    print!("{}", render_customer(customer_id, &res));
    Ok(())
}

/// The `ltv customer` text view.
pub fn render_customer(customer_id: &str, res: &Value) -> String {
    let mut lines = vec![String::new(), bold(&format!("Customer {customer_id}"))];
    match res.get("cohort").filter(|c| js_truthy(c)) {
        Some(cohort) => {
            let t = |key: &str| template(cohort.get(key));
            lines.push(String::new());
            lines.push(format!("  Acquired:        {}", t("acquisition_date")));
            lines.push(format!("  Channel:         {}", template_or(cohort.get("acquisition_channel"), dash)));
            lines.push(format!("  Campaign:        {}", template_or(cohort.get("acquisition_campaign"), dash)));
            lines.push(format!("  Country:         {}", template_or(cohort.get("country"), dash)));
            lines.push(format!(
                "  Reactivated:     {}",
                if truthy(cohort.get("is_reactivated")) { yellow("yes") } else { "no".to_string() }
            ));
            lines.push(format!("  Monthly cohort:  {}", t("cohort_period_monthly")));
        }
        None => lines.push(dim("  (no cohort row — customer not in customer_cohorts)")),
    }
    let realised = |key: &str| res.get("realised").and_then(|r| r.get(key));
    lines.push(String::new());
    lines.push(bold("  Realised LTV:"));
    lines.push(format!("    30d:           {}", fmt_money(realised("ltv_30d"))));
    lines.push(format!("    90d:           {}", fmt_money(realised("ltv_90d"))));
    lines.push(format!("    12m:           {}", fmt_money(realised("ltv_12m"))));
    lines.push(format!("    full:          {}", fmt_money(realised("ltv_full"))));
    lines.push(dim("    after-COGS variants in --json"));
    lines.push(String::new());
    lines.push(format!("  Revenue points:  {}", format_number(Some(length_of(res.get("revenue"))))));
    lines.iter().map(|line| format!("{line}\n")).collect()
}

// ── signals ────────────────────────────────────────────────────────────────

pub async fn signals_list(ctx: &Ctx, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching signal availability...", web_app::signals(&client)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    let rows = array_at(&res, "/data/signals");
    let mut grid = table(&["Signal", "State", "Available", "Reason / Notes"]);
    for row in rows {
        let availability = |key: &str| row.get("availability").and_then(|a| a.get(key));
        let available = match availability("available") {
            None | Some(Value::Null) => dash(),
            Some(v) if js_truthy(v) => green("yes"),
            Some(_) => red("no"),
        };
        grid.add_row(vec![
            cell(row.get("id")),
            if row.get("state").and_then(Value::as_str) == Some("live") { green("live") } else { dim("stub") },
            available,
            cell_or(availability("reason"), dash),
        ]);
    }
    println!("{grid}");
    print_count(rows.len() as u64, "signal");
    Ok(())
}

pub async fn click_path(ctx: &Ctx, sample_limit: Option<String>, json: bool) -> Result<()> {
    let mut query = Vec::new();
    if let Some(raw) = sample_limit {
        let n = js_number(&raw);
        if !n.is_finite() || !(1.0..=500.0).contains(&n) {
            arg_error("--sample-limit must be 1..500", &["vendo measurement signals click-path --sample-limit 100"]);
        }
        query.push(("sampleLimit", Some(js_number_string(n))));
    }
    let client = ctx.client()?;
    let res = run_action("Fetching click-path status...", web_app::click_path(&client, query)).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    print!("{}", render_click_path(res.get("status").unwrap_or(&Value::Null)));
    Ok(())
}

/// The `signals click-path` text view.
pub fn render_click_path(status: &Value) -> String {
    let mut lines = vec![
        String::new(),
        bold("Click-path signal"),
        String::new(),
        format!("  Enabled:        {}", if truthy(status.get("enabled")) { green("yes") } else { red("no") }),
        format!("  Last computed:  {}", time_ago_of(status.get("lastComputedAt"))),
        format!("  Sample rows:    {}", format_number(Some(length_of(status.get("sampleEstimates"))))),
    ];
    let readiness = status.get("readiness");
    let items = readiness.and_then(|r| r.get("readiness")).and_then(Value::as_array).filter(|items| !items.is_empty());
    if let Some(items) = items {
        lines.push(String::new());
        lines.push(bold("  Readiness:"));
        for item in items {
            let ok = truthy(item.get("ok"));
            lines.push(format!("    {} {}", if ok { green("✓") } else { red("✗") }, template(item.get("label"))));
            if !ok && truthy(item.get("detail")) {
                lines.push(format!("      {}", dim(&template(item.get("detail")))));
            }
        }
    } else if let Some(reason) = readiness.and_then(|r| r.get("reason")).filter(|r| js_truthy(r)) {
        lines.push(format!("  Note:           {}", dim(&js_string(reason))));
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n.toLocaleString(undefined, { style: 'currency', currency: 'USD',
    /// maximumFractionDigits: 2 })` and `n.toFixed(2)` in Node 24.16, en-US.
    const NODE: &[(f64, &str, &str)] = &[
        (0.0, "$0.00", "0.00"),
        (-0.0, "-$0.00", "0.00"),
        (1.0, "$1.00", "1.00"),
        (1.005, "$1.01", "1.00"),
        (1.015, "$1.02", "1.01"),
        (1.025, "$1.03", "1.02"),
        (0.005, "$0.01", "0.01"),
        (0.015, "$0.02", "0.01"),
        (0.125, "$0.13", "0.13"),
        (2.675, "$2.68", "2.67"),
        (-1.005, "-$1.01", "-1.00"),
        (-0.001, "-$0.00", "-0.00"),
        (1234.5, "$1,234.50", "1234.50"),
        (1234567.891, "$1,234,567.89", "1234567.89"),
        (1e21, "$1,000,000,000,000,000,000,000.00", "1e+21"),
        (5e-324, "$0.00", "0.00"),
        (-5.0, "-$5.00", "-5.00"),
        (0.30000000000000004, "$0.30", "0.30"),
        (999.995, "$1,000.00", "1000.00"),
        (1000000000000000.5, "$1,000,000,000,000,000.50", "1000000000000000.50"),
        (123456789012345680000.0, "$123,456,789,012,345,680,000.00", "123456789012345683968.00"),
    ];

    #[test]
    fn money_and_ratios_round_like_node() {
        for (n, money, fixed) in NODE {
            assert_eq!(format_usd(*n), *money, "money {n:?}");
            assert_eq!(to_fixed_2(*n), *fixed, "toFixed {n:?}");
        }
        assert!(format_usd(1.7976931348623157e308).starts_with("$179,769,313,486,231,570,000,"));
        assert!(fmt_money(None).contains('—') && fmt_ratio(Some(&Value::Null)).contains('—'));
    }

    #[test]
    fn weight_keys_follow_v8_property_order_and_pad_like_pad_end() {
        assert_eq!(object_keys(&json!({ "b": 1, "10": 2, "a": 3, "2": 4, "02": 5 })), ["2", "10", "b", "a", "02"]);
        assert_eq!(object_keys(&json!([5, 6])), ["0", "1"]);
        assert!(object_keys(&json!(3)).is_empty());
        assert_eq!(pad_end("click_path", 12), "click_path  ");
        assert_eq!(pad_end("survey_response_share", 12), "survey_response_share");
        assert_eq!(pad_end("é", 3), "é  ");
    }

    #[test]
    fn cohort_periods_are_ascii_iso_dates() {
        assert!(is_iso_date("2026-08-01"));
        for bad in ["2026-8-01", "2026-08-01x", "", "２０２６-08-01", "2026/08/01", "2026-08-01\n"] {
            assert!(!is_iso_date(bad), "{bad:?}");
        }
    }

    #[test]
    fn field_rows_offer_both_casings() {
        let row = methodology_as_field(&json!({ "id": "m", "is_system": true, "click_path_model": "linear" }));
        assert_eq!((row["isSystem"].clone(), row["clickPathModel"].clone()), (json!(true), json!("linear")));
        assert_eq!(row["description"], Value::Null);
        let row = ltv_cohort_as_field(&json!({ "cohort_period": "2026-08-01", "cohort_size": 3 }));
        assert_eq!((row["cohortPeriod"].clone(), row["cohortSize"].clone()), (json!("2026-08-01"), json!(3)));
    }
}

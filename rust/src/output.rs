//! Terminal output helpers (port of `src/output.ts`). Colors apply only when
//! stdout is a terminal; owo-colors also honours NO_COLOR / FORCE_COLOR.
//!
//! The table, `--output` and dry-run helpers serve the resource commands that
//! VE-3666/VE-3667 port next; they are tested here with the rest of the module.
#![allow(dead_code)]

use std::{
    future::Future,
    io::{BufRead, IsTerminal, Write},
    time::Duration,
};

use comfy_table::{Table, presets};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Stream};
use serde_json::Value;

use crate::client::ApiError;

/// `print!`/`println!` (see main.rs): write errors are ignored, as Node's
/// `console` does, so a closed pipe never aborts a command (VE-3727).
pub fn write_stdout(args: std::fmt::Arguments, newline: bool) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_fmt(args);
    if newline {
        let _ = out.write_all(b"\n");
    }
}

pub fn write_stdout_bytes(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

/// `eprint!`/`eprintln!`: like [`write_stdout`], for stderr.
pub fn write_stderr(args: std::fmt::Arguments, newline: bool) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_fmt(args);
    if newline {
        let _ = err.write_all(b"\n");
    }
}

/// Whether stdout is a terminal. Never in unit tests: what they assert must
/// not depend on the terminal that runs `cargo test` (VE-3727).
pub fn stdout_is_tty() -> bool {
    !cfg!(test) && std::io::stdout().is_terminal()
}

/// Style `s` for `stream` when owo-colors says that stream takes colour.
/// Never in unit tests, for the same reason as [`stdout_is_tty`].
pub fn paint(s: &str, stream: Stream, style: impl Fn(&str) -> String) -> String {
    if cfg!(test) {
        return s.to_string();
    }
    s.if_supports_color(stream, |t| style(t)).to_string()
}

macro_rules! color_fns {
    ($($name:ident => $method:ident),* $(,)?) => {
        $(pub fn $name(s: &str) -> String {
            paint(s, Stream::Stdout, |t| t.$method().to_string())
        })*
    };
}

color_fns!(bold => bold, dim => dimmed, green => green, red => red, yellow => yellow, blue => blue, cyan => cyan, gray => bright_black);

pub fn color_status(status: &str) -> String {
    match status {
        "active" | "completed" => green(status),
        "running" | "pending" | "queued" => blue(status),
        "paused" | "inactive" | "cancelled" | "canceled" => gray(status),
        "errored" | "failed" => red(status),
        "warning" => yellow(status),
        _ => status.to_string(),
    }
}

/// Bordered table on a terminal; borderless columns two spaces apart when
/// piped (the TS `createTable`). Cells may contain colored strings.
pub fn table(headers: &[&str]) -> Table {
    let mut table = Table::new();
    if stdout_is_tty() {
        table.load_style(presets::UTF8_FULL);
        table.set_header(headers.iter().map(|h| paint(h, Stream::Stdout, |t| t.cyan().bold().to_string())));
    } else {
        table.load_style(presets::NOTHING);
        table.set_header(headers.to_vec());
    }
    table
}

const SPINNER_FRAMES: &[&str] = &[
    "⡀⠀⠀",
    "⡄⠀⠀",
    "⡆⠀⠀",
    "⡇⠀⠀",
    "⣇⠀⠀",
    "⣧⠀⠀",
    "⣷⠀⠀",
    "⣿⠀⠀",
    "⣿⡀⠀",
    "⣿⡄⠀",
    "⣿⡆⠀",
    "⣿⡇⠀",
    "⣿⣇⠀",
    "⣿⣧⠀",
    "⣿⣷⠀",
    "⣿⣿⠀",
    "⣿⣿⡀",
    "⣿⣿⡄",
    "⣿⣿⡆",
    "⣿⣿⡇",
    "⣿⣿⣇",
    "⣿⣿⣧",
    "⣿⣿⣷",
    "⣿⣿⣿",
    "⣿⣿⣿",
    "⠀⠀⠀",
    "",
];

/// A started spinner on stderr, hidden unless stdout is a terminal (ora's
/// `isSilent: !isTTY`). Call `finish_and_clear` to stop it.
pub fn spinner(label: &str) -> ProgressBar {
    if stdout_is_tty() {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner} {msg}").expect("static template").tick_strings(SPINNER_FRAMES),
        );
        bar.set_message(label.to_string());
        bar.enable_steady_tick(Duration::from_millis(60));
        bar
    } else {
        ProgressBar::hidden()
    }
}

/// Run `fut` behind a spinner (the TS `runAction`). Errors propagate to
/// `main`, which prints them and exits 1.
pub async fn run_action<T, E, F>(label: &str, fut: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    let bar = spinner(label);
    let result = fut.await;
    bar.finish_and_clear();
    result
}

pub fn time_ago(value: Option<&str>) -> String {
    time_ago_at(value, jiff::Timestamp::now())
}

pub fn time_ago_at(value: Option<&str>, now: jiff::Timestamp) -> String {
    let Some(raw) = value.filter(|v| !v.is_empty()) else { return dim("—") };
    let Ok(then) = raw.parse::<jiff::Timestamp>() else { return "Invalid Date".to_string() };
    let diff_ms = now.as_millisecond() - then.as_millisecond();
    if diff_ms < 0 {
        return "just now".to_string();
    }
    let seconds = diff_ms / 1000;
    if seconds < 60 {
        return "just now".to_string();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    then.to_zoned(jiff::tz::TimeZone::system()).strftime("%-m/%-d/%Y").to_string()
}

pub fn short_id(id: &str) -> String {
    if id.chars().count() > 12 { format!("{}...", id.chars().take(8).collect::<String>()) } else { id.to_string() }
}

/// `Number.prototype.toLocaleString()` in en-US: grouped thousands, at most
/// three fraction digits.
pub fn format_number(n: Option<f64>) -> String {
    let Some(n) = n else { return dim("—") };
    let rounded = format!("{:.3}", n.abs());
    let (int, frac) = rounded.split_once('.').unwrap_or((&rounded, ""));
    let frac = frac.trim_end_matches('0');
    let mut grouped = String::new();
    for (i, digit) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if n < 0.0 && (int != "0" || !frac.is_empty()) { "-" } else { "" };
    if frac.is_empty() { format!("{sign}{grouped}") } else { format!("{sign}{grouped}.{frac}") }
}

/// `Date.parse` for the ISO shapes the API and users send: offset datetimes
/// (`Z`, `±HH:MM`, and V8's lenient `±HHMM`), bare dates as UTC midnight, and
/// offset-less datetimes as local time. Milliseconds since the epoch.
pub fn js_date_parse(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(ts) = value.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond());
    }
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    if n >= 5 && matches!(chars[n - 5], '+' | '-') && chars[n - 4..].iter().all(char::is_ascii_digit) {
        let with_colon: String = chars[..n - 2].iter().chain([':'].iter()).chain(chars[n - 2..].iter()).collect();
        if let Ok(ts) = with_colon.parse::<jiff::Timestamp>() {
            return Some(ts.as_millisecond());
        }
    }
    if n == 10
        && let Ok(date) = value.parse::<jiff::civil::Date>()
    {
        return date.to_zoned(jiff::tz::TimeZone::UTC).ok().map(|z| z.timestamp().as_millisecond());
    }
    let datetime = value.parse::<jiff::civil::DateTime>().ok()?;
    datetime.to_zoned(jiff::tz::TimeZone::system()).ok().map(|z| z.timestamp().as_millisecond())
}

/// JavaScript `Number(string)`: JS whitespace trimmed, empty is 0, decimal
/// literals (and `Infinity`) with an optional sign, and unsigned `0x`/`0o`/`0b`
/// integers; anything else is NaN.
pub fn js_number(raw: &str) -> f64 {
    let s = raw.trim_matches(is_js_whitespace);
    if s.is_empty() {
        return 0.0;
    }
    let bytes = s.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            return js_radix_integer(&s[2..], radix);
        }
    }
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    if unsigned == "Infinity" {
        return if s.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    // Rust's float grammar is JavaScript's decimal one, plus `inf`/`nan`.
    if !unsigned.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return f64::NAN;
    }
    s.parse().unwrap_or(f64::NAN)
}

/// ECMAScript WhiteSpace and LineTerminator, which `Number()` trims.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn js_radix_integer(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut exact: Option<u128> = Some(0);
    let mut approx = 0.0_f64;
    for c in digits.chars() {
        let Some(digit) = c.to_digit(radix) else { return f64::NAN };
        exact = exact.and_then(|n| n.checked_mul(radix.into())).and_then(|n| n.checked_add(digit.into()));
        approx = approx * f64::from(radix) + f64::from(digit);
    }
    // Exactly rounded while the value fits in 128 bits, close enough beyond.
    exact.map_or(approx, |n| n as f64)
}

/// `Number.prototype.toString()`, which is also how `JSON.stringify` writes a
/// finite number: the shortest digits that round-trip, in plain notation up
/// to 21 integer digits and down to 6 leading fraction zeros.
pub fn js_number_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n == 0.0 {
        return "0".into();
    }
    // Rust's `{:e}` gives the same shortest round-trip digits.
    let scientific = format!("{:e}", n.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let point = exponent.parse::<i32>().expect("{:e} exponent is an integer") + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat(-point as usize))
    } else {
        let e = point - 1;
        let sign = if e < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1);
        if rest.is_empty() { format!("{first}e{sign}{}", e.abs()) } else { format!("{first}.{rest}e{sign}{}", e.abs()) }
    };
    if n < 0.0 { format!("-{body}") } else { body }
}

/// Compact JSON as `JSON.stringify` writes it. JavaScript parsed every number
/// to a double, so integers beyond 2^53 and floats print the JS way
/// (`12345678901234567000`, `1e+21`, `1` for `1.0`). Used for request bodies.
pub fn js_stringify(value: &Value) -> String {
    use serde::Serialize;
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter);
    value.serialize(&mut serializer).expect("JSON values always serialize");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

struct JsFormatter;

const JS_SAFE_INTEGER: u64 = 1 << 53;

impl serde_json::ser::Formatter for JsFormatter {
    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()> {
        writer.write_all(js_number_string(value).as_bytes())
    }

    fn write_u64<W: ?Sized + Write>(&mut self, writer: &mut W, value: u64) -> std::io::Result<()> {
        if value <= JS_SAFE_INTEGER { write!(writer, "{value}") } else { self.write_f64(writer, value as f64) }
    }

    fn write_i64<W: ?Sized + Write>(&mut self, writer: &mut W, value: i64) -> std::io::Result<()> {
        if value.unsigned_abs() <= JS_SAFE_INTEGER {
            write!(writer, "{value}")
        } else {
            self.write_f64(writer, value as f64)
        }
    }
}

/// `Date.prototype.toISOString()`: UTC with milliseconds.
pub fn js_iso_string(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|ts| ts.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|_| "Invalid Date".to_string())
}

/// JavaScript `String(value)` for JSON values, which `--output <field>` uses.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

pub fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).expect("JSON values always serialize"));
}

/// `--output <field>`: one line per row, skipping rows where it's null/absent.
pub fn field_lines(rows: &[Value], field: &str) -> Vec<String> {
    rows.iter().filter_map(|row| row.get(field)).filter(|value| !value.is_null()).map(js_string).collect()
}

pub fn print_field(rows: &[Value], field: &str) {
    for line in field_lines(rows, field) {
        println!("{line}");
    }
}

pub fn print_single_field(item: &Value, field: &str) {
    print_field(std::slice::from_ref(item), field);
}

pub fn print_label(label: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|v| !v.is_null()) {
        println!("  {label}: {}", js_string(value));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Json,
    Field,
    Table,
}

/// `--json` wins, then `--output <field>`; an empty field is the table, as
/// the TS CLI's `if (opts.output)` had it.
pub fn resolve_output_mode(json: bool, output: Option<&str>) -> OutputMode {
    if json {
        OutputMode::Json
    } else if output.is_some_and(|field| !field.is_empty()) {
        OutputMode::Field
    } else {
        OutputMode::Table
    }
}

pub fn print_count(count: u64, label: &str) {
    let plural = if count == 1 { "" } else { "s" };
    println!("{}", dim(&format!("{count} {label}{plural}")));
}

pub fn print_success(message: &str) {
    println!("{} {message}", green("Done:"));
}

pub fn print_dry_run(action: &str, resource_type: &str, resource_id: &str, details: &[(&str, String)]) {
    println!("{} Would {action} {resource_type} {}", yellow("[dry-run]"), short_id(resource_id));
    for (key, value) in details {
        println!("  {key}: {value}");
    }
}

fn red_err(s: &str) -> String {
    paint(s, Stream::Stderr, |t| t.red().to_string())
}

fn dim_err(s: &str) -> String {
    paint(s, Stream::Stderr, |t| t.dimmed().to_string())
}

/// `Error: <message>` plus the request ID when the API gave one.
pub fn format_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<ApiError>() {
        Some(api) => match api.server_request_id.as_ref().or(api.request_id.as_ref()) {
            Some(id) => format!("{}\n{}", api.message, dim_err(&format!("Request ID: {id}"))),
            None => api.message.clone(),
        },
        None => format!("{err:#}"),
    }
}

pub fn print_error(message: &str) {
    eprintln!("{} {message}", red_err("Error:"));
}

/// Usage-style error with examples (the TS `showArgError`), exit 1.
pub fn arg_error(message: &str, examples: &[&str]) -> ! {
    let mut text = format!("{} {message}\n\nUsage:", red_err("Error:"));
    for example in examples {
        text.push_str(&format!("\n  $ {example}"));
    }
    eprintln!("{text}");
    std::process::exit(1);
}

fn prompt(question: &str) -> Option<String> {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// `(y/N)` prompt. Like the TS CLI, a non-interactive stdout confirms.
pub fn confirm(message: &str) -> bool {
    if !stdout_is_tty() {
        return true;
    }
    prompt(&format!("{message} {} ", dim("(y/N)")))
        .map(|answer| answer.trim().eq_ignore_ascii_case("y"))
        .unwrap_or(false)
}

pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub search_text: String,
}

/// Search-then-pick prompt (the TS `searchSelectOption`). `None` when not a
/// terminal, there are no options, or the user cancels.
pub fn search_select_option(message: &str, options: &[SelectOption]) -> Option<String> {
    if !stdout_is_tty() || options.is_empty() {
        return None;
    }
    println!("{message}");
    loop {
        let query = prompt(&format!("Search {} ", dim("(ENTER for all, q to cancel)")))?.trim().to_lowercase();
        if query == "q" {
            return None;
        }
        let filtered: Vec<&SelectOption> = options
            .iter()
            .filter(|o| query.is_empty() || format!("{} {}", o.label, o.search_text).to_lowercase().contains(&query))
            .collect();
        if filtered.is_empty() {
            println!("{}", dim("No matching profiles. Try a different search."));
            continue;
        }
        let displayed = &filtered[..filtered.len().min(10)];
        for (index, option) in displayed.iter().enumerate() {
            println!("  {}. {}", index + 1, option.label);
        }
        if filtered.len() > displayed.len() {
            println!("{}", dim(&format!("  … {} more matches", filtered.len() - displayed.len())));
        }
        let selection = prompt(&format!("Choose an option {} ", dim("(ENTER to search again)")))?.trim().to_string();
        if selection.is_empty() {
            continue;
        }
        if let Ok(index) = selection.parse::<usize>()
            && (1..=displayed.len()).contains(&index)
        {
            return Some(displayed[index - 1].value.clone());
        }
        if let Some(matched) = filtered.iter().find(|o| o.value == selection || o.label == selection) {
            return Some(matched.value.clone());
        }
        println!("{}", dim("Invalid selection. Search again or choose a listed number."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn color_status_returns_plain_text_when_piped() {
        for status in [
            "active",
            "completed",
            "running",
            "pending",
            "paused",
            "inactive",
            "cancelled",
            "canceled",
            "queued",
            "errored",
            "failed",
            "warning",
            "something-else",
        ] {
            assert_eq!(color_status(status), status);
        }
    }

    #[test]
    fn time_ago_buckets() {
        assert!(time_ago_at(None, at("2026-01-15T12:00:00Z")).contains('—'));
        assert_eq!(time_ago_at(Some("2026-01-15T13:00:00Z"), at("2026-01-15T12:00:00Z")), "just now");
        assert_eq!(time_ago_at(Some("2026-01-15T12:00:00Z"), at("2026-01-15T12:00:30Z")), "just now");
        assert_eq!(time_ago_at(Some("2026-01-15T12:00:00Z"), at("2026-01-15T12:05:00Z")), "5m ago");
        assert_eq!(time_ago_at(Some("2026-01-15T12:00:00Z"), at("2026-01-15T15:00:00Z")), "3h ago");
        assert_eq!(time_ago_at(Some("2026-01-15T12:00:00Z"), at("2026-01-20T12:00:00Z")), "5d ago");
        let old = time_ago_at(Some("2026-01-01T12:00:00Z"), at("2026-03-15T12:00:00Z"));
        assert!(old.contains("/2026") && !old.contains("ago"), "{old}");
    }

    #[test]
    fn js_dates_match_date_parse_and_to_iso_string() {
        let iso = |s: &str| js_date_parse(s).map(js_iso_string);
        assert_eq!(iso("2026-09-30T00:00:00Z").as_deref(), Some("2026-09-30T00:00:00.000Z"));
        assert_eq!(iso("2026-10-04T22:53:56.658342+00:00").as_deref(), Some("2026-10-04T22:53:56.658Z"));
        assert_eq!(iso("2026-07-01T00:00:00-0500").as_deref(), Some("2026-07-01T05:00:00.000Z"));
        assert_eq!(iso("2026-06-29").as_deref(), Some("2026-06-29T00:00:00.000Z"));
        assert_eq!(js_date_parse("invalid"), None);
        assert!(js_date_parse("2026-06-29T06:30:00").is_some(), "naive datetimes parse as local time");
    }

    #[test]
    fn short_id_truncates_after_twelve_characters() {
        assert_eq!(short_id("550e8400-e29b-41d4-a716-446655440000"), "550e8400...");
        assert_eq!(short_id("abc123"), "abc123");
        assert_eq!(short_id("123456789012"), "123456789012");
        assert_eq!(short_id("1234567890123"), "12345678...");
    }

    #[test]
    fn format_number_matches_en_us_to_locale_string() {
        assert_eq!(format_number(Some(1234567.0)), "1,234,567");
        assert_eq!(format_number(Some(0.0)), "0");
        assert_eq!(format_number(Some(0.4)), "0.4");
        assert_eq!(format_number(Some(1234.5678)), "1,234.568");
        assert_eq!(format_number(Some(-1234.0)), "-1,234");
        assert!(format_number(None).contains('—'));
    }

    #[test]
    fn js_string_matches_javascript() {
        assert_eq!(js_string(&json!("a")), "a");
        assert_eq!(js_string(&json!(3)), "3");
        assert_eq!(js_string(&json!(1.0)), "1");
        assert_eq!(js_string(&json!(1.5)), "1.5");
        assert_eq!(js_string(&json!(true)), "true");
        assert_eq!(js_string(&json!(["a", "b", null, 1])), "a,b,,1");
        assert_eq!(js_string(&json!({ "a": 1 })), "[object Object]");
    }

    #[test]
    fn field_lines_skip_missing_and_null_values() {
        let rows = vec![json!({ "id": "a" }), json!({ "id": null }), json!({}), json!({ "id": 2 })];
        assert_eq!(field_lines(&rows, "id"), ["a", "2"]);
    }

    #[test]
    fn output_mode_prefers_json_then_field() {
        assert_eq!(resolve_output_mode(true, Some("id")), OutputMode::Json);
        assert_eq!(resolve_output_mode(false, Some("id")), OutputMode::Field);
        assert_eq!(resolve_output_mode(false, None), OutputMode::Table);
    }

    #[test]
    fn format_error_appends_the_request_id() {
        let err = anyhow::Error::new(ApiError {
            message: "Bad request".into(),
            status: 400,
            code: None,
            request_id: Some("cli-123".into()),
            server_request_id: Some("req_456".into()),
            details: None,
            status_text: Some("Bad Request".into()),
        });
        assert_eq!(format_error(&err), "Bad request\nRequest ID: req_456");
        assert_eq!(format_error(&anyhow::anyhow!("plain")), "plain");
    }

    #[test]
    fn an_empty_output_field_is_the_table() {
        assert_eq!(resolve_output_mode(false, Some("")), OutputMode::Table);
        assert_eq!(resolve_output_mode(true, Some("")), OutputMode::Json);
    }

    /// `Number(s)` as Node 24.16 evaluates it (VE-3727).
    const JS_NUMBERS: &[(&str, &str)] = &[
        ("5", "5"),
        (" 2.5 ", "2.5"),
        ("1e1", "10"),
        ("0x10", "16"),
        ("0X1f", "31"),
        ("0b11", "3"),
        ("0B11", "3"),
        ("0o7", "7"),
        ("0O17", "15"),
        ("-0x10", "NaN"),
        ("+0x10", "NaN"),
        ("0x", "NaN"),
        ("0b2", "NaN"),
        ("0o8", "NaN"),
        ("Infinity", "Infinity"),
        ("-Infinity", "-Infinity"),
        ("+Infinity", "Infinity"),
        ("infinity", "NaN"),
        ("inf", "NaN"),
        ("NaN", "NaN"),
        ("", "0"),
        ("   ", "0"),
        ("\t\n 5 \n", "5"),
        (".5", "0.5"),
        ("5.", "5"),
        (".", "NaN"),
        ("1e", "NaN"),
        ("e1", "NaN"),
        ("1_000", "NaN"),
        ("12345678901234567890", "12345678901234567000"),
        ("0.1", "0.1"),
        ("-0", "-0"),
        ("1e21", "1e+21"),
        ("1e-7", "1e-7"),
        ("00012", "12"),
        ("0012.5", "12.5"),
        ("1.5e+3", "1500"),
        ("1.5E-3", "0.0015"),
        ("\u{a0}5\u{a0}", "5"),
        ("\u{feff}5", "5"),
        ("\u{2028}5\u{2029}", "5"),
        ("\u{85}5", "NaN"),
        ("\u{180e}5", "NaN"),
        ("\u{ff11}\u{ff12}", "NaN"),
        ("5abc", "NaN"),
        ("abc", "NaN"),
        ("0x1g", "NaN"),
        ("--5", "NaN"),
        ("+-5", "NaN"),
        ("0xffffffffffffffffffff", "1.2089258196146292e+24"),
        ("0x1fffffffffffff", "9007199254740991"),
        ("0x20000000000001", "9007199254740992"),
        ("2.0", "2"),
        ("0x2", "2"),
        ("+2", "2"),
        ("2e0", "2"),
        (" 0b10 ", "2"),
        ("-5", "-5"),
        ("+.5", "0.5"),
        ("-.5e1", "-5"),
        ("1e400", "Infinity"),
        ("-1e400", "-Infinity"),
        ("0.0000001", "1e-7"),
        ("1E+2", "100"),
        ("0e0", "0"),
        ("0x0", "0"),
        ("007", "7"),
        ("08", "8"),
        ("1.", "1"),
        ("1..2", "NaN"),
        ("1e5e5", "NaN"),
        ("0b", "NaN"),
        ("0o", "NaN"),
        ("\u{b}5\u{c}", "5"),
    ];

    /// `JSON.stringify(n)` in Node 24.16.
    const JS_NUMBER_STRINGS: &[(f64, &str)] = &[
        (0.0, "0"),
        (0.0, "0"),
        (1.0, "1"),
        (-1.0, "-1"),
        (16.0, "16"),
        (0.1, "0.1"),
        (1.5, "1.5"),
        (1e+21, "1e+21"),
        (1e-07, "1e-7"),
        (1.2345678901234568e+20, "123456789012345680000"),
        (1.2345678901234567e+19, "12345678901234567000"),
        (1e-06, "0.000001"),
        (1e-06, "0.000001"),
        (1.7976931348623157e+308, "1.7976931348623157e+308"),
        (5e-324, "5e-324"),
        (9007199254740992.0, "9007199254740992"),
        (9007199254740994.0, "9007199254740994"),
        (100.0, "100"),
        (1e+20, "100000000000000000000"),
        (1.23e-18, "1.23e-18"),
        (-1.5e-09, "-1.5e-9"),
        (31.4159, "31.4159"),
        (0.000123, "0.000123"),
        (1234.5678, "1234.5678"),
        (9.95e+20, "995000000000000000000"),
        (1e+300, "1e+300"),
        (1.8446744073709552e+19, "18446744073709552000"),
        (-1e-07, "-1e-7"),
        (4.35, "4.35"),
        (0.19999999999999998, "0.19999999999999998"),
    ];

    fn js_number_text(n: f64) -> String {
        if n.is_nan() {
            "NaN".into()
        } else if n == 0.0 && n.is_sign_negative() {
            "-0".into()
        } else {
            js_number_string(n)
        }
    }

    #[test]
    fn js_number_matches_javascript_number() {
        for (input, expected) in JS_NUMBERS {
            assert_eq!(js_number_text(js_number(input)), *expected, "Number({input:?})");
        }
    }

    #[test]
    fn js_number_string_matches_json_stringify() {
        for (n, expected) in JS_NUMBER_STRINGS {
            assert_eq!(js_number_string(*n), *expected, "{n:?}");
        }
        assert_eq!(js_number_string(-0.0), "0");
    }

    #[test]
    fn js_stringify_writes_numbers_the_way_javascript_does() {
        let body = json!({ "a": 12345678901234567890u64, "b": 1.0, "c": -0.0, "d": [1e21, 0.1, -5, 9007199254740993u64], "e": "x\u{2028}" });
        assert_eq!(
            js_stringify(&body),
            "{\"a\":12345678901234567000,\"b\":1,\"c\":0,\"d\":[1e+21,0.1,-5,9007199254740992],\"e\":\"x\u{2028}\"}"
        );
    }
}

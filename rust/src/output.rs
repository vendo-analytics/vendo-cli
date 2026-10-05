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

mod locale;
mod usd_patterns;
mod v8_json;
mod ymd_patterns;

/// JavaScript truthiness for a JSON value.
pub fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => js_number_of(n) != 0.0,
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

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
    let Some(then_ms) = js_date_parse(raw) else { return "Invalid Date".to_string() };
    let diff_ms = now.as_millisecond() - then_ms;
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
    to_locale_date_string(then_ms)
}

/// `id.length > 12 ? id.slice(0, 8) + '...' : id`, in UTF-16 units like JavaScript.
pub fn short_id(id: &str) -> String {
    if js_length(id) > 12 { format!("{}...", js_slice(id, 8)) } else { id.to_string() }
}

/// A string's `length`: UTF-16 code units.
pub fn js_length(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(0, end)` in UTF-16 units. A surrogate pair cut in half leaves a lone surrogate,
/// which Node prints as U+FFFD.
pub fn js_slice(s: &str, end: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(end).collect();
    String::from_utf16_lossy(&units)
}

/// `formatNumber`: `n.toLocaleString()` in the user's locale (see [`locale`]), a dash for none.
pub fn format_number(n: Option<&Value>) -> String {
    match n.filter(|v| !v.is_null()) {
        Some(value) => js_to_locale_string(value),
        None => dim("—"),
    }
}

/// `n.toLocaleString()` for a JavaScript number, in the user's locale.
pub fn to_locale_number(n: f64) -> String {
    locale::with_current(|format| format.number(n))
}

/// The measurement views' money format, `n.toLocaleString(undefined, { style: 'currency',
/// currency: 'USD', maximumFractionDigits: 2 })`, in the user's locale.
pub fn format_usd(n: f64) -> String {
    locale::with_current(|format| format.usd(n))
}

/// `value.toLocaleString()`: numbers in the user's locale, strings as they are, arrays element by
/// element joined with ",", objects as `[object Object]`.
pub fn js_to_locale_string(value: &Value) -> String {
    match value {
        Value::Number(n) => to_locale_number(js_number_of(n)),
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_to_locale_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        other => js_string(other),
    }
}

/// `${object.field}`: `undefined` when the field is missing, else `String(value)`.
pub fn js_template(value: Option<&Value>) -> String {
    value.map(js_string).unwrap_or_else(|| "undefined".to_string())
}

/// `a ?? b`.
pub fn js_nullish<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    a.filter(|v| !v.is_null()).or(b)
}

/// `items.join(sep)`: null elements are empty.
pub fn js_join(items: &[Value], sep: &str) -> String {
    items.iter().map(|item| if item.is_null() { String::new() } else { js_string(item) }).collect::<Vec<_>>().join(sep)
}

/// `colorStatus(status)` in a template: the known statuses coloured, anything else as is.
pub fn js_color_status(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(status)) => color_status(status),
        other => js_template(other),
    }
}

/// `if (value)`: the value as a string when truthy.
pub fn js_if(value: Option<&Value>) -> Option<String> {
    value.filter(|v| js_truthy(v)).map(js_string)
}

/// The failure counters: `value && value > 0 ? String(value) : …`.
pub fn js_positive(value: Option<&Value>) -> Option<String> {
    value.filter(|v| js_truthy(v) && js_greater_than(v, 0.0)).map(js_string)
}

/// `printLabel(label, colorStatus(value))`: no line when the status is null or missing.
pub fn print_status_label(label: &str, value: Option<&Value>) {
    if let Some(status) = value.filter(|v| !v.is_null()) {
        println!("  {label}: {}", js_color_status(Some(status)));
    }
}

/// The list footer, `printCount(res.meta?.pagination?.total ?? res.data.length, label)`.
pub fn print_list_count(res: &Value, rows: usize, label: &str) {
    println!("{}", dim(&list_count(res, rows, label)));
}

/// `${count} ${label}${count === 1 ? '' : 's'}`: the total as JavaScript prints it, singular only
/// for the number 1 (a total sent as the string "1" is plural).
fn list_count(res: &Value, rows: usize, label: &str) -> String {
    let total =
        res.pointer("/meta/pagination/total").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| Value::from(rows));
    let plural = if matches!(&total, Value::Number(n) if js_number_of(n) == 1.0) { "" } else { "s" };
    format!("{} {label}{plural}", js_string(&total))
}

/// JavaScript `ToNumber` (objects become NaN, arrays go through their string form).
pub fn js_to_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => js_number_of(n),
        Value::String(s) => js_number(s),
        Value::Array(_) => js_number(&js_string(value)),
        Value::Object(_) => f64::NAN,
    }
}

/// `value > n` for a number `n`.
pub fn js_greater_than(value: &Value, n: f64) -> bool {
    js_to_number(value) > n
}

/// `value + 1`: concatenation when the value is a string (or becomes one), else addition.
pub fn js_plus_one(value: &Value) -> String {
    match value {
        Value::String(_) | Value::Array(_) | Value::Object(_) => format!("{}1", js_string(value)),
        other => js_number_string(js_to_number(other) + 1.0),
    }
}

/// `a === b` for JSON values: numbers by value, strings and booleans exactly, and arrays or
/// objects never (they are different objects).
pub fn js_strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => js_number_of(x) == js_number_of(y),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// `new Date(ms).toLocaleDateString()` in the user's locale and time zone.
pub fn to_locale_date_string(ms: i64) -> String {
    locale::with_current(|format| format.date(ms, &jiff::tz::TimeZone::system()))
}

/// `new Date(ms).toLocaleTimeString()` in the user's locale and time zone.
pub fn to_locale_time_string(ms: i64) -> String {
    locale::with_current(|format| format.time(ms, &jiff::tz::TimeZone::system()))
}

/// JavaScript `parseInt(s, 10)`: leading whitespace, an optional sign, then as many decimal
/// digits as there are. `None` (NaN) without a digit.
pub fn js_parse_int(s: &str) -> Option<f64> {
    let s = s.trim_start_matches(is_js_whitespace);
    let (negative, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let n: f64 = rest[..digits].parse().ok()?;
    Some(if negative { -n } else { n })
}

/// `Date.parse` as Node evaluates it (see `js_date`): milliseconds since the
/// epoch, `None` where JavaScript gives NaN.
pub fn js_date_parse(value: &str) -> Option<i64> {
    crate::js_date::parse(value)
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

    /// Numbers parsed from text (an API response or a `--*-file`): through a double, as
    /// `JSON.parse` then `JSON.stringify` did, so `1.0` is `1` and `1e400` is `null`.
    fn write_number_str<W: ?Sized + Write>(&mut self, writer: &mut W, value: &str) -> std::io::Result<()> {
        match value.parse::<f64>() {
            Ok(n) if n.is_finite() => self.write_f64(writer, n),
            _ => writer.write_all(b"null"),
        }
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
    crate::js_date::iso_string(ms)
}

/// JavaScript `String(value)` for JSON values, which `--output <field>` uses.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number_string(js_number_of(n)),
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

/// The number JavaScript's `JSON.parse` makes of a JSON number: the nearest double, ±Infinity
/// beyond the range. Numbers keep the API's text (serde_json `arbitrary_precision`).
pub fn js_number_of(n: &serde_json::Number) -> f64 {
    n.as_str().parse().unwrap_or(f64::NAN)
}

/// JavaScript's property order, which `JSON.parse` and `JSON.stringify` keep:
/// array-index keys ("0" to "4294967294") first in ascending order, then the
/// rest in insertion order, at every level.
pub fn js_key_order(map: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let (mut indexes, rest): (Vec<_>, Vec<_>) = map.into_iter().partition(|(key, _)| array_index(key).is_some());
    indexes.sort_by_key(|(key, _)| array_index(key));
    indexes.into_iter().chain(rest).map(|(key, value)| (key, js_key_order_value(value))).collect()
}

fn js_key_order_value(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(js_key_order(map)),
        Value::Array(items) => Value::Array(items.into_iter().map(js_key_order_value).collect()),
        other => other,
    }
}

/// A canonical array index: digits without a leading zero, below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
    let canonical = !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) && (key == "0" || !key.starts_with('0'));
    canonical.then(|| key.parse::<u32>().ok()).flatten().filter(|n| *n < u32::MAX)
}

/// The TS client's `toCamelCaseDeep`: `_x` becomes `X` for x in a-z and 0-9, in every key; a
/// key that maps onto an earlier one replaces its value in place, as object assignment does.
pub fn camel_case_keys_deep(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(snake_to_camel(key), camel_case_keys_deep(value));
            }
            Value::Object(js_key_order(out))
        }
        Value::Array(items) => Value::Array(items.iter().map(camel_case_keys_deep).collect()),
        other => other.clone(),
    }
}

fn snake_to_camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut chars = key.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&next) if c == '_' && (next.is_ascii_lowercase() || next.is_ascii_digit()) => {
                out.push(next.to_ascii_uppercase());
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// `JSON.parse(text)`: serde_json parses, and an error reads the way V8 words it.
pub fn parse_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|err| v8_json::parse_error(text).unwrap_or_else(|| err.to_string()))
}

/// The text `res.json()` parses: UTF-8 with a leading byte order mark dropped, invalid bytes as
/// U+FFFD, as fetch decodes a body.
pub fn fetch_body_text(body: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body))
}

/// `--json` output: pretty-printed with two spaces like `JSON.stringify(data, null, 2)`, and
/// numbers exactly as the API sent them (decided 2026-10-05), which is what the TS CLI printed
/// for anything a Node server sends.
pub fn json_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON values always serialize")
}

pub fn print_json(value: &Value) {
    println!("{}", json_pretty(value));
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
        // An empty X-Request-Id doesn't count: the CLI's own ID is shown instead.
        Some(api) => match [&api.server_request_id, &api.request_id].into_iter().flatten().find(|id| !id.is_empty()) {
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

/// What a prompt read.
#[derive(Debug, PartialEq)]
pub enum Answer {
    Line(String),
    /// Ctrl-C, Ctrl-D or the end of the input.
    Closed,
}

/// One answer: up to and including a newline. A line holding the terminal's
/// interrupt character (Ctrl-C, see [`InterruptAsInput`]) or the end of the
/// input closes the prompt instead.
fn read_answer(input: &mut impl BufRead, interrupt: Option<u8>) -> Answer {
    let mut line = Vec::new();
    loop {
        let buf = match input.fill_buf() {
            Ok(buf) => buf,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Answer::Closed,
        };
        if buf.is_empty() {
            return Answer::Closed;
        }
        match buf.iter().position(|b| *b == b'\n' || Some(*b) == interrupt) {
            Some(end) => {
                let closed = Some(buf[end]) == interrupt;
                line.extend_from_slice(&buf[..=end]);
                input.consume(end + 1);
                return if closed { Answer::Closed } else { Answer::Line(String::from_utf8_lossy(&line).into_owned()) };
            }
            None => {
                let n = buf.len();
                line.extend_from_slice(buf);
                input.consume(n);
            }
        }
    }
}

fn prompt(question: &str) -> Answer {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let terminal = InterruptAsInput::enable();
    let answer = read_answer(&mut std::io::stdin().lock(), terminal.key);
    drop(terminal);
    answer
}

/// Ctrl-C or Ctrl-D at a prompt: Node's readline closed and the question
/// never resolved, so the TS CLI exited 0 with nothing printed or changed.
fn quit_quietly() -> ! {
    std::process::exit(0)
}

/// `(y/N)` prompt. Like the TS CLI, a non-interactive stdout confirms.
pub fn confirm(message: &str) -> bool {
    if !stdout_is_tty() {
        return true;
    }
    match prompt(&format!("{message} {} ", dim("(y/N)"))) {
        Answer::Line(answer) => answer.trim().eq_ignore_ascii_case("y"),
        Answer::Closed => quit_quietly(),
    }
}

/// While a prompt reads the terminal, its interrupt key (Ctrl-C) arrives as
/// input and ends the line instead of raising SIGINT, as Node's readline
/// read the terminal raw. The settings are restored on drop.
struct InterruptAsInput {
    #[cfg(unix)]
    saved: Option<libc::termios>,
    key: Option<u8>,
}

impl InterruptAsInput {
    #[cfg(unix)]
    fn enable() -> Self {
        let off = InterruptAsInput { saved: None, key: None };
        let fd = libc::STDIN_FILENO;
        // SAFETY: plain termios calls on stdin; `settings` is fully written by
        // tcgetattr before it is read or passed back to tcsetattr.
        unsafe {
            if libc::isatty(fd) != 1 {
                return off;
            }
            let mut settings: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut settings) != 0 {
                return off;
            }
            let saved = settings;
            let key = settings.c_cc[libc::VINTR];
            if key == libc::_POSIX_VDISABLE {
                return off;
            }
            // ECHOCTL off too: readline echoed nothing for Ctrl-C or Ctrl-D.
            settings.c_lflag &= !(libc::ISIG | libc::ECHOCTL);
            settings.c_cc[libc::VEOL] = key;
            if libc::tcsetattr(fd, libc::TCSANOW, &settings) != 0 {
                return off;
            }
            InterruptAsInput { saved: Some(saved), key: Some(key) }
        }
    }

    #[cfg(not(unix))]
    fn enable() -> Self {
        InterruptAsInput { key: None }
    }
}

#[cfg(unix)]
impl Drop for InterruptAsInput {
    fn drop(&mut self) {
        if let Some(saved) = &self.saved {
            // SAFETY: puts back the settings `enable` read from the same descriptor.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, saved);
            }
        }
    }
}

/// A picker answer as a 0-based index: `Number(answer)` must be an integer
/// from 1 to `count`, so `2.0` and `0x2` pick the second option, as in TS.
fn selection_index(answer: &str, count: usize) -> Option<usize> {
    let n = js_number(answer);
    (n.fract() == 0.0 && n >= 1.0 && n <= count as f64).then(|| n as usize - 1)
}

pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub search_text: String,
}

/// Search-then-pick prompt (the TS `searchSelectOption`). `None` when not a
/// terminal, there are no options, or the user types `q`; Ctrl-C and Ctrl-D
/// end the command quietly.
pub fn search_select_option(message: &str, options: &[SelectOption]) -> Option<String> {
    if !stdout_is_tty() || options.is_empty() {
        return None;
    }
    let ask = |question: String| match prompt(&question) {
        Answer::Line(line) => line,
        Answer::Closed => quit_quietly(),
    };
    println!("{message}");
    loop {
        let query = ask(format!("Search {} ", dim("(ENTER for all, q to cancel)"))).trim().to_lowercase();
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
        let selection = ask(format!("Choose an option {} ", dim("(ENTER to search again)"))).trim().to_string();
        if selection.is_empty() {
            continue;
        }
        if let Some(index) = selection_index(&selection, displayed.len()) {
            return Some(displayed[index].value.clone());
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
        assert_eq!(format_number(Some(&json!(1234567.0))), "1,234,567");
        assert_eq!(format_number(Some(&json!(0.0))), "0");
        assert_eq!(format_number(Some(&json!(0.4))), "0.4");
        assert_eq!(format_number(Some(&json!(1234.5678))), "1,234.568");
        assert_eq!(format_number(Some(&json!(-1234.0))), "-1,234");
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

    #[test]
    fn an_answer_ends_at_a_newline_and_closes_on_interrupt_or_end_of_input() {
        let read = |bytes: &[u8]| read_answer(&mut std::io::Cursor::new(bytes.to_vec()), Some(3));
        assert_eq!(read(b"y\n"), Answer::Line("y\n".into()));
        assert_eq!(read(b"2.0\nrest"), Answer::Line("2.0\n".into()));
        for closed in [&b""[..], b"\x03", b"ab\x03", b"a\x03b\n", b"no newline"] {
            assert_eq!(read(closed), Answer::Closed, "{closed:?}");
        }
        // Without an interrupt key (stdin is not a terminal) 0x03 is just input.
        let mut piped = std::io::Cursor::new(b"\x03\n".to_vec());
        assert_eq!(read_answer(&mut piped, None), Answer::Line("\x03\n".into()));
    }

    #[test]
    fn picker_numbers_are_read_with_javascript_number() {
        for (answer, expected) in [
            ("2", Some(1)),
            ("2.0", Some(1)),
            ("0x2", Some(1)),
            ("+2", Some(1)),
            ("2e0", Some(1)),
            ("1", Some(0)),
            ("3", None),
            ("0", None),
            ("1.5", None),
            ("abc", None),
            ("Infinity", None),
        ] {
            assert_eq!(selection_index(answer, 2), expected, "{answer:?}");
        }
    }

    #[test]
    fn parse_int_matches_javascript() {
        for (input, expected) in [
            ("5", Some(5.0)),
            ("  3abc", Some(3.0)),
            ("+4", Some(4.0)),
            (" -1", Some(-1.0)),
            ("x3", None),
            ("", None),
            ("0x10", Some(0.0)),
            ("1e3", Some(1.0)),
            ("\u{663}", None),
            ("4.9", Some(4.0)),
            ("\u{a0}2", Some(2.0)),
            ("99999999999999999999", Some(1e20)),
        ] {
            assert_eq!(js_parse_int(input), expected, "{input:?}");
        }
    }

    #[test]
    fn old_dates_and_numbers_use_the_locale() {
        // Under `cargo test` the locale is en-US, whatever LANG is.
        let old = time_ago_at(Some("2026-01-01T12:00:00Z"), at("2026-03-15T12:00:00Z"));
        assert_eq!(old, to_locale_date_string(js_date_parse("2026-01-01T12:00:00Z").unwrap()));
        // Half away from zero on the shortest digits, as Node's ICU does: 1.0005 is 1.001, not 1.
        assert_eq!(format_number(Some(&json!(1.0005))), "1.001");
        assert_eq!(format_number(Some(&json!(-0.0))), "-0");
        assert_eq!(format_number(Some(&json!(-0.0001))), "-0");
        assert_eq!(format_number(Some(&json!(-0.0005))), "-0.001");
    }

    /// A body as a Node server writes it (`JSON.stringify`) and what `node dist/cli.js … --json`
    /// printed for it against a stub (VE-3728).
    const NODE_BODY: &str = r#"{"data":{"accountId":"acct-alpha","a":0.000001,"b":1e+21,"c":1.5,"d":0,"e":9007199254740992,"f":1,"g":1e+21,"h":1e+21,"i":1e-7,"j":12345678901234567000,"k":100,"l":0,"m":1.5,"n":[100000000000000000000,2.5e-7,-1.5e+300]}}"#;
    pub(crate) const TS_JSON_OUTPUT: &str = r#"{
  "data": {
    "accountId": "acct-alpha",
    "a": 0.000001,
    "b": 1e+21,
    "c": 1.5,
    "d": 0,
    "e": 9007199254740992,
    "f": 1,
    "g": 1e+21,
    "h": 1e+21,
    "i": 1e-7,
    "j": 12345678901234567000,
    "k": 100,
    "l": 0,
    "m": 1.5,
    "n": [
      100000000000000000000,
      2.5e-7,
      -1.5e+300
    ]
  }
}"#;

    #[test]
    fn json_output_keeps_numbers_as_the_api_sent_them() {
        let body: Value = serde_json::from_str(NODE_BODY).unwrap();
        assert_eq!(json_pretty(&body), TS_JSON_OUTPUT);
        // Forms a Node server never writes print as sent too (decided 2026-10-05), except that
        // serde_json writes integers and exponents canonically: -0 is 0 and 1E21 is 1e+21, as
        // JSON.stringify writes them. TS printed 1, 9007199254740992 and 1e+21 for the others.
        let raw: Value = serde_json::from_str(r#"{"a":1.0,"b":-0,"c":9007199254740993,"d":1E21}"#).unwrap();
        assert_eq!(json_pretty(&raw), "{\n  \"a\": 1.0,\n  \"b\": 0,\n  \"c\": 9007199254740993,\n  \"d\": 1e+21\n}");
        // Key order and number reads still work.
        assert_eq!(body["data"].as_object().unwrap().keys().next().unwrap(), "accountId");
        assert_eq!((body["data"]["k"].as_u64(), body["data"]["c"].as_f64()), (Some(100), Some(1.5)));
    }

    #[test]
    fn js_string_reads_numbers_as_javascript_doubles() {
        let raw: Value =
            serde_json::from_str(r#"[1.0, -0, 9007199254740993, 1E21, 1e400, 0.000001, 1e-7, 12.50]"#).unwrap();
        let strings: Vec<String> = raw.as_array().unwrap().iter().map(js_string).collect();
        assert_eq!(strings, ["1", "0", "9007199254740992", "1e+21", "Infinity", "0.000001", "1e-7", "12.5"]);
        assert!(!js_truthy(&raw[1]) && js_truthy(&raw[4]));
    }

    #[test]
    fn request_bodies_write_numbers_like_json_stringify() {
        // A config file goes through JSON.parse then JSON.stringify in the TS CLI.
        let file: Value =
            serde_json::from_str(r#"{"a":1.0,"b":-0,"c":9007199254740993,"d":1E21,"e":1e400,"f":[0.000001]}"#).unwrap();
        assert_eq!(js_stringify(&file), r#"{"a":1,"b":0,"c":9007199254740992,"d":1e+21,"e":null,"f":[0.000001]}"#);
    }

    #[test]
    fn camel_case_keys_like_the_ts_client() {
        let value: Value =
            serde_json::from_str(r#"{"frequency_value":1,"a__b":2,"x_1":3,"_leading":4,"trailing_":5,"UPPER_CASE":6,"nested":{"run_now_at":[{"f_g":1.0}]},"2":7}"#)
                .unwrap();
        assert_eq!(
            js_stringify(&camel_case_keys_deep(&value)),
            r#"{"2":7,"frequencyValue":1,"a_B":2,"x1":3,"Leading":4,"trailing_":5,"UPPER_CASE":6,"nested":{"runNowAt":[{"fG":1}]}}"#
        );
    }

    #[test]
    fn javascript_coercions_match_node() {
        let cases: Vec<(Value, &str, bool, bool, &str)> = vec![
            // value, `v + 1`, `v > 1`, `v > 0`, `v.toLocaleString()` (from Node)
            (json!(5), "6", true, true, "5"),
            (json!("5"), "51", true, true, "5"),
            (json!("abc"), "abc1", false, false, "abc"),
            (json!(""), "1", false, false, ""),
            (json!(true), "2", false, true, "true"),
            (json!(false), "1", false, false, "false"),
            (json!(null), "1", false, false, ""),
            (json!([3]), "31", true, true, "3"),
            (json!([1, 2]), "1,21", false, false, "1,2"),
            (json!({}), "[object Object]1", false, false, "[object Object]"),
            (json!(2.5), "3.5", true, true, "2.5"),
            (json!("0x10"), "0x101", true, true, "0x10"),
            (json!(" 7 "), " 7 1", true, true, " 7 "),
        ];
        for (value, plus_one, gt_one, gt_zero, locale) in cases {
            assert_eq!(js_plus_one(&value), plus_one, "{value} + 1");
            assert_eq!(js_greater_than(&value, 1.0), gt_one, "{value} > 1");
            assert_eq!(js_greater_than(&value, 0.0), gt_zero, "{value} > 0");
            if !value.is_null() {
                assert_eq!(js_to_locale_string(&value), locale, "{value}.toLocaleString()");
            }
        }
        assert_eq!(js_to_locale_string(&json!([1234.5, null, "x", [5678]])), "1,234.5,,x,5,678");
        assert_eq!(format_number(Some(&json!("1234"))), "1234");
        assert_eq!(format_number(Some(&json!(1234.5))), "1,234.5");
        assert!(format_number(Some(&Value::Null)).contains('—') && format_number(None).contains('—'));
    }

    #[test]
    fn templates_joins_and_equality_follow_javascript() {
        assert_eq!(js_template(None), "undefined");
        assert_eq!(js_template(Some(&json!(null))), "null");
        assert_eq!(js_template(Some(&json!(1.0))), "1");
        assert_eq!(
            js_join(&[json!(1), json!(null), json!("a"), json!(2.5), json!([3, null])], ", "),
            "1, , a, 2.5, 3,"
        );
        assert_eq!(js_nullish(Some(&json!(null)), Some(&json!("b"))), Some(&json!("b")));
        assert_eq!(js_nullish(Some(&json!("")), Some(&json!("b"))), Some(&json!("")));
        assert_eq!(js_nullish(None, None), None);
        assert!(js_strict_equals(&json!(5), &json!(5.0)) && js_strict_equals(&json!(null), &json!(null)));
        assert!(!js_strict_equals(&json!("5"), &json!(5)) && !js_strict_equals(&json!([1]), &json!([1])));
        assert_eq!(js_color_status(Some(&json!("active"))), "active");
        assert_eq!(js_color_status(None), "undefined");
        assert_eq!(js_color_status(Some(&json!(null))), "null");
    }

    #[test]
    fn list_footers_print_the_total_as_javascript_does() {
        let footer = |meta: Value| list_count(&json!({ "data": [], "meta": meta }), 3, "app");
        assert_eq!(footer(json!({ "pagination": { "total": 1 } })), "1 app");
        assert_eq!(footer(json!({ "pagination": { "total": 1.0 } })), "1 app");
        assert_eq!(footer(json!({ "pagination": { "total": "1" } })), "1 apps");
        assert_eq!(footer(json!({ "pagination": { "total": 2.5 } })), "2.5 apps");
        assert_eq!(footer(json!({ "pagination": { "total": 0 } })), "0 apps");
        assert_eq!(footer(json!({ "pagination": { "total": "12" } })), "12 apps");
        assert_eq!(footer(json!({ "pagination": { "total": null } })), "3 apps");
        assert_eq!(footer(json!({ "pagination": [] })), "3 apps");
        assert_eq!(footer(json!(null)), "3 apps");
        assert_eq!(list_count(&json!({ "data": [{}] }), 1, "job"), "1 job");
    }

    #[test]
    fn short_ids_count_utf16_units_like_javascript() {
        assert_eq!(short_id("ab\u{1f600}cdefghijklmnop"), "ab\u{1f600}cdef...");
        // slice(0, 8) can split a surrogate pair; Node prints the lone half as U+FFFD.
        assert_eq!(short_id("abcdefg\u{1f600}xyzw"), "abcdefg\u{fffd}...");
        // Six emoji are 12 units, so unchanged; seven are cut after four.
        assert_eq!(short_id(&"\u{1f600}".repeat(6)), "\u{1f600}".repeat(6));
        assert_eq!(short_id(&"\u{1f600}".repeat(7)), format!("{}...", "\u{1f600}".repeat(4)));
        assert_eq!(js_slice("ab\u{1f600}cd", 3), "ab\u{fffd}");
        assert_eq!(js_length("ab\u{1f600}"), 4);
    }
}

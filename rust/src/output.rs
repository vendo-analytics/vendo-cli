//! Terminal output helpers (port of `src/output.ts`). Colors apply only when
//! stdout is a terminal; owo-colors also honours NO_COLOR / FORCE_COLOR.
//!
//! The table, `--output` and dry-run helpers serve the resource commands that
//! VE-3666/VE-3667 port next; they are tested here with the rest of the module.
#![allow(dead_code)]

use std::{
    future::Future,
    io::{BufRead, IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
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

/// `printCount(count, label)` for a count the API sent, as it sent it.
pub fn print_count_of(count: Option<&Value>, label: &str) {
    println!("{}", dim(&count_text(count, label)));
}

fn list_count(res: &Value, rows: usize, label: &str) -> String {
    let rows = Value::from(rows);
    count_text(Some(res.pointer("/meta/pagination/total").filter(|v| !v.is_null()).unwrap_or(&rows)), label)
}

/// `${count} ${label}${count === 1 ? '' : 's'}`: the count as a template literal prints it (a
/// missing one is `undefined`), singular only for the number 1 (the string "1" is plural).
fn count_text(count: Option<&Value>, label: &str) -> String {
    let plural = if matches!(count, Some(Value::Number(n)) if js_number_of(n) == 1.0) { "" } else { "s" };
    format!("{} {label}{plural}", js_template(count))
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
pub fn array_index(key: &str) -> Option<u32> {
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

/// `JSON.parse(text)`: serde_json parses, keys take JavaScript's property order (array-index keys
/// first, as the TS CLI printed and iterated them), and an error reads the way V8 words it.
pub fn parse_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text)
        .map(js_key_order_value)
        .map_err(|err| v8_json::parse_error(text).unwrap_or_else(|| err.to_string()))
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

/// The request ID an error shows: the server's `X-Request-Id`, else the CLI's own. An empty
/// `X-Request-Id` doesn't count.
fn shown_request_id(api: &ApiError) -> Option<&str> {
    [&api.server_request_id, &api.request_id].into_iter().flatten().map(String::as_str).find(|id| !id.is_empty())
}

/// `Error: <message>` plus the request ID when the API gave one.
pub fn format_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<ApiError>() {
        Some(api) => match shown_request_id(api) {
            Some(id) => format!("{}\n{}", api.message, dim_err(&format!("Request ID: {id}"))),
            None => api.message.clone(),
        },
        None => format!("{err:#}"),
    }
}

pub fn print_error(message: &str) {
    eprintln!("{} {message}", red_err("Error:"));
}

/// Set when the command was given `--json` (VE-3831): its errors print as [`error_json`].
static JSON_ERRORS: AtomicBool = AtomicBool::new(false);

pub fn set_json_errors(on: bool) {
    JSON_ERRORS.store(on, Ordering::Relaxed);
}

/// The one shape every error takes with `--json` (VE-3831), documented in AGENTS.md:
/// `{"error":{"message","code","status","requestId"}}`. `message` is what the text error says
/// after `Error: `; `code` is the API's `error.code`; `status` the HTTP status the API answered;
/// `requestId` the ID the text error shows. Each is `null` where it does not apply: an error
/// raised by the CLI itself has a message only, and no response means no status.
pub fn error_json(err: &anyhow::Error) -> Value {
    match err.downcast_ref::<ApiError>() {
        Some(api) => api_error_json(api),
        None => error_value(&format!("{err:#}"), None, None, None),
    }
}

/// [`error_json`] for an API error the command reports without failing (a poll that `jobs watch`
/// retries).
pub fn api_error_json(api: &ApiError) -> Value {
    error_value(
        &api.message,
        api.code.as_deref(),
        api.status_text.is_some().then_some(api.status),
        shown_request_id(api),
    )
}

/// [`error_json`] for an error that has only a message: one the CLI raised, or a usage error.
pub fn message_error_json(message: &str) -> Value {
    error_value(message, None, None, None)
}

fn error_value(message: &str, code: Option<&str>, status: Option<u16>, request_id: Option<&str>) -> Value {
    serde_json::json!({
        "error": { "message": strip_ansi(message), "code": code, "status": status, "requestId": request_id },
    })
}

/// `message` without terminal styling: a hint styled for a terminal stays plain in JSON.
fn strip_ansi(message: &str) -> String {
    let mut plain = String::with_capacity(message.len());
    let mut chars = message.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameters and intermediates, then the final byte (`m` for colours).
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

/// One line on stderr, so a script reads the last line of stderr as the error.
pub fn print_error_json(error: &Value) {
    eprintln!("{}", serde_json::to_string(error).expect("JSON values always serialize"));
}

/// How a command's error reaches stderr: `Error: …` (and the request ID), or with `--json`
/// [`error_json`]. The exit code is the caller's, as before.
pub fn report_error(err: &anyhow::Error) {
    if JSON_ERRORS.load(Ordering::Relaxed) {
        print_error_json(&error_json(err));
    } else {
        print_error(&format_error(err));
    }
}

/// Usage-style error with examples (the TS `showArgError`), exit 1. With `--json` it is
/// [`message_error_json`], without the examples.
pub fn arg_error(message: &str, examples: &[&str]) -> ! {
    if JSON_ERRORS.load(Ordering::Relaxed) {
        print_error_json(&message_error_json(message));
        std::process::exit(1);
    }
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
/// Leaving a menu ([`choose_command`]) or a list or question for a missing value ([`choose_value`],
/// [`ask_text`]) ends the same way.
pub fn quit_quietly() -> ! {
    std::process::exit(0)
}

/// Whether `CI` or `VENDO_NO_INPUT` turns every prompt off, also at a terminal (VE-3826,
/// decided by Yalcin 2026-10-06): CI runners can give a job a terminal, where a question or a
/// menu would wait for no one. There is no `--no-input` flag. Every prompt asks by it: the
/// questions, the group menu and the profile picker through [`can_prompt`], and login's
/// "Press ENTER to open in the browser" read at a terminal (a stdin that is no terminal it reads
/// as before).
pub fn prompts_off() -> bool {
    turns_prompts_off(std::env::var_os("CI").as_deref())
        || turns_prompts_off(std::env::var_os("VENDO_NO_INPUT").as_deref())
}

/// A value of `CI` or `VENDO_NO_INPUT` that turns prompts off: any but empty, `0` and `false`
/// (in any case).
fn turns_prompts_off(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false"))
}

/// Whether someone can answer a question: it is shown on stdout and read from
/// stdin, so both must be terminals, and [`prompts_off`] must not hold. Never in
/// unit tests, for the same reason as [`stdout_is_tty`]. [`confirm`] asks by it
/// (VE-3823), the profile picker ([`search_select_option`]) too, and the menu of
/// a group run without its command by it and more ([`can_show_menu`], VE-3826).
pub fn can_prompt() -> bool {
    !cfg!(test) && !prompts_off() && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Whether a group run without its command opens its menu ([`choose_command`]):
/// someone can answer ([`can_prompt`]), and stderr, where inquire draws the
/// menu, is a terminal that can draw it. With stderr redirected (`vendo apps
/// 2>err.log`) the menu would wait for keys with nothing on the screen, and
/// `TERM=dumb` says the terminal moves no cursor (decided by Yalcin 2026-10-06),
/// so the usage error stays. The questions still ask on `TERM=dumb`. A stdin that is a terminal
/// opened for writing only ([`stdin_reads`]) gives no keys either.
#[cfg(feature = "menu")]
pub fn can_show_menu() -> bool {
    can_prompt()
        && stdin_reads()
        && std::io::stderr().is_terminal()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

/// Whether stdin was opened for reading. A terminal opened write-only (`vendo apps
/// 0>/dev/ttys004`) is a terminal all the same, but reading it fails at once, every time (EBADF):
/// crossterm's read loop spun on that from the first key on (2026-10-06). The y/N questions and the
/// profile picker read it once and end as for Ctrl-D.
#[cfg(feature = "menu")]
fn stdin_reads() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: F_GETFL only reads stdin's status flags.
        let flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL) };
        flags >= 0 && flags & libc::O_ACCMODE != libc::O_WRONLY
    }
    #[cfg(not(unix))]
    true
}

/// What happens before a delete, cancel or reset (VE-3823).
#[derive(Debug, PartialEq)]
enum Consent {
    /// `--yes`.
    Given,
    /// Ask the person at the terminal.
    Ask,
    /// No `--yes` and no one to ask: stop.
    Refused,
}

fn consent(yes: bool, interactive: bool) -> Consent {
    match (yes, interactive) {
        (true, _) => Consent::Given,
        (false, true) => Consent::Ask,
        (false, false) => Consent::Refused,
    }
}

/// An answer to `(y/N)` that confirms: `answer.trim().toLowerCase() === 'y'`.
fn accepts(answer: &str) -> bool {
    answer.trim().eq_ignore_ascii_case("y")
}

/// Confirm a delete, cancel or reset; `Ok(false)` when the person says no.
/// `--yes` goes ahead. At a terminal it asks `question (y/N)` as the TS CLI
/// did; Ctrl-C or Ctrl-D there ends the command quietly. Without a terminal
/// (a script, a pipe, an agent) or with [`prompts_off`], and without `--yes`, it fails with
/// "`what` Re-run with --yes to confirm." before anything is changed, where
/// the TS CLI went ahead (decided by Yalcin, 2026-10-05, VE-3823).
pub fn confirm(yes: bool, question: &str, what: &str) -> anyhow::Result<bool> {
    match consent(yes, can_prompt()) {
        Consent::Given => Ok(true),
        Consent::Ask => match prompt(&format!("{question} {} ", dim("(y/N)"))) {
            Answer::Line(answer) => Ok(accepts(&answer)),
            Answer::Closed => quit_quietly(),
        },
        Consent::Refused => Err(anyhow::anyhow!("{what} Re-run with --yes to confirm.")),
    }
}

/// A row of [`choose_command`]'s menu.
#[cfg(feature = "menu")]
pub struct MenuCommand {
    pub name: String,
    pub about: String,
}

/// A row as the menu shows it: names padded to one width, as the help's `Commands:` list does.
#[cfg(feature = "menu")]
struct MenuRow<'a> {
    command: &'a MenuCommand,
    width: usize,
}

#[cfg(feature = "menu")]
impl std::fmt::Display for MenuRow<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let MenuCommand { name, about } = self.command;
        write!(f, "{name:<width$}  {about}", width = self.width)
    }
}

/// The arrow-key menu of a group run without its command (VE-3826): `title`, then every command
/// with its description; ↑↓ move, typing filters by name and description, Enter chooses, and
/// the chosen name replaces the menu after the title. Esc, Ctrl-C and Ctrl-D leave quietly
/// ([`quit_quietly`]), like Ctrl-C at a question, the menu replaced by the title and
/// `<canceled>`, and so does a terminal that hangs up while the menu is open ([`HangUpWatch`]).
/// On a screen too short for every command the list scrolls ([`menu_page`]).
/// `None` when the menu cannot run (no commands, the terminal refused it). Only where
/// [`can_show_menu`]; inquire reads the keys from stdin and draws on stderr.
#[cfg(feature = "menu")]
pub fn choose_command(title: &str, commands: &[MenuCommand]) -> Option<usize> {
    use inquire::Select;
    let width = commands.iter().map(|command| command.name.chars().count()).max()?;
    let rows: Vec<MenuRow> = commands.iter().map(|command| MenuRow { command, width }).collect();
    let shown: Vec<String> = rows.iter().map(MenuRow::to_string).collect();
    let (page, height) = menu_page(title, &shown, Select::<MenuRow>::DEFAULT_HELP_MESSAGE, screen_size());
    let menu = Select::new(title, rows).with_page_size(page).with_formatter(&|row| row.value.command.name.clone());
    run_prompt(title, height, || menu.raw_prompt()).map(|chosen| chosen.index)
}

/// A row of [`choose_value`]'s list: what the list shows (typing filters by it), and what the
/// answered line shows after the title once it is chosen.
#[cfg(feature = "menu")]
pub struct ValueRow {
    pub shown: String,
    pub answer: String,
}

#[cfg(feature = "menu")]
impl std::fmt::Display for ValueRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.shown)
    }
}

/// The arrow-key list a command missing a value opens (VE-3881), drawn and left as the group menu
/// is ([`choose_command`]): `title` (the command, `vendo apps get`), then `rows`; ↑↓ move, typing
/// filters (by substring, in any case), Enter chooses, and the chosen row's answer replaces the
/// list after the title. `note` follows the menu's hint (`… type to filter · newest 500 shown`).
/// Keys typed before the list opened, while it loaded, are thrown away ([`discard_typed_ahead`]):
/// an Enter among them would choose the first row unseen. Esc, Ctrl-C, Ctrl-D and a terminal that
/// hangs up leave quietly, as at the menu. `None` when the list cannot run (no rows, the terminal
/// refused it). Only where [`can_show_menu`].
#[cfg(feature = "menu")]
pub fn choose_value(title: &str, rows: &[ValueRow], note: Option<&str>) -> Option<usize> {
    use inquire::Select;
    let hint = Select::<&ValueRow>::DEFAULT_HELP_MESSAGE?;
    let hint = match note {
        Some(note) => format!("{hint} · {note}"),
        None => hint.to_string(),
    };
    let shown: Vec<&str> = rows.iter().map(|row| row.shown.as_str()).collect();
    let (page, height) = menu_page(title, &shown, Some(&hint), screen_size());
    let list = Select::new(title, rows.iter().collect())
        .with_page_size(page)
        .with_help_message(&hint)
        .with_formatter(&|row| row.value.answer.clone());
    run_prompt(title, height, || {
        discard_typed_ahead();
        list.raw_prompt()
    })
    .map(|chosen| chosen.index)
}

/// The one-line question a command missing a value with no list asks (VE-3881): `title` (the
/// command and the option, `vendo apps create --name`), then what is typed; Enter answers, and an
/// empty answer is refused with inquire's "A response is required." line above the question. Keys
/// typed before it opened are thrown away ([`discard_typed_ahead`]). Esc, Ctrl-C, Ctrl-D and a
/// terminal that hangs up leave quietly, as at the group menu. `None` when the question cannot run.
/// Only where [`can_show_menu`].
#[cfg(feature = "menu")]
pub fn ask_text(title: &str) -> Option<String> {
    let question = inquire::Text::new(title).with_validator(inquire::validator::ValueRequiredValidator::default());
    // The question, and the refusal above it.
    run_prompt(title, 2, || {
        discard_typed_ahead();
        question.prompt()
    })
}

/// Throws away the keys typed before a list or question for a missing value opened (VE-3881): those
/// the terminal holds unread (`tcflush`), typed while the list loaded, and those crossterm read
/// already and keeps for its next read, typed with the Enter that answered the menu or the list
/// before. Until inquire turns raw mode on, the terminal holds an Enter typed twice, or pressed
/// again while `Fetching apps...` showed, as a line; inquire then read it as Enter and chose the
/// first row, unseen, and `apps pause` or `apps delete --yes` acted on it. A y/N question takes
/// such an Enter as its default, No. Called once the prompt's [`HangUpWatch`] runs: on a terminal
/// that hung up, crossterm's read would spin.
#[cfg(feature = "menu")]
fn discard_typed_ahead() {
    #[cfg(unix)]
    // SAFETY: tcflush only discards what stdin's terminal received and nothing read yet.
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
    while crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
        if crossterm::event::read().is_err() {
            break;
        }
    }
}

/// Runs an inquire prompt titled `title` that takes up to `height` lines, as the group menu runs
/// (VE-3826): room below the cursor for it and the line after it, so that drawing it never scrolls
/// the screen, its first line saved (DECSC) for [`clear_menu`], and [`HangUpWatch`] while it is
/// open. Esc and Ctrl-D ([`quit_quietly`]) leave the title and `<canceled>`, Ctrl-C too (inquire
/// leaves the prompt standing for it, so [`clear_menu`] redraws it), and so does a terminal that
/// hangs up or a prompt left in the background for good. `None` when inquire could not run it.
#[cfg(feature = "menu")]
fn run_prompt<T>(title: &str, height: usize, prompt: impl FnOnce() -> inquire::error::InquireResult<T>) -> Option<T> {
    use crossterm::{cursor, queue, style::Print};
    use inquire::InquireError;
    let below = u16::try_from(height).unwrap_or(u16::MAX);
    let mut stderr = std::io::stderr();
    let _ = queue!(stderr, Print("\n".repeat(below.into())), cursor::MoveUp(below), cursor::SavePosition)
        .and_then(|()| stderr.flush());
    let watch = HangUpWatch::start();
    let answer = prompt();
    // The prompt has closed: a hang-up from here on is the command's to meet.
    drop(watch);
    match answer {
        Ok(answer) => Some(answer),
        Err(InquireError::OperationCanceled) => quit_quietly(),
        Err(InquireError::OperationInterrupted) => {
            let _ = clear_menu(&mut stderr, title, inquire::ui::RenderConfig::default());
            quit_quietly()
        }
        // inquire could not draw on a terminal that hung up (a key read before the hang-up
        // redraws the prompt), or from the background: as when the watch sees it first.
        Err(_) if menu_lost() => quit_quietly(),
        Err(_) => None,
    }
}

/// How often [`HangUpWatch`] asks again whether the terminal hung up, at most: on macOS a `poll`
/// that began while a key waited to be read does not wake for a hang-up.
#[cfg(all(feature = "menu", unix))]
const HANG_UP_CHECK: Duration = Duration::from_millis(250);

/// The terminals a prompt ([`run_prompt`]) uses: stdin, where its keys come from, and stderr, where
/// inquire draws it. The same terminal, unless a program gave `vendo` two.
#[cfg(all(feature = "menu", unix))]
const MENU_TERMINALS: [libc::c_int; 2] = [libc::STDIN_FILENO, libc::STDERR_FILENO];

/// While a prompt is open ([`run_prompt`]: the group menu, or a list or question for a missing value),
/// a thread that ends the CLI as Ctrl-D does ([`quit_quietly`]: exit 0, nothing run) once the menu
/// can no longer read its keys: a terminal it
/// uses hung up ([`MENU_TERMINALS`]; stdin's settings are put back first, as far as its terminal
/// still takes them), or the menu is in the background of its terminal for good
/// ([`reads_fail_in_background`]; that terminal is another job's now and is left as it is).
///
/// A terminal hangs up when its window closes or the program that opened the pseudo-terminal
/// drops it. SIGHUP then ends a process whose controlling terminal it is, as at a shell; nothing
/// ends one that only has it as stdin (a terminal opened with `O_NOCTTY`, or one whose parent
/// died). There crossterm, which reads the keys for inquire, spun at a whole core for hours
/// (2026-10-06): reading a terminal that hung up returns at once, every time, with the end of the
/// input (or an I/O error while Linux is still hanging it up), and crossterm's read loop
/// (`UnixInternalEventSource::try_read`) keeps going after either, as it only stops for a key or
/// `WouldBlock`. Reading from the background fails at once, every time, too. inquire loops on
/// crossterm's `event::read` and has no public way to read the keys otherwise, so the watch runs
/// beside them.
///
/// It asks `poll` for POLLHUP alone: reported once the terminal hangs up (with POLLERR on Linux),
/// it reads nothing, so it takes no key from the menu, and keys waiting to be read do not wake it.
/// macOS reports POLLHUP only when asked for it (not when `events` is 0) and does not wake a `poll`
/// that began while a key waited to be read; a new one sees the hang-up at once, so it asks again
/// every [`HANG_UP_CHECK`], and looks at the background each time. On Linux the hang-up wakes any
/// `poll` on the terminal.
#[cfg(all(feature = "menu", unix))]
struct HangUpWatch {
    /// Whether the menu is open. The watch ends the CLI only while it is and holding the lock, so a
    /// command chosen in the menu never runs as the CLI exits.
    open: std::sync::Arc<std::sync::Mutex<bool>>,
}

#[cfg(all(feature = "menu", unix))]
impl HangUpWatch {
    fn start() -> Self {
        let open = std::sync::Arc::new(std::sync::Mutex::new(true));
        let watched = std::sync::Arc::clone(&open);
        // Before inquire turns raw mode on.
        let settings = terminal_settings(libc::STDIN_FILENO);
        let watch = move || {
            let hung_up = loop {
                match hung_up(&MENU_TERMINALS, HANG_UP_CHECK) {
                    Some(true) => break true,
                    // poll cannot watch these terminals.
                    None => return,
                    Some(false) => {}
                }
                if !*Self::lock(&watched) {
                    // The menu closed.
                    return;
                }
                if reads_fail_in_background() {
                    break false;
                }
            };
            let open = Self::lock(&watched);
            if *open {
                if hung_up {
                    restore_terminal(settings.as_ref());
                }
                quit_quietly();
            }
        };
        // Without the thread the menu runs as before.
        let _ = std::thread::Builder::new().name("menu hang-up watch".into()).spawn(watch);
        HangUpWatch { open }
    }

    fn lock(open: &std::sync::Mutex<bool>) -> std::sync::MutexGuard<'_, bool> {
        open.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(all(feature = "menu", unix))]
impl Drop for HangUpWatch {
    /// The menu has closed; the thread ends at its next check.
    fn drop(&mut self) {
        *Self::lock(&self.open) = false;
    }
}

/// Whether the menu can no longer read its keys, as [`HangUpWatch`] tells it, now.
#[cfg(all(feature = "menu", unix))]
fn menu_lost() -> bool {
    hung_up(&MENU_TERMINALS, Duration::ZERO) == Some(true) || reads_fail_in_background()
}

/// Whether one of the terminals `fds` has hung up, waiting up to `timeout` for it ([`HangUpWatch`]
/// says how). `Some(false)` when none has by then, or a signal cut the wait short; `None` when
/// `poll` cannot watch one (POLLNVAL) or fails.
#[cfg(all(feature = "menu", unix))]
fn hung_up(fds: &[libc::c_int], timeout: Duration) -> Option<bool> {
    let mut polled: Vec<libc::pollfd> =
        fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLHUP, revents: 0 }).collect();
    let timeout = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: poll on the pollfds in `polled`, which outlives the call.
    if unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, timeout) } < 0 {
        return (std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted).then_some(false);
    }
    if polled.iter().any(|fd| fd.revents & libc::POLLNVAL != 0) {
        return None;
    }
    Some(polled.iter().any(|fd| fd.revents & (libc::POLLHUP | libc::POLLERR) != 0))
}

/// Whether the menu is in the background of its controlling terminal (stdin) for good, where
/// reading its keys fails at once, every time (EIO): its process group is not the terminal's
/// foreground group, and the group is orphaned (no member's parent is in another group of its
/// session, such as a shell that could bring it back with `fg`) or SIGTTIN is ignored or blocked.
/// Elsewhere in the background a read stops the process (SIGTTIN) until job control brings it back,
/// and the menu goes on.
///
/// It happens when a program that ran `vendo <group>` from a shell exits while the menu is open:
/// the shell takes its terminal back, and each key typed at the shell then made crossterm's read
/// fail, spinning at a whole core until the window closed (2026-10-06). Only the kernel's refusal
/// tells an orphaned group: `tcdrain`, which reads nothing and changes nothing, makes the check a
/// read makes, with SIGTTOU in place of SIGTTIN, and fails with EIO for an orphaned group (macOS
/// and Linux alike); a group that is not orphaned it stops with SIGTTOU, as the read would, and the
/// call returns once job control brings it back. With SIGTTOU ignored or blocked `tcdrain` cannot
/// tell, so the menu ends then too. Never where stdin is not the controlling terminal (`tcgetpgrp`
/// fails), as for a terminal opened with `O_NOCTTY`, nor where the terminal has no foreground group
/// (0 on Linux, which then lets the read through).
#[cfg(all(feature = "menu", unix))]
fn reads_fail_in_background() -> bool {
    // SAFETY: tcgetpgrp, getpgrp and tcdrain on stdin.
    let (foreground, own) = unsafe { (libc::tcgetpgrp(libc::STDIN_FILENO), libc::getpgrp()) };
    if foreground <= 0 || foreground == own {
        return false;
    }
    if [libc::SIGTTIN, libc::SIGTTOU].into_iter().any(held_off) {
        return true;
    }
    // SAFETY: as above.
    let drained = unsafe { libc::tcdrain(libc::STDIN_FILENO) };
    drained < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EIO)
}

/// Whether `signal` is ignored, or blocked in this thread: [`HangUpWatch`]'s has the signal mask of
/// the thread that started it, where inquire reads the keys.
#[cfg(all(feature = "menu", unix))]
fn held_off(signal: libc::c_int) -> bool {
    // SAFETY: sigaction and pthread_sigmask only write the signal's action and this thread's mask
    // into values that outlive the calls, and change neither; sigismember reads the mask.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        let mut mask: libc::sigset_t = std::mem::zeroed();
        (libc::sigaction(signal, std::ptr::null(), &mut action) == 0 && action.sa_sigaction == libc::SIG_IGN)
            || (libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut mask) == 0
                && libc::sigismember(&mask, signal) == 1)
    }
}

/// `fd`'s terminal settings, `None` when it has none.
#[cfg(all(feature = "menu", unix))]
fn terminal_settings(fd: libc::c_int) -> Option<libc::termios> {
    // SAFETY: tcgetattr fully writes `settings` before it is read, and only when it returns 0.
    unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        (libc::tcgetattr(fd, &mut settings) == 0).then_some(settings)
    }
}

/// What inquire's terminal undoes as the menu ends, done for a terminal that hung up as far as it
/// still takes it (macOS keeps its settings; Linux refuses everything): the settings from before
/// raw mode, put back on stdin itself (crossterm's `disable_raw_mode` writes to `/dev/tty` when
/// stdin no longer counts as a terminal, which on Linux it does not once it hung up), and bracketed
/// paste off, as crossterm's `DisableBracketedPaste` writes it. The cursor shows already while the
/// menu waits for a key.
#[cfg(all(feature = "menu", unix))]
fn restore_terminal(settings: Option<&libc::termios>) {
    if let Some(settings) = settings {
        // SAFETY: puts back settings tcgetattr read from the same descriptor.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, settings);
        }
    }
    let _ = std::io::stderr().write_all(b"\x1b[?2004l");
}

/// Elsewhere than on Unix the menu has no hang-up watch.
#[cfg(all(feature = "menu", not(unix)))]
struct HangUpWatch;

#[cfg(all(feature = "menu", not(unix)))]
impl HangUpWatch {
    fn start() -> Self {
        HangUpWatch
    }
}

/// Elsewhere than on Unix, whether the menu's terminal is gone cannot be told.
#[cfg(all(feature = "menu", not(unix)))]
fn menu_lost() -> bool {
    false
}

/// How many commands the menu lists at once, and the most lines it then takes, on a screen of
/// `columns` × `lines`; `rows` are the commands as the menu shows them. inquire wraps a
/// line wider than the screen but does not fit the menu to the screen's height, and a menu
/// taller than the screen drew over itself. So the list holds as many commands as fit between
/// the title and the hint, counting the ones that take the most lines ([`lines_taken`]), and
/// scrolls (`^`, `v`); at least one. The screen's last line stays free for the line after the
/// menu: Enter or Esc there scrolled the answer off a full screen. The lists for a missing value
/// (VE-3881) fit the same way.
#[cfg(feature = "menu")]
fn menu_page(
    title: &str,
    rows: &[impl AsRef<str>],
    hint: Option<&str>,
    (columns, lines): (usize, usize),
) -> (usize, usize) {
    let height = |text: &str| lines_taken(text, columns);
    let lines = lines.saturating_sub(1);
    // `? <title> ` and the cursor's space; the hint in brackets; a row after `> ` (or `^ `, `v `).
    let mut used = height(&format!("? {title}  ")) + hint.map_or(0, |hint| height(&format!("[{hint}]")));
    let mut tallest: Vec<usize> = rows.iter().map(|row| height(&format!("> {}", row.as_ref()))).collect();
    tallest.sort_unstable_by(|a, b| b.cmp(a));
    let mut page = 0;
    for row in tallest {
        if page > 0 && used + row > lines {
            break;
        }
        used += row;
        page += 1;
    }
    (page, used)
}

/// How many lines `text` takes on a screen `columns` wide, wrapped as inquire wraps a line: by the
/// columns each character takes (`unicode-width`: two for a wide one such as 東, as the terminal
/// draws it), a character that does not fit on the line going to the next. Counted one column a
/// character, the rows of an app named in Japanese took more lines than counted, and the list ran
/// off the top of a short screen, the title and the row marked `>` with it (VE-3881).
#[cfg(feature = "menu")]
fn lines_taken(text: &str, columns: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let (mut lines, mut used) = (1, 0);
    for c in text.chars() {
        let width = c.width().unwrap_or(0);
        if used > 0 && width > columns.saturating_sub(used) {
            lines += 1;
            used = 0;
        }
        used += width;
    }
    lines
}

/// The terminal's columns and lines as inquire reads them, and 80 × 24 when they are unknown,
/// as inquire assumes.
#[cfg(feature = "menu")]
fn screen_size() -> (usize, usize) {
    match crossterm::terminal::size() {
        Ok((columns @ 1.., lines @ 1..)) => (columns.into(), lines.into()),
        _ => (80, 24),
    }
}

/// Ctrl-C leaves the screen as Esc and Ctrl-D do. For those two inquire redraws the menu as its
/// title and `<canceled>`; for Ctrl-C it returns with the whole menu standing, and at the bottom
/// of the screen with the cursor on the hint, which the shell's next prompt overwrote. So: back to
/// the menu's first line, saved by [`run_prompt`], clear from there down, and write inquire's
/// line for Esc.
#[cfg(feature = "menu")]
fn clear_menu(out: &mut impl Write, title: &str, config: inquire::ui::RenderConfig) -> std::io::Result<()> {
    use crossterm::{
        cursor, queue,
        terminal::{Clear, ClearType},
    };
    queue!(out, cursor::RestorePosition, Clear(ClearType::FromCursorDown))?;
    write_styled(out, config.prompt_prefix)?;
    write!(out, " ")?;
    write_styled(out, inquire::ui::Styled::new(title).with_style_sheet(config.prompt))?;
    write!(out, " ")?;
    write_styled(out, config.canceled_prompt_indicator)?;
    writeln!(out)?;
    out.flush()
}

/// `styled` as inquire's crossterm terminal writes it.
#[cfg(feature = "menu")]
fn write_styled(out: &mut impl Write, styled: inquire::ui::Styled<&str>) -> std::io::Result<()> {
    use crossterm::{
        queue,
        style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    };
    use inquire::ui::Attributes;
    let inquire::ui::Styled { content, style } = styled;
    if let Some(color) = style.fg {
        queue!(out, SetForegroundColor(color.into()))?;
    }
    if let Some(color) = style.bg {
        queue!(out, SetBackgroundColor(color.into()))?;
    }
    if style.att.contains(Attributes::BOLD) {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }
    if style.att.contains(Attributes::ITALIC) {
        queue!(out, SetAttribute(Attribute::Italic))?;
    }
    queue!(out, Print(content))?;
    if style.fg.is_some() {
        queue!(out, SetForegroundColor(Color::Reset))?;
    }
    if style.bg.is_some() {
        queue!(out, SetBackgroundColor(Color::Reset))?;
    }
    if !style.att.is_empty() {
        queue!(out, SetAttribute(Attribute::Reset))?;
    }
    Ok(())
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

/// Search-then-pick prompt (the TS `searchSelectOption`). `None` when no one can
/// answer ([`can_prompt`]: the TS CLI asked whenever stdout was a terminal, so
/// `echo 1 | vendo profile switch` read its answers from the pipe; decided by
/// Yalcin 2026-10-06, VE-3826), there are no options, or the user types `q`;
/// Ctrl-C and Ctrl-D end the command quietly.
pub fn search_select_option(message: &str, options: &[SelectOption]) -> Option<String> {
    if !can_prompt() || options.is_empty() {
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

    fn api_error(status: u16, code: Option<&str>, server_id: Option<&str>, answered: bool) -> anyhow::Error {
        anyhow::Error::new(ApiError {
            message: "Job not found".into(),
            status,
            code: code.map(Into::into),
            request_id: answered.then(|| "cli-123".into()),
            server_request_id: server_id.map(Into::into),
            details: Some(json!({ "ignored": true })),
            status_text: answered.then(|| "Not Found".into()),
        })
    }

    #[test]
    fn json_errors_have_one_shape_with_null_where_nothing_applies() {
        // VE-3831: the fields the text error shows, plus the API's code and HTTP status.
        let shape = |message: &str, code: Value, status: Value, request_id: Value| json!({ "error": { "message": message, "code": code, "status": status, "requestId": request_id } });
        assert_eq!(
            error_json(&api_error(404, Some("NOT_FOUND"), Some("req_456"), true)),
            shape("Job not found", json!("NOT_FOUND"), json!(404), json!("req_456"))
        );
        // The request ID the text shows: the CLI's own when the server sent none (or an empty one).
        for server_id in [None, Some("")] {
            assert_eq!(
                error_json(&api_error(500, None, server_id, true)),
                shape("Job not found", Value::Null, json!(500), json!("cli-123"))
            );
        }
        // No response (a timeout is 408 inside the client, a network failure 0): no status, no request ID.
        for status in [408, 0] {
            assert_eq!(
                error_json(&api_error(status, None, None, false)),
                shape("Job not found", Value::Null, Value::Null, Value::Null)
            );
        }
        let local = anyhow::anyhow!("No API key configured.");
        assert_eq!(error_json(&local), shape("No API key configured.", Value::Null, Value::Null, Value::Null));
        assert_eq!(message_error_json("x"), shape("x", Value::Null, Value::Null, Value::Null));
        // Keys in this order, on one line.
        let line = serde_json::to_string(&error_json(&api_error(404, Some("NOT_FOUND"), None, true))).unwrap();
        assert_eq!(
            line,
            r#"{"error":{"message":"Job not found","code":"NOT_FOUND","status":404,"requestId":"cli-123"}}"#
        );
    }

    #[test]
    fn json_error_messages_carry_no_terminal_styling() {
        let styled = "No profile found.\n\u{1b}[2m  Run `vendo profile list`.\u{1b}[0m";
        assert_eq!(message_error_json(styled)["error"]["message"], "No profile found.\n  Run `vendo profile list`.");
        assert_eq!(strip_ansi("plain [brackets] stay"), "plain [brackets] stay");
        assert_eq!(strip_ansi("\u{1b}[31mError:\u{1b}[39m é"), "Error: é");
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
    fn yes_goes_ahead_a_terminal_asks_and_otherwise_the_command_stops() {
        // VE-3823: the TS CLI went ahead whenever stdout was not a terminal.
        assert_eq!(consent(true, true), Consent::Given);
        assert_eq!(consent(true, false), Consent::Given);
        assert_eq!(consent(false, true), Consent::Ask);
        assert_eq!(consent(false, false), Consent::Refused);
    }

    #[cfg(feature = "menu")]
    #[test]
    fn the_menu_lists_as_many_commands_as_the_screen_holds() {
        let hint = Some("↑↓ to move, enter to select, type to filter");
        // Rows as wide as `vendo apps`' rows, and as `vendo destinations`', whose refresh-source row
        // takes two lines on 80 columns.
        let rows = |widths: &[usize]| widths.iter().map(|width| "x".repeat(*width)).collect::<Vec<_>>();
        let apps = rows(&[23, 39, 25, 22, 29, 37, 26, 23]);
        let destinations = rows(&[37, 39, 37, 105, 35, 43, 50, 74, 36]);
        // Every command with the title and the hint, if they fit with a line to spare.
        assert_eq!(menu_page("vendo apps", &apps, hint, (120, 40)), (8, 10));
        assert_eq!(menu_page("vendo apps", &apps, hint, (80, 11)), (8, 10));
        assert_eq!(menu_page("vendo destinations", &destinations, hint, (80, 13)), (9, 12));
        // Else as many as fit, the rows taking the most lines counted, and the list scrolls.
        assert_eq!(menu_page("vendo apps", &apps, hint, (80, 10)), (7, 9));
        assert_eq!(menu_page("vendo apps", &apps, hint, (120, 6)), (3, 5));
        assert_eq!(menu_page("vendo destinations", &destinations, hint, (80, 12)), (8, 11));
        assert_eq!(menu_page("vendo destinations", &destinations, hint, (80, 8)), (4, 7));
        // On 40 columns the hint takes two lines and refresh-source three.
        assert_eq!(menu_page("vendo destinations", &destinations, hint, (40, 8)), (1, 6));
        // At least one, however short the screen.
        assert_eq!(menu_page("vendo apps", &apps, hint, (80, 2)), (1, 3));
        // A row of an app named in Japanese (VE-3881): 62 characters, but 87 columns on the screen
        // after `> `, so two lines on 80 columns, and three such rows fit on a 10-line screen, not seven.
        let wide = format!("b0000000...  {}東京N  shopify  source  active", "東京ストア本店".repeat(3));
        assert_eq!((wide.chars().count(), lines_taken(&format!("> {wide}"), 80)), (62, 2));
        assert_eq!(menu_page("vendo apps get", &vec![wide; 8], hint, (80, 10)), (3, 8));
    }

    #[cfg(feature = "menu")]
    #[test]
    fn a_line_takes_the_columns_its_characters_take_as_inquire_wraps_it() {
        assert_eq!(lines_taken("", 80), 1);
        assert_eq!(lines_taken(&"x".repeat(80), 80), 1);
        assert_eq!(lines_taken(&"x".repeat(81), 80), 2);
        // A wide character takes two columns.
        assert_eq!(lines_taken(&"東".repeat(40), 80), 1);
        assert_eq!(lines_taken(&"東".repeat(41), 80), 2);
        // One that does not fit in the last column goes to the next line: 160 columns, three lines.
        assert_eq!(lines_taken(&format!("x{}x", "東".repeat(79)), 80), 3);
    }

    #[cfg(feature = "menu")]
    #[test]
    fn the_canceled_line_is_styled_as_inquire_styles_it() {
        use inquire::ui::{Attributes, Color, Styled};
        let write = |styled| {
            let mut out = Vec::new();
            write_styled(&mut out, styled).unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!(write(Styled::new("?").with_fg(Color::LightGreen)), "\x1b[38;5;10m?\x1b[39m");
        assert_eq!(write(Styled::new("<canceled>").with_fg(Color::DarkRed)), "\x1b[38;5;1m<canceled>\x1b[39m");
        assert_eq!(write(Styled::new("vendo apps")), "vendo apps");
        assert_eq!(write(Styled::new("b").with_attr(Attributes::BOLD)), "\x1b[1mb\x1b[0m");
    }

    #[cfg(feature = "menu")]
    #[test]
    fn ctrl_c_redraws_the_menu_as_esc_leaves_it() {
        // Back to the menu's first line (DECRC), clear from there down, then the title and `<canceled>`.
        let mut out = Vec::new();
        clear_menu(&mut out, "vendo apps", inquire::ui::RenderConfig::empty()).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b8\x1b[J? vendo apps <canceled>\n");
        let mut out = Vec::new();
        clear_menu(&mut out, "vendo apps", inquire::ui::RenderConfig::default_colored()).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b8\x1b[J\x1b[38;5;10m?\x1b[39m vendo apps \x1b[38;5;1m<canceled>\x1b[39m\n"
        );
    }

    #[cfg(all(feature = "menu", unix))]
    #[test]
    fn a_terminal_hangs_up_when_its_controller_closes_and_a_waiting_key_is_no_hang_up() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let pseudo_terminal = || {
            // SAFETY: posix_openpt, grantpt, unlockpt, ptsname and open on descriptors this test owns.
            unsafe {
                let controller = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
                assert!(controller >= 0, "posix_openpt failed");
                assert_eq!((libc::grantpt(controller), libc::unlockpt(controller)), (0, 0));
                let terminal = libc::open(libc::ptsname(controller), libc::O_RDWR | libc::O_NOCTTY);
                assert!(terminal >= 0, "open failed");
                (OwnedFd::from_raw_fd(controller), OwnedFd::from_raw_fd(terminal))
            }
        };
        let (controller, terminal) = pseudo_terminal();
        let watched = [terminal.as_raw_fd()];
        assert_eq!(hung_up(&watched, Duration::ZERO), Some(false));
        // A key waiting to be read is no hang-up, and does not end the wait: the watch would spin
        // until the menu read it. In raw mode, as the menu has it, so that the key can be read
        // (POLLIN) at once: in the line mode a new terminal starts in, it waits for a line end.
        let mut raw = terminal_settings(watched[0]).expect("a terminal has settings");
        // SAFETY: cfmakeraw and tcsetattr on settings tcgetattr read from the same descriptor.
        unsafe {
            libc::cfmakeraw(&mut raw);
            assert_eq!(libc::tcsetattr(watched[0], libc::TCSANOW, &raw), 0);
        }
        std::fs::File::from(controller.try_clone().unwrap()).write_all(b"k").unwrap();
        let mut key = libc::pollfd { fd: watched[0], events: libc::POLLIN, revents: 0 };
        // SAFETY: poll on one pollfd, which outlives the call.
        assert_eq!(unsafe { libc::poll(&mut key, 1, 1000) }, 1, "the key can be read");
        let asked = std::time::Instant::now();
        assert_eq!(hung_up(&watched, Duration::from_millis(200)), Some(false));
        assert!(asked.elapsed() >= Duration::from_millis(150), "returned after {:?}", asked.elapsed());
        // The controller closes: the key still waits, and the terminal has hung up, also when it is
        // watched with another that is still up (the menu's keys and its screen on two terminals).
        let (_up, other) = pseudo_terminal();
        assert_eq!(hung_up(&[other.as_raw_fd(), watched[0]], Duration::ZERO), Some(false));
        drop(controller);
        assert_eq!(hung_up(&watched, Duration::ZERO), Some(true));
        assert_eq!(hung_up(&[other.as_raw_fd(), watched[0]], Duration::ZERO), Some(true));
        assert_eq!(hung_up(&[other.as_raw_fd()], Duration::ZERO), Some(false));
    }

    #[cfg(feature = "menu")]
    #[test]
    fn menu_rows_line_up_like_the_help_commands_list() {
        let commands = [("list", "List all apps"), ("diagnose", "Show apps that need attention")]
            .map(|(name, about)| MenuCommand { name: name.to_string(), about: about.to_string() });
        let rows: Vec<String> = commands.iter().map(|command| MenuRow { command, width: 8 }.to_string()).collect();
        assert_eq!(rows, ["list      List all apps", "diagnose  Show apps that need attention"]);
    }

    #[test]
    fn ci_and_vendo_no_input_turn_prompts_off_unless_unset_empty_0_or_false() {
        // VE-3826 (Yalcin, 2026-10-06): one rule for both settings.
        for value in ["true", "1", "TRUE", "yes", "woodpecker"] {
            assert!(turns_prompts_off(Some(std::ffi::OsStr::new(value))), "{value:?}");
        }
        for value in ["", "0", "false", "False", "FALSE"] {
            assert!(!turns_prompts_off(Some(std::ffi::OsStr::new(value))), "{value:?}");
        }
        assert!(!turns_prompts_off(None));
    }

    #[test]
    fn without_a_terminal_confirm_needs_yes() {
        // Unit tests never see a terminal, like a script or an agent.
        assert!(!can_prompt());
        assert!(confirm(true, "Delete app abc?", "This deletes app abc.").unwrap());
        let err = confirm(false, "Delete app abc?", "This deletes app abc.").unwrap_err();
        assert_eq!(format_error(&err), "This deletes app abc. Re-run with --yes to confirm.");
    }

    #[test]
    fn only_y_confirms_at_the_question() {
        // `answer.trim().toLowerCase() === 'y'`, as in TS.
        for answer in ["y\n", "Y\n", " y \n", "y\r\n"] {
            assert!(accepts(answer), "{answer:?}");
        }
        for answer in ["\n", "n\n", "N\n", "yes\n", "yy\n"] {
            assert!(!accepts(answer), "{answer:?}");
        }
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
    fn parsed_json_keys_follow_javascript_property_order() {
        // `JSON.parse` puts array-index keys first, ascending, at every level; the TS CLI printed
        // and iterated them in that order.
        let value =
            parse_json(r#"{"b":1,"10":2,"a":3,"2":{"z":1,"1":2},"02":5,"4294967295":6,"4294967294":7}"#).unwrap();
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&value), ["2", "10", "4294967294", "b", "a", "02", "4294967295"]);
        assert_eq!(keys(&value["2"]), ["1", "z"]);
        assert_eq!(
            json_pretty(&parse_json(r#"[{"y":1.0,"0":true}]"#).unwrap()),
            "[\n  {\n    \"0\": true,\n    \"y\": 1.0\n  }\n]"
        );
    }

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
        assert_eq!(js_template(Some(&json!(false))), "false");
        assert_eq!(js_template(Some(&json!(["a", null]))), "a,");
        assert_eq!(js_template(Some(&json!({ "a": 1 }))), "[object Object]");
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
        // `printCount(res.total, 'metric')` and the like: the count as a template literal prints it.
        assert_eq!(count_text(None, "metric"), "undefined metrics");
        assert_eq!(count_text(Some(&json!(null)), "metric"), "null metrics");
        assert_eq!(count_text(Some(&json!("5")), "metric"), "5 metrics");
        assert_eq!(count_text(Some(&json!(2.5)), "cohort"), "2.5 cohorts");
        assert_eq!(count_text(Some(&json!(1)), "cohort"), "1 cohort");
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

//! Terminal output helpers. Colors switch off when the stream is not a TTY
//! (and, unlike the TS CLI, also honour NO_COLOR / FORCE_COLOR).

use std::{future::Future, io::IsTerminal, time::Duration};

use comfy_table::{Attribute, Cell, Color, Table, presets};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Stream};
use serde_json::Value;

use crate::client::ApiError;

pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn bold(s: &str) -> String {
    s.if_supports_color(Stream::Stdout, |t| t.bold()).to_string()
}
pub fn dim(s: &str) -> String {
    s.if_supports_color(Stream::Stdout, |t| t.dimmed()).to_string()
}
pub fn green(s: &str) -> String {
    s.if_supports_color(Stream::Stdout, |t| t.green()).to_string()
}

pub fn status_color(status: &str) -> Option<Color> {
    match status {
        "active" | "completed" => Some(Color::Green),
        "running" | "pending" => Some(Color::Blue),
        "paused" | "inactive" | "cancelled" => Some(Color::Grey),
        "errored" | "failed" => Some(Color::Red),
        "warning" => Some(Color::Yellow),
        _ => None,
    }
}

pub fn colored_cell(text: &str, color: Option<Color>) -> Cell {
    match color {
        Some(color) => Cell::new(text).fg(color),
        None => Cell::new(text),
    }
}

/// Bordered table on a TTY; borderless two-space columns when piped.
pub fn table(headers: &[&str]) -> Table {
    let mut table = Table::new();
    if stdout_is_tty() {
        table.load_style(presets::UTF8_FULL);
        table.set_header(
            headers
                .iter()
                .map(|h| Cell::new(h).add_attribute(Attribute::Bold).fg(Color::Cyan)),
        );
    } else {
        table.load_style(presets::NOTHING);
        table.set_header(headers.to_vec());
    }
    table
}

const SPINNER_FRAMES: &[&str] = &[
    "⡀⠀⠀", "⡄⠀⠀", "⡆⠀⠀", "⡇⠀⠀", "⣇⠀⠀", "⣧⠀⠀", "⣷⠀⠀", "⣿⠀⠀", "⣿⡀⠀", "⣿⡄⠀", "⣿⡆⠀", "⣿⡇⠀", "⣿⣇⠀",
    "⣿⣧⠀", "⣿⣷⠀", "⣿⣿⠀", "⣿⣿⡀", "⣿⣿⡄", "⣿⣿⡆", "⣿⣿⡇", "⣿⣿⣇", "⣿⣿⣧", "⣿⣿⣷", "⣿⣿⣿", "⣿⣿⣿", "⠀⠀⠀",
    "",
];

/// Run a future behind a spinner (stderr, only when stdout is a TTY — same as ora).
pub async fn run_action<T, F>(label: &str, fut: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let bar = if stdout_is_tty() {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .expect("static template")
                .tick_strings(SPINNER_FRAMES),
        );
        bar.set_message(label.to_string());
        bar.enable_steady_tick(Duration::from_millis(60));
        bar
    } else {
        ProgressBar::hidden()
    };
    let result = fut.await;
    bar.finish_and_clear();
    result
}

pub fn time_ago(value: Option<&str>) -> String {
    let Some(raw) = value else { return "—".into() };
    let Ok(then) = raw.parse::<jiff::Timestamp>() else { return raw.to_string() };
    let seconds = jiff::Timestamp::now().as_second() - then.as_second();
    if seconds < 60 {
        return "just now".into();
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
    then.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%-m/%-d/%Y")
        .to_string()
}

pub fn short_id(id: &str) -> String {
    if id.chars().count() > 12 {
        format!("{}...", id.chars().take(8).collect::<String>())
    } else {
        id.to_string()
    }
}

pub fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).expect("Value always serializes"));
}

/// `--output <field>`: one line per row. Arrays of scalars join with commas
/// (as JS `String([...])` did); objects print as compact JSON instead of
/// `[object Object]`.
pub fn print_field(rows: &[Value], field: &str) {
    for row in rows {
        match row.get(field) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) => println!("{s}"),
            Some(Value::Array(items)) => println!(
                "{}",
                items
                    .iter()
                    .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Some(other) => println!("{other}"),
        }
    }
}

pub fn print_count(count: u64, label: &str) {
    let plural = if count == 1 { "" } else { "s" };
    println!("{}", dim(&format!("{count} {label}{plural}")));
}

pub fn print_success(message: &str) {
    println!("{} {message}", green("Done:"));
}

pub fn print_error(err: &anyhow::Error) {
    let red = |s: &str| s.if_supports_color(Stream::Stderr, |t| t.red()).to_string();
    let dim = |s: &str| s.if_supports_color(Stream::Stderr, |t| t.dimmed()).to_string();
    match err.downcast_ref::<ApiError>() {
        Some(api) => {
            eprintln!("{} {}", red("Error:"), api.message);
            if let Some(id) = api.server_request_id.as_ref().or(api.request_id.as_ref()) {
                eprintln!("{}", dim(&format!("Request ID: {id}")));
            }
        }
        None => eprintln!("{} {err:#}", red("Error:")),
    }
}

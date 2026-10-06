//! Asking for a value a command is missing (VE-3881, decided by Yalcin 2026-10-07, CLI 1.1).
//!
//! Where the group menu opens ([`output::can_show_menu`]: stdin, stdout and stderr terminals,
//! prompts not off, `TERM` not `dumb`), a command typed without a value it requires asks for it
//! instead of stopping with clap's usage error. A value with choices opens an arrow-key list with
//! type-to-filter ([`output::choose_value`]), loaded from the account or the catalog; free text is a
//! one-line question ([`output::ask_text`]). Optional values are not asked. The values go into the
//! words where they would have been typed and [`crate::cli::parse`] parses them again, so the
//! command runs exactly as if they had been typed: global options, the y/N of a delete (VE-3823) and
//! `--json` as usual, and an ID goes in whole, so no short-ID lookup follows (VE-3831). Esc, Ctrl-C
//! and Ctrl-D leave quietly, running nothing.
//!
//! Without a terminal or with prompts off nothing here runs, and nothing is read or sent: the usage
//! error stays, byte for byte (`rust/tests/snapshots/usage/`). A value [`VALUES`] does not name keeps
//! it at a terminal too.

// `ApiError` carries request IDs and error details for the one error a
// command reports; its size doesn't matter on that path.
#![allow(clippy::result_large_err)]

use std::ffi::OsString;

use serde_json::Value;

use crate::{
    client::{ApiError, Client, payload},
    commands::{apps::role_label, catalog},
    context::Ctx,
    jobs::Job,
    output::{self, ValueRow, short_id},
    short_ids::{self, Listing},
};

/// How a missing value is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// One of the account's apps, listed as `vendo apps list` lists them (newest first); its full ID.
    App,
    /// A platform ready to connect, listed as `vendo catalog list` lists them by default (VE-3829);
    /// its app type.
    Platform,
    /// A one-line question; what is typed.
    Text,
}

/// Every required value a command asks for: the command as the tree names it, the value's clap ID,
/// and how it is asked for. A value not here keeps the usage error.
const VALUES: [(&str, &str, Ask); 7] = [
    ("apps get", "id", Ask::App),
    ("apps pause", "id", Ask::App),
    ("apps resume", "id", Ask::App),
    ("apps delete", "id", Ask::App),
    ("apps update", "id", Ask::App),
    ("apps create", "app_type", Ask::Platform),
    ("apps create", "name", Ask::Text),
];

fn asked_by(command: &str, value: &str) -> Option<Ask> {
    VALUES.iter().find(|(c, v, _)| *c == command && *v == value).map(|(_, _, ask)| *ask)
}

/// For a command whose words (`typed`, after [`crate::cli`]'s rewrites of hidden paths) leave out
/// values it requires, at a terminal where the group menu opens: asks for each, in clap's order, and
/// returns `args` (the words as given) with them where they would have been typed. `None` keeps the
/// usage error: another error, no terminal to ask at, a value [`VALUES`] does not name (all three
/// known before anything is read, sent or printed), nothing to choose from, or a prompt that cannot
/// run. With no API key, a list of the account's and no account, or a list that cannot be read, the
/// CLI ends with the error the command would give (exit 1, the JSON error with `--json`), before
/// anything is asked.
pub async fn missing_values(
    root: &clap::Command,
    args: &[OsString],
    typed: &[OsString],
    err: &clap::Error,
    json: bool,
) -> Option<Vec<OsString>> {
    if err.kind() != clap::error::ErrorKind::MissingRequiredArgument || !output::can_show_menu() {
        return None;
    }
    let LeftOut { path, args: left_out, profile, debug } = left_out(root, typed)?;
    let command = path[1..].join(" ");
    let wanted: Vec<(&clap::Arg, Ask)> = left_out
        .into_iter()
        .map(|arg| Some((arg, asked_by(&command, arg.get_id().as_str())?)))
        .collect::<Option<_>>()?;
    let ctx = Ctx::new(profile, debug);
    let client = ready(&ctx, &wanted).unwrap_or_else(|err| fail(&err, json));
    let mut answers = Vec::new();
    for (arg, ask) in wanted {
        // `vendo apps get`, `vendo apps create --type`: the command so far.
        let title = match arg.get_long() {
            Some(long) => format!("{} --{long}", path.join(" ")),
            None => path.join(" "),
        };
        let answer = match ask {
            Ask::App => choose_app(&client, &title, json).await,
            Ask::Platform => choose_platform(&client, &title, json).await,
            Ask::Text => output::ask_text(&title),
        };
        answers.push((arg, answer?));
    }
    Some(with_values(args, &answers))
}

/// What the words leave out of the command they run.
struct LeftOut<'a> {
    /// The command as the tree names it: `["vendo", "destinations", "get"]` for `vendo int get`.
    path: Vec<&'a str>,
    /// The required values not given, in clap's order.
    args: Vec<&'a clap::Arg>,
    /// `--profile` and `--debug`, wherever they were typed.
    profile: Option<String>,
    debug: bool,
}

/// What `typed` leaves out, read from clap's partial matches (`ignore_errors`); `None` when it leaves
/// out no required value.
fn left_out<'a>(root: &'a clap::Command, typed: &[OsString]) -> Option<LeftOut<'a>> {
    let matches = root.clone().ignore_errors(true).try_get_matches_from(typed).ok()?;
    let profile = matches.try_get_one::<String>("profile").ok().flatten().cloned();
    let debug = matches.try_get_one::<bool>("debug").ok().flatten().copied().unwrap_or(false);
    let (mut command, mut matched, mut path) = (root, &matches, vec![root.get_name()]);
    while let Some((name, sub)) = matched.subcommand() {
        command = command.find_subcommand(name)?;
        path.push(command.get_name());
        matched = sub;
    }
    let args: Vec<&clap::Arg> = command
        .get_arguments()
        .filter(|arg| arg.is_required_set() && !matched.contains_id(arg.get_id().as_str()))
        .collect();
    (!args.is_empty()).then_some(LeftOut { path, args, profile, debug })
}

/// The client the lists and the command need, checked before anything is asked: the API key (the
/// error a command gives without one, or for a `VENDO_PROFILE` that names no profile), and for a
/// list of the account's apps the account (the client's own error). The platforms are the key's
/// (the catalog route needs no account), so `apps create` lists them without one.
fn ready(ctx: &Ctx, wanted: &[(&clap::Arg, Ask)]) -> anyhow::Result<Client> {
    let client = ctx.client()?;
    if wanted.iter().any(|(_, ask)| *ask == Ask::App) {
        client.require_account()?;
    }
    Ok(client)
}

/// Ends the CLI with `err` as the command would end with it: `Error: …`, or with `--json` the JSON
/// error, exit 1.
fn fail(err: &anyhow::Error, json: bool) -> ! {
    output::set_json_errors(json);
    output::report_error(err);
    std::process::exit(1)
}

/// An app of the account's, chosen from the list behind the spinner `vendo apps list` shows: its
/// full ID, answered as its short ID and name.
async fn choose_app(client: &Client, title: &str, json: bool) -> Option<String> {
    let (apps, cut) =
        output::run_action("Fetching apps...", apps(client)).await.unwrap_or_else(|err| fail(&err.into(), json));
    let cells = |app: &Value| {
        let text = |key: &str| Job(app).text(key).unwrap_or_default();
        vec![short_id(&text("id")), text("displayName"), text("appType"), role_label(app), text("state")]
    };
    let answer = |app: &Value| {
        let (id, name) = (Job(app).id(), Job(app).text("displayName").unwrap_or_default());
        if name.is_empty() { short_id(&id) } else { format!("{} ({name})", short_id(&id)) }
    };
    let chosen = choose(title, "apps", &apps, cut, cells, answer)?;
    Some(Job(&apps[chosen]).id())
}

/// A platform ready to connect, chosen from the list `vendo catalog list` shows by default: its app
/// type.
async fn choose_platform(client: &Client, title: &str, json: bool) -> Option<String> {
    let platforms = output::run_action("Fetching catalog...", platforms(client))
        .await
        .unwrap_or_else(|err| fail(&err.into(), json));
    let app_type = |platform: &Value| Job(platform).text("appType").unwrap_or_default();
    let cells = |platform: &Value| {
        let text = |key: &str| Job(platform).text(key).unwrap_or_default();
        vec![text("displayName"), app_type(platform), text("category"), catalog::roles(platform)]
    };
    let chosen = choose(title, "platforms", &platforms, false, cells, app_type)?;
    Some(app_type(&platforms[chosen]))
}

/// The index of the one of `items` chosen in the list titled `title`, each shown as its `cells` and
/// answered as `answer`; `cut` when the list holds only the newest. With none to choose from it says
/// so (`No apps to choose from.`) and is `None`: the usage error follows.
fn choose(
    title: &str,
    plural: &str,
    items: &[Value],
    cut: bool,
    cells: impl Fn(&Value) -> Vec<String>,
    answer: impl Fn(&Value) -> String,
) -> Option<usize> {
    if items.is_empty() {
        eprintln!("No {plural} to choose from.");
        return None;
    }
    let shown = padded(items.iter().map(cells).collect());
    let rows: Vec<ValueRow> =
        shown.into_iter().zip(items).map(|(shown, item)| ValueRow { shown, answer: answer(item) }).collect();
    let note = format!("newest {} shown", short_ids::PAGE * short_ids::MAX_PAGES);
    output::choose_value(title, &rows, cut.then_some(note.as_str()))
}

/// The rows of a list: each cell padded to its column's width (in characters, as the menu counts
/// them), two spaces apart as the tables' columns are, the last column as it is. Plain text: typing
/// filters by what the row shows.
fn padded(rows: Vec<Vec<String>>) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|i| rows.iter().filter_map(|row| row.get(i)).map(|cell| cell.chars().count()).max().unwrap_or(0))
        .collect();
    rows.into_iter()
        .map(|row| {
            let last = row.len().saturating_sub(1);
            let cells = row.into_iter().enumerate();
            let cells =
                cells.map(|(i, cell)| if i < last { format!("{cell:<width$}", width = widths[i]) } else { cell });
            cells.collect::<Vec<_>>().join("  ")
        })
        .collect()
}

/// The account's apps in the apps route's order (newest first), each once: up to
/// [`short_ids::MAX_PAGES`] pages of [`short_ids::PAGE`], the bounds of a short-ID lookup; and
/// whether there were more than that.
async fn apps(client: &Client) -> Result<(Vec<Value>, bool), ApiError> {
    let mut apps: Vec<Value> = Vec::new();
    for page in 0..short_ids::MAX_PAGES {
        let (rows, more) = short_ids::list_page(client, Listing::Apps, None, page * short_ids::PAGE).await?;
        let read = rows.len();
        for row in rows {
            // A row inserted between two page reads moves the next page down, so the same row can
            // come back at its top.
            let id = row.get("id").and_then(Value::as_str);
            if id.is_some_and(|id| !apps.iter().any(|seen| seen.get("id").and_then(Value::as_str) == Some(id))) {
                apps.push(row);
            }
        }
        if !more || read == 0 {
            return Ok((apps, false));
        }
    }
    Ok((apps, true))
}

/// The platforms ready to connect: `GET /catalog` with no query, which leaves out the ones on
/// request, as `vendo catalog list` asks for them; one response, sorted by name.
async fn platforms(client: &Client) -> Result<Vec<Value>, ApiError> {
    let res = client.get("/catalog", &[]).await?;
    Ok(payload(&res).as_array().cloned().unwrap_or_default())
}

/// `args` with each answered value where it would have been typed: an option as one word,
/// `--type=shopify`, before a `--` if one was typed, else at the end; an argument at the end, after
/// a `--` when it starts with `-` and none was typed (an ID never does).
fn with_values(args: &[OsString], answers: &[(&clap::Arg, String)]) -> Vec<OsString> {
    let mut args = args.to_vec();
    let separator = |args: &[OsString]| args.iter().skip(1).position(|arg| arg == "--").map(|at| at + 1);
    for (arg, value) in answers {
        match arg.get_long() {
            Some(long) => {
                let at = separator(&args).unwrap_or(args.len());
                args.insert(at, format!("--{long}={value}").into());
            }
            None => {
                if value.starts_with('-') && separator(&args).is_none() {
                    args.push("--".into());
                }
                args.push(value.into());
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// The commands whose values are not asked for yet: they keep the usage error until their
    /// part of VE-3881 is built (sources and destinations; jobs, models, metrics and the catalog;
    /// measurement and the dictionary). Empty once every value is asked for.
    const PENDING: [&str; 31] = [
        "sources get",
        "sources sync",
        "sources pause",
        "sources resume",
        "sources delete",
        "sources create",
        "sources update",
        "destinations get",
        "destinations sync",
        "destinations refresh-source",
        "destinations pause",
        "destinations resume",
        "destinations delete",
        "destinations create",
        "destinations update",
        "jobs get",
        "jobs cancel",
        "catalog get",
        "catalog credential-schema",
        "dictionary search",
        "dictionary get",
        "metrics get",
        "metrics create",
        "metrics update",
        "metrics activate",
        "metrics delete",
        "models get",
        "measurement methodologies get",
        "measurement rules preview",
        "measurement ltv cohort",
        "measurement ltv customer",
    ];

    /// Every command in the tree, hidden ones too, as typed after `vendo`, with the IDs of the values
    /// it requires.
    fn commands(cmd: &clap::Command, path: &[&str], found: &mut Vec<(String, Vec<String>)>) {
        for sub in cmd.get_subcommands().filter(|sub| sub.get_name() != "help") {
            let path = [path, &[sub.get_name()]].concat();
            if sub.has_subcommands() {
                commands(sub, &path, found);
            } else {
                let required = sub.get_arguments().filter(|arg| arg.is_required_set());
                found.push((path.join(" "), required.map(|arg| arg.get_id().to_string()).collect()));
            }
        }
    }

    #[test]
    fn every_required_value_is_asked_for_or_its_command_is_pending() {
        let mut found = Vec::new();
        commands(&crate::cli::command(), &[], &mut found);
        let requiring: Vec<&(String, Vec<String>)> =
            found.iter().filter(|(_, required)| !required.is_empty()).collect();
        let values: usize = requiring.iter().map(|(_, required)| required.len()).sum();
        assert_eq!((requiring.len(), values), (37, 43), "{requiring:?}");
        for (command, required) in &requiring {
            let asked: Vec<bool> = required.iter().map(|id| asked_by(command, id).is_some()).collect();
            if PENDING.contains(&command.as_str()) {
                assert!(asked.iter().all(|asked| !asked), "{command} is pending but has values in VALUES");
            } else {
                assert!(asked.iter().all(|asked| *asked), "{command}: a required value has no row in VALUES");
            }
        }
        for (command, value, _) in VALUES {
            let required = found.iter().find(|(c, _)| c == command).map(|(_, required)| required);
            assert!(required.is_some_and(|r| r.iter().any(|id| id == value)), "{command} {value} is no required value");
        }
        for command in PENDING {
            assert!(requiring.iter().any(|(c, _)| c == command), "pending {command} requires no value");
        }
    }

    /// [`LeftOut`] as plain values: the path, the values' IDs, `--profile` and `--debug`.
    type Left = (Vec<String>, Vec<String>, Option<String>, bool);

    fn left_out_of(args: &[&str]) -> Option<Left> {
        let root = crate::cli::command();
        let LeftOut { path, args, profile, debug } = left_out(&root, &os(args))?;
        let ids = args.iter().map(|arg| arg.get_id().to_string()).collect();
        Some((path.iter().map(|word| word.to_string()).collect(), ids, profile, debug))
    }

    #[test]
    fn the_values_left_out_are_the_required_ones_not_typed_in_clap_order() {
        let path = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        let ids = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        assert_eq!(
            left_out_of(&["vendo", "apps", "create", "--role", "destination"]),
            Some((path(&["vendo", "apps", "create"]), ids(&["app_type", "name"]), None, false))
        );
        assert_eq!(
            left_out_of(&["vendo", "apps", "create", "--name", "Shop"]),
            Some((path(&["vendo", "apps", "create"]), ids(&["app_type"]), None, false))
        );
        // The tree's names, not the aliases typed; the global options wherever they were typed.
        assert_eq!(
            left_out_of(&["vendo", "int", "get", "--profile", "beta", "--debug"]),
            Some((path(&["vendo", "destinations", "get"]), ids(&["id"]), Some("beta".into()), true))
        );
        assert_eq!(
            left_out_of(&["vendo", "--profile", "beta", "apps", "delete", "--yes"]),
            Some((path(&["vendo", "apps", "delete"]), ids(&["id"]), Some("beta".into()), false))
        );
        // Nothing left out, or nothing to ask in an optional value.
        assert_eq!(left_out_of(&["vendo", "apps", "get", "a1"]), None);
        assert_eq!(left_out_of(&["vendo", "jobs", "tail"]), None);
    }

    #[test]
    fn answers_go_where_they_would_have_been_typed() {
        let root = crate::cli::command();
        let create = root.find_subcommand("apps").unwrap().find_subcommand("create").unwrap();
        let get = root.find_subcommand("apps").unwrap().find_subcommand("get").unwrap();
        let arg = |cmd: &clap::Command, id: &str| cmd.get_arguments().find(|arg| arg.get_id() == id).unwrap().clone();
        let (app_type, name, id) = (arg(create, "app_type"), arg(create, "name"), arg(get, "id"));
        let words = |args: Vec<OsString>| args.into_iter().map(|arg| arg.into_string().unwrap()).collect::<Vec<_>>();
        // Options as one word each, in order, at the end.
        assert_eq!(
            words(with_values(
                &os(&["vendo", "apps", "create", "--json"]),
                &[(&app_type, "shopify".into()), (&name, "-My Shop".into())]
            )),
            ["vendo", "apps", "create", "--json", "--type=shopify", "--name=-My Shop"]
        );
        // Before a `--`.
        assert_eq!(
            words(with_values(&os(&["vendo", "apps", "create", "--", "x"]), &[(&app_type, "shopify".into())])),
            ["vendo", "apps", "create", "--type=shopify", "--", "x"]
        );
        // An argument at the end, after a `--` when it starts with `-` and none was typed.
        assert_eq!(
            words(with_values(&os(&["vendo", "apps", "get", "--json"]), &[(&id, "a1b2".into())])),
            ["vendo", "apps", "get", "--json", "a1b2"]
        );
        assert_eq!(
            words(with_values(&os(&["vendo", "apps", "get"]), &[(&id, "-a1".into())])),
            ["vendo", "apps", "get", "--", "-a1"]
        );
        assert_eq!(
            words(with_values(&os(&["vendo", "apps", "get", "--"]), &[(&id, "-a1".into())])),
            ["vendo", "apps", "get", "--", "-a1"]
        );
    }

    #[test]
    fn list_rows_pad_each_column_but_the_last() {
        let rows = vec![
            vec!["a1b2c3d4...".into(), "Menu Shop".into(), "shopify".into(), "active".into()],
            vec!["e5f6a7b8...".into(), "Analytics BQ".into(), "bigquery".into(), "inactive".into()],
            vec!["0c0d0e0f...".into(), "".into(), "bigquery".into(), "active".into()],
        ];
        assert_eq!(
            padded(rows),
            [
                "a1b2c3d4...  Menu Shop     shopify   active",
                "e5f6a7b8...  Analytics BQ  bigquery  inactive",
                "0c0d0e0f...                bigquery  active",
            ]
        );
        // Widths in characters, as the menu counts them.
        assert_eq!(
            padded(vec![vec!["Café".into(), "x".into()], vec!["Shop".into(), "y".into()]]),
            ["Café  x", "Shop  y"]
        );
    }
}

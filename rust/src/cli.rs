//! Command-line definition. Command names, descriptions, flags and examples
//! follow the TypeScript CLI (`src/cli.ts`, `src/commands/*`), and parsing
//! follows commander's rules where clap's differ (VE-3727): a repeated flag
//! takes the last value, an option's value may start with `-`, and
//! `-V`/`--version` works after a command too ([`preprocess`]).

use std::ffi::OsString;

use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "vendo", about = "Vendo CLI — manage your data pipeline from the terminal", args_override_self = true)]
pub struct Cli {
    /// Use a specific account profile (or set VENDO_PROFILE)
    #[arg(long, global = true, value_name = "name", help_heading = GLOBAL_OPTIONS)]
    pub profile: Option<String>,
    /// Enable verbose request diagnostics
    #[arg(long, global = true, help_heading = GLOBAL_OPTIONS)]
    pub debug: bool,
    #[command(subcommand)]
    pub command: Command,
}

/// Every screen but the root's lists `--profile` and `--debug` apart from the command's own options (VE-3827).
const GLOBAL_OPTIONS: &str = "Global options";

/// The clap command: [`Cli`] plus what commander did implicitly. Every
/// option value may start with `-` (`--frequency -5`, `--name -Prod`), and the
/// root lists `-V, --version`, which [`preprocess`] handles before clap runs.
/// The root help is sectioned ([`HELP_SECTIONS`]) and group screens have no `help` row (VE-3827).
pub fn command() -> clap::Command {
    let cmd = no_help_rows(allow_hyphen_values(Cli::command()))
        .arg(Arg::new("version").short('V').long("version").action(ArgAction::SetTrue).help("Print version"));
    let template = root_help_template(&cmd);
    cmd.help_template(template)
}

fn allow_hyphen_values(cmd: clap::Command) -> clap::Command {
    cmd.mut_args(|arg| {
        if !arg.is_positional() && arg.get_action().takes_values() { arg.allow_hyphen_values(true) } else { arg }
    })
    .mut_subcommands(allow_hyphen_values)
}

/// Group screens list their commands without clap's `help` row. The root keeps its `help`
/// command, so `vendo help <group> <command>` works, and [`rewrite_hidden_paths`] turns
/// `vendo <group> help <command>` into it.
fn no_help_rows(cmd: clap::Command) -> clap::Command {
    cmd.mut_subcommands(|sub| {
        let sub = if sub.has_subcommands() { sub.disable_help_subcommand(true) } else { sub };
        no_help_rows(sub)
    })
}

/// The sections of `vendo --help`, in order. Every visible command sits in exactly one: a test
/// fails when one is missing, so a new command cannot drop out of the root help. Move a command
/// by moving its name. `help` is clap's own command, listed since VE-3893 with `version`. "Account"
/// is "Getting started" and "Account" in one, first (VE-4109, Yalcin 2026-10-10).
pub const HELP_SECTIONS: [(&str, &[&str]); 3] = [
    (
        "Account",
        &["login", "logout", "workspace", "status", "help", "version", "profile", "mcp", "completions", "update"],
    ),
    ("Data pipeline", &["apps", "sources", "destinations", "jobs"]),
    ("Data catalog", &["catalog", "dictionary", "metrics", "models", "measurement"]),
];

/// The root help fits 80 columns: a description that would go further wraps onto lines of its own
/// in its column (VE-4109).
pub const HELP_COLUMNS: usize = 80;

/// clap's `help` command, which the root help lists (VE-3893). clap adds it only when it builds
/// the tree to parse, so the tree [`root_help_template`] reads has none: its row says what clap's
/// command says it does, in clap's words (a test checks them), with no commands under it.
const CLAP_HELP: (&str, &str) = ("help", "Print this message or the help of the given subcommand(s)");

/// One row of the root help's command list: how far in it starts, what it names and its description.
#[derive(Debug, PartialEq)]
struct HelpRow {
    indent: usize,
    label: String,
    about: String,
}

/// The rows of one section (VE-4109, Yalcin 2026-10-10: "combine help and commands, list commands under
/// the help"): a command that runs, one row; a group, a row with its name capitalized (`Apps`) and
/// description, then a row for every visible command under it, nested ones included, by its full path
/// (`apps list`, `measurement ltv cohort`) and its own description. Hidden commands and aliases stay out.
fn section_rows(root: &clap::Command, names: &[&str]) -> Vec<HelpRow> {
    let about = |command: &clap::Command| command.get_about().map(ToString::to_string).unwrap_or_default();
    let mut rows = Vec::new();
    for &name in names {
        match root.find_subcommand(name) {
            Some(group) if group.has_subcommands() => {
                rows.push(HelpRow { indent: 2, label: capitalized(name), about: about(group) });
                for (path, leaf) in leaves(group, name) {
                    rows.push(HelpRow { indent: 4, label: path, about: about(leaf) });
                }
            }
            Some(command) => rows.push(HelpRow { indent: 2, label: name.to_string(), about: about(command) }),
            None if name == CLAP_HELP.0 => {
                rows.push(HelpRow { indent: 2, label: name.to_string(), about: CLAP_HELP.1.to_string() })
            }
            None => {}
        }
    }
    rows
}

/// `apps` → `Apps`: a group's row in the root help, which names the group as Yalcin's layout does.
fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The visible commands that run under `group`, depth first in its help's order, by their path from
/// the root (`measurement ltv cohort`).
fn leaves<'a>(group: &'a clap::Command, path: &str) -> Vec<(String, &'a clap::Command)> {
    let mut found = Vec::new();
    for command in group.get_subcommands().filter(|c| !c.is_hide_set()) {
        let path = format!("{path} {}", command.get_name());
        if command.has_subcommands() { found.extend(leaves(command, &path)) } else { found.push((path, command)) }
    }
    found
}

/// The rows as lines: labels in `literal`, descriptions in one column per section, two spaces after
/// its longest label, wrapped at word breaks to fit [`HELP_COLUMNS`] (a word longer than the room
/// stays whole).
fn section_lines(rows: &[HelpRow], literal: &clap::builder::styling::Style) -> Vec<String> {
    let column = rows.iter().map(|row| row.indent + row.label.chars().count()).max().unwrap_or(0) + 2;
    let room = HELP_COLUMNS.saturating_sub(column).max(1);
    let mut lines = Vec::new();
    for row in rows {
        let pad = column - row.indent - row.label.chars().count();
        let mut text = vec![String::new()];
        for word in row.about.split_whitespace() {
            let line = text.last_mut().unwrap();
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > room {
                text.push(word.to_string());
            } else {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
        }
        let first = format!("{}{literal}{}{literal:#}{:pad$}{}", " ".repeat(row.indent), row.label, "", text[0]);
        lines.push(first.trim_end().to_string());
        lines.extend(text[1..].iter().map(|more| format!("{}{more}", " ".repeat(column))));
    }
    lines
}

/// The root help: [`HELP_SECTIONS`], each with its rows ([`section_rows`]), read from the command
/// tree so the list cannot drift from what the CLI runs. `vendo <command> --help` keeps the flags
/// and examples. Styled like clap's own lists; clap drops the styles without colour.
fn root_help_template(root: &clap::Command) -> String {
    let styles = root.get_styles();
    let (header, literal) = (styles.get_header(), styles.get_literal());
    let mut sections = String::new();
    for (title, names) in HELP_SECTIONS {
        sections.push_str(&format!("{header}{title}:{header:#}\n"));
        for line in section_lines(&section_rows(root, names), literal) {
            sections.push_str(&line);
            sections.push('\n');
        }
        sections.push('\n');
    }
    format!(
        "{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{sections}{header}Options:{header:#}\n{{options}}\n\nSee `vendo <command> --help` for flags and examples."
    )
}

/// What [`parse`] read: the command, and whether it was given `--json` (VE-3831).
pub struct Parsed {
    pub cli: Cli,
    pub json: bool,
}

/// Parse with [`command`]; usage errors and `--help` exit like clap does, a usage error as
/// JSON when the words include `--json` ([`exit_with`]). A group run without its command opens
/// its menu on a terminal, and the command chosen there is parsed as if typed ([`chosen_command`]).
/// `--profile` typed last with no name opens the profile list there ([`chosen_profile`], VE-3892).
/// A command missing required values asks for them there, once, and runs with them as if they had
/// been typed ([`asked_values`], VE-3881).
pub async fn parse(mut args: Vec<OsString>) -> Parsed {
    let mut asked = false;
    loop {
        let cmd = command();
        let typed = rewrite_hidden_paths(&cmd, args.clone());
        let json_word = json_word(&typed);
        let err = match cmd.clone().try_get_matches_from(typed.clone()) {
            Ok(matches) => {
                let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|err| exit_with(err, json_word));
                typed_as_doctor(&cmd, &typed, &mut cli.command);
                return Parsed { cli, json: json_flag(&matches) };
            }
            Err(err) => err,
        };
        if let Some(chosen) = chosen_command(&cmd, &args, &err) {
            args = chosen;
            continue;
        }
        if let Some(chosen) = chosen_profile(&cmd, &args, &err) {
            args = chosen;
            continue;
        }
        if !asked {
            asked = true;
            if let Some(filled) = asked_values(&cmd, &args, &typed, &err, json_word).await {
                args = filled;
                continue;
            }
        }
        exit_with(err, json_word)
    }
}

/// `vendo doctor` is `workspace`'s hidden alias (VE-3891), and clap does not say which name was
/// typed: the first command word says it, so `doctor` keeps doctor's exit code.
fn typed_as_doctor(root: &clap::Command, typed: &[OsString], command: &mut Command) {
    if let Command::Workspace { doctor, .. } = command {
        *doctor = command_words(root, typed).path.first().is_some_and(|word| word == "doctor");
    }
}

/// For a command typed without values it requires (clap's missing-arguments error), where someone
/// can answer and see the question ([`crate::output::can_show_menu`], the group menu's rule):
/// `args` with each value asked for and put where it would have been typed ([`crate::ask`],
/// VE-3881). `None` keeps the usage error.
#[cfg(feature = "menu")]
async fn asked_values(
    root: &clap::Command,
    args: &[OsString],
    typed: &[OsString],
    err: &clap::Error,
    json: bool,
) -> Option<Vec<OsString>> {
    crate::ask::missing_values(root, args, typed, err, json).await
}

/// A build without the `menu` feature asks for nothing (VE-3881): a missing value is the usage
/// error at a terminal too, as a bare group is ([`menu_choice`]).
#[cfg(not(feature = "menu"))]
async fn asked_values(
    _root: &clap::Command,
    _args: &[OsString],
    _typed: &[OsString],
    _err: &clap::Error,
    _json: bool,
) -> Option<Vec<OsString>> {
    None
}

/// For a group run without its command (`vendo apps`) where someone can answer and see the menu
/// ([`crate::output::can_show_menu`]): the group's menu, then `args` with the chosen name where
/// it would have been typed. [`parse`] parses them like typed arguments, so the global options,
/// `MOVED` and the command's own usage errors apply, and a chosen group opens its own menu
/// (VE-3826). `None` keeps the usage error: another error, the root, no terminal, or a menu that
/// cannot show.
fn chosen_command(root: &clap::Command, args: &[OsString], err: &clap::Error) -> Option<Vec<OsString>> {
    use clap::error::ErrorKind::{DisplayHelpOnMissingArgumentOrSubcommand, MissingSubcommand};
    // clap says the first when nothing follows the group, the second when a global option does.
    if !matches!(err.kind(), DisplayHelpOnMissingArgumentOrSubcommand | MissingSubcommand) {
        return None;
    }
    let (group, title, at) = bare_group(root, args)?;
    let chosen = menu_choice(group, &title)?;
    let mut args = args.to_vec();
    args.insert(at, chosen.into());
    Some(args)
}

/// The name of the command chosen in `group`'s menu, where it can show.
#[cfg(feature = "menu")]
fn menu_choice<'a>(group: &'a clap::Command, title: &str) -> Option<&'a str> {
    if !crate::output::can_show_menu() {
        return None;
    }
    let commands: Vec<&clap::Command> = group.get_subcommands().filter(|command| !command.is_hide_set()).collect();
    let rows: Vec<crate::output::MenuCommand> = commands
        .iter()
        .map(|command| crate::output::MenuCommand {
            name: command.get_name().to_string(),
            about: command.get_about().map(ToString::to_string).unwrap_or_default(),
        })
        .collect();
    Some(commands[crate::output::choose_command(title, &rows)?].get_name())
}

/// A build without the `menu` feature has no menu (VE-3826): a group run without its command is
/// the usage error at a terminal too, as without one. The release's size check builds it to
/// measure what the menu adds (`scripts/menu-size.sh`).
#[cfg(not(feature = "menu"))]
fn menu_choice<'a>(_group: &'a clap::Command, _title: &str) -> Option<&'a str> {
    None
}

/// For words that end in `--profile` with no name (clap's "a value is required" usage error), where
/// someone can answer and see the list ([`crate::output::can_show_menu`], the group menu's rule): the
/// saved profiles in a list ([`crate::ask::profile`]; VE-3892, decided by Yalcin 2026-10-07), then
/// `args` with the chosen one ([`with_profile`]), which [`parse`] parses like typed arguments. `None`
/// keeps the usage error: another error, no terminal, no profile saved, or a list that cannot open.
fn chosen_profile(root: &clap::Command, args: &[OsString], err: &clap::Error) -> Option<Vec<OsString>> {
    let (title, alone) = profile_without_name(root, args, err)?;
    let name = profile_choice(&title)?;
    Some(with_profile(args, alone, &name))
}

/// For words that end in `--profile` with no name: the list's title, and whether `--profile` stands
/// alone (after global options only). The title is `vendo --profile`, or the command so far as the tree
/// names it (`vendo destinations list --profile` for `vendo int list --profile`, `vendo workspace
/// --profile` for `vendo profile current --profile`), as the lists for a missing value are titled
/// (VE-3881). Only the last word can be a `--profile` with no name: clap takes the word after it as
/// its name, `--` and words starting with `-` too. `None` for any other error.
fn profile_without_name(root: &clap::Command, args: &[OsString], err: &clap::Error) -> Option<(String, bool)> {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    let (last, before) = args.split_last()?;
    let context = |kind| match err.get(kind) {
        Some(ContextValue::String(text)) => Some(text.as_str()),
        _ => None,
    };
    let no_name = err.kind() == ErrorKind::InvalidValue
        && context(ContextKind::InvalidValue) == Some("")
        && context(ContextKind::InvalidArg).is_some_and(|arg| arg.starts_with("--profile "));
    if !no_name || last != "--profile" || before.is_empty() {
        return None;
    }
    let words = command_words(root, &rewrite_hidden_paths(root, before.to_vec()));
    if words.help {
        return None;
    }
    if words.path.is_empty() {
        return words.whole.then(|| (format!("{} --profile", root.get_name()), true));
    }
    let mut path = vec![root.get_name()];
    let mut current = root;
    for word in &words.path {
        current = current.find_subcommand(word)?;
        path.push(current.get_name());
    }
    Some((format!("{} --profile", path.join(" ")), false))
}

/// `args`, which end in `--profile`, with the profile `name` chosen for it. On its own (`alone`) the
/// words become `vendo profile switch <name>` after the global options typed: the chosen profile
/// becomes the active one, and they print exactly what that prints. After a command, `--profile=<name>`
/// takes the place of `--profile`, so the command runs as if `--profile <name>` had been typed, with
/// that profile for it alone; the saved active profile stays.
fn with_profile(args: &[OsString], alone: bool, name: &str) -> Vec<OsString> {
    let mut args = args[..args.len().saturating_sub(1)].to_vec();
    if alone {
        args.extend(["profile", "switch"].map(OsString::from));
        // A name that starts with `-` is no flag after `--`.
        if name.starts_with('-') {
            args.push("--".into());
        }
        args.push(name.into());
    } else {
        args.push(format!("--profile={name}").into());
    }
    args
}

/// The profile chosen in the list titled `title`, where it can open.
#[cfg(feature = "menu")]
fn profile_choice(title: &str) -> Option<String> {
    crate::ask::profile(title)
}

/// A build without the `menu` feature has no list (VE-3892): `--profile` with no name is the usage
/// error at a terminal too, as a bare group is ([`menu_choice`]).
#[cfg(not(feature = "menu"))]
fn profile_choice(_title: &str) -> Option<String> {
    None
}

/// The group `args` run without its command, if they name one and nothing but global options:
/// the group, its path as the tree names it (`vendo profile` for `vendo config`), and where its
/// command goes in `args`. Not the root: a bare `vendo` prints the help as before.
fn bare_group<'a>(root: &'a clap::Command, args: &[OsString]) -> Option<(&'a clap::Command, String, usize)> {
    let words = command_words(root, args);
    let group = words.group.filter(|_| words.whole && !words.help)?;
    let at = words.at.last()? + 1;
    let mut path = vec![root.get_name()];
    let mut current = root;
    for word in &words.path {
        current = current.find_subcommand(word)?;
        path.push(current.get_name());
    }
    Some((group, path.join(" "), at))
}

/// Whether the command that runs was given its `--json` flag.
fn json_flag(matches: &clap::ArgMatches) -> bool {
    match matches.subcommand() {
        Some((_, sub)) => json_flag(sub),
        None => matches!(matches.try_get_one::<bool>("json"), Ok(Some(true))),
    }
}

/// `--json` among the words before a `--`: what asked for JSON when the words do not parse.
fn json_word(args: &[OsString]) -> bool {
    args.iter().skip(1).take_while(|arg| *arg != "--").any(|arg| arg == "--json")
}

/// A usage error with `--json` prints as [`crate::output::message_error_json`] (clap's first
/// paragraph, without `error: `, and its tips, [`usage_message`]) and keeps clap's exit code, 2
/// (VE-3831). Help, `-h` and a group run without its command print as before.
fn exit_with(err: clap::Error, json: bool) -> ! {
    use clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand;
    if json && err.use_stderr() && err.kind() != DisplayHelpOnMissingArgumentOrSubcommand {
        crate::output::print_error_json(&crate::output::message_error_json(&usage_message(&err)));
        std::process::exit(err.exit_code());
    }
    err.exit()
}

/// `error: unexpected argument '--bogus' found` → `unexpected argument '--bogus' found`: the
/// first paragraph of clap's message, then each of its tips on a line of its own
/// (`tip: a similar subcommand exists: 'list'`), which an agent can correct a typo by (Yalcin,
/// 2026-10-06). The usage and the `--help` line after them are left out.
fn usage_message(err: &clap::Error) -> String {
    let rendered = err.render().to_string();
    let mut paragraphs = rendered.split("\n\n");
    let first = paragraphs.next().unwrap_or_default().trim_end();
    let mut message = first.strip_prefix("error: ").unwrap_or(first).to_string();
    let tips = paragraphs.flat_map(str::lines).map(str::trim).filter(|line| line.starts_with("tip: "));
    for tip in tips {
        message.push('\n');
        message.push_str(tip);
    }
    message
}

/// Commands that moved to a command elsewhere in the tree in CLI 1.1 (VE-3827), where clap's
/// aliases cannot point: `vendo <group> <name> …` runs `vendo <target> …`. The moves within
/// `profile` are clap's hidden aliases: `config` for `profile` (so `config set` and `config list`
/// run `profile set` and `profile list`) and `use` for `profile switch`. Either way the old path
/// prints exactly what its target prints, `--help` and usage errors included, and no help screen
/// or completion script shows it.
const MOVED: [(&str, &str, &[&str]); 3] = [
    ("profile", "current", &["workspace"]),
    ("config", "show", &["workspace"]),
    ("config", "reset", &["logout", "--all"]),
];

/// What the CLI still runs but no longer shows, rewritten before clap parses: a [`MOVED`]
/// command, and `vendo <group> help [<command>…]` (the `help` row the group screens dropped),
/// which runs `vendo help <group> [<command>…]`. A `help` after a command that is not a group is
/// that command's argument (`vendo dictionary search help`) and stays.
fn rewrite_hidden_paths(root: &clap::Command, mut args: Vec<OsString>) -> Vec<OsString> {
    let CommandWords { at, mut path, help, .. } = command_words(root, &args);
    let moved = MOVED.iter().find(|(from, name, _)| path.len() >= 2 && path[0] == *from && path[1] == *name);
    if let Some((_, _, target)) = moved {
        // `vendo help config reset` shows `vendo help logout`: a flag is not a command.
        let target = target.iter().filter(|word| !help || !word.starts_with('-')).map(|word| word.to_string());
        path.splice(..2, target);
    } else if !help || at.first().is_some_and(|&first| args[first] == "help") {
        return args;
    }
    let words = help.then(|| "help".to_string()).into_iter().chain(path).map(OsString::from);
    for &i in at.iter().rev() {
        args.remove(i);
    }
    args.splice(at[0]..at[0], words);
    args
}

/// The command words at the start of `args`, each naming a command of the group before it, as
/// clap reads them; the global options may sit between them. Read up to any other option, or a
/// word after a command that is not a group.
struct CommandWords<'a> {
    /// Where each command word is, `help` included.
    at: Vec<usize>,
    /// The command words but `help`, as typed.
    path: Vec<String>,
    /// Whether one of them is `help`.
    help: bool,
    /// The group the words end in (the root when there are none); `None` once a word is not a
    /// group or names no command.
    group: Option<&'a clap::Command>,
    /// Whether the words and global options are all there is.
    whole: bool,
}

fn command_words<'a>(root: &'a clap::Command, args: &[OsString]) -> CommandWords<'a> {
    let mut words = CommandWords { at: Vec::new(), path: Vec::new(), help: false, group: Some(root), whole: false };
    let mut i = 1;
    while let (Some(current), Some(word)) = (words.group, args.get(i).and_then(|arg| arg.to_str())) {
        match word {
            "--debug" => {}
            "--profile" => i += 1,
            _ if word.starts_with("--profile=") => {}
            _ if word.starts_with('-') => return words,
            "help" if !words.help => {
                words.at.push(i);
                words.help = true;
            }
            _ => {
                words.at.push(i);
                words.path.push(word.to_string());
                words.group = current.find_subcommand(word).filter(|command| command.has_subcommands());
            }
        }
        i += 1;
    }
    words.whole = i == args.len();
    words
}

#[derive(Debug, PartialEq)]
pub enum Invocation {
    /// `-V`/`--version`: print the bare version and exit 0.
    Version,
    /// Arguments for [`parse`].
    Run(Vec<OsString>),
}

/// What commander did before any command saw its arguments. `-V` or
/// `--version` anywhere before `--` prints the version, unless it is the value
/// of `--profile` or `update --version <v>` (`self-update`, its hidden alias since VE-4109; it installs
/// that version: an accepted difference, commander printed the version). A `--` before the
/// command name is dropped, so `vendo -- workspace` runs `workspace`.
pub fn preprocess(args: Vec<OsString>) -> Invocation {
    let mut out = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    out.extend(iter.next());
    let mut command: Option<OsString> = None;
    while let Some(arg) = iter.next() {
        let in_update = command.as_ref().is_some_and(|c| c == "update" || c == "self-update");
        match arg.to_str() {
            Some("--") => {
                if command.is_some() {
                    out.push(arg);
                }
                out.extend(iter);
                break;
            }
            Some("-V") => return Invocation::Version,
            Some("--version") if !in_update => return Invocation::Version,
            Some("--version" | "--profile") => {
                out.push(arg);
                out.extend(iter.next());
            }
            Some(word) if command.is_none() && !word.starts_with('-') => {
                command = Some(arg.clone());
                out.push(arg);
            }
            _ => out.push(arg),
        }
    }
    Invocation::Run(out)
}

#[derive(Subcommand)]
pub enum Command {
    /// Authenticate with your Vendo account
    // `init` was the first-time setup command until CLI 1.1; `login` does what it did and stays its
    // hidden alias, so `vendo init` prints exactly what `vendo login` prints (VE-3825).
    #[command(
        alias = "init",
        after_help = "Examples:\n  $ vendo login\n  $ vendo login --env staging\n  $ vendo login --force\n  $ vendo login --api-key vendo_sk_... --account <account-id>"
    )]
    Login {
        /// API key for headless/CI login (requires --account)
        #[arg(long, value_name = "key")]
        api_key: Option<String>,
        /// Account ID for headless/CI login (requires --api-key)
        #[arg(long, value_name = "id")]
        account: Option<String>,
        /// Target instance: "staging" or "prod" (default: VENDO_API_URL/profile, else prod)
        #[arg(long, value_name = "environment", value_parser = ENVIRONMENTS)]
        env: Option<String>,
        /// Explicit API base URL (overrides --env)
        #[arg(long, value_name = "url")]
        base_url: Option<String>,
        /// Sign in through the browser again, even when the saved API key works
        #[arg(long)]
        force: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove stored credentials
    #[command(after_help = "Examples:\n  $ vendo logout\n  $ vendo logout --all\n  $ vendo logout --all --yes")]
    Logout {
        /// Remove every saved profile, not just the active one
        #[arg(long)]
        all: bool,
        /// Skip the confirmation prompt for --all
        #[arg(short, long)]
        yes: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage account profiles
    // `config` was a group of its own until CLI 1.1; its commands moved here (VE-3827, see `MOVED`).
    #[command(alias = "config")]
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Account health overview
    #[command(after_help = "Examples:\n  $ vendo status\n  $ vendo status --json")]
    Status {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Show the current account, your profiles and setup checks
    // `whoami` and `doctor` on one screen (VE-3891, Yalcin 2026-10-07); both stay as hidden aliases that
    // print what it prints, as do `profile current` and `config show` (MOVED).
    #[command(aliases = ["whoami", "doctor"], after_help = "Examples:\n  $ vendo workspace\n  $ vendo workspace --json")]
    Workspace {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Typed as `vendo doctor`, which keeps doctor's exit code: any failing check, setup checks
        /// too, exits 1 (VE-3891 exit codes, Yalcin 2026-10-07). Set by [`parse`], never a flag.
        #[arg(skip)]
        doctor: bool,
    },
    /// Manage apps
    Apps {
        #[command(subcommand)]
        command: AppsCommand,
    },
    /// Manage data sources
    Sources {
        #[command(subcommand)]
        command: SourcesCommand,
    },
    /// Manage data export destinations
    // "Destination" is the customer word (vendo-web-v2 glossary); the API and the code say integration.
    // The old names stay as hidden aliases so existing scripts keep working (VE-3828).
    #[command(name = "destinations", aliases = ["integrations", "int"])]
    Integrations {
        #[command(subcommand)]
        command: IntegrationsCommand,
    },
    /// Monitor sync jobs
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Browse available platforms
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// Browse the data dictionary catalog
    Dictionary {
        #[command(subcommand)]
        command: DictionaryCommand,
    },
    /// Manage custom metrics in the Metrics Library
    Metrics {
        #[command(subcommand)]
        command: MetricsCommand,
    },
    /// Manage data models
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    /// Inspect Marketing Measurement methodologies, LTV, and signals
    Measurement {
        #[command(subcommand)]
        command: MeasurementCommand,
    },
    /// Show how to connect an MCP client (Claude, Cursor, Windsurf) to Vendo
    #[command(after_help = "Examples:\n  $ vendo mcp\n  $ vendo mcp --json\n  $ vendo mcp --show-key")]
    Mcp {
        /// Output only the mcpServers JSON block
        #[arg(long)]
        json: bool,
        /// Embed your actual API key instead of a ${VENDO_API_KEY} placeholder
        #[arg(long)]
        show_key: bool,
    },
    /// Generate shell completion script (bash, zsh, fish)
    #[command(
        after_help = "Examples:\n  $ vendo completions bash\n  $ vendo completions zsh\n  $ vendo completions fish"
    )]
    Completions {
        // Without a shell it explains itself and whether completions are set up (VE-3830).
        #[arg(value_name = "shell")]
        shell: Option<Shell>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// List every command, or with --json the command tree with arguments and flags
    // Read from this tree at runtime, so it lists what runs (VE-3831, `commands/tree.rs`). Hidden since
    // VE-4109, when the root help came to list every command: bare, it prints that help.
    #[command(hide = true, after_help = "Examples:\n  $ vendo commands\n  $ vendo commands --json")]
    Commands {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Print version
    // What `--version` and `-V` print, which `preprocess` handles before clap runs; the command lists
    // the version in the help and takes --json like every command (VE-3893).
    #[command(after_help = "Examples:\n  $ vendo version\n  $ vendo version --json")]
    Version {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Update the Vendo CLI using the hosted installer
    // `self-update` until VE-4109 (Yalcin 2026-10-10), now its hidden alias: it prints exactly what `update` prints.
    #[command(aliases = ["self-update"], after_help = "Examples:\n  $ vendo update\n  $ vendo update --version 0.3.0")]
    Update {
        /// Install a specific version
        #[arg(long = "version", value_name = "version")]
        install_version: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    /// List all configured profiles
    #[command(after_help = "Examples:\n  $ vendo profile list")]
    List {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Switch to a different profile
    #[command(
        alias = "use",
        after_help = "Examples:\n  $ vendo profile switch\n  $ vendo profile switch myprofile\n  $ vendo profile switch --account <accountId>"
    )]
    Switch {
        // Its own id: `profile` is the global `--profile` (VE-3727).
        #[arg(id = "profile_name", value_name = "profile")]
        profile: Option<String>,
        /// Switch by account ID instead of profile name
        #[arg(long, value_name = "accountId")]
        account: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Set values on the active profile
    #[command(
        after_help = "Examples:\n  $ vendo profile set --api-key <key>\n  $ vendo profile set --account <id>\n  $ vendo profile set --api-key <key> --account <id>"
    )]
    Set {
        /// API key for authentication
        #[arg(long, value_name = "key")]
        api_key: Option<String>,
        /// Base URL for the Vendo API
        #[arg(long, value_name = "url")]
        base_url: Option<String>,
        /// Account ID to operate on
        #[arg(long, value_name = "id")]
        account: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum AppsCommand {
    /// List all apps
    #[command(
        after_help = "Examples:\n  $ vendo apps list\n  $ vendo apps list --role source\n  $ vendo apps list --output id"
    )]
    List {
        /// Filter by state (active, inactive)
        #[arg(long, value_name = "state", value_parser = STATES)]
        state: Option<String>,
        /// Filter by app type
        #[arg(long = "type", value_name = "type")]
        app_type: Option<String>,
        /// Filter by capability (source, destination) — derived from permissions
        #[arg(long, value_name = "role", value_parser = ROLES)]
        role: Option<String>,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Show apps that need attention
    #[command(after_help = "Examples:\n  $ vendo apps diagnose\n  $ vendo apps diagnose --json")]
    Diagnose {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Get app details
    #[command(after_help = "Examples:\n  $ vendo apps get <appId>\n  $ vendo apps get <appId> --json")]
    Get {
        #[arg(value_name = "appId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Pause an app
    #[command(after_help = "Examples:\n  $ vendo apps pause <appId>\n  $ vendo apps pause <appId> --dry-run")]
    Pause {
        #[arg(value_name = "appId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Resume a paused app
    #[command(after_help = "Examples:\n  $ vendo apps resume <appId>\n  $ vendo apps resume <appId> --dry-run")]
    Resume {
        #[arg(value_name = "appId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Delete an app (soft delete)
    #[command(
        after_help = "Examples:\n  $ vendo apps delete <appId>\n  $ vendo apps delete <appId> --yes\n  $ vendo apps delete <appId> --dry-run"
    )]
    Delete {
        #[arg(value_name = "appId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Create a new app
    #[command(
        after_help = "Examples:\n  $ vendo apps create --type onesignal --name \"OneSignal Prod\" --role destination --credentials-file onesignal.json\n  $ vendo apps create --type bigquery --name \"Analytics BQ\" --role source --credentials-file bq-sa.json"
    )]
    Create {
        /// App type (e.g. google_ads, onesignal). See: vendo catalog list
        #[arg(long = "type", value_name = "appType", required = true)]
        app_type: String,
        /// Human-readable name for this app
        #[arg(long, value_name = "displayName", required = true)]
        name: String,
        /// Comma-separated capability: source, destination (derives default permissions)
        #[arg(long, value_name = "role", default_value = "source", value_parser = ROLES)]
        role: String,
        // `help =`, not a doc comment: clap drops a doc comment's trailing period, and TS keeps it.
        #[arg(
            long,
            value_name = "permissions",
            help = "Comma-separated granular permissions (e.g. performance_data,send_conversions). Overrides --role.",
            value_parser = PERMISSIONS
        )]
        permissions: Option<String>,
        /// Path to a JSON file with the credential payload
        #[arg(long, value_name = "path")]
        credentials_file: Option<String>,
        /// Path to a JSON file with app-specific config
        #[arg(long, value_name = "path")]
        config_file: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Update an app
    #[command(
        after_help = "Examples:\n  $ vendo apps update <appId> --name \"New name\"\n  $ vendo apps update <appId> --credentials-file rotated.json"
    )]
    Update {
        #[arg(value_name = "appId")]
        id: String,
        /// New display name
        #[arg(long, value_name = "displayName")]
        name: Option<String>,
        /// Comma-separated capability: source, destination (derives permissions)
        #[arg(long, value_name = "role", value_parser = ROLES)]
        role: Option<String>,
        #[arg(
            long,
            value_name = "permissions",
            help = "Comma-separated granular permissions. Overrides --role.",
            value_parser = PERMISSIONS
        )]
        permissions: Option<String>,
        /// Replace credentials from a JSON file
        #[arg(long, value_name = "path")]
        credentials_file: Option<String>,
        /// Replace config from a JSON file
        #[arg(long, value_name = "path")]
        config_file: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SourcesCommand {
    /// List all data sources
    #[command(
        after_help = "Examples:\n  $ vendo sources list\n  $ vendo sources list --state active\n  $ vendo sources list --output id"
    )]
    List {
        /// Filter by state (active, inactive)
        #[arg(long, value_name = "state", value_parser = STATES)]
        state: Option<String>,
        /// Filter by sync type
        #[arg(long = "type", value_name = "type")]
        sync_type: Option<String>,
        /// Filter by app ID
        #[arg(long, value_name = "appId")]
        app: Option<String>,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Get source details
    #[command(after_help = "Examples:\n  $ vendo sources get <sourceId>\n  $ vendo sources get <sourceId> --json")]
    Get {
        #[arg(value_name = "sourceId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Trigger a manual sync
    #[command(
        after_help = "Examples:\n  $ vendo sources sync <sourceId>\n  $ vendo sources sync <sourceId> --watch\n  $ vendo sources sync <sourceId> --dry-run"
    )]
    Sync {
        #[arg(value_name = "sourceId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Watch the job until it completes
        #[arg(long)]
        watch: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id for job ID)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Pause a data source
    #[command(
        after_help = "Examples:\n  $ vendo sources pause <sourceId>\n  $ vendo sources pause <sourceId> --dry-run"
    )]
    Pause {
        #[arg(value_name = "sourceId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Resume a paused data source
    #[command(
        after_help = "Examples:\n  $ vendo sources resume <sourceId>\n  $ vendo sources resume <sourceId> --dry-run"
    )]
    Resume {
        #[arg(value_name = "sourceId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Delete a data source (soft delete)
    #[command(
        after_help = "Examples:\n  $ vendo sources delete <sourceId>\n  $ vendo sources delete <sourceId> --yes\n  $ vendo sources delete <sourceId> --dry-run"
    )]
    Delete {
        #[arg(value_name = "sourceId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Create a new data source
    #[command(
        after_help = "Examples:\n  $ vendo sources create --app <appId> --sync-type google_ads\n  $ vendo sources create --app <appId> --sync-type shopify --import-tasks orders,customers --run-now"
    )]
    Create {
        /// App ID (must have source role)
        #[arg(long, value_name = "appId", required = true)]
        app: String,
        /// Connector type (e.g. google_ads)
        #[arg(long, value_name = "syncType", required = true)]
        sync_type: String,
        /// Comma-separated import task IDs (streams to pull)
        #[arg(long, value_name = "tasks")]
        import_tasks: Option<String>,
        /// Sync frequency value
        #[arg(long, value_name = "value", default_value = "24")]
        frequency: String,
        /// Sync frequency unit (hours, days)
        #[arg(long, value_name = "unit", default_value = "hours", value_parser = SOURCE_UNITS)]
        unit: String,
        /// Path to a JSON file with source-specific config
        #[arg(long, value_name = "path")]
        config_file: Option<String>,
        /// Trigger an initial sync immediately after creation
        #[arg(long)]
        run_now: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Update a data source
    #[command(
        after_help = "Examples:\n  $ vendo sources update <sourceId> --frequency 6 --unit hours\n  $ vendo sources update <sourceId> --import-tasks orders,customers"
    )]
    Update {
        #[arg(value_name = "sourceId")]
        id: String,
        /// Comma-separated import task IDs
        #[arg(long, value_name = "tasks")]
        import_tasks: Option<String>,
        /// Sync frequency value
        #[arg(long, value_name = "value")]
        frequency: Option<String>,
        /// Sync frequency unit (hours, days)
        #[arg(long, value_name = "unit", value_parser = SOURCE_UNITS)]
        unit: Option<String>,
        /// Replace config from a JSON file
        #[arg(long, value_name = "path")]
        config_file: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum IntegrationsCommand {
    /// List all destinations
    #[command(
        after_help = "Examples:\n  $ vendo destinations list\n  $ vendo destinations list --state active\n  $ vendo destinations list --output id"
    )]
    List {
        /// Filter by state (active, inactive)
        #[arg(long, value_name = "state", value_parser = STATES)]
        state: Option<String>,
        /// Filter by status
        #[arg(long, value_name = "status", value_parser = DESTINATION_STATUSES)]
        status: Option<String>,
        /// Filter by data type
        #[arg(long = "type", value_name = "type")]
        data_type: Option<String>,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Get destination details
    #[command(
        after_help = "Examples:\n  $ vendo destinations get <destinationId>\n  $ vendo destinations get <destinationId> --json"
    )]
    Get {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Trigger a manual sync
    #[command(
        after_help = "Examples:\n  $ vendo destinations sync <destinationId>\n  $ vendo destinations sync <destinationId> --watch\n  $ vendo destinations sync <destinationId> --dry-run"
    )]
    Sync {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Watch the job until it completes
        #[arg(long)]
        watch: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id for job ID)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Check source-data availability for a window and trigger top-up imports for missing ranges
    #[command(
        after_help = "Examples:\n  $ vendo destinations refresh-source <destinationId>\n  $ vendo destinations refresh-source <destinationId> --from 2026-06-29 --to 2026-07-02\n  $ vendo destinations refresh-source <destinationId> --json"
    )]
    RefreshSource {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Window start — ISO datetime or YYYY-MM-DD (default: 7 days before --to)
        #[arg(long, value_name = "date")]
        from: Option<String>,
        /// Window end — ISO datetime or YYYY-MM-DD (default: now)
        #[arg(long, value_name = "date")]
        to: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Pause a destination
    #[command(
        after_help = "Examples:\n  $ vendo destinations pause <destinationId>\n  $ vendo destinations pause <destinationId> --dry-run"
    )]
    Pause {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Resume a paused destination
    #[command(
        after_help = "Examples:\n  $ vendo destinations resume <destinationId>\n  $ vendo destinations resume <destinationId> --dry-run"
    )]
    Resume {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Delete a destination (soft delete)
    #[command(
        after_help = "Examples:\n  $ vendo destinations delete <destinationId>\n  $ vendo destinations delete <destinationId> --yes\n  $ vendo destinations delete <destinationId> --dry-run"
    )]
    Delete {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Create a new destination (source → destination pipeline)
    #[command(
        after_help = "Examples:\n  $ vendo destinations create --dest-app <onesignal-app-id> --data-type events --config-file tasks.json\n  $ vendo destinations create --source-app <bq-id> --dest-app <os-id> --data-type user_properties --config-file tasks.json --run-now"
    )]
    Create {
        /// Destination app ID (must have destination role)
        #[arg(long, value_name = "appId", required = true)]
        dest_app: String,
        /// Source app ID (optional for some data types)
        #[arg(long, value_name = "appId")]
        source_app: Option<String>,
        /// Data type (e.g. events, user_properties, conversions)
        #[arg(long, value_name = "type", required = true)]
        data_type: String,
        /// Path to JSON with { global?, tasks: [...] }
        #[arg(long, value_name = "path", required = true)]
        config_file: String,
        /// Path to JSON with schedule overrides
        #[arg(long, value_name = "path")]
        schedule_file: Option<String>,
        /// Sync frequency value
        #[arg(long, value_name = "value", default_value = "1")]
        frequency: String,
        /// Sync frequency unit (hours, days, weeks, months)
        #[arg(long, value_name = "unit", default_value = "days", value_parser = DESTINATION_UNITS)]
        unit: String,
        /// Trigger first sync immediately
        #[arg(long)]
        run_now: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Update a destination
    #[command(
        after_help = "Examples:\n  $ vendo destinations update <destinationId> --frequency 6 --unit hours\n  $ vendo destinations update <destinationId> --config-file new-tasks.json"
    )]
    Update {
        #[arg(value_name = "destinationId")]
        id: String,
        /// Replace config from a JSON file
        #[arg(long, value_name = "path")]
        config_file: Option<String>,
        /// Replace schedule from a JSON file
        #[arg(long, value_name = "path")]
        schedule_file: Option<String>,
        /// Sync frequency value
        #[arg(long, value_name = "value")]
        frequency: Option<String>,
        /// Sync frequency unit
        #[arg(long, value_name = "unit", value_parser = DESTINATION_UNITS)]
        unit: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum CatalogCommand {
    /// List the platforms ready to connect
    #[command(
        after_help = "Examples:\n  $ vendo catalog list\n  $ vendo catalog list --role source\n  $ vendo catalog list --output appType"
    )]
    List {
        /// Filter by category
        #[arg(long, value_name = "category", value_parser = CATEGORIES)]
        category: Option<String>,
        /// Filter by role (source, destination)
        #[arg(long, value_name = "role", value_parser = ROLES)]
        role: Option<String>,
        /// List every platform, including the ones on request
        #[arg(long)]
        all: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. appType)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Get details for a specific platform
    #[command(after_help = "Examples:\n  $ vendo catalog get shopify\n  $ vendo catalog get bigquery --json")]
    Get {
        #[arg(value_name = "appType")]
        app_type: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the credential fields required to create an app of this type
    // Hidden (VE-3827): `catalog get` shows the same fields. Not an alias of it: scripts may parse
    // this command's own output, which stays.
    #[command(
        hide = true,
        after_help = "Examples:\n  $ vendo catalog credential-schema onesignal\n  $ vendo catalog credential-schema bigquery --json"
    )]
    CredentialSchema {
        #[arg(value_name = "appType")]
        app_type: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum DictionaryCommand {
    /// List catalog definitions (events by default)
    #[command(
        after_help = "Examples:\n  $ vendo dictionary list\n  $ vendo dictionary list --type event --query checkout\n  $ vendo dictionary list --type prop -q email --json\n  $ vendo dictionary list --output subjectId"
    )]
    List {
        #[arg(
            long = "type",
            value_name = "type",
            default_value = "event",
            help = crate::dictionary::type_help(),
            value_parser = SUBJECT_TYPES
        )]
        subject_type: String,
        /// Text search across subject ID, name and description
        #[arg(short, long, value_name = "text")]
        query: Option<String>,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. subjectId, displayName)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Search one subject type by text in subject ID, name and description
    #[command(
        after_help = "Examples:\n  $ vendo dictionary search checkout\n  $ vendo dictionary search email --type prop --json"
    )]
    Search {
        #[arg(value_name = "query")]
        query: String,
        #[arg(
            long = "type",
            value_name = "type",
            default_value = "event",
            help = crate::dictionary::type_help(),
            value_parser = SUBJECT_TYPES
        )]
        subject_type: String,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. subjectId, displayName)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Look up one catalog definition by the subject ID from list or search, or an alias such as event:<name>
    #[command(
        after_help = "Examples:\n  $ vendo dictionary get <subjectId>\n  $ vendo dictionary get event:checkout_completed\n  $ vendo dictionary get event:checkout_completed --json"
    )]
    Get {
        #[arg(value_name = "subjectId")]
        subject_id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum MetricsCommand {
    /// List all custom metrics
    #[command(
        after_help = "Examples:\n  $ vendo metrics list\n  $ vendo metrics list --status active\n  $ vendo metrics list --output id"
    )]
    List {
        /// Filter by status (draft, active, archived)
        #[arg(long, value_name = "status", value_parser = METRIC_STATUSES)]
        status: Option<String>,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. id, name)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Get metric details
    #[command(after_help = "Examples:\n  $ vendo metrics get <metricId>\n  $ vendo metrics get <metricId> --json")]
    Get {
        #[arg(value_name = "metricId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Create a new metric
    #[command(
        after_help = "Examples:\n  $ vendo metrics create --name \"ROAS\" --definition roas.query.json\n  $ vendo metrics create --name \"Total Revenue\" --definition revenue.query.json --format currency\n  $ vendo metrics create --name \"CTR\" --definition ctr.query.json --format percentage"
    )]
    Create {
        /// Metric name
        #[arg(long, value_name = "name", required = true)]
        name: String,
        /// QuerySpec v2 definition JSON file
        #[arg(long, value_name = "file", required = true)]
        definition: String,
        /// Description
        #[arg(long, value_name = "desc")]
        description: Option<String>,
        /// Display format: number, currency, percentage, multiplier
        #[arg(long, value_name = "format", default_value = "number", value_parser = METRIC_FORMATS)]
        format: String,
        /// Unit suffix (e.g., "$", "%")
        #[arg(long, value_name = "unit")]
        unit: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Update a metric
    #[command(
        after_help = "Examples:\n  $ vendo metrics update <metricId> --name \"New Name\"\n  $ vendo metrics update <metricId> --status active\n  $ vendo metrics update <metricId> --definition revised.query.json"
    )]
    Update {
        #[arg(value_name = "metricId")]
        id: String,
        /// New name
        #[arg(long, value_name = "name")]
        name: Option<String>,
        /// New description
        #[arg(long, value_name = "desc")]
        description: Option<String>,
        /// New QuerySpec v2 definition JSON file
        #[arg(long, value_name = "file")]
        definition: Option<String>,
        /// New format
        #[arg(long, value_name = "format", value_parser = METRIC_FORMATS)]
        format: Option<String>,
        /// New unit
        #[arg(long, value_name = "unit")]
        unit: Option<String>,
        /// New status
        #[arg(long, value_name = "status", value_parser = METRIC_STATUSES)]
        status: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Activate a draft metric
    #[command(after_help = "Examples:\n  $ vendo metrics activate <metricId>")]
    Activate {
        #[arg(value_name = "metricId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a metric
    #[command(after_help = "Examples:\n  $ vendo metrics delete <metricId>\n  $ vendo metrics delete <metricId> -y")]
    Delete {
        #[arg(value_name = "metricId")]
        id: String,
        /// Skip confirmation
        #[arg(short, long)]
        yes: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ModelsCommand {
    /// List all data models
    #[command(
        after_help = "Examples:\n  $ vendo models list\n  $ vendo models list --valid\n  $ vendo models list --output id"
    )]
    List {
        /// Filter by data type
        #[arg(long = "type", value_name = "type")]
        data_type: Option<String>,
        /// Only show valid models
        #[arg(long)]
        valid: bool,
        /// Only show invalid models
        #[arg(long)]
        invalid: bool,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. id, name)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Get model details
    #[command(after_help = "Examples:\n  $ vendo models get <modelId>\n  $ vendo models get <modelId> --json")]
    Get {
        #[arg(value_name = "modelId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum MeasurementCommand {
    /// List measurement methodologies
    Methodologies {
        #[command(subcommand)]
        command: MethodologiesCommand,
    },
    /// Methodology segmentation rules
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// Cohort lifetime-value views
    Ltv {
        #[command(subcommand)]
        command: LtvCommand,
    },
    /// Measurement signal availability
    Signals {
        #[command(subcommand)]
        command: SignalsCommand,
    },
}

#[derive(Subcommand)]
pub enum MethodologiesCommand {
    /// List methodologies (system + account)
    #[command(
        after_help = "Examples:\n  $ vendo measurement methodologies list\n  $ vendo measurement methodologies list --no-system\n  $ vendo measurement methodologies list --json\n  $ vendo measurement methodologies list --output id"
    )]
    List {
        /// Exclude system-seeded methodologies
        #[arg(long = "no-system")]
        no_system: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print one field per row (e.g. id, name)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Show one methodology by ID
    #[command(
        after_help = "Examples:\n  $ vendo measurement methodologies get <methodologyId>\n  $ vendo measurement methodologies get <methodologyId> --json"
    )]
    Get {
        #[arg(value_name = "methodologyId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum RulesCommand {
    /// Preview which segmentation rule fires for recent rows
    #[command(
        after_help = "Examples:\n  $ vendo measurement rules preview --from 2025-01-01 --to 2025-01-31\n  $ vendo measurement rules preview --from 2025-01-01 --to 2025-01-31 --limit 100 --json"
    )]
    Preview {
        /// Inclusive ISO date (YYYY-MM-DD)
        #[arg(long, value_name = "date", required = true)]
        from: String,
        /// Inclusive ISO date (YYYY-MM-DD)
        #[arg(long, value_name = "date", required = true)]
        to: String,
        /// Max distinct contexts to return
        #[arg(long, value_name = "n", default_value = "50")]
        limit: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum LtvCommand {
    /// List cohort LTV rows
    #[command(
        after_help = "Examples:\n  $ vendo measurement ltv list\n  $ vendo measurement ltv list --granularity weekly --segment channel:meta\n  $ vendo measurement ltv list --from 2025-01-01 --to 2025-06-30 --no-predicted"
    )]
    List {
        /// daily | weekly | monthly
        #[arg(long, value_name = "value", default_value = "monthly", value_parser = GRANULARITIES)]
        granularity: String,
        /// Segment key (e.g. "all", "channel:meta")
        #[arg(long, value_name = "key", default_value = "all")]
        segment: String,
        /// Inclusive ISO cohort period (YYYY-MM-DD)
        #[arg(long, value_name = "period")]
        from: Option<String>,
        /// Inclusive ISO cohort period (YYYY-MM-DD)
        #[arg(long, value_name = "period")]
        to: Option<String>,
        /// Max rows
        #[arg(long, value_name = "n", default_value = "50")]
        limit: String,
        /// Skip the naive-decay prediction join
        #[arg(long = "no-predicted")]
        no_predicted: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print one field per row
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Show one cohort by period — retention matrix + cumulative LTV curve + prediction
    #[command(
        after_help = "Examples:\n  $ vendo measurement ltv cohort 2025-01-01\n  $ vendo measurement ltv cohort 2025-01-01 --granularity weekly --segment channel:meta\n  $ vendo measurement ltv cohort 2025-01-01 --json"
    )]
    Cohort {
        #[arg(value_name = "period")]
        period: String,
        /// daily | weekly | monthly
        #[arg(long, value_name = "value", default_value = "monthly", value_parser = GRANULARITIES)]
        granularity: String,
        /// Segment key
        #[arg(long, value_name = "key", default_value = "all")]
        segment: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Show one customer's cohort + realised LTV + revenue timeline
    #[command(
        after_help = "Examples:\n  $ vendo measurement ltv customer cust_abc123\n  $ vendo measurement ltv customer cust_abc123 --json"
    )]
    Customer {
        #[arg(value_name = "customerId")]
        customer_id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum SignalsCommand {
    /// Per-signal availability summary
    #[command(after_help = "Examples:\n  $ vendo measurement signals list\n  $ vendo measurement signals list --json")]
    List {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Click-path signal status — readiness, last-computed, recent SignalEstimates
    #[command(
        after_help = "Examples:\n  $ vendo measurement signals click-path\n  $ vendo measurement signals click-path --sample-limit 50 --json"
    )]
    ClickPath {
        /// Number of recent SignalEstimates to fetch (1..500)
        #[arg(long, value_name = "n")]
        sample_limit: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum JobsCommand {
    /// List sync jobs
    #[command(
        after_help = "Examples:\n  $ vendo jobs list\n  $ vendo jobs list --status running\n  $ vendo jobs list --source <sourceId>\n  $ vendo jobs list --output id"
    )]
    List {
        /// Filter by status (queued, pending, running, completed, warning, failed, canceled)
        #[arg(long, value_name = "status", value_parser = JOB_STATUSES)]
        status: Option<String>,
        /// Filter by job type (import, export, data_quality, attribution)
        #[arg(long = "type", value_name = "type", value_parser = JOB_TYPES)]
        job_type: Option<String>,
        /// Filter by source ID
        #[arg(long, value_name = "sourceId")]
        source: Option<String>,
        /// Filter by destination ID
        #[arg(long, value_name = "integrationId")]
        integration: Option<String>,
        /// Number of results
        #[arg(long, value_name = "n", default_value = "20")]
        limit: String,
        /// Pagination offset
        #[arg(long, value_name = "n", default_value = "0")]
        offset: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Print a single field per row (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Get job details
    #[command(after_help = "Examples:\n  $ vendo jobs get <jobId>\n  $ vendo jobs get <jobId> --json")]
    Get {
        #[arg(value_name = "jobId")]
        job_id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Cancel a pending or running job
    #[command(
        after_help = "Examples:\n  $ vendo jobs cancel <jobId>\n  $ vendo jobs cancel <jobId> --yes\n  $ vendo jobs cancel <jobId> --dry-run"
    )]
    Cancel {
        #[arg(value_name = "jobId")]
        job_id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
        /// Preview the action without executing
        #[arg(long)]
        dry_run: bool,
        /// Print a single field (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
    /// Watch running and pending jobs (live polling)
    #[command(
        after_help = "Examples:\n  $ vendo jobs watch\n  $ vendo jobs watch --source <sourceId>\n  $ vendo jobs watch --interval 10"
    )]
    Watch {
        /// Polling interval in seconds
        #[arg(long, value_name = "seconds", default_value = "5")]
        interval: String,
        /// Filter by source ID
        #[arg(long, value_name = "sourceId")]
        source: Option<String>,
        /// Filter by destination ID
        #[arg(long, value_name = "integrationId")]
        integration: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Tail a single job or the latest job for a source/destination
    #[command(
        after_help = "Examples:\n  $ vendo jobs tail <jobId>\n  $ vendo jobs tail --source <sourceId>\n  $ vendo jobs tail --source <sourceId> --next\n  $ vendo jobs tail --integration <integrationId>"
    )]
    Tail {
        #[arg(value_name = "jobId")]
        job_id: Option<String>,
        /// Tail the latest job for a source
        #[arg(long, value_name = "sourceId")]
        source: Option<String>,
        /// Tail the latest job for a destination
        #[arg(long, value_name = "integrationId")]
        integration: Option<String>,
        /// Wait for the next new job when tailing a source or destination
        #[arg(long)]
        next: bool,
        /// Polling interval in seconds
        #[arg(long, value_name = "seconds", default_value = "3")]
        interval: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

/// TAB suggestions for a flag whose values come from a fixed list (VE-3830). The completion
/// scripts offer the values, and parsing still takes any string and passes it on as before: the
/// API decides, so a list that falls behind it never blocks a value. clap learns the values only
/// while a script is generated ([`suggesting`]), so help screens and parse errors (a flag given no
/// value) read as they did.
#[derive(Clone)]
pub struct Suggest(&'static [&'static str]);

thread_local! {
    static SUGGESTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `generate` with every [`Suggest`] listing its values, as the completion scripts need them.
pub fn suggesting<T>(generate: impl FnOnce() -> T) -> T {
    SUGGESTING.set(true);
    let result = generate();
    SUGGESTING.set(false);
    result
}

impl clap::builder::TypedValueParser for Suggest {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<String, clap::Error> {
        clap::builder::StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        let values = self.0.iter().map(|value| clap::builder::PossibleValue::new(*value));
        SUGGESTING.get().then(|| Box::new(values) as Box<dyn Iterator<Item = _>>)
    }
}

// Each list names where its values come from: a vendo-web-v2 path is on its `staging` branch.

/// `apps`, `sources` and `destinations list --state`: the two states their list routes filter on
/// (`apps/web/app/api/v1/_lib/route-handlers/{apps,sources,integrations}/collection.ts`).
const STATES: Suggest = Suggest(&["active", "inactive"]);
/// `--role`: of `apps list`, the route's `capability` filter (`route-handlers/apps/collection.ts`); of `catalog
/// list`, its `role` filter (`apps/web/app/api/v1/catalog/route.ts`); of `apps create|update`, the roles the CLI
/// turns into permissions (`commands/apps.rs`).
const ROLES: Suggest = Suggest(&["source", "destination"]);
/// `apps create|update --permissions`: the keys of `GRANULAR_PERMISSIONS` (`apps/web/lib/vendo/constants.ts`),
/// which the apps routes take as `z.enum(granularPermissionKeys)`.
const PERMISSIONS: Suggest = Suggest(&[
    "performance_data",
    "read_leads",
    "read_organic_content",
    "send_conversions",
    "read_warehouse",
    "write_warehouse",
    "sync_audiences",
    "campaign_changes",
    "campaign_adjust_bids",
    "campaign_adjust_spend",
    "campaign_toggle",
    "google_tag_manager",
    "google_analytics",
]);
/// `sources create|update --unit`: `syncFrequencyUnit` (`route-handlers/sources/{collection,item}.ts`).
const SOURCE_UNITS: Suggest = Suggest(&["hours", "days"]);
/// `destinations create|update --unit`: `schedule.frequencyUnit` (`route-handlers/integrations/{collection,item}.ts`).
const DESTINATION_UNITS: Suggest = Suggest(&["hours", "days", "weeks", "months"]);
/// `destinations list --status`: `IntegrationStatus` (`apps/web/lib/vendo/integration-lifecycle/types.ts`), the
/// `app_status` values of `integrations.status` that `GET /api/v1/integrations` filters on, less `deleted`: the route
/// leaves deleted destinations out. (Its doc comment names only six.)
const DESTINATION_STATUSES: Suggest = Suggest(&[
    "pending",
    "running",
    "completed",
    "warning",
    "errored",
    "auth_expired",
    "failed_permanently",
    "paused",
    "draft",
]);
/// `catalog list --category`: `IntegrationCategory` (`apps/web/lib/vendo/integrations/types.ts`), the category of
/// every catalog entry `GET /api/v1/catalog` filters on.
const CATEGORIES: Suggest = Suggest(&[
    "advertising",
    "analytics",
    "crm",
    "mobile_attribution",
    "payment",
    "platforms",
    "messaging",
    "webinar",
    "data",
    "ai",
]);
/// `dictionary list|search --type`: the subject types the server accepts, as the help lists them.
const SUBJECT_TYPES: Suggest = Suggest(&crate::dictionary::SUBJECT_TYPES);
/// `metrics list|update --status`: `MetricStatusSchema` (`apps/web/app/api/metrics/route.ts`).
const METRIC_STATUSES: Suggest = Suggest(&["draft", "active", "archived"]);
/// `metrics create|update --format`: `custom_metrics.format` as `savedMetricFormatDefaults` reads it
/// (`apps/web/lib/vendo/catalog/format-defaults.ts`), the formats the help lists.
const METRIC_FORMATS: Suggest = Suggest(&["number", "currency", "percentage", "multiplier"]);
/// `measurement ltv list|cohort --granularity`: `VALID_GRANULARITIES` (`apps/web/app/api/measurement/ltv/route.ts`).
const GRANULARITIES: Suggest = Suggest(&["daily", "weekly", "monthly"]);
/// `jobs list --status`, which its help lists: the statuses `GET /api/v1/jobs` documents (`route-handlers/jobs/collection.ts`), which are
/// vendo-pipelines-v2's `JobStatus` (`app/models/job_status.py`). The API spells `canceled` with one l.
const JOB_STATUSES: Suggest = Suggest(&["queued", "pending", "running", "completed", "warning", "failed", "canceled"]);
/// `jobs list --type`, which its help lists: the job types `GET /api/v1/jobs` documents for `job_type` (`route-handlers/jobs/collection.ts`).
/// `jobs.job_type` allows more (internal DAG steps), which still pass.
const JOB_TYPES: Suggest = Suggest(&["import", "export", "data_quality", "attribution"]);
/// `login --env`: the instances its help names, as `ConfigStore::resolve_login_base_url` reads them.
const ENVIRONMENTS: Suggest = Suggest(&["staging", "prod"]);

/// The shells today's CLI supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl From<Shell> for clap_complete::Shell {
    fn from(shell: Shell) -> Self {
        match shell {
            Shell::Bash => clap_complete::Shell::Bash,
            Shell::Zsh => clap_complete::Shell::Zsh,
            Shell::Fish => clap_complete::Shell::Fish,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let cmd = command();
        let args = rewrite_hidden_paths(&cmd, os(args));
        let matches = cmd.try_get_matches_from(args)?;
        Cli::from_arg_matches(&matches)
    }

    fn switch_args(cli: Cli) -> (Option<String>, Option<String>) {
        match cli.command {
            Command::Profile { command: ProfileCommand::Switch { profile, account, .. } } => (profile, account),
            _ => panic!("not a switch"),
        }
    }

    #[test]
    fn the_global_profile_is_not_the_switch_positional() {
        for args in [
            &["vendo", "--profile", "beta", "profile", "switch"][..],
            &["vendo", "profile", "switch", "--profile", "beta"],
            &["vendo", "config", "use", "--profile", "beta"],
        ] {
            let cli = parse(args).unwrap();
            assert_eq!(cli.profile.as_deref(), Some("beta"), "{args:?}");
            assert_eq!(switch_args(cli), (None, None), "{args:?}");
        }
        let cli = parse(&["vendo", "--profile", "alpha", "profile", "switch", "--account", "acct-beta"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("alpha"));
        assert_eq!(switch_args(cli), (None, Some("acct-beta".into())));
        let cli = parse(&["vendo", "profile", "switch", "beta"]).unwrap();
        assert_eq!(cli.profile, None);
        assert_eq!(switch_args(cli), (Some("beta".into()), None));
    }

    #[test]
    fn repeated_flags_take_the_last_value() {
        let cli =
            parse(&["vendo", "--profile", "a", "jobs", "list", "--limit", "5", "--limit", "7", "--json", "--json"])
                .unwrap();
        let Command::Jobs { command: JobsCommand::List { limit, json, .. } } = cli.command else { panic!() };
        assert_eq!((limit.as_str(), json), ("7", true));
        assert_eq!(
            parse(&["vendo", "--profile", "a", "--profile", "b", "whoami"]).unwrap().profile.as_deref(),
            Some("b")
        );
        assert!(parse(&["vendo", "--debug", "whoami", "--debug"]).unwrap().debug);
    }

    #[test]
    fn option_values_may_start_with_a_hyphen() {
        let cli = parse(&["vendo", "sources", "update", "s1", "--frequency", "-5"]).unwrap();
        let Command::Sources { command: SourcesCommand::Update { frequency, .. } } = cli.command else { panic!() };
        assert_eq!(frequency.as_deref(), Some("-5"));
        let cli = parse(&["vendo", "apps", "update", "a1", "--name", "-Prod"]).unwrap();
        let Command::Apps { command: AppsCommand::Update { name, .. } } = cli.command else { panic!() };
        assert_eq!(name.as_deref(), Some("-Prod"));
        let cli = parse(&["vendo", "jobs", "list", "--offset", "-1"]).unwrap();
        let Command::Jobs { command: JobsCommand::List { offset, .. } } = cli.command else { panic!() };
        assert_eq!(offset, "-1");
        let cli = parse(&["vendo", "jobs", "watch", "--interval", "-1"]).unwrap();
        let Command::Jobs { command: JobsCommand::Watch { interval, .. } } = cli.command else { panic!() };
        assert_eq!(interval, "-1");
        assert_eq!(parse(&["vendo", "--profile", "-x", "whoami"]).unwrap().profile.as_deref(), Some("-x"));
    }

    #[test]
    fn positionals_still_reject_hyphen_values() {
        assert!(parse(&["vendo", "apps", "get", "-5"]).is_err());
        assert!(parse(&["vendo", "jobs", "tail", "-5"]).is_err());
    }

    #[test]
    fn version_anywhere_prints_the_bare_version() {
        for args in [
            &["vendo", "--version"][..],
            &["vendo", "-V"],
            &["vendo", "whoami", "--version"],
            &["vendo", "jobs", "list", "-V"],
            &["vendo", "--profile", "x", "status", "--json", "--version"],
            &["vendo", "self-update", "-V"],
            &["vendo", "update", "-V"],
        ] {
            assert_eq!(preprocess(os(args)), Invocation::Version, "{args:?}");
        }
    }

    #[test]
    fn version_is_a_value_where_an_option_takes_it() {
        // `update --version <v>` installs that version (an accepted difference), as `self-update`, its
        // hidden alias, does (VE-4109).
        for old in ["self-update", "update"] {
            let args = os(&["vendo", old, "--version", "0.3.0"]);
            assert_eq!(preprocess(args.clone()), Invocation::Run(args), "{old}");
            let Command::Update { install_version, .. } = parse(&["vendo", old, "--version", "0.3.0"]).unwrap().command
            else {
                panic!("{old}")
            };
            assert_eq!(install_version.as_deref(), Some("0.3.0"));
        }
        // `--profile --version` names a profile, as commander reads it.
        let args = os(&["vendo", "--profile", "--version", "whoami"]);
        assert_eq!(preprocess(args.clone()), Invocation::Run(args));
        // After `--` nothing is an option.
        let args = os(&["vendo", "apps", "get", "--", "--version"]);
        assert_eq!(preprocess(args.clone()), Invocation::Run(args));
    }

    #[test]
    fn version_is_also_a_command_that_takes_json() {
        // VE-3893: `vendo version` prints what `--version` prints, and takes --json like every command.
        let args = os(&["vendo", "version"]);
        assert_eq!(preprocess(args.clone()), Invocation::Run(args));
        assert!(matches!(parse(&["vendo", "version"]).unwrap().command, Command::Version { json: false }));
        let cli = parse(&["vendo", "--profile", "beta", "version", "--json", "--debug"]).unwrap();
        assert!(matches!(cli.command, Command::Version { json: true }) && cli.debug);
        assert!(parse(&["vendo", "version", "extra"]).is_err(), "it takes no argument");
        // `-V` and `--version` are still read before clap runs, after `version` too.
        assert_eq!(preprocess(os(&["vendo", "version", "--version"])), Invocation::Version);
        assert_eq!(preprocess(os(&["vendo", "version", "--json", "-V"])), Invocation::Version);
    }

    #[test]
    fn metrics_take_native_query_spec_files_and_no_legacy_authoring_flags() {
        // Port of the TS metrics-command test (VE-2637): --definition is required on create, and
        // the retired builder flags are gone.
        assert!(parse(&["vendo", "metrics", "create", "--name", "ROAS"]).is_err());
        let cli = parse(&["vendo", "metrics", "create", "--name", "ROAS", "--definition", "roas.query.json"]).unwrap();
        let Command::Metrics { command: MetricsCommand::Create { definition, format, .. } } = cli.command else {
            panic!()
        };
        assert_eq!((definition.as_str(), format.as_str()), ("roas.query.json", "number"));
        for flag in ["--type", "--formula"] {
            let create = ["vendo", "metrics", "create", "--name", "N", "--definition", "d.json", flag, "x"];
            assert!(parse(&create).is_err(), "create {flag}");
            assert!(parse(&["vendo", "metrics", "update", "m1", flag, "x"]).is_err(), "update {flag}");
        }
    }

    #[test]
    fn measurement_negated_flags_and_defaults_match_commander() {
        let cli = parse(&["vendo", "measurement", "methodologies", "list", "--no-system"]).unwrap();
        let Command::Measurement {
            command: MeasurementCommand::Methodologies { command: MethodologiesCommand::List { no_system, .. } },
        } = cli.command
        else {
            panic!()
        };
        assert!(no_system);
        let cli = parse(&["vendo", "measurement", "ltv", "list", "--no-predicted"]).unwrap();
        let Command::Measurement {
            command:
                MeasurementCommand::Ltv {
                    command: LtvCommand::List { granularity, segment, limit, no_predicted, from, to, .. },
                },
        } = cli.command
        else {
            panic!()
        };
        assert_eq!(
            (granularity.as_str(), segment.as_str(), limit.as_str(), no_predicted, from, to),
            ("monthly", "all", "50", true, None, None)
        );
        assert!(
            parse(&["vendo", "measurement", "rules", "preview", "--from", "2026-01-01"]).is_err(),
            "--to is required"
        );
        let cli = parse(&["vendo", "metrics", "delete", "m1", "-y"]).unwrap();
        let Command::Metrics { command: MetricsCommand::Delete { yes, json, .. } } = cli.command else { panic!() };
        assert!(yes && !json);
    }

    #[test]
    fn dictionary_lists_events_by_default_and_takes_a_short_query_flag() {
        // Port of the TS dictionary-command test.
        let cli = parse(&["vendo", "dictionary", "list", "-q", "email"]).unwrap();
        let Command::Dictionary { command: DictionaryCommand::List { subject_type, query, limit, offset, .. } } =
            cli.command
        else {
            panic!()
        };
        assert_eq!(
            (subject_type.as_str(), query.as_deref(), limit.as_str(), offset.as_str()),
            ("event", Some("email"), "20", "0")
        );
        let cli = parse(&["vendo", "dictionary", "search", "checkout", "--type", "prop"]).unwrap();
        let Command::Dictionary { command: DictionaryCommand::Search { query, subject_type, .. } } = cli.command else {
            panic!()
        };
        assert_eq!((query.as_str(), subject_type.as_str()), ("checkout", "prop"));
        assert!(
            parse(&["vendo", "dictionary", "search", "x", "-q", "y"]).is_err(),
            "search takes its query as an argument"
        );
        let cli = parse(&["vendo", "dictionary", "get", "event:checkout_completed", "--json"]).unwrap();
        let Command::Dictionary { command: DictionaryCommand::Get { subject_id, json } } = cli.command else {
            panic!()
        };
        assert_eq!((subject_id.as_str(), json), ("event:checkout_completed", true));
        assert!(parse(&["vendo", "specs", "list"]).is_err(), "specs is gone (VE-2545)");
    }

    #[test]
    fn the_type_help_names_every_subject_type_the_server_accepts() {
        let help = command().find_subcommand("dictionary").unwrap().find_subcommand("list").unwrap().clone();
        let arg = help.get_arguments().find(|a| a.get_id() == "subject_type").unwrap();
        assert_eq!(
            arg.get_help().map(|h| h.to_string()).as_deref(),
            Some("Filter by subject type (event, prop, group, column, metric, model, audience)")
        );
    }

    #[test]
    fn a_separator_before_the_command_is_dropped() {
        assert_eq!(preprocess(os(&["vendo", "--", "whoami"])), Invocation::Run(os(&["vendo", "whoami"])));
        assert_eq!(
            preprocess(os(&["vendo", "--debug", "--", "whoami", "--json"])),
            Invocation::Run(os(&["vendo", "--debug", "whoami", "--json"]))
        );
        let after = os(&["vendo", "apps", "get", "--", "-5"]);
        assert_eq!(preprocess(after.clone()), Invocation::Run(after));
        let Command::Apps { command: AppsCommand::Get { id, .. } } =
            parse(&["vendo", "apps", "get", "--", "-5"]).unwrap().command
        else {
            panic!()
        };
        assert_eq!(id, "-5");
    }

    #[test]
    fn every_visible_command_is_in_exactly_one_help_section() {
        // Built as clap parses with it, so clap's `help` command is among them: the root help lists it (VE-3893).
        let mut cmd = command();
        cmd.build();
        let mut listed: Vec<&str> = HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        listed.sort_unstable();
        let mut commands: Vec<&str> =
            cmd.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| c.get_name()).collect();
        commands.sort_unstable();
        // A command missing here is missing from `vendo --help`: add it to a section of HELP_SECTIONS.
        assert_eq!(listed, commands, "HELP_SECTIONS must name every visible command once");
        assert!(listed.contains(&"help") && listed.contains(&"version"), "{listed:?}");
        // The root help describes clap's `help` as clap does.
        let help = cmd.find_subcommand(CLAP_HELP.0).unwrap();
        assert_eq!(help.get_about().map(ToString::to_string).as_deref(), Some(CLAP_HELP.1));
    }

    /// Every visible command that runs, by its path, with its description, depth first in help order:
    /// what the root help lists (VE-4109).
    fn runnable(cmd: &clap::Command, path: &str, found: &mut Vec<(String, String)>) {
        for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
            let path = if path.is_empty() { sub.get_name().to_string() } else { format!("{path} {}", sub.get_name()) };
            if sub.has_subcommands() {
                runnable(sub, &path, found);
            } else {
                found.push((path, sub.get_about().map(ToString::to_string).unwrap_or_default()));
            }
        }
    }

    #[test]
    fn the_root_help_lists_every_visible_command_and_subcommand_in_its_section() {
        // VE-4109: each group's row, then every command under it by its full path, each with its description.
        let cmd = command();
        let help = cmd.clone().render_help().to_string();
        let lines: Vec<&str> = help.lines().collect();
        assert!(lines.iter().all(|line| line.chars().count() <= HELP_COLUMNS), "{help}");
        let mut listed: Vec<(String, String)> = Vec::new();
        let mut at = 0;
        for (title, names) in HELP_SECTIONS {
            at += lines[at..].iter().position(|line| *line == format!("{title}:")).expect(title) + 1;
            let end = at + lines[at..].iter().position(|line| line.is_empty()).unwrap();
            // Rows two or four spaces in; a description that wraps goes on in its column, further in.
            let mut rows: Vec<(usize, String, String)> = Vec::new();
            let mut columns = Vec::new();
            for line in &lines[at..end] {
                let indent = line.len() - line.trim_start().len();
                if indent > 4 {
                    columns.push(indent);
                    let row = rows.last_mut().unwrap();
                    row.2 = format!("{} {}", row.2, line.trim());
                    continue;
                }
                let (label, about) = line.trim_start().split_once("  ").unwrap();
                columns.push(line.len() - about.trim_start().len());
                rows.push((indent, label.to_string(), about.trim_start().to_string()));
            }
            // Descriptions, and the lines they wrap onto, start in one column.
            assert!(columns.iter().all(|column| *column == columns[0]), "{title}: {columns:?}");
            for (indent, label, about) in rows {
                if indent == 4 {
                    listed.push((label, about));
                    continue;
                }
                assert_eq!(indent, 2, "{label}");
                let name = label.to_lowercase();
                match cmd.find_subcommand(&name) {
                    Some(group) if group.has_subcommands() => {
                        assert!(names.contains(&name.as_str()), "{name} in {title}");
                        assert_eq!(label, capitalized(&name));
                        assert_eq!(Some(about), group.get_about().map(ToString::to_string));
                    }
                    Some(_) => listed.push((label, about)),
                    None => assert_eq!((label.as_str(), about.as_str()), CLAP_HELP),
                }
            }
            at = end;
        }
        // Every command that runs, in help order (HELP_SECTIONS, then each group's own order).
        let mut expected = Vec::new();
        let mut all = Vec::new();
        runnable(&cmd, "", &mut all);
        for name in HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter()) {
            expected
                .extend(all.iter().filter(|(path, _)| path == name || path.starts_with(&format!("{name} "))).cloned());
        }
        assert_eq!(listed, expected);
        assert!(listed.iter().any(|(path, _)| path == "measurement ltv cohort"), "nested groups list their commands");
        // "Account" comes first, "Getting started" and "Account" in one (VE-4109); `help` and `version` follow
        // `status`, in clap's words for -h and -V (VE-3893).
        let titles: Vec<&str> =
            lines.iter().copied().filter(|line| line.ends_with(':') && !line.starts_with(' ')).collect();
        assert_eq!(titles, ["Account:", "Data pipeline:", "Data catalog:", "Options:"]);
        let account = lines.iter().position(|line| *line == "Account:").unwrap();
        let rows: Vec<&str> = lines[account + 1..].iter().take_while(|line| !line.is_empty()).copied().collect();
        assert_eq!(
            rows[4..6],
            [
                "  help              Print this message or the help of the given subcommand(s)",
                "  version           Print version"
            ]
        );
        assert!(!lines.contains(&"Commands:"), "the sections replace the Commands: list");
        // Hidden commands and aliases stay out, `commands` among them since VE-4109.
        for hidden in ["commands", "config", "init", "whoami", "doctor", "integrations", "catalog credential-schema"] {
            assert!(!listed.iter().any(|(path, _)| path == hidden), "{hidden}");
            assert!(!help.contains(&format!("  {hidden} ")), "{hidden}");
        }
    }

    #[test]
    fn long_descriptions_wrap_in_their_column() {
        let rows = [
            HelpRow { indent: 2, label: "Group".into(), about: "Short".into() },
            HelpRow { indent: 4, label: "group leaf".into(), about: "word ".repeat(30).trim_end().into() },
            HelpRow { indent: 2, label: "x".into(), about: format!("{} end", "a".repeat(90)) },
        ];
        let lines = section_lines(&rows, &clap::builder::styling::Style::new());
        // The column: two spaces after the longest label, `    group leaf`.
        assert_eq!(lines[0], "  Group         Short");
        assert!(lines[1].starts_with("    group leaf  word word"), "{lines:?}");
        assert!(lines[1..4].iter().all(|line| line.chars().count() <= HELP_COLUMNS), "{lines:?}");
        assert!(lines[2].starts_with(&format!("{}word", " ".repeat(16))), "{lines:?}");
        // A word longer than the room stays whole.
        let long = lines.iter().position(|line| line.contains("aaaa")).unwrap();
        assert_eq!(lines[long], format!("  x{}{}", " ".repeat(13), "a".repeat(90)));
        assert_eq!(lines[long + 1], format!("{}end", " ".repeat(16)));
    }

    #[test]
    fn no_group_has_a_help_command_but_the_root() {
        fn check(cmd: &clap::Command, path: &str) {
            for sub in cmd.get_subcommands().filter(|sub| sub.has_subcommands()) {
                let path = format!("{path} {}", sub.get_name());
                assert!(sub.is_disable_help_subcommand_set(), "{path} lists a help command");
                check(sub, &path);
            }
        }
        let cmd = command();
        assert!(!cmd.is_disable_help_subcommand_set(), "`vendo help <command>` stays");
        check(&cmd, "vendo");
    }

    #[test]
    fn hidden_paths_run_their_target() {
        let rewrite = |args: &[&str]| -> Vec<String> {
            rewrite_hidden_paths(&command(), os(args)).iter().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        for (from, to) in [
            // Moved to a command in another group.
            (&["vendo", "profile", "current"][..], &["vendo", "workspace"][..]),
            (&["vendo", "profile", "current", "--json"], &["vendo", "workspace", "--json"]),
            (&["vendo", "config", "show", "--json"], &["vendo", "workspace", "--json"]),
            (&["vendo", "config", "reset"], &["vendo", "logout", "--all"]),
            (&["vendo", "config", "reset", "-y"], &["vendo", "logout", "--all", "-y"]),
            // Global options before and between the words stay.
            (&["vendo", "--profile", "beta", "profile", "current"], &["vendo", "--profile", "beta", "workspace"]),
            (&["vendo", "config", "--debug", "reset", "--yes"], &["vendo", "logout", "--all", "--debug", "--yes"]),
            (&["vendo", "--profile=beta", "config", "show"], &["vendo", "--profile=beta", "workspace"]),
            // A group's `help` command is the root's.
            (&["vendo", "apps", "help", "list"], &["vendo", "help", "apps", "list"]),
            (&["vendo", "apps", "help"], &["vendo", "help", "apps"]),
            (&["vendo", "measurement", "ltv", "help", "cohort"], &["vendo", "help", "measurement", "ltv", "cohort"]),
            (&["vendo", "help", "profile", "current"], &["vendo", "help", "workspace"]),
            (&["vendo", "help", "config", "reset"], &["vendo", "help", "logout"]),
            (&["vendo", "config", "help", "show"], &["vendo", "help", "workspace"]),
        ] {
            assert_eq!(rewrite(from), to, "{from:?}");
        }
        for unchanged in [
            // clap's hidden aliases of `workspace` (VE-3891), which clap reads as typed.
            &["vendo", "whoami"][..],
            &["vendo", "doctor", "--json"],
            &["vendo", "help"],
            &["vendo", "help", "apps", "list"],
            &["vendo", "config", "set", "--account", "a"],
            &["vendo", "profile"],
            &["vendo", "profile", "show"],
            &["vendo", "config", "current"],
            // `help` as a command's argument, after an option, or after `--`.
            &["vendo", "dictionary", "search", "help"],
            &["vendo", "apps", "get", "help"],
            &["vendo", "apps", "--help", "help"],
            &["vendo", "profile", "--", "current"],
        ] {
            assert_eq!(rewrite(unchanged), unchanged, "{unchanged:?}");
        }
    }

    #[test]
    fn a_group_run_without_its_command_is_found_with_where_its_command_goes() {
        // VE-3826: the menu's group and title, and where the chosen command goes in the arguments.
        let cmd = command();
        let bare = |args: &[&str]| {
            bare_group(&cmd, &os(args)).map(|(group, title, at)| (group.get_name().to_string(), title, at))
        };
        for (args, group, title, at) in [
            (&["vendo", "apps"][..], "apps", "vendo apps", 2),
            (&["vendo", "--profile", "beta", "apps"], "apps", "vendo apps", 4),
            (&["vendo", "--profile=beta", "--debug", "apps"], "apps", "vendo apps", 4),
            (&["vendo", "apps", "--debug"], "apps", "vendo apps", 2),
            (&["vendo", "apps", "--profile", "beta"], "apps", "vendo apps", 2),
            (&["vendo", "measurement"], "measurement", "vendo measurement", 2),
            (&["vendo", "measurement", "--debug", "ltv"], "ltv", "vendo measurement ltv", 4),
            // Old names open the group they name, titled as the tree names it.
            (&["vendo", "config"], "profile", "vendo profile", 2),
            (&["vendo", "int"], "destinations", "vendo destinations", 2),
        ] {
            assert_eq!(bare(args), Some((group.to_string(), title.to_string(), at)), "{args:?}");
        }
        for args in [
            // The root prints the help as before.
            &["vendo"][..],
            &["vendo", "--debug"],
            // A command, or no command.
            &["vendo", "apps", "list"],
            &["vendo", "apps", "get"],
            &["vendo", "bogus"],
            &["vendo", "apps", "bogus"],
            // Anything but a global option.
            &["vendo", "apps", "--json"],
            &["vendo", "apps", "-h"],
            &["vendo", "apps", "--"],
            &["vendo", "apps", "--profile"],
            &["vendo", "help", "apps"],
            &["vendo", "apps", "help"],
        ] {
            assert_eq!(bare(args), None, "{args:?}");
        }
        // The chosen command where it goes parses as if typed, the global options with it.
        let (_, _, at) = bare_group(&cmd, &os(&["vendo", "apps", "--debug"])).unwrap();
        let mut args = vec!["vendo", "apps", "--debug"];
        args.insert(at, "list");
        let cli = parse(&args).unwrap();
        assert!(cli.debug && matches!(cli.command, Command::Apps { command: AppsCommand::List { .. } }));
    }

    #[test]
    fn a_profile_flag_typed_last_with_no_name_is_found_with_its_title() {
        // VE-3892: the list's title, and whether `--profile` stands alone.
        let cmd = command();
        // The error clap gives for the words [`parse`] hands it, the hidden paths rewritten.
        let without_name = |args: &[&str]| {
            let err = cmd.clone().try_get_matches_from(rewrite_hidden_paths(&cmd, os(args))).err()?;
            profile_without_name(&cmd, &os(args), &err)
        };
        for (args, title, alone) in [
            (&["vendo", "--profile"][..], "vendo --profile", true),
            (&["vendo", "--debug", "--profile"], "vendo --profile", true),
            (&["vendo", "--profile", "beta", "--profile"], "vendo --profile", true),
            (&["vendo", "apps", "list", "--profile"], "vendo apps list --profile", false),
            (&["vendo", "apps", "list", "--json", "--profile"], "vendo apps list --profile", false),
            (&["vendo", "--debug", "apps", "list", "--profile"], "vendo apps list --profile", false),
            (&["vendo", "apps", "--profile"], "vendo apps --profile", false),
            (&["vendo", "apps", "get", "--profile"], "vendo apps get --profile", false),
            (&["vendo", "dictionary", "search", "orders", "--profile"], "vendo dictionary search --profile", false),
            // Old names, titled as the tree names the command they run.
            (&["vendo", "int", "list", "--profile"], "vendo destinations list --profile", false),
            (&["vendo", "config", "use", "--profile"], "vendo profile switch --profile", false),
            (&["vendo", "profile", "current", "--profile"], "vendo workspace --profile", false),
            (&["vendo", "whoami", "--profile"], "vendo workspace --profile", false),
        ] {
            assert_eq!(without_name(args), Some((title.to_string(), alone)), "{args:?}");
        }
        for args in [
            // A name, `--` and a word starting with `-` too: clap takes the word after `--profile`.
            &["vendo", "--profile", "beta", "apps", "list"][..],
            &["vendo", "apps", "list", "--profile", "--"],
            &["vendo", "apps", "list", "--profile", "--json"],
            &["vendo", "--profile", "--"],
            // Other errors.
            &["vendo", "help", "--profile"],
            &["vendo", "apps", "bogus", "--profile"],
            &["vendo", "--profile", "apps"],
            &["vendo", "completions", "bogus", "--profile"],
            &["vendo", "apps", "list", "--limit"],
        ] {
            assert_eq!(without_name(args), None, "{args:?}");
        }
    }

    #[test]
    fn the_chosen_profile_goes_where_it_runs_as_typed() {
        // VE-3892: on its own, `profile switch <name>` after the global options; after a command,
        // `--profile=<name>` in place of `--profile`.
        let words = |args: Vec<OsString>| args.into_iter().map(|arg| arg.into_string().unwrap()).collect::<Vec<_>>();
        assert_eq!(
            words(with_profile(&os(&["vendo", "--profile"]), true, "beta")),
            ["vendo", "profile", "switch", "beta"]
        );
        assert_eq!(
            words(with_profile(&os(&["vendo", "--debug", "--profile"]), true, "-x")),
            ["vendo", "--debug", "profile", "switch", "--", "-x"]
        );
        assert_eq!(
            words(with_profile(&os(&["vendo", "apps", "list", "--json", "--profile"]), false, "-x")),
            ["vendo", "apps", "list", "--json", "--profile=-x"]
        );
        // Each parses as if typed.
        let cli = parse(&["vendo", "--debug", "profile", "switch", "--", "-x"]).unwrap();
        let Command::Profile { command: ProfileCommand::Switch { profile, .. } } = cli.command else { panic!() };
        assert_eq!((cli.debug, cli.profile, profile.as_deref()), (true, None, Some("-x")));
        let cli = parse(&["vendo", "apps", "list", "--json", "--profile=-x"]).unwrap();
        assert!(cli.profile.as_deref() == Some("-x") && matches!(cli.command, Command::Apps { .. }));
    }

    #[test]
    fn the_config_commands_parse_as_their_profile_commands() {
        let cli = parse(&["vendo", "config", "set", "--account", "acct-x"]).unwrap();
        let Command::Profile { command: ProfileCommand::Set { account, api_key, base_url, .. } } = cli.command else {
            panic!()
        };
        assert_eq!((account.as_deref(), api_key, base_url), (Some("acct-x"), None, None));
        assert!(matches!(
            parse(&["vendo", "config", "list"]).unwrap().command,
            Command::Profile { command: ProfileCommand::List { json: false } }
        ));
        assert_eq!(switch_args(parse(&["vendo", "config", "use", "beta"]).unwrap()), (Some("beta".into()), None));
        let Command::Workspace { json, .. } = parse(&["vendo", "profile", "current", "--json"]).unwrap().command else {
            panic!()
        };
        assert!(json);
        assert!(matches!(parse(&["vendo", "config", "show"]).unwrap().command, Command::Workspace { json: false, .. }));
        // whoami and doctor are workspace's hidden aliases (VE-3891).
        assert!(matches!(
            parse(&["vendo", "whoami", "--json"]).unwrap().command,
            Command::Workspace { json: true, .. }
        ));
        assert!(matches!(parse(&["vendo", "doctor"]).unwrap().command, Command::Workspace { json: false, .. }));
        // Only `doctor` keeps doctor's exit code (VE-3891 exit codes), however the globals sit.
        let doctor = |args: &[&str]| {
            let cmd = command();
            let typed = rewrite_hidden_paths(&cmd, os(args));
            let mut parsed = Cli::from_arg_matches(&cmd.clone().try_get_matches_from(typed.clone()).unwrap()).unwrap();
            typed_as_doctor(&cmd, &typed, &mut parsed.command);
            let Command::Workspace { doctor, .. } = parsed.command else { panic!("{args:?}") };
            doctor
        };
        for args in [
            &["vendo", "doctor"][..],
            &["vendo", "--debug", "doctor", "--json"],
            &["vendo", "--profile", "doctor", "doctor"],
        ] {
            assert!(doctor(args), "{args:?}");
        }
        for args in [
            &["vendo", "workspace"][..],
            &["vendo", "whoami"],
            &["vendo", "profile", "current"],
            &["vendo", "config", "show"],
            &["vendo", "--profile", "doctor", "workspace"],
        ] {
            assert!(!doctor(args), "{args:?}");
        }
        let Command::Logout { all, yes, .. } = parse(&["vendo", "config", "reset", "--yes"]).unwrap().command else {
            panic!()
        };
        assert!(all && yes);
        assert!(parse(&["vendo", "profile", "current", "--all"]).is_err(), "workspace's flags only");
        let Command::Catalog { command: CatalogCommand::CredentialSchema { app_type, json } } =
            parse(&["vendo", "catalog", "credential-schema", "shopify", "--json"]).unwrap().command
        else {
            panic!()
        };
        assert_eq!((app_type.as_str(), json), ("shopify", true));
    }

    /// Every argument that lists values to clap: `(path and flag, values)`. A switch (`--json`)
    /// takes no value, so its parser's `true`/`false` are not offered.
    fn offered_values(cmd: &clap::Command, path: &str, found: &mut Vec<(String, String)>) {
        for arg in cmd.get_arguments().filter(|arg| arg.get_action().takes_values()) {
            let values: Vec<String> = arg.get_possible_values().iter().map(|v| v.get_name().to_string()).collect();
            if !values.is_empty() {
                let name = arg.get_long().map_or_else(|| format!("<{}>", arg.get_id()), |long| format!("--{long}"));
                found.push((format!("{path} {name}"), values.join(" ")));
            }
        }
        for sub in cmd.get_subcommands() {
            offered_values(sub, &format!("{path} {}", sub.get_name()), found);
        }
    }

    #[test]
    fn fixed_list_flags_offer_their_values_on_tab() {
        // VE-3830: the lists, with where each comes from, are the `Suggest` constants.
        let mut found = Vec::new();
        suggesting(|| offered_values(&command(), "vendo", &mut found));
        let suggested = |flag: &str, list: Suggest| (format!("vendo {flag}"), list.0.join(" "));
        let expected = vec![
            suggested("login --env", ENVIRONMENTS),
            suggested("apps list --state", STATES),
            suggested("apps list --role", ROLES),
            suggested("apps create --role", ROLES),
            suggested("apps create --permissions", PERMISSIONS),
            suggested("apps update --role", ROLES),
            suggested("apps update --permissions", PERMISSIONS),
            suggested("sources list --state", STATES),
            suggested("sources create --unit", SOURCE_UNITS),
            suggested("sources update --unit", SOURCE_UNITS),
            suggested("destinations list --state", STATES),
            suggested("destinations list --status", DESTINATION_STATUSES),
            suggested("destinations create --unit", DESTINATION_UNITS),
            suggested("destinations update --unit", DESTINATION_UNITS),
            suggested("jobs list --status", JOB_STATUSES),
            suggested("jobs list --type", JOB_TYPES),
            suggested("catalog list --category", CATEGORIES),
            suggested("catalog list --role", ROLES),
            suggested("dictionary list --type", SUBJECT_TYPES),
            suggested("dictionary search --type", SUBJECT_TYPES),
            suggested("metrics list --status", METRIC_STATUSES),
            suggested("metrics create --format", METRIC_FORMATS),
            suggested("metrics update --format", METRIC_FORMATS),
            suggested("metrics update --status", METRIC_STATUSES),
            suggested("measurement ltv list --granularity", GRANULARITIES),
            suggested("measurement ltv cohort --granularity", GRANULARITIES),
            // The one list clap always knew, which the help shows.
            ("vendo completions <shell>".to_string(), "bash zsh fish".to_string()),
        ];
        assert_eq!(found, expected);
    }

    #[test]
    fn clap_learns_the_suggested_values_only_for_the_completion_scripts() {
        // Outside `suggesting`, no `Suggest` flag lists values, so the help is as it was (the help snapshots
        // check it) and a flag given no value fails as it did before VE-3830, without "[possible values: …]".
        let mut found = Vec::new();
        offered_values(&command(), "vendo", &mut found);
        assert_eq!(found, [("vendo completions <shell>".to_string(), "bash zsh fish".to_string())]);
        for args in [&["vendo", "jobs", "list", "--status"][..], &["vendo", "apps", "create", "--permissions"]] {
            let error = parse(args).err().unwrap().to_string();
            assert!(error.starts_with("error: a value is required for '--"), "{error}");
            assert!(!error.contains("possible values"), "{error}");
        }
    }

    #[test]
    fn jobs_help_names_the_values_tab_offers() {
        // The API's values, as TAB offers them (Yalcin, 2026-10-06).
        let command = command();
        let list = command.find_subcommand("jobs").unwrap().find_subcommand("list").unwrap();
        let help = |flag: &str| {
            let arg = list.get_arguments().find(|arg| arg.get_long() == Some(flag)).unwrap();
            arg.get_help().unwrap().to_string()
        };
        assert_eq!(help("status"), format!("Filter by status ({})", JOB_STATUSES.0.join(", ")));
        assert_eq!(help("type"), format!("Filter by job type ({})", JOB_TYPES.0.join(", ")));
    }

    #[test]
    fn values_off_a_suggested_list_still_pass() {
        // TAB suggests; the API decides, as before VE-3830.
        let Command::Jobs { command: JobsCommand::List { status, .. } } =
            parse(&["vendo", "jobs", "list", "--status", "running,queued"]).unwrap().command
        else {
            panic!()
        };
        assert_eq!(status.as_deref(), Some("running,queued"));
        let Command::Dictionary { command: DictionaryCommand::List { subject_type, .. } } =
            parse(&["vendo", "dictionary", "list", "--type", "table"]).unwrap().command
        else {
            panic!()
        };
        assert_eq!(subject_type, "table");
        let create = ["vendo", "apps", "create", "--type", "x", "--name", "n", "--role", "source,destination"];
        let Command::Apps { command: AppsCommand::Create { role, permissions, .. } } =
            parse(&[&create[..], &["--permissions", "performance_data,new_one"]].concat()).unwrap().command
        else {
            panic!()
        };
        assert_eq!((role.as_str(), permissions.as_deref()), ("source,destination", Some("performance_data,new_one")));
        let Command::Login { env, .. } = parse(&["vendo", "login", "--env", "Staging"]).unwrap().command else {
            panic!()
        };
        assert_eq!(env.as_deref(), Some("Staging"));
        let Command::Completions { shell, .. } = parse(&["vendo", "completions"]).unwrap().command else { panic!() };
        assert_eq!(shell, None);
        assert!(parse(&["vendo", "completions", "tcsh"]).is_err(), "the shell is still checked");
    }
}

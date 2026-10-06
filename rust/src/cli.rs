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
/// by moving its name.
pub const HELP_SECTIONS: [(&str, &[&str]); 4] = [
    ("Getting started", &["login", "logout", "whoami", "status", "doctor", "commands"]),
    ("Data pipeline", &["apps", "sources", "destinations", "jobs"]),
    ("Data catalog", &["catalog", "dictionary", "metrics", "models", "measurement"]),
    ("Account", &["profile", "mcp", "completions", "self-update"]),
];

/// Where the root help wraps a group's list of commands (clap's own width when it wraps).
const HELP_WIDTH: usize = 100;

/// The root help: [`HELP_SECTIONS`], each group followed by the commands under it, read from
/// the command tree so the list cannot drift from what the CLI runs. `vendo <command> --help`
/// keeps the flags and examples. Styled like clap's own lists; clap drops the styles without colour.
fn root_help_template(root: &clap::Command) -> String {
    let styles = root.get_styles();
    let (header, literal) = (styles.get_header(), styles.get_literal());
    let width = root.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| c.get_name().len()).max().unwrap_or(0);
    let indent = " ".repeat(2 + width + 2);
    let mut sections = String::new();
    for (title, names) in HELP_SECTIONS {
        sections.push_str(&format!("{header}{title}:{header:#}\n"));
        for command in names.iter().filter_map(|name| root.find_subcommand(name)) {
            let (name, pad) = (command.get_name(), width - command.get_name().len());
            let about = command.get_about().map(ToString::to_string).unwrap_or_default();
            sections.push_str(&format!("  {literal}{name}{literal:#}{:pad$}  {about}\n", ""));
            // The commands under a group, comma-separated, wrapped below its description.
            let paths = command_paths(command);
            let mut line = Vec::new();
            for (i, path) in paths.iter().enumerate() {
                let item = if i + 1 < paths.len() { format!("{path},") } else { path.clone() };
                let used = indent.len() + line.iter().map(|item: &String| item.len() + 1).sum::<usize>();
                if !line.is_empty() && used + item.len() > HELP_WIDTH {
                    sections.push_str(&format!("{indent}{}\n", styled(&line, literal)));
                    line.clear();
                }
                line.push(item);
            }
            if !line.is_empty() {
                sections.push_str(&format!("{indent}{}\n", styled(&line, literal)));
            }
        }
        sections.push('\n');
    }
    format!(
        "{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{sections}{header}Options:{header:#}\n{{options}}\n\nSee `vendo <command> --help` for flags and examples."
    )
}

/// `list,` `get` → the command names in `style`, the commas and spaces plain.
fn styled(items: &[String], style: &clap::builder::styling::Style) -> String {
    let item = |item: &String| match item.strip_suffix(',') {
        Some(path) => format!("{style}{path}{style:#},"),
        None => format!("{style}{item}{style:#}"),
    };
    items.iter().map(item).collect::<Vec<_>>().join(" ")
}

/// The visible commands under `group`, as typed after its name: `list`, `ltv cohort`.
fn command_paths(group: &clap::Command) -> Vec<String> {
    let mut paths = Vec::new();
    for command in group.get_subcommands().filter(|c| !c.is_hide_set()) {
        let name = command.get_name();
        if command.has_subcommands() {
            paths.extend(command_paths(command).into_iter().map(|path| format!("{name} {path}")));
        } else {
            paths.push(name.to_string());
        }
    }
    paths
}

/// What [`parse`] read: the command, and whether it was given `--json` (VE-3831).
pub struct Parsed {
    pub cli: Cli,
    pub json: bool,
}

/// Parse with [`command`]; usage errors and `--help` exit like clap does, a usage error as
/// JSON when the words include `--json` ([`exit_with`]). A group run without its command opens
/// its menu on a terminal, and the command chosen there is parsed as if typed ([`chosen_command`]).
pub fn parse(mut args: Vec<OsString>) -> Parsed {
    loop {
        let cmd = command();
        let typed = rewrite_hidden_paths(&cmd, args.clone());
        let json_word = json_word(&typed);
        let err = match cmd.clone().try_get_matches_from(typed) {
            Ok(matches) => {
                let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|err| exit_with(err, json_word));
                return Parsed { cli, json: json_flag(&matches) };
            }
            Err(err) => err,
        };
        match chosen_command(&cmd, &args, &err) {
            Some(chosen) => args = chosen,
            None => exit_with(err, json_word),
        }
    }
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
const MOVED: [(&str, &str, &[&str]); 3] =
    [("profile", "current", &["whoami"]), ("config", "show", &["whoami"]), ("config", "reset", &["logout", "--all"])];

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
/// of `--profile` or `self-update --version <v>` (which installs that version:
/// an accepted difference, commander printed the version). A `--` before the
/// command name is dropped, so `vendo -- whoami` runs `whoami`.
pub fn preprocess(args: Vec<OsString>) -> Invocation {
    let mut out = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    out.extend(iter.next());
    let mut command: Option<OsString> = None;
    while let Some(arg) = iter.next() {
        let in_self_update = command.as_ref().is_some_and(|c| c == "self-update");
        match arg.to_str() {
            Some("--") => {
                if command.is_some() {
                    out.push(arg);
                }
                out.extend(iter);
                break;
            }
            Some("-V") => return Invocation::Version,
            Some("--version") if !in_self_update => return Invocation::Version,
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
    /// Show the current authenticated account
    #[command(after_help = "Examples:\n  $ vendo whoami\n  $ vendo whoami --json")]
    Whoami {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
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
    /// Run local configuration and connectivity checks
    #[command(after_help = "Examples:\n  $ vendo doctor\n  $ vendo doctor --json")]
    Doctor {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// List every command, or with --json the command tree with arguments and flags
    // Read from this tree at runtime, so it lists what runs (VE-3831, `commands/tree.rs`).
    #[command(after_help = "Examples:\n  $ vendo commands\n  $ vendo commands --json")]
    Commands {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Update the Vendo CLI using the hosted installer
    #[command(after_help = "Examples:\n  $ vendo self-update\n  $ vendo self-update --version 0.3.0")]
    SelfUpdate {
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
    /// Set configuration values
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
        ] {
            assert_eq!(preprocess(os(args)), Invocation::Version, "{args:?}");
        }
    }

    #[test]
    fn version_is_a_value_where_an_option_takes_it() {
        // `self-update --version <v>` installs that version (an accepted difference).
        let args = os(&["vendo", "self-update", "--version", "0.3.0"]);
        assert_eq!(preprocess(args.clone()), Invocation::Run(args.clone()));
        let Command::SelfUpdate { install_version, .. } =
            parse(&["vendo", "self-update", "--version", "0.3.0"]).unwrap().command
        else {
            panic!()
        };
        assert_eq!(install_version.as_deref(), Some("0.3.0"));
        // `--profile --version` names a profile, as commander reads it.
        let args = os(&["vendo", "--profile", "--version", "whoami"]);
        assert_eq!(preprocess(args.clone()), Invocation::Run(args));
        // After `--` nothing is an option.
        let args = os(&["vendo", "apps", "get", "--", "--version"]);
        assert_eq!(preprocess(args.clone()), Invocation::Run(args));
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
        let cmd = command();
        let mut listed: Vec<&str> = HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        listed.sort_unstable();
        let mut commands: Vec<&str> =
            cmd.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| c.get_name()).collect();
        commands.sort_unstable();
        // A command missing here is missing from `vendo --help`: add it to a section of HELP_SECTIONS.
        assert_eq!(listed, commands, "HELP_SECTIONS must name every visible command once");
    }

    #[test]
    fn the_root_help_lists_every_visible_command_and_subcommand_in_its_section() {
        let cmd = command();
        let help = cmd.clone().render_help().to_string();
        let lines: Vec<&str> = help.lines().collect();
        let mut at = 0;
        for (title, names) in HELP_SECTIONS {
            at += lines[at..].iter().position(|line| *line == format!("{title}:")).expect(title) + 1;
            let end = at + lines[at..].iter().position(|line| line.is_empty()).unwrap();
            let section = &lines[at..end];
            for name in names {
                let command = cmd.find_subcommand(name).unwrap();
                // `  <name>  <about>`, then the commands under it, comma-separated, on indented lines.
                let row = section.iter().position(|line| line.split_whitespace().next() == Some(name)).expect(name);
                let about = command.get_about().unwrap().to_string();
                assert!(section[row].starts_with(&format!("  {name} ")) && section[row].ends_with(&about), "{name}");
                let below: Vec<&str> =
                    section[row + 1..].iter().take_while(|line| line.starts_with("     ")).copied().collect();
                let joined = below.join(" ");
                let listed: Vec<&str> = joined.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
                assert_eq!(listed, command_paths(command), "vendo {name}");
                assert!(below.iter().all(|line| line.len() <= HELP_WIDTH), "{name}: {below:?}");
            }
            at = end;
        }
        // The section rows are the visible commands, in order: nothing hidden, no clap `help` row.
        let options = lines.iter().position(|line| *line == "Options:").unwrap();
        let rows: Vec<&str> = lines[..options]
            .iter()
            .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let names: Vec<&str> = HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        assert_eq!(rows, names);
        assert!(!lines.contains(&"Commands:"), "the sections replace the Commands: list");
        // What the groups list leaves out: hidden commands and aliases.
        assert_eq!(command_paths(cmd.find_subcommand("catalog").unwrap()), ["list", "get"]);
        assert_eq!(command_paths(cmd.find_subcommand("profile").unwrap()), ["list", "switch", "set"]);
        assert_eq!(
            command_paths(cmd.find_subcommand("measurement").unwrap())[..3],
            ["methodologies list", "methodologies get", "rules preview"]
        );
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
            (&["vendo", "profile", "current"][..], &["vendo", "whoami"][..]),
            (&["vendo", "profile", "current", "--json"], &["vendo", "whoami", "--json"]),
            (&["vendo", "config", "show", "--json"], &["vendo", "whoami", "--json"]),
            (&["vendo", "config", "reset"], &["vendo", "logout", "--all"]),
            (&["vendo", "config", "reset", "-y"], &["vendo", "logout", "--all", "-y"]),
            // Global options before and between the words stay.
            (&["vendo", "--profile", "beta", "profile", "current"], &["vendo", "--profile", "beta", "whoami"]),
            (&["vendo", "config", "--debug", "reset", "--yes"], &["vendo", "logout", "--all", "--debug", "--yes"]),
            (&["vendo", "--profile=beta", "config", "show"], &["vendo", "--profile=beta", "whoami"]),
            // A group's `help` command is the root's.
            (&["vendo", "apps", "help", "list"], &["vendo", "help", "apps", "list"]),
            (&["vendo", "apps", "help"], &["vendo", "help", "apps"]),
            (&["vendo", "measurement", "ltv", "help", "cohort"], &["vendo", "help", "measurement", "ltv", "cohort"]),
            (&["vendo", "help", "profile", "current"], &["vendo", "help", "whoami"]),
            (&["vendo", "help", "config", "reset"], &["vendo", "help", "logout"]),
            (&["vendo", "config", "help", "show"], &["vendo", "help", "whoami"]),
        ] {
            assert_eq!(rewrite(from), to, "{from:?}");
        }
        for unchanged in [
            &["vendo", "whoami"][..],
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
        let Command::Whoami { json } = parse(&["vendo", "profile", "current", "--json"]).unwrap().command else {
            panic!()
        };
        assert!(json);
        assert!(matches!(parse(&["vendo", "config", "show"]).unwrap().command, Command::Whoami { json: false }));
        let Command::Logout { all, yes, .. } = parse(&["vendo", "config", "reset", "--yes"]).unwrap().command else {
            panic!()
        };
        assert!(all && yes);
        assert!(parse(&["vendo", "profile", "current", "--all"]).is_err(), "whoami's flags only");
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

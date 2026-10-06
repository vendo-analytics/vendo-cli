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
    /// Use a specific account profile
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
const HELP_SECTIONS: [(&str, &[&str]); 4] = [
    ("Getting started", &["login", "init", "logout", "whoami", "status", "doctor"]),
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

/// Parse with [`command`]; usage errors and `--help` exit like clap does.
pub fn parse(args: Vec<OsString>) -> Cli {
    let cmd = command();
    let args = rewrite_hidden_paths(&cmd, args);
    let matches = cmd.get_matches_from(args);
    Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit())
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
    // The command words, each naming a command of the group before it, and where they are.
    // Global options may sit between them.
    let (mut at, mut path, mut help) = (Vec::new(), Vec::new(), false);
    let mut group = Some(root);
    let mut i = 1;
    while let (Some(current), Some(word)) = (group, args.get(i).and_then(|arg| arg.to_str())) {
        match word {
            "--debug" => {}
            "--profile" => i += 1,
            _ if word.starts_with("--profile=") => {}
            _ if word.starts_with('-') => break,
            "help" if !help => {
                at.push(i);
                help = true;
            }
            _ => {
                at.push(i);
                path.push(word.to_string());
                group = current.find_subcommand(word).filter(|command| command.has_subcommands());
            }
        }
        i += 1;
    }
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
    #[command(
        after_help = "Examples:\n  $ vendo login\n  $ vendo login --env staging\n  $ vendo login --api-key vendo_sk_... --account <account-id>"
    )]
    Login {
        /// API key for headless/CI login (requires --account)
        #[arg(long, value_name = "key")]
        api_key: Option<String>,
        /// Account ID for headless/CI login (requires --api-key)
        #[arg(long, value_name = "id")]
        account: Option<String>,
        /// Target instance: "staging" or "prod" (default: VENDO_API_URL/profile, else prod)
        #[arg(long, value_name = "environment")]
        env: Option<String>,
        /// Explicit API base URL (overrides --env)
        #[arg(long, value_name = "url")]
        base_url: Option<String>,
    },
    /// Guide first-time Vendo CLI setup
    #[command(after_help = "Examples:\n  $ vendo init\n  $ vendo init --env staging")]
    Init {
        /// Target instance: "staging" or "prod" (default: VENDO_API_URL/profile, else prod)
        #[arg(long, value_name = "environment")]
        env: Option<String>,
        /// Explicit API base URL (overrides --env)
        #[arg(long, value_name = "url")]
        base_url: Option<String>,
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
        #[arg(value_name = "shell")]
        shell: Shell,
    },
    /// Run local configuration and connectivity checks
    #[command(after_help = "Examples:\n  $ vendo doctor\n  $ vendo doctor --json")]
    Doctor {
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
    },
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    /// List all configured profiles
    #[command(after_help = "Examples:\n  $ vendo profile list")]
    List,
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
        #[arg(long, value_name = "state")]
        state: Option<String>,
        /// Filter by app type
        #[arg(long = "type", value_name = "type")]
        app_type: Option<String>,
        /// Filter by capability (source, destination) — derived from permissions
        #[arg(long, value_name = "role")]
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
        #[arg(long, value_name = "role", default_value = "source")]
        role: String,
        // `help =`, not a doc comment: clap drops a doc comment's trailing period, and TS keeps it.
        #[arg(
            long,
            value_name = "permissions",
            help = "Comma-separated granular permissions (e.g. performance_data,send_conversions). Overrides --role."
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
        #[arg(long, value_name = "role")]
        role: Option<String>,
        #[arg(long, value_name = "permissions", help = "Comma-separated granular permissions. Overrides --role.")]
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
        #[arg(long, value_name = "state")]
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
        #[arg(long, value_name = "unit", default_value = "hours")]
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
        #[arg(long, value_name = "unit")]
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
        #[arg(long, value_name = "state")]
        state: Option<String>,
        /// Filter by status
        #[arg(long, value_name = "status")]
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
        #[arg(long, value_name = "unit", default_value = "days")]
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
        #[arg(long, value_name = "unit")]
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
    /// List all available platforms
    #[command(
        after_help = "Examples:\n  $ vendo catalog list\n  $ vendo catalog list --role source\n  $ vendo catalog list --output appType"
    )]
    List {
        /// Filter by category
        #[arg(long, value_name = "category")]
        category: Option<String>,
        /// Filter by role (source, destination)
        #[arg(long, value_name = "role")]
        role: Option<String>,
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
        #[arg(long = "type", value_name = "type", default_value = "event", help = crate::dictionary::type_help())]
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
        #[arg(long = "type", value_name = "type", default_value = "event", help = crate::dictionary::type_help())]
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
        #[arg(long, value_name = "status")]
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
        #[arg(long, value_name = "format", default_value = "number")]
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
        #[arg(long, value_name = "format")]
        format: Option<String>,
        /// New unit
        #[arg(long, value_name = "unit")]
        unit: Option<String>,
        /// New status
        #[arg(long, value_name = "status")]
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
        #[arg(long, value_name = "value", default_value = "monthly")]
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
        #[arg(long, value_name = "value", default_value = "monthly")]
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
        /// Filter by status (pending, running, completed, failed, cancelled)
        #[arg(long, value_name = "status")]
        status: Option<String>,
        /// Filter by job type (import, export)
        #[arg(long = "type", value_name = "type")]
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
    },
}

/// The shells today's CLI supports.
#[derive(Clone, Copy, ValueEnum)]
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
            Command::Profile { command: ProfileCommand::Switch { profile, account } } => (profile, account),
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
        let Command::SelfUpdate { install_version } =
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
    fn the_config_commands_parse_as_their_profile_commands() {
        let cli = parse(&["vendo", "config", "set", "--account", "acct-x"]).unwrap();
        let Command::Profile { command: ProfileCommand::Set { account, api_key, base_url } } = cli.command else {
            panic!()
        };
        assert_eq!((account.as_deref(), api_key, base_url), (Some("acct-x"), None, None));
        assert!(matches!(
            parse(&["vendo", "config", "list"]).unwrap().command,
            Command::Profile { command: ProfileCommand::List }
        ));
        assert_eq!(switch_args(parse(&["vendo", "config", "use", "beta"]).unwrap()), (Some("beta".into()), None));
        let Command::Whoami { json } = parse(&["vendo", "profile", "current", "--json"]).unwrap().command else {
            panic!()
        };
        assert!(json);
        assert!(matches!(parse(&["vendo", "config", "show"]).unwrap().command, Command::Whoami { json: false }));
        let Command::Logout { all, yes } = parse(&["vendo", "config", "reset", "--yes"]).unwrap().command else {
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
}

//! Command-line definition. Command names, descriptions, flags and examples
//! follow the TypeScript CLI (`src/cli.ts`, `src/commands/*`).

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "vendo", version, about = "Vendo CLI — manage your data pipeline from the terminal")]
pub struct Cli {
    /// Use a specific account profile
    #[arg(long, global = true, value_name = "name")]
    pub profile: Option<String>,
    /// Enable verbose request diagnostics
    #[arg(long, global = true)]
    pub debug: bool,
    #[command(subcommand)]
    pub command: Command,
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
    #[command(after_help = "Examples:\n  $ vendo logout\n  $ vendo logout --all")]
    Logout {
        /// Remove every saved profile, not just the active one
        #[arg(long)]
        all: bool,
    },
    /// Manage CLI configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Manage account profiles
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
    /// Monitor sync jobs
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
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
    Completions { shell: Shell },
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
pub enum ConfigCommand {
    /// Set configuration values
    #[command(
        after_help = "Examples:\n  $ vendo config set --api-key <key>\n  $ vendo config set --account <id>\n  $ vendo config set --api-key <key> --account <id>"
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
    /// Inspect low-level CLI configuration
    #[command(after_help = "Examples:\n  $ vendo config show")]
    Show,
    /// Alias for `vendo profile switch`
    #[command(after_help = "Examples:\n  $ vendo config use <profile>\n  $ vendo config use --account <accountId>")]
    Use {
        profile: Option<String>,
        /// Switch by account ID instead of profile name
        #[arg(long, value_name = "accountId")]
        account: Option<String>,
    },
    /// Alias for `vendo profile list`
    #[command(after_help = "Examples:\n  $ vendo config list")]
    List,
    /// Delete all CLI configuration
    #[command(after_help = "Examples:\n  $ vendo config reset\n  $ vendo config reset --yes")]
    Reset {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    /// List all configured profiles
    #[command(after_help = "Examples:\n  $ vendo profile list")]
    List,
    /// Show the current effective profile
    #[command(after_help = "Examples:\n  $ vendo profile current")]
    Current,
    /// Switch to a different profile
    #[command(
        after_help = "Examples:\n  $ vendo profile switch\n  $ vendo profile switch myprofile\n  $ vendo profile switch --account <accountId>"
    )]
    Switch {
        profile: Option<String>,
        /// Switch by account ID instead of profile name
        #[arg(long, value_name = "accountId")]
        account: Option<String>,
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
        /// Filter by integration ID
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
        /// Filter by integration ID
        #[arg(long, value_name = "integrationId")]
        integration: Option<String>,
    },
    /// Tail a single job or the latest job for a source/integration
    #[command(
        after_help = "Examples:\n  $ vendo jobs tail <jobId>\n  $ vendo jobs tail --source <sourceId>\n  $ vendo jobs tail --source <sourceId> --next\n  $ vendo jobs tail --integration <integrationId>"
    )]
    Tail {
        #[arg(value_name = "jobId")]
        job_id: Option<String>,
        /// Tail the latest job for a source
        #[arg(long, value_name = "sourceId")]
        source: Option<String>,
        /// Tail the latest job for an integration
        #[arg(long, value_name = "integrationId")]
        integration: Option<String>,
        /// Wait for the next new job when tailing a source or integration
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

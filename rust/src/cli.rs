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
    #[command(after_help = "Examples:\n  $ vendo login\n  $ vendo login --env staging\n  $ vendo login --api-key vendo_sk_... --account <account-id>")]
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
    #[command(after_help = "Examples:\n  $ vendo completions bash\n  $ vendo completions zsh\n  $ vendo completions fish")]
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
    #[command(after_help = "Examples:\n  $ vendo config set --api-key <key>\n  $ vendo config set --account <id>\n  $ vendo config set --api-key <key> --account <id>")]
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
    #[command(after_help = "Examples:\n  $ vendo profile switch\n  $ vendo profile switch myprofile\n  $ vendo profile switch --account <accountId>")]
    Switch {
        profile: Option<String>,
        /// Switch by account ID instead of profile name
        #[arg(long, value_name = "accountId")]
        account: Option<String>,
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

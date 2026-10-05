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
    /// Manage app connections
    Apps {
        #[command(subcommand)]
        command: AppsCommand,
    },
    /// Manage data sources
    Sources {
        #[command(subcommand)]
        command: SourcesCommand,
    },
    /// Manage data export integrations
    #[command(visible_alias = "int")]
    Integrations {
        #[command(subcommand)]
        command: IntegrationsCommand,
    },
    /// Monitor sync jobs
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Browse available integration types
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
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
pub enum AppsCommand {
    /// List all app connections
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
    /// Show app connections that need attention
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
    /// Pause an app connection
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
    /// Resume a paused app connection
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
    /// Delete an app connection (soft delete)
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
    /// Create a new app connection
    #[command(
        after_help = "Examples:\n  $ vendo apps create --type onesignal --name \"OneSignal Prod\" --role destination --credentials-file onesignal.json\n  $ vendo apps create --type bigquery --name \"Analytics BQ\" --role source --credentials-file bq-sa.json"
    )]
    Create {
        /// App type (e.g. google_ads, onesignal). See: vendo catalog list
        #[arg(long = "type", value_name = "appType", required = true)]
        app_type: String,
        /// Human-readable name for this connection
        #[arg(long, value_name = "displayName", required = true)]
        name: String,
        /// Comma-separated capability: source, destination (derives default permissions)
        #[arg(long, value_name = "role", default_value = "source")]
        role: String,
        /// Comma-separated granular permissions (e.g. performance_data,send_conversions). Overrides --role.
        #[arg(long, value_name = "permissions")]
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
    /// Update an app connection
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
        /// Comma-separated granular permissions. Overrides --role.
        #[arg(long, value_name = "permissions")]
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
        /// App connection ID (must have source role)
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
    /// List all integrations
    #[command(
        after_help = "Examples:\n  $ vendo integrations list\n  $ vendo int list --state active\n  $ vendo integrations list --output id"
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
    /// Get integration details
    #[command(
        after_help = "Examples:\n  $ vendo integrations get <integrationId>\n  $ vendo int get <integrationId> --json"
    )]
    Get {
        #[arg(value_name = "integrationId")]
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Trigger a manual sync
    #[command(
        after_help = "Examples:\n  $ vendo integrations sync <integrationId>\n  $ vendo int sync <integrationId> --watch\n  $ vendo integrations sync <integrationId> --dry-run"
    )]
    Sync {
        #[arg(value_name = "integrationId")]
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
        after_help = "Examples:\n  $ vendo integrations refresh-source <integrationId>\n  $ vendo int refresh-source <integrationId> --from 2026-06-29 --to 2026-07-02\n  $ vendo integrations refresh-source <integrationId> --json"
    )]
    RefreshSource {
        #[arg(value_name = "integrationId")]
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
    /// Pause an integration
    #[command(
        after_help = "Examples:\n  $ vendo integrations pause <integrationId>\n  $ vendo int pause <integrationId> --dry-run"
    )]
    Pause {
        #[arg(value_name = "integrationId")]
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
    /// Resume a paused integration
    #[command(
        after_help = "Examples:\n  $ vendo integrations resume <integrationId>\n  $ vendo integrations resume <integrationId> --dry-run"
    )]
    Resume {
        #[arg(value_name = "integrationId")]
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
    /// Delete an integration (soft delete)
    #[command(
        after_help = "Examples:\n  $ vendo integrations delete <integrationId>\n  $ vendo int delete <integrationId> --yes\n  $ vendo integrations delete <integrationId> --dry-run"
    )]
    Delete {
        #[arg(value_name = "integrationId")]
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
    /// Create a new integration (source → destination pipeline)
    #[command(
        after_help = "Examples:\n  $ vendo int create --dest-app <onesignal-app-id> --data-type events --config-file tasks.json\n  $ vendo int create --source-app <bq-id> --dest-app <os-id> --data-type user_properties --config-file tasks.json --run-now"
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
    /// Update an integration
    #[command(
        after_help = "Examples:\n  $ vendo int update <integrationId> --frequency 6 --unit hours\n  $ vendo int update <integrationId> --config-file new-tasks.json"
    )]
    Update {
        #[arg(value_name = "integrationId")]
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
    /// List all available integration types
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
    /// Get details for a specific integration type
    #[command(after_help = "Examples:\n  $ vendo catalog get shopify\n  $ vendo catalog get bigquery --json")]
    Get {
        #[arg(value_name = "appType")]
        app_type: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the credential fields required to create an app of this type
    #[command(
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

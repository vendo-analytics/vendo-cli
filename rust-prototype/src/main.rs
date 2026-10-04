//! PROTOTYPE — throwaway Rust port of a slice of the `vendo` CLI
//! (login, whoami, apps list, completions). See NOTES.md for the question it answers.

mod client;
mod config;
mod login;
mod output;
mod update_check;

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use clap::{CommandFactory, Parser, Subcommand};
use comfy_table::Color;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    client::Client,
    config::Config,
    login::{LoginArgs, Me},
    output::{bold, colored_cell, dim, green, status_color},
};

#[derive(Parser)]
#[command(
    name = "vendo",
    version,
    about = "Vendo CLI — manage your data pipeline from the terminal"
)]
struct Cli {
    /// Use a specific account profile
    #[arg(long, global = true, value_name = "name")]
    profile: Option<String>,
    /// Enable verbose request diagnostics
    #[arg(long, global = true)]
    debug: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with your Vendo account
    #[command(after_help = "Examples:\n  $ vendo login\n  $ vendo login --env staging\n  $ vendo login --api-key vendo_sk_... --account <account-id>")]
    Login(LoginArgs),
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
    /// Generate shell completion script
    #[command(after_help = "Examples:\n  $ vendo completions bash\n  $ vendo completions zsh\n  $ vendo completions fish")]
    Completions { shell: clap_complete::Shell },
}

#[derive(Subcommand)]
enum AppsCommand {
    /// List all app connections
    #[command(after_help = "Examples:\n  $ vendo apps list\n  $ vendo apps list --state active --json\n  $ vendo apps list --output id")]
    List {
        /// Filter by state (active, inactive)
        #[arg(long)]
        state: Option<String>,
        /// Filter by app type
        #[arg(long = "type", value_name = "type")]
        app_type: Option<String>,
        /// Filter by capability (source, destination)
        #[arg(long)]
        role: Option<String>,
        /// Number of results
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Pagination offset
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// Output raw JSON (the API response, verbatim)
        #[arg(long, conflicts_with = "output")]
        json: bool,
        /// Print a single field per row (e.g. id)
        #[arg(long, value_name = "field")]
        output: Option<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            output::print_error(&err);
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let debug = cli.debug
        || std::env::var("VENDO_DEBUG")
            .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
    let mut config = Config::load(cli.profile);

    match cli.command {
        Command::Login(args) => login::run(&mut config, args, debug).await,
        Command::Whoami { json } => whoami(&config, json, debug).await,
        Command::Apps { command: AppsCommand::List { state, app_type, role, limit, offset, json, output } } => {
            let client = client_from(&config, debug)?;
            let res = output::run_action(
                "Fetching apps...",
                client.get("/apps", &[
                    ("state", state),
                    ("app_type", app_type),
                    ("capability", role),
                    ("limit", Some(limit.to_string())),
                    ("offset", Some(offset.to_string())),
                ]),
            )
            .await?;
            apps_list(&res, json, output.as_deref())
        }
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "vendo", &mut std::io::stdout());
            Ok(())
        }
    }
}

fn client_from(config: &Config, debug: bool) -> Result<Client> {
    let effective = config.effective();
    let api_key = effective.api_key.ok_or_else(|| {
        anyhow!("No API key configured. Run `vendo login` or `vendo config set --api-key <key>` or set VENDO_API_KEY.")
    })?;
    Ok(Client::new(api_key, effective.base_url, effective.account_id, debug))
}

async fn whoami(config: &Config, json: bool, debug: bool) -> Result<()> {
    update_check::check(config.path()).await;
    let client = client_from(config, debug)?;
    let mut res = output::run_action("Checking identity...", client.get("/me", &[])).await?;
    let effective = config.effective();

    if json {
        let mut cfg = Map::new();
        if let Some(name) = &effective.selected_profile {
            cfg.insert("selectedProfile".into(), json!(name));
        }
        cfg.insert("apiKeySource".into(), json!(effective.api_key_source));
        cfg.insert("baseUrl".into(), json!(effective.base_url));
        cfg.insert("baseUrlSource".into(), json!(effective.base_url_source));
        if let Some(id) = &effective.account_id {
            cfg.insert("accountId".into(), json!(id));
        }
        cfg.insert("accountIdSource".into(), json!(effective.account_id_source));
        if let Value::Object(obj) = &mut res {
            obj.insert("config".into(), Value::Object(cfg));
        }
        output::print_json(&res);
        return Ok(());
    }

    let me: Me = serde_json::from_value(res["data"].take())?;
    let account = me.account_slug.clone().unwrap_or_else(|| me.account_id.clone());
    println!();
    println!("{}", bold(me.account_name.as_deref().unwrap_or(&account)));
    println!();
    println!("  Account:     {account}");
    println!("  Account ID:  {}", me.account_id);
    println!(
        "  Profile:     {}",
        effective.selected_profile.clone().unwrap_or_else(|| dim("none selected"))
    );
    println!("  Base URL:    {}", effective.base_url);
    println!("  API Key:     {}", dim(me.api_key_id.as_deref().unwrap_or("unknown")));
    if me.scopes.is_empty() {
        println!("  Scopes:      {}", dim("full access"));
    } else {
        println!("  Scopes:      {}", me.scopes.join(", "));
    }

    let overrides = effective.env_override_names();
    if !overrides.is_empty() {
        println!();
        println!("{}", dim(&format!("  Env overrides active: {}", overrides.join(", "))));
    }

    let profiles = config.profile_summaries();
    if profiles.len() > 1 {
        println!();
        println!("{}", bold("  Profiles"));
        for profile in profiles {
            let marker = if profile.active { green("*") } else { " ".into() };
            let mut label = vec![profile.name, profile.account_id.unwrap_or_else(|| "no account".into())];
            if profile.base_url != config::DEFAULT_BASE_URL {
                label.push(profile.base_url);
            }
            println!("    {marker} {}", label.join("  "));
        }
        println!();
        println!(
            "{}",
            dim("  Switch with `vendo profile switch`, target one command with `vendo --profile <name> ...`, or use `vendo config use` as a compatibility alias.")
        );
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppItem {
    id: String,
    app_type: String,
    display_name: String,
    #[serde(default)]
    permissions: Vec<String>,
    roles: Option<Vec<String>>,
    state: String,
    access_status: Option<String>,
    last_sync_at: Option<String>,
}

const SOURCE_PERMISSIONS: &[&str] = &["performance_data"];
const DESTINATION_PERMISSIONS: &[&str] = &[
    "send_conversions",
    "sync_audiences",
    "campaign_changes",
    "campaign_adjust_bids",
    "campaign_adjust_spend",
    "campaign_toggle",
];

fn apps_list(res: &Value, json: bool, field: Option<&str>) -> Result<()> {
    if json {
        output::print_json(res);
        return Ok(());
    }
    let rows = res["data"].as_array().cloned().unwrap_or_default();
    if let Some(field) = field {
        output::print_field(&rows, field);
        return Ok(());
    }

    let apps: Vec<AppItem> = serde_json::from_value(Value::Array(rows))?;
    let mut table = output::table(&["ID", "Name", "Type", "Role", "State", "Status", "Last Sync"]);
    for app in &apps {
        let role = match &app.roles {
            Some(roles) if roles.is_empty() => "—".to_string(),
            Some(roles) => roles.join(", "),
            // Older servers omit roles: derive from permissions.
            None => {
                let has = |set: &[&str]| app.permissions.iter().any(|p| set.contains(&p.as_str()));
                match (has(SOURCE_PERMISSIONS), has(DESTINATION_PERMISSIONS)) {
                    (true, true) => "source, destination".into(),
                    (false, true) => "destination".into(),
                    (true, false) => "source".into(),
                    (false, false) => "—".into(),
                }
            }
        };
        let status = match (app.state.as_str(), app.access_status.as_deref()) {
            ("inactive", _) => colored_cell("paused", Some(Color::DarkGrey)),
            (_, None) => colored_cell("not checked", Some(Color::DarkGrey)),
            (_, Some("auth_expired")) => colored_cell("reconnect required", Some(Color::Red)),
            (_, Some(status)) => colored_cell(status, status_color(status)),
        };
        table.add_row(vec![
            colored_cell(&output::short_id(&app.id), Some(Color::DarkGrey)),
            colored_cell(&app.display_name, None),
            colored_cell(&app.app_type, None),
            colored_cell(&role, None),
            colored_cell(&app.state, status_color(&app.state)),
            status,
            colored_cell(&output::time_ago(app.last_sync_at.as_deref()), app.last_sync_at.is_none().then_some(Color::DarkGrey)),
        ]);
    }
    println!("{table}");
    let total = res["meta"]["pagination"]["total"].as_u64().unwrap_or(apps.len() as u64);
    output::print_count(total, "app");
    Ok(())
}

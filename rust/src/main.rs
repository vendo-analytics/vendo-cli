//! The `vendo` CLI in Rust (Linear project "Vendo CLI in Rust"). Behaviour
//! matches the TypeScript CLI; `pnpm parity` compares the two on staging.

mod cli;
mod client;
mod commands;
mod config;
mod context;
mod identity;
mod output;
mod profile_display;
mod update_check;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::{
    cli::{Cli, Command, ConfigCommand, ProfileCommand},
    commands::{account, health, login},
    config::{ConfigStore, EnvVars, default_config_path},
    context::Ctx,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let debug = cli.debug
        || std::env::var("VENDO_DEBUG")
            .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
    let home = std::env::home_dir().unwrap_or_default();
    let ctx = Ctx {
        store: ConfigStore::new(default_config_path(&home), cli.profile.clone(), EnvVars::from_process()),
        home,
        debug,
    };

    match run(&ctx, cli.command).await {
        Ok(code) => code,
        Err(err) => {
            output::print_error(&output::format_error(&err));
            ExitCode::from(1)
        }
    }
}

async fn run(ctx: &Ctx, command: Command) -> anyhow::Result<ExitCode> {
    let ok = ExitCode::SUCCESS;
    match command {
        Command::Login { api_key, account, env, base_url } => {
            login::run(ctx, api_key, account, env, base_url).await.map(|_| ok)
        }
        Command::Init { env, base_url } => account::init(ctx, env, base_url).await.map(|_| ok),
        Command::Logout { all } => account::logout(ctx, all).map(|_| ok),
        Command::Config { command } => match command {
            ConfigCommand::Set { api_key, base_url, account } => {
                account::config_set(ctx, api_key, base_url, account).map(|_| ok)
            }
            ConfigCommand::Show => {
                account::config_show(ctx);
                Ok(ok)
            }
            ConfigCommand::Use { profile, account } => account::config_use(ctx, profile, account).map(|_| ok),
            ConfigCommand::List => {
                account::config_list(ctx);
                Ok(ok)
            }
            ConfigCommand::Reset { yes } => {
                account::config_reset(ctx, yes);
                Ok(ok)
            }
        },
        Command::Profile { command } => match command {
            ProfileCommand::List => {
                account::profile_list(ctx);
                Ok(ok)
            }
            ProfileCommand::Current => {
                account::profile_current(ctx);
                Ok(ok)
            }
            ProfileCommand::Switch { profile, account } => account::profile_switch(ctx, profile, account).map(|_| ok),
        },
        Command::Status { json } => health::status(ctx, json).await.map(|_| ok),
        Command::Whoami { json } => account::whoami(ctx, json).await.map(|_| ok),
        Command::Mcp { json, show_key } => {
            account::mcp(ctx, json, show_key);
            Ok(ok)
        }
        Command::Completions { shell } => {
            clap_complete::generate(
                clap_complete::Shell::from(shell),
                &mut Cli::command(),
                "vendo",
                &mut std::io::stdout(),
            );
            Ok(ok)
        }
        Command::Doctor { json } => health::doctor(ctx, json).await,
        Command::SelfUpdate { install_version } => health::self_update(ctx, install_version),
    }
}

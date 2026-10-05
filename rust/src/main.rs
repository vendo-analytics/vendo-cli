//! The `vendo` CLI in Rust (Linear project "Vendo CLI in Rust"). Behaviour
//! matches the TypeScript CLI; `pnpm parity` compares the two on staging.

mod cli;
mod client;
mod commands;
mod config;
mod context;
mod identity;
mod jobs;
mod output;
mod profile_display;
mod source_refresh;
mod update_check;
mod watch;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::{
    cli::{
        AppsCommand, CatalogCommand, Cli, Command, ConfigCommand, IntegrationsCommand, JobsCommand, ProfileCommand,
        SourcesCommand,
    },
    commands::{
        account, apps, catalog, health, integrations, jobs as jobs_cmd, login,
        pipeline_resource::{self as resource, ActionOpts},
        sources,
    },
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
        Command::Apps { command } => match command {
            AppsCommand::List { state, app_type, role, limit, offset, json, output } => {
                apps::list(ctx, apps::ListArgs { state, app_type, role, limit, offset, json, output }).await.map(|_| ok)
            }
            AppsCommand::Diagnose { json } => apps::diagnose(ctx, json).await.map(|_| ok),
            AppsCommand::Get { id, json } => apps::get(ctx, &id, json).await.map(|_| ok),
            AppsCommand::Pause { id, json, dry_run, output } => {
                resource::state_action(ctx, resource::APP, &id, "pause", ActionOpts { json, dry_run, output })
                    .await
                    .map(|_| ok)
            }
            AppsCommand::Resume { id, json, dry_run, output } => {
                resource::state_action(ctx, resource::APP, &id, "resume", ActionOpts { json, dry_run, output })
                    .await
                    .map(|_| ok)
            }
            AppsCommand::Delete { id, json, yes, dry_run, output } => {
                resource::delete(ctx, resource::APP, &id, yes, ActionOpts { json, dry_run, output }).await.map(|_| ok)
            }
            AppsCommand::Create { app_type, name, role, permissions, credentials_file, config_file, json, output } => {
                let args =
                    apps::CreateArgs { app_type, name, role, permissions, credentials_file, config_file, json, output };
                apps::create(ctx, args).await.map(|_| ok)
            }
            AppsCommand::Update { id, name, role, permissions, credentials_file, config_file, json, output } => {
                let args = apps::UpdateArgs { name, role, permissions, credentials_file, config_file, json, output };
                apps::update(ctx, &id, args).await.map(|_| ok)
            }
        },
        Command::Sources { command } => match command {
            SourcesCommand::List { state, sync_type, app, limit, offset, json, output } => {
                sources::list(ctx, sources::ListArgs { state, sync_type, app, limit, offset, json, output })
                    .await
                    .map(|_| ok)
            }
            SourcesCommand::Get { id, json } => sources::get(ctx, &id, json).await.map(|_| ok),
            SourcesCommand::Sync { id, json, watch, dry_run, output } => {
                let opts = ActionOpts { json, dry_run, output };
                resource::sync(ctx, resource::SOURCE, &id, watch, opts, sources::dry_run_fields).await.map(|_| ok)
            }
            SourcesCommand::Pause { id, json, dry_run, output } => {
                resource::state_action(ctx, resource::SOURCE, &id, "pause", ActionOpts { json, dry_run, output })
                    .await
                    .map(|_| ok)
            }
            SourcesCommand::Resume { id, json, dry_run, output } => {
                resource::state_action(ctx, resource::SOURCE, &id, "resume", ActionOpts { json, dry_run, output })
                    .await
                    .map(|_| ok)
            }
            SourcesCommand::Delete { id, json, yes, dry_run, output } => {
                resource::delete(ctx, resource::SOURCE, &id, yes, ActionOpts { json, dry_run, output })
                    .await
                    .map(|_| ok)
            }
            SourcesCommand::Create {
                app,
                sync_type,
                import_tasks,
                frequency,
                unit,
                config_file,
                run_now,
                json,
                output,
            } => {
                let args = sources::CreateArgs {
                    app,
                    sync_type,
                    import_tasks,
                    frequency,
                    unit,
                    config_file,
                    run_now,
                    json,
                    output,
                };
                sources::create(ctx, args).await.map(|_| ok)
            }
            SourcesCommand::Update { id, import_tasks, frequency, unit, config_file, json, output } => {
                let args = sources::UpdateArgs { import_tasks, frequency, unit, config_file, json, output };
                sources::update(ctx, &id, args).await.map(|_| ok)
            }
        },
        Command::Integrations { command } => match command {
            IntegrationsCommand::List { state, status, data_type, limit, offset, json, output } => {
                let args = integrations::ListArgs { state, status, data_type, limit, offset, json, output };
                integrations::list(ctx, args).await.map(|_| ok)
            }
            IntegrationsCommand::Get { id, json } => integrations::get(ctx, &id, json).await.map(|_| ok),
            IntegrationsCommand::Sync { id, json, watch, dry_run, output } => {
                let opts = ActionOpts { json, dry_run, output };
                resource::sync(ctx, resource::INTEGRATION, &id, watch, opts, integrations::dry_run_fields)
                    .await
                    .map(|_| ok)
            }
            IntegrationsCommand::RefreshSource { id, from, to, json } => {
                integrations::refresh_source(ctx, &id, from, to, json).await
            }
            IntegrationsCommand::Pause { id, json, dry_run, output } => {
                let opts = ActionOpts { json, dry_run, output };
                resource::state_action(ctx, resource::INTEGRATION, &id, "pause", opts).await.map(|_| ok)
            }
            IntegrationsCommand::Resume { id, json, dry_run, output } => {
                let opts = ActionOpts { json, dry_run, output };
                resource::state_action(ctx, resource::INTEGRATION, &id, "resume", opts).await.map(|_| ok)
            }
            IntegrationsCommand::Delete { id, json, yes, dry_run, output } => {
                resource::delete(ctx, resource::INTEGRATION, &id, yes, ActionOpts { json, dry_run, output })
                    .await
                    .map(|_| ok)
            }
            IntegrationsCommand::Create {
                dest_app,
                source_app,
                data_type,
                config_file,
                schedule_file,
                frequency,
                unit,
                run_now,
                json,
                output,
            } => {
                let args = integrations::CreateArgs {
                    dest_app,
                    source_app,
                    data_type,
                    config_file,
                    schedule_file,
                    frequency,
                    unit,
                    run_now,
                    json,
                    output,
                };
                integrations::create(ctx, args).await.map(|_| ok)
            }
            IntegrationsCommand::Update { id, config_file, schedule_file, frequency, unit, json, output } => {
                let args = integrations::UpdateArgs { config_file, schedule_file, frequency, unit, json, output };
                integrations::update(ctx, &id, args).await.map(|_| ok)
            }
        },
        Command::Jobs { command } => match command {
            JobsCommand::List { status, job_type, source, integration, limit, offset, json, output } => {
                let args = jobs_cmd::ListArgs { status, job_type, source, integration, limit, offset, json, output };
                jobs_cmd::list(ctx, args).await.map(|_| ok)
            }
            JobsCommand::Get { job_id, json } => jobs_cmd::get(ctx, &job_id, json).await.map(|_| ok),
            JobsCommand::Cancel { job_id, json, yes, dry_run, output } => {
                jobs_cmd::cancel(ctx, &job_id, json, yes, dry_run, output).await.map(|_| ok)
            }
            JobsCommand::Watch { interval, source, integration } => {
                jobs_cmd::watch(ctx, &interval, source, integration).await.map(|_| ok)
            }
            JobsCommand::Tail { job_id, source, integration, next, interval } => {
                jobs_cmd::tail(ctx, jobs_cmd::TailArgs { job_id, source, integration, next, interval }).await
            }
        },
        Command::Catalog { command } => match command {
            CatalogCommand::List { category, role, json, output } => {
                catalog::list(ctx, category, role, json, output).await.map(|_| ok)
            }
            CatalogCommand::Get { app_type, json } => catalog::get(ctx, &app_type, json).await.map(|_| ok),
            CatalogCommand::CredentialSchema { app_type, json } => {
                catalog::credential_schema(ctx, &app_type, json).await.map(|_| ok)
            }
        },
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

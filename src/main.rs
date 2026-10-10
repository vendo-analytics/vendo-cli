//! The `vendo` CLI. It replaced the TypeScript CLI at 1.1.0 (VE-3669), whose
//! behaviour it kept apart from the intended differences in CHANGELOG.md.

// Print like Node's `console`: when stdout or stderr is closed (`vendo … |
// head -1`) the write is dropped and the command carries on, where std's
// macros panic. Defined before the modules so they replace std's everywhere.
macro_rules! print {
    ($($arg:tt)*) => { $crate::output::write_stdout(::std::format_args!($($arg)*), false) };
}
macro_rules! println {
    () => { $crate::output::write_stdout(::std::format_args!(""), true) };
    ($($arg:tt)*) => { $crate::output::write_stdout(::std::format_args!($($arg)*), true) };
}
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::output::write_stderr(::std::format_args!($($arg)*), false) };
}
macro_rules! eprintln {
    () => { $crate::output::write_stderr(::std::format_args!(""), true) };
    ($($arg:tt)*) => { $crate::output::write_stderr(::std::format_args!($($arg)*), true) };
}

// Asking for a value a command is missing, at a terminal (VE-3881); part of the `menu` feature.
#[cfg(feature = "menu")]
mod ask;
// A list command's rows to choose from at a terminal, then an item's actions (VE-3894).
mod browse;
mod cli;
mod client;
mod commands;
mod config;
mod context;
mod dictionary;
mod identity;
mod jobs;
mod js_date;
mod js_text;
mod output;
mod profile_display;
mod short_ids;
mod source_refresh;
mod update_check;
mod watch;
mod web_app;

use std::{ffi::OsString, process::ExitCode};

use crate::{
    cli::{
        AppsCommand, CatalogCommand, Command, DictionaryCommand, IntegrationsCommand, Invocation, JobsCommand,
        LtvCommand, MeasurementCommand, MethodologiesCommand, MetricsCommand, ModelsCommand, ProfileCommand,
        RulesCommand, SignalsCommand, SourcesCommand,
    },
    commands::{
        account, apps, catalog, completions, dictionary as dictionary_cmd, health, integrations, jobs as jobs_cmd,
        login, measurement, metrics, models,
        pipeline_resource::{self as resource, ActionOpts},
        sources, tree, version, workspace,
    },
    context::Ctx,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = match cli::preprocess(std::env::args_os().collect()) {
        Invocation::Version => {
            version::run(false);
            return ExitCode::SUCCESS;
        }
        Invocation::Run(args) => args,
    };
    let program = args.first().cloned().unwrap_or_else(|| "vendo".into());
    let mut parsed = cli::parse(args).await;
    loop {
        let cli::Parsed { cli, json } = parsed;
        output::set_json_errors(json);
        let ctx = Ctx::new(cli.profile.clone(), cli.debug);
        let code = match run(&ctx, cli.command).await {
            Ok(code) => code,
            Err(err) => {
                output::report_error(&err);
                return ExitCode::from(1);
            }
        };
        // An action chosen for an item of a selectable list (VE-3894) runs as if typed, with
        // `--profile` and `--debug` as given, and the CLI ends with its exit code.
        let Some(words) = browse::chosen() else { return code };
        parsed = cli::parse(action_args(program.clone(), cli.profile, cli.debug, words)).await;
    }
}

/// The words of an action chosen in a selectable list (VE-3894) as typed: the program's name,
/// `--profile=<name>` when `--profile` was given (VENDO_PROFILE carries over in the environment),
/// `--debug` when it was given, then the action's words (`apps pause <full ID>`).
fn action_args(program: OsString, profile: Option<String>, debug: bool, words: Vec<OsString>) -> Vec<OsString> {
    let globals = profile.map(|name| OsString::from(format!("--profile={name}"))).into_iter();
    let debug = debug.then(|| OsString::from("--debug")).into_iter();
    std::iter::once(program).chain(globals).chain(debug).chain(words).collect()
}

async fn run(ctx: &Ctx, command: Command) -> anyhow::Result<ExitCode> {
    let ok = ExitCode::SUCCESS;
    match command {
        Command::Login { api_key, account, env, base_url, force, json } => {
            login::run(ctx, login::LoginArgs { api_key, account, env, base_url, force, json }).await.map(|_| ok)
        }
        Command::Logout { all, yes, json } => account::logout(ctx, all, yes, json).map(|_| ok),
        Command::Profile { command } => match command {
            ProfileCommand::List { json } => account::profile_list(ctx, json).await.map(|_| ok),
            ProfileCommand::Switch { profile, account, json } => {
                account::profile_switch(ctx, profile, account, json).map(|_| ok)
            }
            ProfileCommand::Set { api_key, base_url, account, json } => {
                account::profile_set(ctx, api_key, base_url, account, json).map(|_| ok)
            }
        },
        Command::Status { json } => health::status(ctx, json).await.map(|_| ok),
        Command::Workspace { json, doctor } => workspace::run(ctx, json, doctor).await,
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
            JobsCommand::Watch { interval, source, integration, json } => {
                jobs_cmd::watch(ctx, &interval, source, integration, json).await.map(|_| ok)
            }
            JobsCommand::Tail { job_id, source, integration, next, interval, json } => {
                jobs_cmd::tail(ctx, jobs_cmd::TailArgs { job_id, source, integration, next, interval, json }).await
            }
        },
        Command::Catalog { command } => match command {
            CatalogCommand::List { category, role, all, json, output } => {
                catalog::list(ctx, category, role, all, json, output).await.map(|_| ok)
            }
            CatalogCommand::Get { app_type, json } => catalog::get(ctx, &app_type, json).await.map(|_| ok),
            CatalogCommand::CredentialSchema { app_type, json } => {
                catalog::credential_schema(ctx, &app_type, json).await.map(|_| ok)
            }
        },
        Command::Dictionary { command } => match command {
            DictionaryCommand::List { subject_type, query, limit, offset, json, output } => {
                let args = dictionary_cmd::PageArgs { subject_type, query, limit, offset, json, output };
                dictionary_cmd::page(ctx, "Fetching dictionary...", args, true).await.map(|_| ok)
            }
            DictionaryCommand::Search { query, subject_type, limit, offset, json, output } => {
                let args = dictionary_cmd::PageArgs { subject_type, query: Some(query), limit, offset, json, output };
                dictionary_cmd::page(ctx, "Searching dictionary...", args, false).await.map(|_| ok)
            }
            DictionaryCommand::Get { subject_id, json } => {
                dictionary_cmd::get(ctx, &subject_id, json).await.map(|_| ok)
            }
        },
        Command::Metrics { command } => match command {
            MetricsCommand::List { status, limit, offset, json, output } => {
                metrics::list(ctx, metrics::ListArgs { status, limit, offset, json, output }).await.map(|_| ok)
            }
            MetricsCommand::Get { id, json } => metrics::get(ctx, &id, json).await.map(|_| ok),
            MetricsCommand::Create { name, definition, description, format, unit, json } => {
                let args = metrics::CreateArgs { name, definition, description, format, unit, json };
                metrics::create(ctx, args).await.map(|_| ok)
            }
            MetricsCommand::Update { id, name, description, definition, format, unit, status, json } => {
                let args = metrics::UpdateArgs { name, description, definition, format, unit, status, json };
                metrics::update(ctx, &id, args).await.map(|_| ok)
            }
            MetricsCommand::Activate { id, json } => metrics::activate(ctx, &id, json).await.map(|_| ok),
            MetricsCommand::Delete { id, yes, json } => metrics::delete(ctx, &id, yes, json).await.map(|_| ok),
        },
        Command::Models { command } => match command {
            ModelsCommand::List { data_type, valid, invalid, limit, offset, json, output } => {
                let args = models::ListArgs { data_type, valid, invalid, limit, offset, json, output };
                models::list(ctx, args).await.map(|_| ok)
            }
            ModelsCommand::Get { id, json } => models::get(ctx, &id, json).await.map(|_| ok),
        },
        Command::Measurement { command } => match command {
            MeasurementCommand::Methodologies { command } => match command {
                MethodologiesCommand::List { no_system, json, output } => {
                    measurement::methodologies_list(ctx, no_system, json, output).await.map(|_| ok)
                }
                MethodologiesCommand::Get { id, json } => {
                    measurement::methodologies_get(ctx, &id, json).await.map(|_| ok)
                }
            },
            MeasurementCommand::Rules { command: RulesCommand::Preview { from, to, limit, json } } => {
                measurement::rules_preview(ctx, from, to, &limit, json).await.map(|_| ok)
            }
            MeasurementCommand::Ltv { command } => match command {
                LtvCommand::List { granularity, segment, from, to, limit, no_predicted, json, output } => {
                    let args =
                        measurement::LtvListArgs { granularity, segment, from, to, limit, no_predicted, json, output };
                    measurement::ltv_list(ctx, args).await.map(|_| ok)
                }
                LtvCommand::Cohort { period, granularity, segment, json } => {
                    measurement::ltv_cohort(ctx, &period, granularity, segment, json).await.map(|_| ok)
                }
                LtvCommand::Customer { customer_id, json } => {
                    measurement::ltv_customer(ctx, &customer_id, json).await.map(|_| ok)
                }
            },
            MeasurementCommand::Signals { command } => match command {
                SignalsCommand::List { json } => measurement::signals_list(ctx, json).await.map(|_| ok),
                SignalsCommand::ClickPath { sample_limit, json } => {
                    measurement::click_path(ctx, sample_limit, json).await.map(|_| ok)
                }
            },
        },
        Command::Completions { shell, json } => {
            completions::run(ctx, shell, json);
            Ok(ok)
        }
        Command::Commands { json } => {
            tree::run(json);
            Ok(ok)
        }
        Command::Version { json } => {
            version::run(json);
            Ok(ok)
        }
        Command::Update { install_version, json } => health::self_update(ctx, install_version, json),
    }
}

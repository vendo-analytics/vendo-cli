//! Selectable lists (VE-3894, decided by Yalcin 2026-10-07, CLI 1.1): "items first then actions,
//! but still keep the actions so we can go directly to the action without the list too".
//!
//! Where the group menu opens ([`output::can_show_menu`]: stdin, stdout and stderr terminals, prompts
//! not off, stdin readable, `TERM` not `dumb`) and a list command would print its table (no `--json`,
//! no `--output`, not even an empty one), it shows the table's rows, from the same requests, as an
//! arrow-key list with type-to-filter instead ([`output::choose_value`], VE-3881's list): each row the
//! table's cells as plain text on one line (`menu::one_line`, padded per column as VE-3881's rows are), the
//! table's footer after the hint (`· 57 apps`), titled with the command as the tree names it
//! (`vendo apps list`) and answered as VE-3881 answers an item (`a1b2c3d4... (Menu Shop)`). Enter on an
//! item shows exactly what the group's `get` shows of it, from `get`'s own code, spinner and requests,
//! then a short list of the actions that apply to it ([`Group::actions`]), each with its description as
//! the help lists it, and `back`. An action runs exactly as typed: its words (`apps pause <full ID>`)
//! are kept ([`chosen`]) and `main` parses them again after `--profile` and `--debug` as given, so a
//! delete's y/N, `VENDO_PROFILE` (the environment carries over), `VENDO_DEBUG` and the rest apply; the
//! full ID needs no short-ID lookup, and the CLI ends with the action's exit code. Back opens the list
//! again, its rows as they were (no request), the cursor on the item just viewed and the filter
//! cleared; nothing above it is erased. Esc, Ctrl-C, Ctrl-D and a terminal that hangs up, on any of
//! these lists, leave quietly, exit 0, nothing run. `get` and every action command work as before.
//!
//! The table prints as before, byte for byte, wherever the list does not open: without a terminal,
//! with prompts off, on `TERM=dumb`, with stderr redirected or a write-only stdin, with `--json` or
//! `--output`, and built without the `menu` feature. It prints at a terminal too for an empty list,
//! and when the terminal refuses the first list.
//!
//! Every visible list command ([`Group`]): `vendo apps list`, `vendo sources list`, `vendo destinations
//! list` (its hidden `integrations list` and `int list` too), `vendo jobs list`, `vendo catalog list`,
//! `vendo dictionary list` (not `dictionary search`, which prints the same table), `vendo metrics list`,
//! `vendo models list`, `vendo measurement methodologies list`, `vendo measurement ltv list`, `vendo
//! measurement signals list` and `vendo profile list` (its hidden `config list` too), which prints
//! lines rather than a table ([`profiles`]).

use std::{
    ffi::OsString,
    sync::{Mutex, PoisonError},
};

use anyhow::Result;
use serde_json::Value;

use crate::{client::Client, output};

/// A list command's table: its header, each row's cells styled as the table shows them, and the
/// footer under it, plain (`57 apps`). The table path prints them ([`Table::print`]); a selectable
/// list shows the same cells plain and the footer in its hint.
pub struct Table<'a> {
    pub header: &'a [&'a str],
    pub cells: Vec<Vec<String>>,
    pub footer: String,
}

impl Table<'_> {
    /// The table, then its footer dimmed: what the list command prints without a terminal.
    pub fn print(self) {
        let mut grid = output::table(self.header);
        for row in self.cells {
            grid.add_row(row);
        }
        println!("{grid}");
        println!("{}", output::dim(&self.footer));
    }
}

/// The list commands whose rows can be chosen (VE-3894).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group {
    /// `vendo apps list`.
    Apps,
    /// `vendo sources list`.
    Sources,
    /// `vendo destinations list` (hidden `integrations list`, `int list`): the API's integrations.
    Destinations,
    /// `vendo jobs list`.
    Jobs,
    /// `vendo catalog list`: the platforms, by their app type.
    Catalog,
    /// `vendo dictionary list`: the entries of one subject type, by their subject ID.
    Dictionary,
    /// `vendo metrics list`: the web app's metrics.
    Metrics,
    /// `vendo models list`.
    Models,
    /// `vendo measurement methodologies list`: shown from the list's own row, as `methodologies get`
    /// reads the same route.
    Methodologies,
    /// `vendo measurement ltv list`: the cohorts of the list's `--granularity` and `--segment`, which
    /// the cohort shown takes too.
    Cohorts { granularity: String, segment: String },
    /// `vendo measurement signals list`: nothing to show of a signal (the group has no `get`).
    Signals,
    /// `vendo profile list` (hidden `config list`): `saved`, the saved `activeProfile`, which
    /// `profile switch` would not change.
    Profiles { saved: Option<String> },
}

/// The words of the action chosen for an item, kept by the list (`menu::browse`) for `main` to parse
/// and run once the list command has returned ([`chosen`]).
static CHOSEN: Mutex<Option<Vec<OsString>>> = Mutex::new(None);

/// The words of the action chosen in a selectable list, as typed after `vendo` and the global options
/// (`apps pause <full ID>`), once: `main` runs them with `--profile` and `--debug` as given.
pub fn chosen() -> Option<Vec<OsString>> {
    CHOSEN.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// What a list command shows in place of `table` (its `rows`, the items the response listed, one per
/// row of cells): the table, or where it can open ([`opens`]) the rows to choose from. `output` is
/// the command's `--output`.
pub async fn shown(
    client: &Client,
    group: Group,
    rows: &[Value],
    table: Table<'_>,
    output: Option<&str>,
) -> Result<()> {
    if opens(false, output) && !rows.is_empty() {
        let (cells, footer) = (table.cells.clone(), table.footer.clone());
        return browse(Some(client), group, rows, cells, Some(&footer), || table.print()).await;
    }
    table.print();
    Ok(())
}

/// What `vendo profile list` shows in place of its lines (`print`): where the list can open
/// ([`opens`], no `--json`) and there are profiles, `rows` (one per profile, as `--json` prints it) to
/// choose from, each shown as its `cells`; Enter offers `profile switch` unless the profile is the
/// saved active one (`saved`), and shows nothing first, as there is no `profile get`.
pub async fn profiles(
    saved: impl FnOnce() -> Option<String>,
    rows: &[Value],
    cells: Vec<Vec<String>>,
    print: impl FnOnce(),
) -> Result<()> {
    if opens(false, None) && !rows.is_empty() {
        return browse(None, Group::Profiles { saved: saved() }, rows, cells, None, print).await;
    }
    print();
    Ok(())
}

/// Whether a list command shows its rows to choose from rather than its table: no `--json`, no
/// `--output` (an empty one, which prints the table, counts too), and a terminal the group menu opens
/// on ([`output::can_show_menu`], the rule every prompt asks by).
#[cfg(feature = "menu")]
pub fn opens(json: bool, output: Option<&str>) -> bool {
    !json && output.is_none() && output::can_show_menu()
}

/// A build without the `menu` feature has no selectable list (VE-3894): the table prints at a
/// terminal too, as a bare group is the usage error there (`cli::menu_choice`).
#[cfg(not(feature = "menu"))]
pub fn opens(_json: bool, _output: Option<&str>) -> bool {
    false
}

/// Never reached without the `menu` feature, where nothing [`opens`]: the table.
#[cfg(not(feature = "menu"))]
async fn browse(
    _client: Option<&Client>,
    _group: Group,
    _rows: &[Value],
    _cells: Vec<Vec<String>>,
    _note: Option<&str>,
    print: impl FnOnce(),
) -> Result<()> {
    print();
    Ok(())
}

#[cfg(feature = "menu")]
use menu::browse;

/// The lists themselves: only with the `menu` feature.
#[cfg(feature = "menu")]
mod menu {
    use std::ffi::OsString;

    use anyhow::{Context, Result};
    use serde_json::Value;

    use super::{CHOSEN, Group, PoisonError};
    use crate::{
        ask::{Listed, named, padded},
        client::Client,
        commands::{apps, catalog, dictionary, integrations, jobs, measurement, metrics, models, sources},
        jobs::{ACTIVE_JOB_STATUSES, Job},
        output::{self, ValueRow, short_id},
    };

    /// The last row of an item's action menu, which opens the list again.
    const BACK: (&str, &str) = ("back", "Back to the list");

    impl Group {
        /// The group as the tree names it, after `vendo`.
        pub(super) fn path(&self) -> &'static str {
            match self {
                Group::Apps => "apps",
                Group::Sources => "sources",
                Group::Destinations => "destinations",
                Group::Jobs => "jobs",
                Group::Catalog => "catalog",
                Group::Dictionary => "dictionary",
                Group::Metrics => "metrics",
                Group::Models => "models",
                Group::Methodologies => "measurement methodologies",
                Group::Cohorts { .. } => "measurement ltv",
                Group::Signals => "measurement signals",
                Group::Profiles { .. } => "profile",
            }
        }

        /// The item's full ID, which `get` and its actions take: a platform's app type, a dictionary
        /// entry's subject ID, a cohort's period (`ltv cohort`'s), a profile's name, any other item's
        /// `id` (a signal's is its name, `click_path`).
        fn id(&self, row: &Value) -> String {
            match self {
                Group::Catalog => Job(row).text("appType").unwrap_or_default(),
                Group::Dictionary => Job(row).text("subjectId").unwrap_or_default(),
                Group::Cohorts { .. } => Job(row).text("cohort_period").unwrap_or_default(),
                Group::Profiles { .. } => Job(row).text("name").unwrap_or_default(),
                _ => Job(row).id(),
            }
        }

        /// How the answered line names the item, as VE-3881's lists name it: `a1b2c3d4... (Menu Shop)`,
        /// `9c0d1e2f... (Analytics BQ → Demo Pixel)`, a job by its short ID alone, on one line as its row
        /// is ([`one_line`]). A platform by its app type alone, a dictionary entry by its subject ID (not
        /// a short ID, in full) and its display name: `<subject ID> (Checkout Completed)`. A methodology
        /// as VE-3881's list names it, a cohort by its period, a signal by its ID and a profile by its
        /// name, each alone.
        fn answer(&self, row: &Value) -> String {
            let listed = match self {
                Group::Apps => Listed::Apps,
                Group::Sources => Listed::Sources,
                Group::Destinations => Listed::Destinations,
                Group::Jobs => Listed::Jobs,
                Group::Metrics => Listed::Metrics,
                Group::Models => Listed::Models,
                Group::Methodologies => Listed::Methodologies,
                Group::Catalog | Group::Cohorts { .. } | Group::Signals | Group::Profiles { .. } => {
                    return one_line(&self.id(row));
                }
                Group::Dictionary => {
                    let name = Job(row).text("displayName").unwrap_or_default();
                    return one_line(&named(self.id(row), &name));
                }
            };
            one_line(&named(short_id(&self.id(row)), &listed.name(row)))
        }

        /// Shows what the group's `get` shows of the item, from its code, spinner and requests; the item
        /// as that request returned it. A request that fails is `get`'s error (exit 1). A methodology as
        /// `methodologies get` shows it, from the list's row (its `get` reads the same route, so nothing
        /// is sent); a cohort as `ltv cohort` shows it with the list's `--granularity` and
        /// `--segment`; nothing for a signal or a profile, which have no `get` (the row itself).
        async fn show(&self, client: Option<&Client>, row: &Value) -> Result<Value> {
            let id = self.id(row);
            match self {
                Group::Methodologies => {
                    print!("{}", measurement::render_methodology(row));
                    return Ok(row.clone());
                }
                Group::Signals | Group::Profiles { .. } => return Ok(row.clone()),
                _ => {}
            }
            // Every other list command has its client (`shown`).
            let client = client.context("no client to show the item with")?;
            match self {
                Group::Apps => apps::show(client, &id).await,
                Group::Sources => sources::show(client, &id).await,
                Group::Destinations => integrations::show(client, &id).await,
                Group::Jobs => jobs::show(client, &id).await,
                Group::Catalog => catalog::show(client, &id).await,
                Group::Dictionary => dictionary::show(client, &id).await,
                Group::Metrics => metrics::show(client, &id).await,
                Group::Models => models::show(client, &id).await,
                Group::Cohorts { granularity, segment } => {
                    measurement::show_cohort(client, &id, granularity.clone(), segment.clone()).await
                }
                Group::Methodologies | Group::Signals | Group::Profiles { .. } => Ok(row.clone()),
            }
        }

        /// The commands of the group that apply to `item`, as `get`'s request returned it, in the order
        /// the action menu lists them. Apps, sources and destinations: `pause` when active, `resume`
        /// when inactive (neither for any other state; vendo-web-v2 `lib/vendo/apps/queries.ts`,
        /// active|inactive, and the routes do not refuse by state), then `update` and `delete`, always
        /// (the API refuses to delete an app still in use, with the next step, VE-3756). Sources and
        /// destinations first `sync` when active (`sources/sync.ts` and `integrations/sync.ts` refuse
        /// it otherwise), destinations then `refresh-source` when they have a source app
        /// (`lib/server/source-refresh.ts` refuses it without one, `no_source_app`). Jobs: `tail` and
        /// `cancel` while queued, pending or running (`jobs/cancel.ts` cancels only those; a finished
        /// job's tail would repeat what `get` showed), nothing otherwise. Metrics: `activate` for a draft
        /// (the help's 'Activate a draft metric'; not an archived one), then `update` and `delete`,
        /// always. Platforms, dictionary entries and models: nothing (their groups have no command that
        /// changes one; the hidden `catalog credential-schema` is never offered, and creating an app with
        /// a platform is another group's command). Methodologies and cohorts: nothing (the CLI has no
        /// command that changes one; `ltv customer` takes a customer). Signals: the `click_path` signal
        /// its group's `click-path` (which takes no ID), any other nothing. Profiles: `switch` unless the
        /// profile is the saved `activeProfile` (not the `*`, which `--profile` and `VENDO_PROFILE` move:
        /// switching to the saved one changes nothing). Only visible commands of the group that take the
        /// item's ID (but `click-path`): never a hidden one, another group's (`jobs tail --source`) or
        /// one that takes no ID.
        pub(super) fn actions(&self, item: &Value) -> Vec<&'static str> {
            let item = Job(item);
            let state = item.text("state");
            let sync = (state.as_deref() == Some("active")).then_some("sync");
            let pause_or_resume = match state.as_deref() {
                Some("active") => Some("pause"),
                Some("inactive") => Some("resume"),
                _ => None,
            };
            let lifecycle = pause_or_resume.into_iter().chain(["update", "delete"]);
            match self {
                Group::Apps => lifecycle.collect(),
                Group::Sources => sync.into_iter().chain(lifecycle).collect(),
                Group::Destinations => {
                    let source_app = item.text("sourceAppId").filter(|id| !id.is_empty());
                    let refresh = source_app.map(|_| "refresh-source");
                    sync.into_iter().chain(refresh).chain(lifecycle).collect()
                }
                Group::Jobs => {
                    let status = item.status();
                    let active = ACTIVE_JOB_STATUSES.split(',').any(|active| active == status);
                    if active { vec!["tail", "cancel"] } else { Vec::new() }
                }
                Group::Metrics => {
                    let draft = (item.status() == "draft").then_some("activate");
                    draft.into_iter().chain(["update", "delete"]).collect()
                }
                Group::Signals => {
                    if item.text("id").as_deref() == Some("click_path") {
                        vec!["click-path"]
                    } else {
                        Vec::new()
                    }
                }
                Group::Profiles { saved } => {
                    let name = item.text("name");
                    if name.is_some() && name == *saved { Vec::new() } else { vec!["switch"] }
                }
                Group::Catalog | Group::Dictionary | Group::Models | Group::Methodologies | Group::Cohorts { .. } => {
                    Vec::new()
                }
            }
        }

        /// The words `action` runs as for the item, as typed after `vendo`: the group as the tree names
        /// it (whatever alias was typed), the action and the full ID, after `--` should it start with
        /// `-` (an ID never does; a profile's name might). A signal's `click-path` takes no ID: `measurement
        /// signals click-path`.
        pub(super) fn words(&self, action: &str, row: &Value) -> Vec<OsString> {
            let id = self.id(row);
            let mut words: Vec<OsString> = self.path().split(' ').map(OsString::from).collect();
            words.push(action.into());
            if *self == Group::Signals {
                return words;
            }
            if id.starts_with('-') {
                words.push("--".into());
            }
            words.push(id.into());
            words
        }
    }

    /// The rows of a list (`cells`, one per item of `rows`, `note` after the hint) to choose from;
    /// `print`, what the command prints without a list, when the terminal refuses the first list. Enter
    /// shows the item ([`Group::show`]), then its action menu ([`choose_action`]): an action is kept for
    /// `main` to run, Back opens the list again with the cursor on that item. A list or action menu
    /// that cannot run after the first leaves quietly (exit 0).
    pub(super) async fn browse(
        client: Option<&Client>,
        group: Group,
        rows: &[Value],
        cells: Vec<Vec<String>>,
        note: Option<&str>,
        print: impl FnOnce(),
    ) -> Result<()> {
        let plain = cells.iter().map(|row| row.iter().map(|cell| one_line(cell)).collect()).collect();
        let items: Vec<ValueRow> = padded(plain)
            .into_iter()
            .zip(rows)
            .map(|(shown, row)| ValueRow { shown, answer: group.answer(row) })
            .collect();
        let title = format!("vendo {} list", group.path());
        let Some(mut at) = output::choose_value(&title, &items, note, 0) else {
            print();
            return Ok(());
        };
        loop {
            let item = group.show(client, &rows[at]).await?;
            match choose_action(&group, &items[at].answer, &group.actions(&item)) {
                Some(Some(action)) => {
                    *CHOSEN.lock().unwrap_or_else(PoisonError::into_inner) = Some(group.words(action, &rows[at]));
                    return Ok(());
                }
                Some(None) => {}
                None => output::quit_quietly(),
            }
            at = output::choose_value(&title, &items, note, at).unwrap_or_else(|| output::quit_quietly());
        }
    }

    /// The action menu of the item answered as `answer`: `actions`, each with its description as the
    /// group's help lists it, then `back`, titled with the group as the tree names it (`vendo apps`),
    /// the cursor on the first row, and the keys typed while the item loaded thrown away. Answered as
    /// the command that runs (`vendo apps pause a1b2c3d4... (Menu Shop)`), or `back`. `Some(None)` for
    /// back; `None` when the menu cannot run.
    fn choose_action(group: &Group, answer: &str, actions: &[&'static str]) -> Option<Option<&'static str>> {
        let tree = crate::cli::command();
        let commands = group.path().split(' ').try_fold(&tree, |command, word| command.find_subcommand(word));
        let about = |name: &str| {
            let command = commands.and_then(|commands| commands.find_subcommand(name));
            command.and_then(clap::Command::get_about).map(ToString::to_string).unwrap_or_default()
        };
        let mut cells: Vec<Vec<String>> = actions.iter().map(|name| vec![name.to_string(), about(name)]).collect();
        cells.push(vec![BACK.0.into(), BACK.1.into()]);
        let answers = actions.iter().map(|name| format!("{name} {answer}")).chain([BACK.0.to_string()]);
        let rows: Vec<ValueRow> =
            padded(cells).into_iter().zip(answers).map(|(shown, answer)| ValueRow { shown, answer }).collect();
        let at = output::choose_value(&format!("vendo {}", group.path()), &rows, None, 0)?;
        Some(actions.get(at).copied())
    }

    /// A table's cell as a row of the list shows it: without styling, on one line (a line end or any
    /// other control character a space), so that a row is one line however the screen breaks it.
    pub(super) fn one_line(cell: &str) -> String {
        output::strip_ansi(cell).chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    }
}

#[cfg(all(test, feature = "menu"))]
mod tests {
    use serde_json::json;

    use super::{Group, menu::one_line};

    const GROUPS: [Group; 12] = [
        Group::Apps,
        Group::Sources,
        Group::Destinations,
        Group::Jobs,
        Group::Catalog,
        Group::Dictionary,
        Group::Metrics,
        Group::Models,
        Group::Methodologies,
        Group::Cohorts { granularity: String::new(), segment: String::new() },
        Group::Signals,
        Group::Profiles { saved: None },
    ];

    #[test]
    fn a_cell_is_shown_plain_on_one_line() {
        assert_eq!(one_line("\u{1b}[2ma1b2c3d4...\u{1b}[0m"), "a1b2c3d4...");
        assert_eq!(one_line("\u{1b}[32mactive\u{1b}[39m"), "active");
        assert_eq!(one_line("Line one\nline two\r\n\tthree"), "Line one line two   three");
        assert_eq!(one_line("bell\u{7}"), "bell ");
        assert_eq!(one_line("東京 Café"), "東京 Café");
    }

    #[test]
    fn an_app_offers_pause_when_active_resume_when_inactive_then_update_and_delete() {
        let actions = |state: serde_json::Value| Group::Apps.actions(&json!({ "state": state }));
        assert_eq!(actions(json!("active")), ["pause", "update", "delete"]);
        assert_eq!(actions(json!("inactive")), ["resume", "update", "delete"]);
        for other in [json!("deleted"), json!("paused"), json!(null), json!(1)] {
            assert_eq!(actions(other.clone()), ["update", "delete"], "{other}");
        }
        assert_eq!(Group::Apps.actions(&json!(null)), ["update", "delete"]);
    }

    #[test]
    fn a_source_offers_sync_and_pause_when_active_resume_when_inactive_then_update_and_delete() {
        let actions = |state: serde_json::Value| Group::Sources.actions(&json!({ "state": state }));
        assert_eq!(actions(json!("active")), ["sync", "pause", "update", "delete"]);
        assert_eq!(actions(json!("inactive")), ["resume", "update", "delete"]);
        for other in [json!("deleted"), json!("paused"), json!("ACTIVE"), json!(null), json!(1)] {
            assert_eq!(actions(other.clone()), ["update", "delete"], "{other}");
        }
    }

    #[test]
    fn a_destination_offers_refresh_source_only_with_a_source_app() {
        let actions = |state: &str, source_app: serde_json::Value| {
            Group::Destinations.actions(&json!({ "state": state, "sourceAppId": source_app }))
        };
        let app = json!("a1b2c3d4-0000-4000-8000-000000000001");
        assert_eq!(actions("active", app.clone()), ["sync", "refresh-source", "pause", "update", "delete"]);
        assert_eq!(actions("inactive", app.clone()), ["refresh-source", "resume", "update", "delete"]);
        assert_eq!(actions("deleted", app), ["refresh-source", "update", "delete"]);
        for none in [json!(null), json!("")] {
            assert_eq!(actions("active", none.clone()), ["sync", "pause", "update", "delete"], "{none}");
            assert_eq!(actions("inactive", none.clone()), ["resume", "update", "delete"], "{none}");
        }
        assert_eq!(Group::Destinations.actions(&json!({ "state": "deleted" })), ["update", "delete"]);
    }

    #[test]
    fn a_job_offers_tail_and_cancel_only_while_queued_pending_or_running() {
        let actions = |status: serde_json::Value| Group::Jobs.actions(&json!({ "status": status }));
        for active in ["queued", "pending", "running"] {
            assert_eq!(actions(json!(active)), ["tail", "cancel"], "{active}");
        }
        let finished = ["completed", "warning", "failed", "canceled", "cancelled", "errored", "Running", ""];
        for status in finished.map(|status| json!(status)).into_iter().chain([json!(null), json!(1)]) {
            assert!(actions(status.clone()).is_empty(), "{status}");
        }
        // A job's state is its status: `state` changes nothing.
        assert!(Group::Jobs.actions(&json!({ "state": "active", "status": "failed" })).is_empty());
    }

    #[test]
    fn a_metric_offers_activate_only_as_a_draft_then_update_and_delete() {
        let actions = |status: serde_json::Value| Group::Metrics.actions(&json!({ "status": status }));
        assert_eq!(actions(json!("draft")), ["activate", "update", "delete"]);
        for other in [json!("active"), json!("archived"), json!("Draft"), json!(""), json!(null), json!(1)] {
            assert_eq!(actions(other.clone()), ["update", "delete"], "{other}");
        }
    }

    #[test]
    fn a_platform_a_dictionary_entry_or_a_model_offers_nothing_but_back() {
        let item = json!({ "status": "draft", "state": "active", "availability": "self_serve", "isValid": true });
        for group in [Group::Catalog, Group::Dictionary, Group::Models] {
            assert!(group.actions(&item).is_empty(), "{group:?}");
        }
    }

    #[test]
    fn a_methodology_or_a_cohort_offers_nothing_but_back() {
        let item = json!({ "id": "click_path", "status": "draft", "state": "active", "is_system": false });
        let cohorts = Group::Cohorts { granularity: "monthly".into(), segment: "all".into() };
        for group in [Group::Methodologies, cohorts] {
            assert!(group.actions(&item).is_empty(), "{group:?}");
        }
    }

    #[test]
    fn only_the_click_path_signal_offers_its_click_path_command() {
        assert_eq!(Group::Signals.actions(&json!({ "id": "click_path", "state": "live" })), ["click-path"]);
        for id in [json!("mmm"), json!("survey"), json!("geo_lift"), json!("Click_Path"), json!(null)] {
            assert!(Group::Signals.actions(&json!({ "id": id, "state": "live" })).is_empty(), "{id}");
        }
        // It takes no ID: the signal is the command.
        let words: Vec<String> = Group::Signals
            .words("click-path", &json!({ "id": "click_path" }))
            .into_iter()
            .map(|word| word.into_string().unwrap())
            .collect();
        assert_eq!(words, ["measurement", "signals", "click-path"]);
    }

    #[test]
    fn a_profile_offers_switch_unless_it_is_the_saved_active_one() {
        // By the saved `activeProfile`, not the `*` that `--profile` and VENDO_PROFILE move.
        let profiles = Group::Profiles { saved: Some("alpha".into()) };
        assert!(profiles.actions(&json!({ "name": "alpha", "active": false })).is_empty());
        assert_eq!(profiles.actions(&json!({ "name": "beta", "active": true })), ["switch"]);
        assert_eq!(profiles.actions(&json!({ "name": "Alpha", "active": false })), ["switch"]);
        // With no saved active profile, every profile can be switched to.
        let unsaved = Group::Profiles { saved: None };
        assert_eq!(unsaved.actions(&json!({ "name": "alpha", "active": true })), ["switch"]);
        let words = |name: &str| -> Vec<String> {
            profiles.words("switch", &json!({ "name": name })).into_iter().map(|w| w.into_string().unwrap()).collect()
        };
        assert_eq!(words("beta"), ["profile", "switch", "beta"]);
        assert_eq!(words("-odd"), ["profile", "switch", "--", "-odd"]);
    }

    #[test]
    fn an_action_runs_as_typed_with_the_full_id() {
        let row = json!({ "id": "a1b2c3d4-0000-4000-8000-000000000001", "displayName": "Menu Shop" });
        let words: Vec<String> =
            Group::Apps.words("pause", &row).into_iter().map(|word| word.into_string().unwrap()).collect();
        assert_eq!(words, ["apps", "pause", "a1b2c3d4-0000-4000-8000-000000000001"]);
        let words: Vec<String> = Group::Apps
            .words("delete", &json!({ "id": "-x" }))
            .into_iter()
            .map(|word| word.into_string().unwrap())
            .collect();
        assert_eq!(words, ["apps", "delete", "--", "-x"]);
        // As the tree names the group, whatever alias was typed.
        let words: Vec<String> = Group::Destinations
            .words("refresh-source", &row)
            .into_iter()
            .map(|word| word.into_string().unwrap())
            .collect();
        assert_eq!(words, ["destinations", "refresh-source", "a1b2c3d4-0000-4000-8000-000000000001"]);
    }

    #[test]
    fn every_action_is_a_visible_command_of_the_group_that_takes_the_items_id() {
        // An ID is the one argument it takes by position (`jobs tail`'s is optional, as it can follow a
        // source's or destination's latest job instead), named as the group's IDs are (`<sourceId>`);
        // `profile switch` takes the profile's name. `signals click-path` takes none: the signal is the
        // command.
        let tree = crate::cli::command();
        let states = [json!("active"), json!("inactive"), json!("deleted"), json!(null)];
        let statuses =
            [json!("running"), json!("queued"), json!("pending"), json!("failed"), json!("draft"), json!(null)];
        let mut offered = std::collections::BTreeSet::new();
        for group in GROUPS {
            let commands = group.path().split(' ').try_fold(&tree, |command, word| command.find_subcommand(word));
            let commands = commands.unwrap_or_else(|| panic!("{group:?}: no {}", group.path()));
            for (state, status) in states.iter().flat_map(|state| statuses.iter().map(move |status| (state, status))) {
                let item = json!({
                    "state": state, "status": status, "sourceAppId": "a1b2c3d4", "id": "click_path", "name": "beta",
                });
                for action in group.actions(&item) {
                    offered.insert((group.path(), action));
                    let command = commands.find_subcommand(action);
                    let command = command.unwrap_or_else(|| panic!("{group:?}: {action} is no command"));
                    assert!(!command.is_hide_set(), "{group:?}: {action} is hidden");
                    let ids: Vec<&clap::Arg> = command.get_arguments().filter(|arg| arg.is_positional()).collect();
                    if group == Group::Signals {
                        assert!(ids.is_empty(), "{group:?}: {action} takes {ids:?}");
                        continue;
                    }
                    assert_eq!(ids.len(), 1, "{group:?}: {action} takes no one ID");
                    let name = ids[0].get_value_names().and_then(|names| names.first()).map(ToString::to_string);
                    let named = |name: &str| match group {
                        Group::Profiles { .. } => name == "profile",
                        _ => name.ends_with("Id"),
                    };
                    assert!(name.as_deref().is_some_and(named), "{group:?}: {action} takes {name:?}");
                }
            }
        }
        // Every action the rules name is offered for some item.
        let lifecycle = ["delete", "pause", "resume", "update"];
        let expected = [
            ("apps", &lifecycle[..]),
            ("sources", &["delete", "pause", "resume", "sync", "update"]),
            ("destinations", &["delete", "pause", "refresh-source", "resume", "sync", "update"]),
            ("jobs", &["cancel", "tail"]),
            ("metrics", &["activate", "delete", "update"]),
            ("measurement signals", &["click-path"]),
            ("profile", &["switch"]),
        ];
        let expected = expected.iter().flat_map(|(group, actions)| actions.iter().map(move |action| (*group, *action)));
        assert_eq!(offered, expected.collect());
    }
}

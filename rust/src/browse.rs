//! Selectable lists (VE-3894, decided by Yalcin 2026-10-07, CLI 1.1): "items first then actions,
//! but still keep the actions so we can go directly to the action without the list too".
//!
//! Where the group menu opens ([`output::can_show_menu`]: stdin, stdout and stderr terminals, prompts
//! not off, stdin readable, `TERM` not `dumb`) and a list command would print its table (no `--json`,
//! no `--output`, not even an empty one), it shows the table's rows, from the same request, as an
//! arrow-key list with type-to-filter instead ([`output::choose_value`], VE-3881's list): each row the
//! table's cells as plain text on one line (`menu::one_line`, padded per column as VE-3881's rows are), the
//! table's footer after the hint (`· 57 apps`), titled with the command as the tree names it
//! (`vendo apps list`) and answered as VE-3881 answers an item (`a1b2c3d4... (Menu Shop)`). Enter on an
//! item shows exactly what the group's `get` shows of it, from `get`'s own code, spinner and request,
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
//! So far: `vendo apps list` ([`Group::Apps`]).

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// `vendo apps list`.
    Apps,
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
        return browse(client, group, rows, table).await;
    }
    table.print();
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
async fn browse(_client: &Client, _group: Group, _rows: &[Value], table: Table<'_>) -> Result<()> {
    table.print();
    Ok(())
}

#[cfg(feature = "menu")]
use menu::browse;

/// The lists themselves: only with the `menu` feature.
#[cfg(feature = "menu")]
mod menu {
    use std::ffi::OsString;

    use anyhow::Result;
    use serde_json::Value;

    use super::{CHOSEN, Group, PoisonError, Table};
    use crate::{
        ask::{Listed, named, padded},
        client::Client,
        commands::apps,
        jobs::Job,
        output::{self, ValueRow, short_id},
    };

    /// The last row of an item's action menu, which opens the list again.
    const BACK: (&str, &str) = ("back", "Back to the list");

    impl Group {
        /// The group as the tree names it, after `vendo`.
        pub(super) fn path(self) -> &'static str {
            match self {
                Group::Apps => "apps",
            }
        }

        /// The item's full ID, which its actions take.
        fn id(self, row: &Value) -> String {
            match self {
                Group::Apps => Job(row).id(),
            }
        }

        /// How the answered line names the item, as VE-3881's lists name it: `a1b2c3d4... (Menu Shop)`, on
        /// one line as its row is ([`one_line`]).
        fn answer(self, row: &Value) -> String {
            let answer = match self {
                Group::Apps => named(short_id(&self.id(row)), &Listed::Apps.name(row)),
            };
            one_line(&answer)
        }

        /// Shows what the group's `get` shows of the item, from its code, spinner and request; the item as
        /// that request returned it. A request that fails is `get`'s error (exit 1).
        async fn show(self, client: &Client, row: &Value) -> Result<Value> {
            match self {
                Group::Apps => apps::show(client, &self.id(row)).await,
            }
        }

        /// The commands of the group that apply to `item`, as `get`'s request returned it, in the order
        /// the action menu lists them. Apps: `pause` when active, `resume` when inactive (neither for
        /// any other state; vendo-web-v2 `lib/vendo/apps/queries.ts`, active|inactive, and the routes do
        /// not refuse by state), then `update` and `delete`, always (the API refuses to delete an app
        /// still in use, with the next step, VE-3756). Only visible commands of the group that take the
        /// item's ID: never a hidden one, another group's or one that takes no ID.
        pub(super) fn actions(self, item: &Value) -> Vec<&'static str> {
            match self {
                Group::Apps => {
                    let state = match Job(item).text("state").as_deref() {
                        Some("active") => Some("pause"),
                        Some("inactive") => Some("resume"),
                        _ => None,
                    };
                    state.into_iter().chain(["update", "delete"]).collect()
                }
            }
        }

        /// The words `action` runs as for the item, as typed after `vendo`: the group as the tree names
        /// it (whatever alias was typed), the action and the full ID, after `--` should it start with
        /// `-` (an ID never does).
        pub(super) fn words(self, action: &str, row: &Value) -> Vec<OsString> {
            let id = self.id(row);
            let mut words: Vec<OsString> = self.path().split(' ').map(OsString::from).collect();
            words.push(action.into());
            if id.starts_with('-') {
                words.push("--".into());
            }
            words.push(id.into());
            words
        }
    }

    /// The rows of `table` (one per item of `rows`) to choose from; the table when the terminal refuses
    /// the first list. Enter shows the item ([`Group::show`]), then its action menu ([`choose_action`]):
    /// an action is kept for `main` to run, Back opens the list again with the cursor on that item. A
    /// list or action menu that cannot run after the first leaves quietly (exit 0).
    pub(super) async fn browse(client: &Client, group: Group, rows: &[Value], table: Table<'_>) -> Result<()> {
        let plain = table.cells.iter().map(|row| row.iter().map(|cell| one_line(cell)).collect()).collect();
        let items: Vec<ValueRow> = padded(plain)
            .into_iter()
            .zip(rows)
            .map(|(shown, row)| ValueRow { shown, answer: group.answer(row) })
            .collect();
        let title = format!("vendo {} list", group.path());
        let note = Some(table.footer.as_str());
        let Some(mut at) = output::choose_value(&title, &items, note, 0) else {
            table.print();
            return Ok(());
        };
        loop {
            let item = group.show(client, &rows[at]).await?;
            match choose_action(group, &items[at].answer, &group.actions(&item)) {
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
    fn choose_action(group: Group, answer: &str, actions: &[&'static str]) -> Option<Option<&'static str>> {
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

    const GROUPS: [Group; 1] = [Group::Apps];

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
    }

    #[test]
    fn every_action_is_a_visible_command_of_the_group_that_takes_the_items_id() {
        let tree = crate::cli::command();
        let states = [json!("active"), json!("inactive"), json!("deleted"), json!(null)];
        for group in GROUPS {
            let commands = group.path().split(' ').try_fold(&tree, |command, word| command.find_subcommand(word));
            let commands = commands.unwrap_or_else(|| panic!("{group:?}: no {}", group.path()));
            for state in &states {
                for action in group.actions(&json!({ "state": state })) {
                    let command = commands.find_subcommand(action);
                    let command = command.unwrap_or_else(|| panic!("{group:?}: {action} is no command"));
                    assert!(!command.is_hide_set(), "{group:?}: {action} is hidden");
                    let ids: Vec<&clap::Arg> =
                        command.get_arguments().filter(|arg| arg.is_positional() && arg.is_required_set()).collect();
                    assert_eq!(ids.len(), 1, "{group:?}: {action} takes no one ID");
                }
            }
        }
    }
}

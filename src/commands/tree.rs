//! `vendo commands` (VE-3831). With `--json`, the whole tree with arguments and flags, read at
//! runtime from the clap tree the CLI parses with, so it cannot drift from what runs, for an agent to
//! read instead of parsing help screens; hidden commands and aliases stay out, as in the help, and so
//! does clap's `help` (VE-3893). Bare, it prints the root help, which lists every command since VE-4109
//! (Yalcin 2026-10-10: "combine help and commands"); `commands` is hidden from that help, but stays in
//! the tree where it was ([`KEPT_IN_THE_TREE`]), so the JSON agents read keeps it.

use clap::{Arg, builder::PossibleValue};
use serde_json::{Value, json};

use crate::{cli, output::print_json};

pub fn run(json: bool) {
    let mut root = cli::command();
    if json {
        print_json(&tree(&root));
    } else {
        // What `vendo help` and `vendo --help` print.
        let _ = root.print_help();
    }
}

/// `commands`, hidden from the help since VE-4109, and the command it follows in the tree, where it
/// was listed before.
const KEPT_IN_THE_TREE: (&str, &str) = ("commands", "status");

/// The commands a help screen lists under `cmd`. clap adds its `help` command only when it
/// builds the tree to parse, so the tree [`cli::command`] returns has none.
fn visible(cmd: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    cmd.get_subcommands().filter(|sub| !sub.is_hide_set())
}

/// The commands under the root in the root help's order ([`cli::HELP_SECTIONS`], which names
/// every visible command once), with [`KEPT_IN_THE_TREE`].
fn top_level(root: &clap::Command) -> Vec<&clap::Command> {
    let mut found = Vec::new();
    for name in cli::HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter()) {
        found.extend(visible(root).find(|cmd| cmd.get_name() == *name));
        if *name == KEPT_IN_THE_TREE.1 {
            found.extend(root.find_subcommand(KEPT_IN_THE_TREE.0));
        }
    }
    found
}

fn about(cmd: &clap::Command) -> Option<String> {
    cmd.get_about().map(ToString::to_string)
}

// ── --json ─────────────────────────────────────────────────────────────────

/// The root as a command node: `vendo`, its options (`--profile` and `--debug` marked global:
/// every command takes them) and its commands.
pub fn tree(root: &clap::Command) -> Value {
    let commands: Vec<Value> = top_level(root).into_iter().map(|cmd| node(cmd, cmd.get_name())).collect();
    node_with(root, "", commands)
}

/// One command: `path` is how it is typed after `vendo` (`measurement ltv list`).
fn node(cmd: &clap::Command, path: &str) -> Value {
    let commands = visible(cmd).map(|sub| node(sub, &format!("{path} {}", sub.get_name()))).collect();
    node_with(cmd, path, commands)
}

fn node_with(cmd: &clap::Command, path: &str, commands: Vec<Value>) -> Value {
    let args: Vec<&Arg> = cmd.get_arguments().filter(|arg| in_the_tree(arg)).collect();
    json!({
        "name": cmd.get_name(),
        "path": path,
        "description": about(cmd),
        "aliases": cmd.get_visible_aliases().collect::<Vec<_>>(),
        "arguments": args.iter().filter(|arg| arg.is_positional()).map(|arg| argument(arg)).collect::<Vec<_>>(),
        "options": args.iter().filter(|arg| !arg.is_positional()).map(|arg| option(arg)).collect::<Vec<_>>(),
        "commands": commands,
    })
}

/// The options and arguments the tree lists: the visible ones, as the help shows them, but not
/// `-h, --help`, which every command takes and the tree never listed (clap added it as it built the
/// tree until VE-4109 put it on each command), and with the root's `-V, --version`, which the root
/// help leaves out since VE-4109 (it lists the `version` command) while it still works.
fn in_the_tree(arg: &Arg) -> bool {
    !matches!(arg.get_action(), clap::ArgAction::Help) && (!arg.is_hide_set() || arg.get_id() == "version")
}

/// A positional argument: `<appId>` (required) or `[shell]`.
fn argument(arg: &Arg) -> Value {
    let (possible, suggested) = values(arg);
    json!({
        "name": value_name(arg),
        "description": arg.get_help().map(ToString::to_string),
        "required": arg.is_required_set(),
        "default": default(arg),
        "possibleValues": possible,
        "suggestedValues": suggested,
    })
}

/// A flag: `--state <state>`, or a switch such as `--json` (no value name).
fn option(arg: &Arg) -> Value {
    let takes_value = arg.get_action().takes_values();
    let (possible, suggested) = values(arg);
    json!({
        "name": arg.get_long().map(|long| format!("--{long}")),
        "short": arg.get_short().map(|short| format!("-{short}")),
        "valueName": takes_value.then(|| value_name(arg)),
        "description": arg.get_help().map(ToString::to_string),
        "required": arg.is_required_set(),
        "default": takes_value.then(|| default(arg)).flatten(),
        "possibleValues": possible,
        "suggestedValues": suggested,
        "global": arg.is_global_set(),
    })
}

fn value_name(arg: &Arg) -> String {
    arg.get_value_names().and_then(|names| names.first()).map_or_else(|| arg.get_id().to_string(), ToString::to_string)
}

fn default(arg: &Arg) -> Option<String> {
    arg.get_default_values().first().map(|value| value.to_string_lossy().into_owned())
}

/// The values parsing accepts (`possibleValues`: anything else is a usage error) and the ones TAB
/// offers while any string passes and the API decides (`suggestedValues`, the `Suggest` lists of
/// VE-3830, which clap sees only while [`cli::suggesting`]). A switch has neither.
fn values(arg: &Arg) -> (Vec<String>, Vec<String>) {
    if !arg.get_action().takes_values() {
        return (Vec::new(), Vec::new());
    }
    let names = |values: Vec<PossibleValue>| -> Vec<String> {
        values.iter().filter(|value| !value.is_hide_set()).map(|value| value.get_name().to_string()).collect()
    };
    let possible = names(arg.get_possible_values());
    let offered = cli::suggesting(|| names(arg.get_possible_values()));
    let suggested = offered.into_iter().filter(|value| !possible.contains(value)).collect();
    (possible, suggested)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(tree: &'a Value, path: &str) -> &'a Value {
        fn walk<'a>(node: &'a Value, path: &str) -> Option<&'a Value> {
            if node["path"] == path {
                return Some(node);
            }
            node["commands"].as_array()?.iter().find_map(|child| walk(child, path))
        }
        walk(tree, path).unwrap_or_else(|| panic!("no command {path:?}"))
    }

    fn paths(node: &Value, found: &mut Vec<String>) {
        found.push(node["path"].as_str().unwrap().to_string());
        for child in node["commands"].as_array().unwrap() {
            paths(child, found);
        }
    }

    #[test]
    fn hidden_commands_and_aliases_stay_out() {
        let tree = tree(&cli::command());
        let mut found = Vec::new();
        paths(&tree, &mut found);
        let hidden = ["catalog credential-schema", "completions", "config", "init", "integrations", "int", "help"];
        for hidden in hidden.into_iter().chain(["profile current"]) {
            assert!(!found.contains(&hidden.to_string()), "{hidden}");
        }
        assert_eq!(find(&tree, "destinations")["aliases"], json!([]), "integrations and int are hidden aliases");
        assert_eq!(find(&tree, "profile switch")["aliases"], json!([]), "use is a hidden alias");
        assert_eq!(found[..3], ["", "login", "logout"]);
        assert!(found.contains(&"commands".to_string()));
    }

    #[test]
    fn version_is_a_command_and_clap_help_is_not() {
        // VE-3893: the root help lists `help` and `version`; the tree has `version`, which runs and takes
        // --json, and leaves clap's `help` out as before.
        let tree = tree(&cli::command());
        let version = find(&tree, "version");
        assert_eq!((&version["description"], &version["arguments"]), (&json!("Print version"), &json!([])));
        let options: Vec<&Value> = version["options"].as_array().unwrap().iter().collect();
        assert_eq!(options.iter().map(|o| o["name"].as_str().unwrap()).collect::<Vec<_>>(), ["--json"]);
        let names: Vec<&str> =
            tree["commands"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert!(!names.contains(&"help"), "{names:?}");
    }

    #[test]
    fn a_command_lists_its_arguments_and_flags_with_values_and_defaults() {
        let tree = tree(&cli::command());
        let create = find(&tree, "apps create");
        assert_eq!(create["name"], "create");
        assert_eq!(create["description"], "Create a new app");
        let option = |name: &str| create["options"].as_array().unwrap().iter().find(|o| o["name"] == name).unwrap();
        assert_eq!(
            option("--type"),
            &json!({
                "name": "--type", "short": null, "valueName": "appType",
                "description": "App type (e.g. google_ads, onesignal). See: vendo catalog list",
                "required": true, "default": null, "possibleValues": [], "suggestedValues": [], "global": false,
            })
        );
        assert_eq!(option("--role")["default"], "source");
        assert_eq!(option("--role")["suggestedValues"], json!(["source", "destination"]));
        assert_eq!(
            option("--json"),
            &json!({
                "name": "--json", "short": null, "valueName": null, "description": "Output raw JSON",
                "required": false, "default": null, "possibleValues": [], "suggestedValues": [], "global": false,
            })
        );
        let delete = find(&tree, "apps delete");
        assert_eq!(
            delete["arguments"],
            json!([{ "name": "appId", "description": null, "required": true, "default": null,
                     "possibleValues": [], "suggestedValues": [] }])
        );
        let yes = delete["options"].as_array().unwrap().iter().find(|o| o["name"] == "--yes").unwrap();
        assert_eq!(yes["short"], "-y");
        // The one list parsing enforces, on `completions`, hidden from the tree since VE-4109.
        let root = cli::command();
        let completions = node(root.find_subcommand("completions").unwrap(), "completions");
        let shell = &completions["arguments"][0];
        assert_eq!((&shell["name"], &shell["required"]), (&json!("shell"), &json!(false)));
        assert_eq!(
            (&shell["possibleValues"], &shell["suggestedValues"]),
            (&json!(["bash", "zsh", "fish"]), &json!([]))
        );
    }

    #[test]
    fn the_root_carries_the_global_options() {
        let tree = tree(&cli::command());
        assert_eq!((&tree["name"], &tree["path"]), (&json!("vendo"), &json!("")));
        let options: Vec<(&str, bool)> = tree["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| (o["name"].as_str().unwrap(), o["global"].as_bool().unwrap()))
            .collect();
        assert_eq!(options, [("--profile", true), ("--debug", true), ("--version", false)]);
        // Commands list their own flags only.
        let workspace = find(&tree, "workspace");
        assert_eq!(workspace["options"].as_array().unwrap().len(), 1, "{workspace}");
        // Its old names are hidden aliases: not in the tree (VE-3891).
        assert_eq!(workspace["aliases"], json!([]));
    }

    #[test]
    fn the_top_level_follows_the_root_help_and_keeps_commands_where_it_was() {
        let tree = tree(&cli::command());
        let names: Vec<&str> =
            tree["commands"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        // All but clap's `help`, which the root help lists (VE-3893) and the tree leaves out, and with
        // `commands`, which the help leaves out since VE-4109, after `status` as before.
        let mut sections: Vec<&str> = cli::HELP_SECTIONS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .filter(|name| *name != "help")
            .collect();
        let status = sections.iter().position(|name| *name == "status").unwrap();
        sections.insert(status + 1, "commands");
        assert_eq!(names, sections);
        let commands = find(&tree, "commands");
        assert_eq!(
            commands["description"],
            "List every command, or with --json the command tree with arguments and flags"
        );
        assert_eq!(commands["options"][0]["name"], "--json");
        assert!(cli::command().find_subcommand("commands").unwrap().is_hide_set(), "not in the help");
    }
}

//! `vendo commands` (VE-3831): every command the help screens show, read at runtime from the clap
//! tree the CLI parses with, so the list cannot drift from what runs. Bare, one line per command
//! with its description; with `--json`, the whole tree with arguments and flags, for an agent to
//! read instead of parsing help screens. Hidden commands and aliases stay out, as in the help.

use clap::{Arg, builder::PossibleValue};
use serde_json::{Value, json};

use crate::{cli, output::print_json};

pub fn run(json: bool) {
    let root = cli::command();
    if json {
        print_json(&tree(&root));
    } else {
        print!("{}", list(&root));
    }
}

/// The commands a help screen lists under `cmd`. clap adds its `help` command only when it
/// builds the tree to parse, so the tree [`cli::command`] returns has none.
fn visible(cmd: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    cmd.get_subcommands().filter(|sub| !sub.is_hide_set())
}

/// The commands under the root in the root help's order ([`cli::HELP_SECTIONS`], which names
/// every visible command once).
fn top_level(root: &clap::Command) -> Vec<&clap::Command> {
    let names = cli::HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter());
    names.filter_map(|name| visible(root).find(|cmd| cmd.get_name() == *name)).collect()
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
    let args: Vec<&Arg> = cmd.get_arguments().filter(|arg| !arg.is_hide_set()).collect();
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

// ── the text list ──────────────────────────────────────────────────────────

/// One line per command that runs (`apps list`, not the `apps` group), in the root help's order:
/// the path, then its description.
fn list(root: &clap::Command) -> String {
    let mut rows = Vec::new();
    for cmd in top_level(root) {
        runnable(cmd, cmd.get_name().to_string(), &mut rows);
    }
    let width = rows.iter().map(|(path, _)| path.len()).max().unwrap_or(0);
    rows.iter().map(|(path, about)| format!("{}\n", format!("{path:width$}  {about}").trim_end())).collect()
}

fn runnable(cmd: &clap::Command, path: String, rows: &mut Vec<(String, String)>) {
    if !cmd.has_subcommands() {
        rows.push((path, about(cmd).unwrap_or_default()));
        return;
    }
    for sub in visible(cmd) {
        runnable(sub, format!("{path} {}", sub.get_name()), rows);
    }
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
        for hidden in ["catalog credential-schema", "config", "init", "integrations", "int", "help", "profile current"]
        {
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
        let text = list(&cli::command());
        assert!(text.lines().any(|line| line.starts_with("version ") && line.ends_with("  Print version")), "{text}");
        assert!(!text.lines().any(|line| line.starts_with("help ")), "{text}");
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
        // The one list parsing enforces.
        let shell = &find(&tree, "completions")["arguments"][0];
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
        let whoami = find(&tree, "whoami");
        assert_eq!(whoami["options"].as_array().unwrap().len(), 1, "{whoami}");
    }

    #[test]
    fn the_top_level_follows_the_root_help() {
        let tree = tree(&cli::command());
        let names: Vec<&str> =
            tree["commands"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        // All but clap's `help`, which the root help lists (VE-3893) and the tree leaves out.
        let sections: Vec<&str> = cli::HELP_SECTIONS.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        let sections: Vec<&str> = sections.into_iter().filter(|name| *name != "help").collect();
        assert_eq!(names, sections);
    }

    #[test]
    fn the_text_lists_every_runnable_command_with_its_description() {
        let root = cli::command();
        let text = list(&root);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].split("  ").next(), Some("login"));
        let path_of = |line: &str| line.split("  ").next().unwrap().trim_end().to_string();
        let listed: Vec<String> = lines.iter().map(|line| path_of(line)).collect();
        // The runnable commands of the JSON tree, in the same order.
        fn leaves(node: &Value, found: &mut Vec<String>) {
            let children = node["commands"].as_array().unwrap();
            if children.is_empty() {
                found.push(node["path"].as_str().unwrap().to_string());
            }
            for child in children {
                leaves(child, found);
            }
        }
        let mut expected = Vec::new();
        leaves(&tree(&root), &mut expected);
        assert_eq!(listed, expected);
        let refresh = lines.iter().find(|line| line.starts_with("destinations refresh-source ")).unwrap();
        assert!(
            refresh.ends_with(
                "  Check source-data availability for a window and trigger top-up imports for missing ranges"
            )
        );
        // Descriptions start in one column.
        let column = |line: &str| line.find(char::is_uppercase).unwrap();
        assert!(lines.iter().all(|line| column(line) == column(lines[0])), "{text}");
    }
}

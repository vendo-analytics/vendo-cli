//! Snapshot tests (VE-3824). Once 1.0.0 deletes the TypeScript CLI, the
//! parity harness no longer catches regressions, so the Rust CLI's behaviour
//! is recorded here with insta, under `rust/tests/snapshots/`:
//!
//! - `help/`: every `vendo … --help` screen, exactly as printed. The list
//!   comes from walking the command lists of the real help output (the root's
//!   sections, each group's `Commands:`), so a new command is recorded
//!   automatically and a removed one leaves a stale snapshot that fails the run.
//! - `output/`: the table and `--json` output of every command that prints
//!   data, and the confirmation of write commands, run against the local stub
//!   with the synthetic account below; and the three completion scripts, whole.
//!
//! What varies between machines or runs is replaced before comparing: the stub
//! URL, HOME, the binary's path, the CLI version, request IDs and the few
//! timestamps relative to now (`[now-…]`).
//!
//! After a deliberate change, from `rust/`: `INSTA_UPDATE=always cargo test --test cli`
//! rewrites the snapshots and deletes stale ones; review `git diff tests/snapshots`.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use jiff::SignedDuration;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param, query_param_is_missing},
};

use super::{
    CLOSED, COLUMN_ID, DICTIONARY, M1, Sandbox, event_item, login_at_browser, methodologies, metric, model,
    mount_sign_in_page, page, serve, text,
};

const SNAPSHOTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots");
const UPDATE: &str = "INSTA_UPDATE=always cargo test --test cli";

/// `INSTA_UPDATE=always` (or `1`, `force`): insta rewrites snapshots in place.
fn updating() -> bool {
    matches!(std::env::var("INSTA_UPDATE").as_deref(), Ok("always" | "1" | "force"))
}

/// Checks the snapshots of one folder and reports every mismatch at the end,
/// so one run writes every `.snap.new` (or, when updating, every snapshot)
/// instead of stopping at the first. It owns the folder's `<prefix>*.snap`
/// files: one it did not check is stale.
struct Recorder {
    dir: PathBuf,
    prefix: String,
    checked: BTreeSet<String>,
    failed: Vec<String>,
}

impl Recorder {
    fn new(folder: &str, prefix: &str) -> Self {
        let dir = Path::new(SNAPSHOTS).join(folder);
        Recorder { dir, prefix: prefix.to_string(), checked: BTreeSet::new(), failed: Vec::new() }
    }

    fn check(&mut self, name: &str, contents: &str) {
        assert!(name.starts_with(&self.prefix), "snapshot {name} must start with {}", self.prefix);
        assert!(self.checked.insert(name.to_string()), "snapshot {name} is recorded twice");
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(&self.dir);
        settings.set_prepend_module_to_snapshot(false);
        settings.set_omit_expression(true);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            settings.bind(|| insta::assert_snapshot!(name, contents));
        }));
        if result.is_err() {
            self.failed.push(format!("{name}: output differs from the snapshot (or it has none yet)"));
        }
    }

    fn finish(mut self) {
        let entries = std::fs::read_dir(&self.dir).map(|dir| dir.flatten().collect::<Vec<_>>()).unwrap_or_default();
        for entry in entries {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(name) = file.strip_suffix(".snap") else { continue };
            if !name.starts_with(&self.prefix) || self.checked.contains(name) {
                continue;
            }
            if updating() {
                std::fs::remove_file(entry.path()).unwrap();
            } else {
                self.failed.push(format!("{name}: stale, nothing records it any more (delete {file})"));
            }
        }
        assert!(
            self.failed.is_empty(),
            "{} snapshot(s) in {} need attention:\n  {}\nReview the .snap.new files (`cargo insta review`), or accept every change with `{UPDATE}` from rust/ and review `git diff tests/snapshots`.",
            self.failed.len(),
            self.dir.display(),
            self.failed.join("\n  ")
        );
    }
}

// ── help screens ────────────────────────────────────────────────────────────

/// The headings that list commands: a group's `Commands:`, and the four sections of the root
/// help (VE-3827), which name every command with the commands under it.
const COMMAND_LISTS: [&str; 5] = ["Commands:", "Getting started:", "Data pipeline:", "Data catalog:", "Account:"];

/// The names in a help screen's command lists. Entries sit two spaces in; anything indented
/// further continues a description, or lists the commands under a group in the root help.
fn subcommands(screen: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut listing = false;
    for line in screen.lines() {
        if COMMAND_LISTS.contains(&line) {
            listing = true;
        } else if line.is_empty() {
            listing = false;
        } else if let Some(entry) = line.strip_prefix("  ").filter(|rest| listing && !rest.starts_with(' ')) {
            names.extend(entry.split_whitespace().next().map(str::to_string));
        }
    }
    names
}

/// The first word the CLI no longer uses for its own objects, if `screen` has one. The customer
/// words follow vendo-web-v2's glossary (`apps/web/CONTEXT.md`, VE-3828): destination, not
/// integration; app, not app connection; platform, not integration type. `int` was the
/// destinations group's visible alias. The `jobs` flag `--integration <integrationId>` keeps
/// its name: flags were not renamed.
fn retired_word(screen: &str) -> Option<&'static str> {
    let lower = screen.replace("--integration <integrationId>", "").to_lowercase();
    if let Some(word) = ["integration", "app connection"].into_iter().find(|word| lower.contains(word)) {
        return Some(word);
    }
    lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| word == "int").then_some("int")
}

/// Records `vendo <path> --help` and, depth first in help order, every
/// command it lists. Returns the number of screens.
fn record_help(sandbox: &Sandbox, recorder: &mut Recorder, path: &[String]) -> usize {
    let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
    args.push("--help");
    let out = sandbox.run(&args);
    let screen = text(&out.stdout);
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), String::new()), "vendo {}", args.join(" "));
    assert_eq!(retired_word(&screen), None, "vendo {} uses a retired word", args.join(" "));
    // `help/vendo.snap` for the root, `help/measurement__ltv__list.snap` for `vendo measurement ltv list`.
    recorder.check(&if path.is_empty() { "vendo".to_string() } else { path.join("__") }, &screen);
    let names = subcommands(&screen);
    // `vendo help <command>` stays, but no screen lists clap's `help` (VE-3827).
    assert!(!names.contains(&"help".to_string()), "vendo {} lists a help command", args.join(" "));
    let mut count = 1;
    for name in names {
        let child: Vec<String> = path.iter().cloned().chain([name]).collect();
        count += record_help(sandbox, recorder, &child);
    }
    count
}

/// Commands hidden from the help screens that still run as themselves (not aliases), so the
/// walk does not find them (VE-3827).
const HIDDEN_COMMANDS: [&[&str]; 1] = [&["catalog", "credential-schema"]];

#[test]
fn every_help_screen_matches_its_snapshot() {
    let sandbox = Sandbox::new(CLOSED);
    let mut recorder = Recorder::new("help", "");
    let mut count = record_help(&sandbox, &mut recorder, &[]);
    for path in HIDDEN_COMMANDS {
        let path: Vec<String> = path.iter().map(|word| word.to_string()).collect();
        let (parent, name) = path.split_at(path.len() - 1);
        let args: Vec<&str> = parent.iter().map(String::as_str).chain(["--help"]).collect();
        let siblings = subcommands(&text(&sandbox.run(&args).stdout));
        assert!(!siblings.contains(&name[0]), "vendo {} is listed: not hidden", path.join(" "));
        count += record_help(&sandbox, &mut recorder, &path);
    }
    eprintln!("checked {count} help screens");
    recorder.finish();
}

#[test]
fn the_old_destinations_names_print_the_same_help() {
    // `integrations` and `int` are hidden aliases of `destinations` (VE-3828).
    let sandbox = Sandbox::new(CLOSED);
    let help = |group: &str, sub: Option<&str>| {
        let args: Vec<&str> = [Some(group), sub, Some("--help")].into_iter().flatten().collect();
        let out = sandbox.run(&args);
        (out.status.code(), out.stdout, out.stderr)
    };
    let subs = subcommands(&text(&help("destinations", None).1));
    assert!(subs.len() >= 9, "{subs:?}");
    for sub in [None].into_iter().chain(subs.iter().map(|s| Some(s.as_str()))) {
        let expected = help("destinations", sub);
        assert_eq!(expected.0, Some(0));
        for old in ["integrations", "int"] {
            assert!(help(old, sub) == expected, "vendo {old} {} --help", sub.unwrap_or_default());
        }
    }
    let root = text(&sandbox.run(&["--help"]).stdout);
    assert!(subcommands(&root).contains(&"destinations".to_string()));
    assert_eq!(retired_word(&root), None);
}

#[test]
fn retired_words_are_found_in_help_text() {
    assert_eq!(retired_word("  integrations  Manage data export integrations [alias: int]"), Some("integration"));
    assert_eq!(retired_word("List all app connections"), Some("app connection"));
    assert_eq!(retired_word("  int  Manage data export destinations"), Some("int"));
    assert_eq!(retired_word("--limit <n>  Number of results (internal, print)"), None);
    assert_eq!(retired_word("      --integration <integrationId>  Filter by destination ID"), None);
}

#[test]
fn the_help_walk_reads_clap_command_lists() {
    let screen = "About\n\nUsage: vendo x <COMMAND>\n\nCommands:\n  list          List things [alias: ls]\n  get-one       Get one\n                that wraps\n\nOptions:\n  -h, --help  Print help\n";
    assert_eq!(subcommands(screen), ["list", "get-one"]);
    assert!(subcommands("Usage: vendo x\n\nOptions:\n  -h, --help  Print help\n").is_empty());
    let root = "About\n\nUsage: vendo <COMMAND>\n\nGetting started:\n  login  Log in\n\nData pipeline:\n  apps   Manage apps\n         list, get\n\nOptions:\n  -h, --help  Print help\n";
    assert_eq!(subcommands(root), ["login", "apps"]);
}

/// The commands under `path` as its help screens list them, as typed after it: `list`, `ltv cohort`.
fn listed_paths(sandbox: &Sandbox, path: &[&str]) -> Vec<String> {
    let screen = text(&sandbox.run(&[path, &["--help"]].concat()).stdout);
    let mut paths = Vec::new();
    for name in subcommands(&screen) {
        let below = listed_paths(sandbox, &[path, &[name.as_str()]].concat());
        if below.is_empty() {
            paths.push(name);
        } else {
            paths.extend(below.into_iter().map(|p| format!("{name} {p}")));
        }
    }
    paths
}

#[test]
fn the_root_help_names_every_command_and_the_commands_under_it() {
    // One sectioned root help (VE-3827): every command, each group followed by the commands its
    // own screens list (the walk above records those screens), so nothing is only one level down.
    let sandbox = Sandbox::new(CLOSED);
    let root = text(&sandbox.run(&["--help"]).stdout);
    let lines: Vec<&str> = root.lines().collect();
    let sections: Vec<&str> = lines.iter().copied().filter(|line| COMMAND_LISTS.contains(line)).collect();
    assert_eq!(sections, COMMAND_LISTS[1..], "the root help's sections, in order");
    let names = subcommands(&root);
    for name in ["login", "destinations", "measurement", "profile"] {
        assert!(names.contains(&name.to_string()), "{name}: {names:?}");
    }
    assert!(!names.contains(&"config".to_string()), "{names:?}");
    for name in &names {
        let row = lines.iter().position(|line| line.starts_with(&format!("  {name} "))).unwrap();
        let below = lines[row + 1..].iter().take_while(|line| line.starts_with("     ")).copied();
        let joined = below.collect::<Vec<_>>().join(" ");
        let listed: Vec<&str> = joined.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        assert_eq!(listed, listed_paths(&sandbox, &[name]), "vendo {name}");
    }
}

#[test]
fn bare_vendo_prints_the_root_help_and_exits_2() {
    let sandbox = Sandbox::new(CLOSED);
    let help = sandbox.run(&["--help"]);
    let bare = sandbox.run(&[]);
    assert_eq!((bare.status.code(), text(&bare.stdout)), (Some(2), String::new()));
    assert_eq!(text(&bare.stderr), text(&help.stdout));
}

/// Every group as typed, depth first in help order: `apps`, `measurement ltv`.
fn groups(sandbox: &Sandbox, path: &[&str], found: &mut Vec<Vec<String>>) {
    let screen = text(&sandbox.run(&[path, &["--help"]].concat()).stdout);
    let names = subcommands(&screen);
    if !path.is_empty() && !names.is_empty() {
        found.push(path.iter().map(|word| word.to_string()).collect());
    }
    for name in &names {
        groups(sandbox, &[path, &[name.as_str()]].concat(), found);
    }
}

#[test]
fn a_bare_group_without_a_terminal_prints_its_help_and_exits_2() {
    // On a terminal a bare group opens a menu of its commands (VE-3826); without one (a pipe, CI,
    // an agent) it is the usage error it was: the group's help screen on stderr, exit 2.
    let sandbox = Sandbox::new(CLOSED);
    let mut found = Vec::new();
    groups(&sandbox, &[], &mut found);
    // The hidden old names of `destinations` and `profile` (VE-3827, VE-3828).
    found.extend([vec!["integrations".to_string()], vec!["int".to_string()], vec!["config".to_string()]]);
    assert!(found.len() >= 16, "{found:?}");
    for group in &found {
        let group: Vec<&str> = group.iter().map(String::as_str).collect();
        let help = sandbox.run(&[&group[..], &["--help"]].concat());
        let bare = sandbox.run(&group);
        assert_eq!((bare.status.code(), text(&bare.stdout)), (Some(2), String::new()), "vendo {group:?}");
        assert_eq!(text(&bare.stderr), text(&help.stdout), "vendo {group:?}");
    }
    // With a global option after it, clap's other wording of the same error, as before.
    let bare = sandbox.run(&["apps", "--debug"]);
    assert_eq!((bare.status.code(), text(&bare.stdout)), (Some(2), String::new()));
    assert_eq!(
        text(&bare.stderr),
        "error: 'vendo apps' requires a subcommand but one was not provided\n  [subcommands: list, diagnose, get, pause, resume, delete, create, update]\n\nUsage: vendo apps [OPTIONS] <COMMAND>\n\nFor more information, try '--help'.\n"
    );
}

#[test]
fn moved_commands_print_their_targets_help() {
    // `config` moved under `profile`, `profile current` and `config show` became `whoami`, and
    // `config reset` `logout --all` (VE-3827): the old paths print exactly what the new ones do.
    let sandbox = Sandbox::new(CLOSED);
    let run = |args: &[&str]| {
        let out = sandbox.run(args);
        (out.status.code(), out.stdout, out.stderr)
    };
    for (old, new) in [
        // No command: the group's usage error, with its help.
        (&["config"][..], &["profile"][..]),
        (&["config", "--help"], &["profile", "--help"]),
        (&["config", "set", "--help"], &["profile", "set", "--help"]),
        (&["config", "use", "--help"], &["profile", "switch", "--help"]),
        (&["config", "list", "--help"], &["profile", "list", "--help"]),
        (&["config", "show", "--help"], &["whoami", "--help"]),
        (&["config", "reset", "--help"], &["logout", "--help"]),
        (&["profile", "current", "--help"], &["whoami", "--help"]),
        (&["help", "config", "set"], &["help", "profile", "set"]),
        (&["help", "profile", "current"], &["help", "whoami"]),
        (&["help", "config", "reset"], &["help", "logout"]),
        // Usage errors name the command that runs.
        (&["config", "set", "--bogus"], &["profile", "set", "--bogus"]),
        (&["profile", "current", "--bogus"], &["whoami", "--bogus"]),
        (&["config", "show", "--all"], &["whoami", "--all"]),
        (&["config", "reset", "--bogus"], &["logout", "--all", "--bogus"]),
        // A group's `help` command, no longer listed, is the root's.
        (&["apps", "help"], &["apps", "--help"]),
        (&["apps", "help", "list"], &["apps", "list", "--help"]),
        (&["measurement", "ltv", "help", "cohort"], &["measurement", "ltv", "cohort", "--help"]),
        (&["help", "apps", "list"], &["apps", "list", "--help"]),
    ] {
        let expected = run(new);
        assert!(run(old) == expected, "vendo {} differs from vendo {}", old.join(" "), new.join(" "));
    }
    assert_eq!(run(&["config", "set", "--help"]).0, Some(0));
    // Not old paths: they stay unknown.
    for args in [&["profile", "show"][..], &["profile", "reset"], &["config", "current"]] {
        assert_eq!(run(args).0, Some(2), "vendo {}", args.join(" "));
    }
}

#[test]
fn init_is_a_hidden_alias_of_login() {
    // `login` does what `init` did since CLI 1.1 (VE-3825): `init` is a hidden alias, so its help and
    // usage errors are login's, and the root help lists only `login`. The output tests in cli.rs and
    // `account_and_profile_output` check that it runs exactly like `login`.
    let sandbox = Sandbox::new(CLOSED);
    let run = |args: &[&str]| {
        let out = sandbox.run(args);
        (out.status.code(), out.stdout, out.stderr)
    };
    for (old, new) in [
        (&["init", "--help"][..], &["login", "--help"][..]),
        (&["help", "init"], &["help", "login"]),
        (&["init", "--bogus"], &["login", "--bogus"]),
        (&["init", "--api-key"], &["login", "--api-key"]),
        (&["--profile", "beta", "init", "-h"], &["--profile", "beta", "login", "-h"]),
    ] {
        let expected = run(new);
        assert!(run(old) == expected, "vendo {} differs from vendo {}", old.join(" "), new.join(" "));
    }
    assert_eq!(run(&["init", "--help"]).0, Some(0));
    let root = text(&sandbox.run(&["--help"]).stdout);
    let names = subcommands(&root);
    assert!(names.contains(&"login".to_string()) && !names.contains(&"init".to_string()), "{names:?}");
    assert!(!root.contains("init"), "{root}");
}

#[test]
fn completions_offer_no_moved_command_and_no_group_help() {
    // Hidden aliases (`config`, `use`) and the rewritten paths (`current`, `show`, `reset`) are
    // not commands in the tree's lists, so no completion script offers them. A command hidden
    // with `hide` still is (clap_complete lists it): `catalog credential-schema`.
    let sandbox = Sandbox::new(CLOSED);
    let bash = text(&sandbox.run(&["completions", "bash"]).stdout);
    let offered = |opts: &str| bash.lines().any(|line| line.trim() == format!("opts=\"{opts}\""));
    assert!(offered("-h --profile --debug --help list switch set"), "vendo profile");
    assert!(offered("-h --profile --debug --help list get credential-schema"), "vendo catalog");
    assert!(offered("-h --profile --debug --help list diagnose get pause resume delete create update"), "vendo apps");
    let root = bash.lines().find(|line| line.trim().starts_with("opts=\"-V -h")).unwrap();
    for word in ["config", "integrations", "int", "init"] {
        assert!(!root.split(['"', ' ']).any(|w| w == word), "vendo {word}: {root}");
    }
}

// ── command output ──────────────────────────────────────────────────────────

/// The groups of `output/`, one test each. A Session's recorder only owns its
/// own group's files, so a renamed or removed group would leave its snapshots
/// behind unnoticed; [`every_output_snapshot_has_a_group`] catches them.
const OUTPUT_GROUPS: [&str; 12] = [
    "account",
    "apps",
    "catalog",
    "commands",
    "completions",
    "destinations",
    "dictionary",
    "jobs",
    "measurement",
    "metrics",
    "models",
    "sources",
];

#[test]
fn every_output_snapshot_has_a_group() {
    let dir = Path::new(SNAPSHOTS).join("output");
    let orphans: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|file| file.ends_with(".snap"))
        .filter(|file| !OUTPUT_GROUPS.iter().any(|group| file.starts_with(&format!("{group}__"))))
        .collect();
    assert!(orphans.is_empty(), "snapshots in {} that no test records (delete them): {orphans:?}", dir.display());
}

/// A sandbox whose profiles point at a stub serving the synthetic account,
/// recording into `output/<group>__<name>.snap`.
struct Session {
    group: &'static str,
    sandbox: Sandbox,
    /// (text, placeholder), replaced in order.
    redactions: Vec<(String, String)>,
    recorder: Recorder,
}

impl Session {
    async fn start(group: &'static str) -> (MockServer, Session) {
        assert!(OUTPUT_GROUPS.contains(&group), "add {group} to OUTPUT_GROUPS");
        let server = MockServer::start().await;
        let sandbox = Sandbox::new(&server.uri());
        let home = sandbox.home.path().to_path_buf();
        let bin = std::fs::canonicalize(env!("CARGO_BIN_EXE_vendo")).unwrap();
        let shown = |p: &Path| p.display().to_string();
        let redactions = vec![
            // The canonical HOME (macOS: /private/var/…) contains the plain one: replace it first.
            (shown(&std::fs::canonicalize(&home).unwrap()), "[home]".to_string()),
            (shown(&home), "[home]".to_string()),
            (shown(&bin), "[bin]".to_string()),
            (shown(bin.parent().unwrap()), "[bin-dir]".to_string()),
            (server.uri(), "[stub]".to_string()),
            (env!("CARGO_PKG_VERSION").to_string(), "[version]".to_string()),
        ];
        let mut session =
            Session { group, sandbox, redactions, recorder: Recorder::new("output", &format!("{group}__")) };
        mount_account(&server, &mut session).await;
        (server, session)
    }

    /// A timestamp `ago` before now, as the API writes it; snapshots show `[now-<label>]`.
    /// Durations stay clear of a unit boundary so "2h ago" holds for the whole run.
    fn ago(&mut self, label: &str, ago: SignedDuration) -> String {
        let at = (jiff::Timestamp::now() - ago).strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        self.redactions.push((at.clone(), format!("[now-{label}]")));
        at
    }

    fn home(&self) -> &Path {
        self.sandbox.home.path()
    }

    /// Write `contents` to `<HOME>/<name>` and return the path.
    fn file(&self, name: &str, contents: &str) -> String {
        let file = self.home().join(name);
        std::fs::write(&file, contents).unwrap();
        file.display().to_string()
    }

    fn record(&mut self, name: &str, args: &[&str]) {
        self.record_with(name, args, |_| {});
    }

    /// Run `vendo <args>`: exit code, stdout and stderr, as printed.
    fn output(&self, args: &[&str]) -> (Option<i32>, Vec<u8>, Vec<u8>) {
        let out = self.sandbox.command(args).output().unwrap();
        (out.status.code(), out.stdout, out.stderr)
    }

    /// Run `vendo <args>` and check exit code, stdout and stderr against `output/<group>__<name>.snap`.
    fn record_with(&mut self, name: &str, args: &[&str], adjust: impl FnOnce(&mut Command)) {
        let mut cmd = self.sandbox.command(args);
        adjust(&mut cmd);
        let out = cmd.output().unwrap();
        self.record_output(name, args, (out.status.code(), text(&out.stdout), text(&out.stderr)));
    }

    /// Check what `vendo <args>` printed, run elsewhere, against `output/<group>__<name>.snap`.
    fn record_output(&mut self, name: &str, args: &[&str], (code, stdout, stderr): (Option<i32>, String, String)) {
        let code = code.map_or_else(|| "killed by a signal".to_string(), |c| c.to_string());
        let mut shown = format!("$ vendo {}\nexit: {code}\n--- stdout ---\n{stdout}", shell_words(args));
        if !stderr.is_empty() {
            if !shown.ends_with('\n') {
                shown.push('\n');
            }
            shown.push_str(&format!("--- stderr ---\n{stderr}"));
        }
        let shown = self.redact(&shown);
        self.recorder.check(&format!("{}__{name}", self.group), &shown);
    }

    fn redact(&self, shown: &str) -> String {
        let mut shown = self.redactions.iter().fold(shown.to_string(), |s, (from, to)| s.replace(from, to));
        // Request IDs: `cli-` and a random UUID.
        let mut at = 0;
        while let Some(found) = shown[at..].find("cli-").map(|i| at + i) {
            let id = found + 4;
            let uuid = shown.get(id..id + 36).filter(|s| s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            if uuid.is_some() {
                shown.replace_range(id..id + 36, "[request-id]");
            }
            at = id;
        }
        shown
    }

    fn finish(self) {
        self.recorder.finish();
    }
}

/// The command line as it would be typed (arguments with spaces or quotes are quoted).
fn shell_words(args: &[&str]) -> String {
    let word = |arg: &&str| {
        if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=,@".contains(c)) {
            arg.to_string()
        } else {
            format!("'{}'", arg.replace('\'', r"'\''"))
        }
    };
    args.iter().map(word).collect::<Vec<_>>().join(" ")
}

// ── the synthetic account ───────────────────────────────────────────────────
// Clearly fake IDs and names, shaped like vendo-web-v2's `/api/v1` serializers
// (AppRecord/AppDetail, SourceRecord/SourceDetail, IntegrationRecord/Detail,
// JobRecord, the catalog and `/me` routes) and its web-app metrics and
// measurement routes. Dates are fixed, more than 30 days back (the CLI prints
// them as dates); the few relative to now exercise "2h ago".

const ACCOUNT: &str = "/api/v1/accounts/acct-alpha";
const APP_SHOP: &str = "a0000001-0000-4000-8000-000000000001";
const APP_ADS: &str = "a0000002-0000-4000-8000-000000000002";
const APP_WAREHOUSE: &str = "a0000003-0000-4000-8000-000000000003";
const APP_NEW: &str = "a0000004-0000-4000-8000-000000000004";
const SRC_SHOP: &str = "50000001-0000-4000-8000-000000000001";
const SRC_ADS: &str = "50000002-0000-4000-8000-000000000002";
const SRC_NEW: &str = "50000003-0000-4000-8000-000000000003";
const INT_ORDERS: &str = "c0000001-0000-4000-8000-000000000001";
const INT_ADS: &str = "c0000002-0000-4000-8000-000000000002";
const INT_NEW: &str = "c0000003-0000-4000-8000-000000000003";
const JOB_RUNNING: &str = "b0000001-0000-4000-8000-000000000001";
const JOB_QUEUED: &str = "b0000002-0000-4000-8000-000000000002";
const JOB_DONE: &str = "b0000003-0000-4000-8000-000000000003";
const JOB_FAILED: &str = "b0000004-0000-4000-8000-000000000004";
const JOB_NEW: &str = "b0000005-0000-4000-8000-000000000005";
const JOB_MISSING: &str = "b0000009-0000-4000-8000-000000000009";
const CREATED: &str = "2026-01-02T08:00:00.000Z";
const SYNCED: &str = "2026-01-05T09:07:03.000Z";
const ACTIVE: &str = "running,pending,queued";

fn pagination(items: Vec<Value>) -> Value {
    let total = items.len();
    json!({ "data": items, "meta": { "pagination": { "total": total, "limit": 20, "offset": 0, "hasMore": false } } })
}

fn data(value: Value) -> Value {
    json!({ "data": value })
}

fn not_found(resource: &str) -> Value {
    json!({ "error": { "code": "NOT_FOUND", "message": format!("{resource} not found") } })
}

/// Merge `over` into `base`, keeping the base's key order.
fn with(mut base: Value, over: Value) -> Value {
    base.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    base
}

async fn jobs_matching(server: &MockServer, query: &[(&str, &str)], body: Value) {
    let mut mock = Mock::given(method("GET")).and(path(format!("{ACCOUNT}/jobs")));
    for (key, value) in query {
        mock = mock.and(query_param(*key, *value));
    }
    mock.respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(server).await;
}

async fn mount_account(server: &MockServer, s: &mut Session) {
    // ── /me ──
    let me = json!({
        "accountId": "acct-alpha", "accountName": "Demo Account (synthetic)", "accountSlug": "demo-account",
        "pictureUrl": null, "apiKeyId": "key_fake_0001", "scopes": [],
        "bigquery": { "projectId": "demo-project", "prodDatasetId": "demo_prod", "sourceDatasetId": "demo_source" },
    });
    serve(server, "GET", "/api/v1/me", 200, data(me)).await;

    // ── apps ──
    let shop = json!({
        "id": APP_SHOP, "accountId": "acct-alpha", "appType": "shopify", "displayName": "Demo Shop",
        "permissions": ["performance_data"], "roles": ["source"], "state": "active", "errorMessage": null,
        "lastSyncAt": s.ago("2h30m", SignedDuration::from_mins(150)), "accessStatus": "connected",
        "accessStatusCheckedAt": s.ago("3h30m", SignedDuration::from_mins(210)), "accessStatusReason": null,
        "createdAt": CREATED, "updatedAt": SYNCED,
    });
    let ads = json!({
        "id": APP_ADS, "accountId": "acct-alpha", "appType": "google_ads", "displayName": "Demo Ads",
        "permissions": ["performance_data", "send_conversions"], "roles": ["source", "destination"], "state": "active",
        "errorMessage": "Token expired for the synthetic test account", "lastSyncAt": SYNCED,
        "accessStatus": "auth_expired", "accessStatusCheckedAt": "2026-01-06T00:00:00.000Z",
        "accessStatusReason": "OAuth token expired (synthetic)", "createdAt": CREATED, "updatedAt": SYNCED,
    });
    let warehouse = json!({
        "id": APP_WAREHOUSE, "accountId": "acct-alpha", "appType": "bigquery", "displayName": "Demo Warehouse",
        "permissions": ["send_conversions"], "roles": ["destination"], "state": "inactive", "errorMessage": null,
        "lastSyncAt": null, "accessStatus": null, "accessStatusCheckedAt": null, "accessStatusReason": null,
        "createdAt": CREATED, "updatedAt": SYNCED,
    });
    let apps = [shop, ads, warehouse];
    serve(server, "GET", &format!("{ACCOUNT}/apps"), 200, pagination(apps.to_vec())).await;
    let detail = json!({
        "config": { "customer_id": "000-000-0000" }, "consecutiveFailureCount": 2, "appTimeZone": "UTC",
        "appAccountId": "000-000-0000", "appAccountCurrency": "USD",
    });
    for app in &apps {
        let id = app["id"].as_str().unwrap();
        serve(server, "GET", &format!("{ACCOUNT}/apps/{id}"), 200, data(with(app.clone(), detail.clone()))).await;
    }
    let created = json!({
        "id": APP_NEW, "accountId": "acct-alpha", "appType": "shopify", "displayName": "Demo Shop 2",
        "permissions": ["performance_data"], "roles": ["source"], "state": "active", "createdAt": CREATED,
        "updatedAt": CREATED,
    });
    serve(server, "POST", &format!("{ACCOUNT}/apps"), 201, data(created)).await;
    let renamed = json!({
        "id": APP_SHOP, "accountId": "acct-alpha", "appType": "shopify", "displayName": "Demo Shop (renamed)",
        "permissions": ["performance_data"], "state": "active", "createdAt": CREATED, "updatedAt": SYNCED,
    });
    serve(server, "PATCH", &format!("{ACCOUNT}/apps/{APP_SHOP}"), 200, data(renamed)).await;
    let paused = json!({ "id": APP_SHOP, "state": "inactive", "message": "App paused successfully" });
    serve(server, "POST", &format!("{ACCOUNT}/apps/{APP_SHOP}/pause"), 200, data(paused)).await;
    let resumed = json!({ "id": APP_WAREHOUSE, "state": "active", "message": "App resumed successfully" });
    serve(server, "POST", &format!("{ACCOUNT}/apps/{APP_WAREHOUSE}/resume"), 200, data(resumed)).await;
    let deleted = json!({ "deleted": true, "id": APP_WAREHOUSE });
    serve(server, "DELETE", &format!("{ACCOUNT}/apps/{APP_WAREHOUSE}"), 200, data(deleted)).await;

    // ── catalog ──
    let entry = |app_type: &str, name: &str, category: &str, roles: Value, self_serve: bool, provider: &str| {
        json!({
            "appType": app_type, "displayName": name, "category": category,
            "description": format!("{name} connector (synthetic description)"),
            "logoUrl": format!("https://example.com/logos/{app_type}.svg"), "supportedRoles": roles, "lifecycle": "ga",
            "selfServe": self_serve, "availability": if self_serve { "self_serve" } else { "request_access" },
            "requestAccessReason": if self_serve { Value::Null } else { json!("request_access_required") },
            "provider": provider,
        })
    };
    let catalog = vec![
        entry("bigquery", "BigQuery", "warehouse", json!(["destination"]), true, "google"),
        entry("google_ads", "Google Ads", "ads", json!(["source", "destination"]), true, "google"),
        entry("shopify", "Shopify", "ecommerce", json!(["source"]), true, "shopify"),
        entry("tiktok_ads", "TikTok Ads", "ads", json!(["source"]), false, "tiktok"),
    ];
    // As the real route answers (VE-3829): the request-access entries (TikTok Ads here) only with
    // `include_request_access=true` (`--all`), and `meta.total` counts the entries it sent.
    for all in [false, true] {
        let request = Mock::given(method("GET")).and(path("/api/v1/catalog"));
        let (request, entries) = if all {
            (request.and(query_param("include_request_access", "true")), catalog.clone())
        } else {
            let ready = catalog.iter().filter(|e| e["availability"] == "self_serve").cloned().collect();
            (request.and(query_param_is_missing("include_request_access")), ready)
        };
        let meta = json!({ "total": entries.len(), "selfServeTotal": 3, "requestAccessTotal": 1 });
        let body = json!({ "data": entries, "meta": meta });
        request.respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(server).await;
    }
    let shopify = with(
        catalog[2].clone(),
        json!({
            "defaultPermissions": { "source": ["performance_data"], "destination": [] },
            "credentialFields": [
                { "name": "shop_domain", "label": "Shop domain", "type": "text", "placeholder": "demo-shop.myshopify.com",
                  "description": "The store's myshopify.com domain" },
                { "name": "access_token", "label": "Admin API access token", "type": "password", "placeholder": "shpat_…",
                  "description": null },
            ],
            "documentationUrl": "https://example.com/docs/shopify",
        }),
    );
    serve(server, "GET", "/api/v1/catalog/shopify", 200, data(shopify)).await;
    let bigquery = with(
        catalog[0].clone(),
        json!({ "defaultPermissions": { "source": [], "destination": ["send_conversions"] }, "credentialFields": [],
                "documentationUrl": null }),
    );
    serve(server, "GET", "/api/v1/catalog/bigquery", 200, data(bigquery)).await;

    // ── sources ──
    let shop_source = json!({
        "id": SRC_SHOP, "accountId": "acct-alpha", "appId": APP_SHOP, "appName": "Demo Shop", "syncType": "shopify",
        "importTasks": ["orders", "customers"], "syncFrequency": "every 6 hours", "syncAnchorTime": "02:00",
        "syncAnchorTimezone": "UTC", "rollingPeriodDays": null, "state": "active", "integrationStatus": "active",
        "lastSyncAt": s.ago("2h30m", SignedDuration::from_mins(150)), "lastError": null, "consecutiveFailures": 0,
        "createdAt": CREATED, "updatedAt": SYNCED,
    });
    let ads_source = json!({
        "id": SRC_ADS, "accountId": "acct-alpha", "appId": APP_ADS, "appName": "Demo Ads", "syncType": "google_ads",
        "importTasks": ["campaign_performance"], "syncFrequency": "daily", "syncAnchorTime": null,
        "syncAnchorTimezone": null, "rollingPeriodDays": 30, "state": "active", "integrationStatus": "errored",
        "lastSyncAt": SYNCED, "lastError": "Token expired for the synthetic test account", "consecutiveFailures": 3,
        "createdAt": CREATED, "updatedAt": SYNCED,
    });
    serve(server, "GET", &format!("{ACCOUNT}/sources"), 200, pagination(vec![shop_source.clone(), ads_source.clone()]))
        .await;
    let shop_detail = with(
        shop_source.clone(),
        json!({
            "appType": "shopify", "appState": "active", "warehouseType": "bigquery", "warehouseConfig": null,
            "datasetId": "demo_shop_raw", "isOnboarding": false, "config": { "shop_domain": "demo-shop.example.com" },
            "latestJobId": JOB_RUNNING, "activeImportJobId": JOB_RUNNING, "earliestDataAt": null, "latestDataAt": null,
            "importProgress": { "latestCheckpointAt": "2026-01-05T09:00:00.000Z", "enabledStreamCount": 2,
                                "checkpointStreamCount": 2, "attentionStreamCount": 0 },
        }),
    );
    serve(server, "GET", &format!("{ACCOUNT}/sources/{SRC_SHOP}"), 200, data(shop_detail)).await;
    let ads_detail = with(
        ads_source,
        json!({
            "appType": "google_ads", "appState": "active", "warehouseType": "bigquery", "warehouseConfig": null,
            "datasetId": "demo_ads_raw", "isOnboarding": false, "config": null, "latestJobId": JOB_FAILED,
            "activeImportJobId": null, "earliestDataAt": null, "latestDataAt": null, "importProgress": null,
        }),
    );
    serve(server, "GET", &format!("{ACCOUNT}/sources/{SRC_ADS}"), 200, data(ads_detail)).await;
    let new_source = with(shop_source.clone(), json!({ "id": SRC_NEW, "importTasks": ["orders"], "lastSyncAt": null }));
    serve(server, "POST", &format!("{ACCOUNT}/sources"), 201, data(new_source)).await;
    serve(server, "PATCH", &format!("{ACCOUNT}/sources/{SRC_SHOP}"), 200, data(shop_source)).await;
    let dispatched = json!({ "jobId": JOB_NEW, "status": "dispatched", "message": "Sync job has been submitted" });
    serve(server, "POST", &format!("{ACCOUNT}/sources/{SRC_ADS}/sync"), 202, data(dispatched.clone())).await;
    let paused = json!({ "id": SRC_ADS, "state": "inactive", "message": "Source paused successfully" });
    serve(server, "POST", &format!("{ACCOUNT}/sources/{SRC_ADS}/pause"), 200, data(paused)).await;
    let resumed = json!({ "id": SRC_ADS, "state": "active", "message": "Source resumed successfully" });
    serve(server, "POST", &format!("{ACCOUNT}/sources/{SRC_ADS}/resume"), 200, data(resumed)).await;
    serve(
        server,
        "DELETE",
        &format!("{ACCOUNT}/sources/{SRC_ADS}"),
        200,
        data(json!({ "deleted": true, "id": SRC_ADS })),
    )
    .await;

    // ── destinations (the API's integrations, routed to /connections) ──
    let orders = json!({
        "id": INT_ORDERS, "accountId": "acct-alpha", "sourceAppId": APP_SHOP, "sourceAppName": "Demo Shop",
        "sourceAppType": "shopify", "destinationAppId": APP_WAREHOUSE, "destinationAppName": "Demo Warehouse",
        "destinationAppType": "bigquery", "dataType": "orders", "config": { "dataset": "demo_exports" },
        "schedule": { "frequency_value": 6, "frequency_unit": "hours" }, "state": "active", "status": "active",
        "isActive": true, "lastSyncAt": s.ago("2h30m", SignedDuration::from_mins(150)), "lastError": null,
        "consecutiveFailures": 0, "importDependencyStatus": null, "latestJobId": JOB_DONE, "createdAt": CREATED,
        "updatedAt": SYNCED,
    });
    let ads_export = json!({
        "id": INT_ADS, "accountId": "acct-alpha", "sourceAppId": APP_ADS, "sourceAppName": "Demo Ads",
        "sourceAppType": "google_ads", "destinationAppId": APP_WAREHOUSE, "destinationAppName": "Demo Warehouse",
        "destinationAppType": "bigquery", "dataType": "ad_performance", "config": null, "schedule": null,
        "state": "inactive", "status": "paused", "isActive": false, "lastSyncAt": SYNCED,
        "lastError": "Destination dataset is missing (synthetic)", "consecutiveFailures": 1,
        "importDependencyStatus": null, "latestJobId": null, "createdAt": CREATED, "updatedAt": SYNCED,
    });
    let connections = format!("{ACCOUNT}/connections");
    serve(server, "GET", &connections, 200, pagination(vec![orders.clone(), ads_export.clone()])).await;
    let app_ref = |id: &str, name: &str, app_type: &str| json!({ "id": id, "displayName": name, "appType": app_type, "state": "active" });
    let orders_detail = with(
        orders.clone(),
        json!({
            "sourceApp": app_ref(APP_SHOP, "Demo Shop", "shopify"),
            "destinationApp": app_ref(APP_WAREHOUSE, "Demo Warehouse", "bigquery"),
            "importDependencyCheckedAt": null, "triggeredImportJobIds": null, "historicalSyncStartDate": "2025-01-01",
        }),
    );
    serve(server, "GET", &format!("{connections}/{INT_ORDERS}"), 200, data(orders_detail)).await;
    serve(server, "GET", &format!("{connections}/{INT_ADS}"), 200, data(ads_export)).await;
    let new_integration = with(orders.clone(), json!({ "id": INT_NEW, "lastSyncAt": null, "latestJobId": null }));
    serve(server, "POST", &connections, 201, data(new_integration)).await;
    serve(server, "PATCH", &format!("{connections}/{INT_ORDERS}"), 200, data(orders)).await;
    serve(server, "POST", &format!("{connections}/{INT_ADS}/sync"), 202, data(dispatched)).await;
    let paused = json!({ "id": INT_ADS, "state": "inactive", "message": "Integration paused successfully" });
    serve(server, "POST", &format!("{connections}/{INT_ADS}/pause"), 200, data(paused)).await;
    let resumed = json!({ "id": INT_ADS, "state": "active", "message": "Integration resumed successfully" });
    serve(server, "POST", &format!("{connections}/{INT_ADS}/resume"), 200, data(resumed)).await;
    serve(server, "DELETE", &format!("{connections}/{INT_ADS}"), 200, data(json!({ "deleted": true, "id": INT_ADS })))
        .await;
    let refresh = json!({
        "status": "importing", "importJobIds": [JOB_NEW], "requestedStart": "2026-01-01T00:00:00.000Z",
        "requestedEnd": "2026-01-08T00:00:00.000Z",
    });
    serve(server, "POST", &format!("{connections}/{INT_ORDERS}/refresh-source"), 202, data(refresh)).await;

    // ── jobs ──
    let job = |over: Value| {
        with(
            json!({
                "id": "", "accountId": "acct-alpha", "sourceId": null, "integrationId": null, "jobType": "import",
                "connectorType": null, "destinationType": null, "status": "", "executionType": null,
                "isOnboarding": false, "minTimestamp": null, "maxTimestamp": null, "startedAt": null,
                "finishedAt": null, "errorMessage": null, "errorCode": null, "errorCategory": null, "parentJobId": null,
                "chunkIndex": null, "totalChunks": null, "progressPct": null, "rowsProcessed": null,
                "rowsWritten": null, "metrics": {}, "createdAt": CREATED,
            }),
            over,
        )
    };
    // Running for 2h and half a minute: "2h" until the run is 30 seconds old.
    let started = s.ago("2h0m30s", SignedDuration::from_secs(2 * 3600 + 30));
    let running = job(json!({
        "id": JOB_RUNNING, "sourceId": SRC_SHOP, "connectorType": "shopify", "status": "running",
        "executionType": "incremental", "startedAt": started, "chunkIndex": 1, "totalChunks": 4, "progressPct": 40,
        "rowsProcessed": 12000, "rowsWritten": 11800, "createdAt": started,
    }));
    let queued = job(json!({
        "id": JOB_QUEUED, "integrationId": INT_ORDERS, "jobType": "export", "connectorType": "bigquery",
        "destinationType": "bigquery", "status": "queued", "createdAt": s.ago("3h30m", SignedDuration::from_mins(210)),
    }));
    let done = job(json!({
        "id": JOB_DONE, "integrationId": INT_ORDERS, "jobType": "export", "connectorType": "bigquery",
        "destinationType": "bigquery", "status": "completed", "executionType": "full", "startedAt": SYNCED,
        "finishedAt": "2026-01-05T10:07:33.000Z", "progressPct": 100, "rowsProcessed": 1234567, "rowsWritten": 1234567,
        "metrics": { "bytes_written": 98765432 }, "createdAt": SYNCED,
    }));
    let failed = job(json!({
        "id": JOB_FAILED, "sourceId": SRC_ADS, "connectorType": "google_ads", "status": "failed",
        "executionType": "incremental", "startedAt": "2026-01-04T00:00:00.000Z",
        "finishedAt": "2026-01-04T00:00:42.000Z",
        "errorMessage": "Google Ads rejected the synthetic test account's token (refresh needed)",
        "errorCode": "AUTH_EXPIRED", "errorCategory": "auth", "rowsProcessed": 0, "rowsWritten": 0,
        "createdAt": "2026-01-04T00:00:00.000Z",
    }));
    // Most specific first: the first mounted match answers.
    jobs_matching(server, &[("status", ACTIVE), ("source_id", SRC_SHOP)], data(json!([running.clone()]))).await;
    jobs_matching(server, &[("status", ACTIVE), ("integration_id", INT_ORDERS)], data(json!([queued.clone()]))).await;
    jobs_matching(server, &[("status", ACTIVE), ("limit", "100")], pagination(vec![running.clone(), queued.clone()]))
        .await;
    jobs_matching(server, &[("status", ACTIVE)], pagination(vec![])).await;
    jobs_matching(server, &[("status", "failed")], pagination(vec![failed.clone()])).await;
    jobs_matching(server, &[], pagination(vec![running.clone(), queued.clone(), done.clone(), failed.clone()])).await;
    for job in [&running, &queued, &done, &failed] {
        let id = job["id"].as_str().unwrap();
        serve(server, "GET", &format!("{ACCOUNT}/jobs/{id}"), 200, data(job.clone())).await;
    }
    serve(server, "GET", &format!("{ACCOUNT}/jobs/{JOB_MISSING}"), 404, not_found("Job")).await;
    let cancelling = json!({
        "id": JOB_RUNNING, "status": "running",
        "message": "Job cancel requested; the job remains active until Prefect finishes cancelling.",
    });
    serve(server, "POST", &format!("{ACCOUNT}/jobs/{JOB_RUNNING}/cancel"), 202, data(cancelling)).await;

    // ── dictionary ──
    let column = json!({
        "subjectId": COLUMN_ID, "subjectType": "column", "displayName": "Customer email",
        "description": "Lowercased email of the latest customer record", "dataType": "string", "semanticType": "email",
        "tags": ["pii", "crm"], "origin": "bq_schema", "lastSeenAt": s.ago("3d12h", SignedDuration::from_hours(84)),
        "status": "active",
    });
    let refunded = json!({
        "subjectId": "fedcba9876543210fedcba9876543210", "subjectType": "event", "displayName": "Order Refunded",
        "description": null, "dataType": null, "semanticType": null, "tags": ["orders"], "origin": "lexicon",
        "lastSeenAt": null, "status": "active",
    });
    Mock::given(path(DICTIONARY))
        .and(query_param("type", "column"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![column.clone()], 1)))
        .mount(server)
        .await;
    serve(server, "GET", DICTIONARY, 200, page(vec![event_item(), refunded], 2)).await;
    let lookup = format!("{DICTIONARY}/lookup");
    for item in [event_item(), column] {
        let id = item["subjectId"].as_str().unwrap().to_string();
        Mock::given(path(lookup.clone()))
            .and(query_param("subject_id", id.as_str()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(data(json!({ "subjectId": id, "found": true, "definition": item }))),
            )
            .mount(server)
            .await;
    }

    // ── metrics (web-app routes) ──
    let roas = metric(json!({ "metric_type": "derived", "updated_at": SYNCED }));
    let draft = metric(json!({
        "id": "7a2d3b0f-2222-4c3b-9a7e-000000000002", "name": "Blended CAC", "description": "", "format": "currency",
        "higher_is_better": false, "unit": "$", "status": "draft", "metric_type": "composed", "updated_at": null,
    }));
    let metrics = json!({ "metrics": [roas.clone(), draft], "total": 2, "limit": 20, "offset": 0 });
    serve(server, "GET", "/api/metrics", 200, metrics).await;
    serve(server, "GET", &format!("/api/metrics/{M1}"), 200, json!({ "metric": roas.clone() })).await;
    let created = with(
        roas.clone(),
        json!({ "id": "8b3e4c1a-3333-4c3b-9a7e-000000000003", "name": "Demo metric", "status": "draft" }),
    );
    serve(server, "POST", "/api/metrics", 201, json!({ "metric": created, "registryWarning": "pending" })).await;
    serve(server, "PATCH", &format!("/api/metrics/{M1}"), 200, json!({ "metric": roas })).await;
    serve(server, "DELETE", &format!("/api/metrics/{M1}"), 200, json!({ "deleted": true, "id": M1 })).await;

    // ── models ──
    // The API's ModelRecord has `modelType` and no `dataType`, which the CLI reads: its
    // "undefined" type is today's behaviour.
    let models = json!({
        "data": [model(json!({ "lastValidatedAt": SYNCED, "createdAt": CREATED })),
                 model(json!({ "id": "22222222-3333-4444-8555-666666666666", "name": "churn_scores", "isValid": false,
                               "modelType": "python", "validationError": "Table not found (synthetic)" }))],
        "meta": { "pagination": { "total": 2, "limit": 20, "offset": 0, "hasMore": false } },
    });
    serve(server, "GET", &format!("{ACCOUNT}/models"), 200, models).await;
    let detail = model(json!({
        "description": "Clean orders (synthetic)", "sqlQuery": "SELECT order_id\nFROM `demo-project.demo_shop_raw.orders`",
        "primaryKeyColumns": ["order_id"], "incrementalColumn": "updated_at", "lastValidatedAt": SYNCED,
        "createdAt": CREATED,
    }));
    serve(server, "GET", &format!("{ACCOUNT}/models/11111111-2222-4333-8444-555555555555"), 200, data(detail)).await;

    // ── measurement (web-app routes) ──
    serve(server, "GET", "/api/measurement/methodologies", 200, methodologies()).await;
    let previews = json!({
        "previews": [
            { "context": { "campaign_objective": "sales", "channel_grouping": null, "custom_label": "" }, "sample_count": 12,
              "resolved_methodology": { "id": "m-system-1", "name": "Last click", "click_path_model": "last_click" },
              "matched_rule_id": null, "via": "default_fallback" },
            { "context": { "campaign_objective": null, "channel_grouping": "Paid Social", "custom_label": "promo" },
              "sample_count": 1234,
              "resolved_methodology": { "id": "m-account-2-with-a-long-id", "name": "Blended", "click_path_model": "linear" },
              "matched_rule_id": "r1", "via": "rule" },
        ],
        "total_distinct_contexts": 2,
    });
    serve(server, "POST", "/api/measurement/methodologies/rules/preview", 200, previews).await;
    let realised = |l30: f64, l90: f64, cac: f64| {
        json!({ "ltv_30d": l30, "ltv_90d": l90, "ltv_12m": null, "ltv_full": null, "cac": cac,
                "cac_ltv_ratio": cac / l90, "payback_period_days": null, "computed_at": null })
    };
    let cohort_row = |period: &str, size: u64, realised: Value| {
        json!({ "cohort_period": period, "cohort_granularity": "monthly", "segment_key": "all", "cohort_size": size,
                "realised": realised, "predicted": null })
    };
    let ltv = json!({ "data": {
        "granularity": "monthly", "segment_key": "all",
        "cohorts": [cohort_row("2026-08-01", 1200, realised(42.5, 85.25, 20.0)),
                    cohort_row("2026-07-01", 980, realised(38.0, 80.0, 25.0))],
        "total_returned": 2,
    } });
    serve(server, "GET", "/api/measurement/ltv", 200, ltv).await;
    let cohort = json!({
        "cohort_period": "2026-08-01", "cohort_granularity": "monthly", "segment_key": "all", "cohort_size": 1200,
        "retention_matrix": [
            { "period_offset_days": 0, "retained_customers": 1200, "retained_revenue": 51000, "retention_rate": 1 },
            { "period_offset_days": 30, "retained_customers": 300, "retained_revenue": 12750, "retention_rate": 0.25 },
        ],
        "cumulative_curve": [
            { "period_offset_days": 0, "cumulative_gross_revenue": 51000, "cumulative_revenue_after_cogs": 40800 },
            { "period_offset_days": 30, "cumulative_gross_revenue": 63750, "cumulative_revenue_after_cogs": 51000 },
        ],
        "prediction": { "method": "naive_decay", "ltv_30d_predicted": 42.5, "ltv_90d_predicted": 85.25,
                        "ltv_12m_predicted": null, "metadata": null, "computed_at": null },
    });
    serve(server, "GET", "/api/measurement/ltv/cohort/2026-08-01", 200, cohort).await;
    let customer = json!({
        "cohort": { "customer_id": "cust_demo_001", "acquisition_date": "2026-01-05", "cohort_period_daily": "2026-01-05",
                    "cohort_period_weekly": "2026-01-05", "cohort_period_monthly": "2026-01-01",
                    "acquisition_channel": "Paid Social", "acquisition_campaign": null, "country": "AU",
                    "is_reactivated": false },
        "revenue": [{ "revenue_period_daily": "2026-01-05", "period_offset_days": 0, "gross_revenue": 42.5 }],
        "realised": { "ltv_30d": 42.5, "ltv_90d": 85.25, "ltv_12m": null, "ltv_full": 85.25 },
    });
    serve(server, "GET", "/api/measurement/ltv/customer/cust_demo_001", 200, customer).await;
    let signals = json!({ "data": { "signals": [
        { "id": "click_path", "state": "live", "availability": { "available": true } },
        { "id": "mmm", "state": "stub", "availability": { "available": false, "reason": "Not enough spend history" } },
        { "id": "geo_lift", "state": "stub", "availability": null },
    ] } });
    serve(server, "GET", "/api/measurement/signals", 200, signals).await;
    let click_path = json!({ "status": {
        "enabled": true, "lastComputedAt": null, "sampleEstimates": [{ "tier_label": "a" }, { "tier_label": "b" }],
        "readiness": { "available": false, "readiness": [
            { "key": "clicks", "label": "Has click data", "ok": true, "detail": null },
            { "key": "conv", "label": "Has conversions", "ok": false, "detail": "No conversions in 30 days" },
        ] },
    } });
    serve(server, "GET", "/api/measurement/signals/click-path", 200, click_path).await;
}

// ── the commands ────────────────────────────────────────────────────────────

#[tokio::test]
async fn account_and_profile_output() {
    let (server, mut s) = Session::start("account").await;
    s.record("whoami", &["whoami"]);
    s.record("whoami_json", &["whoami", "--json"]);
    s.record("status", &["status"]);
    s.record("status_json", &["status", "--json"]);
    // PATH and SHELL pinned: doctor checks both.
    let machine = |cmd: &mut Command| {
        cmd.env("PATH", "/usr/bin:/bin").env("SHELL", "/bin/zsh");
    };
    s.record_with("doctor", &["doctor"], machine);
    s.record_with("doctor_json", &["doctor", "--json"], machine);
    s.record("profile_list", &["profile", "list"]);
    s.record("profile_list_json", &["profile", "list", "--json"]);
    // Moved commands (VE-3827) print exactly what the command they moved to prints.
    for (old, new) in [
        (&["profile", "current"][..], &["whoami"][..]),
        (&["profile", "current", "--json"], &["whoami", "--json"]),
        (&["config", "show"], &["whoami"]),
        (&["config", "show", "--json"], &["whoami", "--json"]),
        (&["config", "list"], &["profile", "list"]),
    ] {
        let expected = s.output(new);
        assert_eq!(expected.0, Some(0), "vendo {}", new.join(" "));
        assert!(s.output(old) == expected, "vendo {} differs from vendo {}", old.join(" "), new.join(" "));
    }
    s.record("mcp", &["mcp"]);
    s.record("mcp_json", &["mcp", "--json"]);
    // A working key is checked and kept (VE-3825); `init`, login's hidden alias, prints the same.
    s.record("login_existing_key", &["login"]);
    assert!(s.output(&["init"]) == s.output(&["login"]), "vendo init differs from vendo login");
    s.record("login_existing_key_json", &["login", "--json"]);

    // Writes, in order: each changes the saved config the next one reads.
    let stub = s.redactions.iter().find(|(_, to)| to == "[stub]").unwrap().0.clone();
    s.record(
        "login",
        &["login", "--api-key", "vendo_sk_fake_gamma_0000", "--account", "acct-gamma", "--base-url", stub.as_str()],
    );
    s.record("profile_set", &["profile", "set", "--account", "acct-alpha"]);
    s.record("profile_set_json", &["profile", "set", "--account", "acct-alpha", "--json"]);
    // Run twice, a write that leaves nothing to differ: the second run prints what the first did.
    let same_twice = |s: &Session, old: &[&str], new: &[&str]| {
        let expected = s.output(new);
        assert_eq!(expected.0, Some(0), "vendo {}", new.join(" "));
        assert!(s.output(old) == expected, "vendo {} differs from vendo {}", old.join(" "), new.join(" "));
    };
    same_twice(&s, &["config", "set", "--account", "acct-alpha"], &["profile", "set", "--account", "acct-alpha"]);
    same_twice(&s, &["config", "use", "beta"], &["profile", "switch", "beta"]);
    s.record("profile_switch", &["profile", "switch", "alpha"]);
    s.record("profile_switch_json", &["profile", "switch", "alpha", "--json"]);
    s.record("logout", &["logout"]);
    // Without a terminal, `logout --all` needs --yes (VE-3823), and so does `config reset`, its old name.
    s.record("logout_all", &["logout", "--all"]);
    assert!(s.output(&["config", "reset"]) == s.output(&["logout", "--all"]), "vendo config reset differs");
    s.record("logout_all_yes", &["logout", "--all", "--yes"]);
    let login =
        ["login", "--api-key", "vendo_sk_fake_gamma_0000", "--account", "acct-gamma", "--base-url", stub.as_str()];
    s.record("login_after_reset", &login);
    // `config reset --yes` removes every profile like `logout --all --yes`, from the same start.
    for (old, new) in [
        (&["config", "reset", "--yes"][..], &["logout", "--all", "--yes"][..]),
        (&["config", "reset", "-y"], &["logout", "--all", "-y"]),
    ] {
        let removed = s.output(old);
        assert_eq!(s.output(&login).0, Some(0));
        assert!(s.output(new) == removed, "vendo {} differs from vendo {}", old.join(" "), new.join(" "));
        assert_eq!(s.output(&login).0, Some(0));
    }
    // With nothing saved, both say so.
    assert_eq!(s.output(&["logout", "--all", "--yes"]).0, Some(0));
    let nothing = s.output(&["logout", "--all", "--yes"]);
    assert_eq!(text(&nothing.1), "No configuration file found.\n");
    assert!(s.output(&["config", "reset", "--yes"]) == nothing, "vendo config reset --yes differs");
    // With no key, login signs in through the browser: the test visits the sign-in page it prints,
    // and the stub's page creates the key (VE-3825).
    mount_sign_in_page(&server).await;
    let login = ["login", "--base-url", stub.as_str()];
    let signed_in = login_at_browser(s.sandbox.command(&login)).await;
    s.record_output("login_browser", &login, signed_in);
    s.finish();
}

#[tokio::test]
async fn apps_output() {
    let (_server, mut s) = Session::start("apps").await;
    s.record("list", &["apps", "list"]);
    s.record("list_json", &["apps", "list", "--json"]);
    s.record("get", &["apps", "get", APP_ADS]);
    s.record("get_json", &["apps", "get", APP_ADS, "--json"]);
    s.record("diagnose", &["apps", "diagnose"]);
    s.record("diagnose_json", &["apps", "diagnose", "--json"]);

    let creds = s.file("shop-credentials.json", r#"{"shop_domain":"demo-shop.example.com","access_token":"fake"}"#);
    s.record("create", &["apps", "create", "--type", "shopify", "--name", "Demo Shop 2", "--credentials-file", &creds]);
    s.record("update", &["apps", "update", APP_SHOP, "--name", "Demo Shop (renamed)"]);
    s.record("pause", &["apps", "pause", APP_SHOP]);
    s.record("pause_dry_run", &["apps", "pause", APP_SHOP, "--dry-run"]);
    s.record("resume", &["apps", "resume", APP_WAREHOUSE]);
    s.record("delete", &["apps", "delete", APP_WAREHOUSE, "--yes"]);
    s.finish();
}

#[tokio::test]
async fn sources_output() {
    let (_server, mut s) = Session::start("sources").await;
    s.record("list", &["sources", "list"]);
    s.record("list_json", &["sources", "list", "--json"]);
    s.record("get", &["sources", "get", SRC_SHOP]);
    s.record("get_json", &["sources", "get", SRC_SHOP, "--json"]);
    s.record("get_errored", &["sources", "get", SRC_ADS]);

    s.record("create", &["sources", "create", "--app", APP_SHOP, "--sync-type", "shopify", "--import-tasks", "orders"]);
    s.record("update", &["sources", "update", SRC_SHOP, "--frequency", "6"]);
    s.record("sync", &["sources", "sync", SRC_ADS]);
    s.record("sync_in_progress", &["sources", "sync", SRC_SHOP]);
    s.record("sync_dry_run", &["sources", "sync", SRC_ADS, "--dry-run"]);
    s.record("pause", &["sources", "pause", SRC_ADS]);
    s.record("resume", &["sources", "resume", SRC_ADS]);
    s.record("delete", &["sources", "delete", SRC_ADS, "--yes"]);
    s.finish();
}

#[tokio::test]
async fn destinations_output() {
    let (_server, mut s) = Session::start("destinations").await;
    s.record("list", &["destinations", "list"]);
    s.record("list_json", &["destinations", "list", "--json"]);
    s.record("get", &["destinations", "get", INT_ORDERS]);
    s.record("get_json", &["destinations", "get", INT_ORDERS, "--json"]);
    s.record("get_paused", &["destinations", "get", INT_ADS]);

    let config = s.file("export-config.json", r#"{"tasks":[{"table":"orders"}]}"#);
    let create = ["destinations", "create", "--dest-app", APP_WAREHOUSE, "--source-app", APP_SHOP];
    s.record("create", &[&create[..], &["--data-type", "orders", "--config-file", config.as_str()][..]].concat());
    s.record("update", &["destinations", "update", INT_ORDERS, "--frequency", "12", "--unit", "hours"]);
    s.record("sync", &["destinations", "sync", INT_ADS]);
    s.record("sync_in_progress", &["destinations", "sync", INT_ORDERS]);
    let refresh = ["destinations", "refresh-source", INT_ORDERS];
    s.record("refresh_source", &[&refresh[..], &["--from", "2026-01-01", "--to", "2026-01-08"][..]].concat());
    s.record("pause", &["destinations", "pause", INT_ADS]);
    s.record("resume", &["destinations", "resume", INT_ADS]);
    s.record("delete", &["destinations", "delete", INT_ADS, "--yes"]);

    // `integrations` and `int` are hidden aliases (VE-3828): the same exit code, stdout and stderr, byte for byte.
    let create = [&create[1..], &["--data-type", "orders", "--config-file", config.as_str()][..]].concat();
    let commands: [&[&str]; 7] = [
        &["list"],
        &["get", INT_ORDERS],
        &create,
        &["update", INT_ORDERS, "--frequency", "12", "--unit", "hours"],
        &["sync", INT_ADS],
        &["pause", INT_ADS],
        &["delete", INT_ADS, "--yes"],
    ];
    for command in commands {
        for json in [&[][..], &["--json"]] {
            let args = |group: &'static str| [&[group][..], command, json].concat();
            let expected = s.output(&args("destinations"));
            assert_eq!(expected.0, Some(0), "vendo destinations {}", args("destinations")[1..].join(" "));
            for old in ["integrations", "int"] {
                assert!(s.output(&args(old)) == expected, "vendo {} differs", args(old).join(" "));
            }
        }
    }
    s.finish();
}

#[tokio::test]
async fn jobs_output() {
    let (_server, mut s) = Session::start("jobs").await;
    // First: the running job's duration counts from when the fixtures were made.
    s.record("list", &["jobs", "list"]);
    s.record("list_json", &["jobs", "list", "--json"]);
    s.record("get", &["jobs", "get", JOB_DONE]);
    s.record("get_failed", &["jobs", "get", JOB_FAILED]);
    s.record("get_json", &["jobs", "get", JOB_FAILED, "--json"]);
    s.record("get_missing", &["jobs", "get", JOB_MISSING]);
    s.record("cancel", &["jobs", "cancel", JOB_RUNNING, "--yes"]);
    s.record("cancel_dry_run", &["jobs", "cancel", JOB_RUNNING, "--dry-run"]);
    // A job that has ended: tailing reads it once (VE-3831).
    s.record("tail_json", &["jobs", "tail", JOB_DONE, "--json"]);
    s.record("tail_failed_json", &["jobs", "tail", JOB_FAILED, "--json"]);
    s.finish();
}

#[tokio::test]
async fn catalog_output() {
    let (_server, mut s) = Session::start("catalog").await;
    s.record("list", &["catalog", "list"]);
    s.record("list_json", &["catalog", "list", "--json"]);
    s.record("list_all", &["catalog", "list", "--all"]);
    s.record("list_all_json", &["catalog", "list", "--all", "--json"]);
    s.record("get", &["catalog", "get", "shopify"]);
    s.record("get_json", &["catalog", "get", "shopify", "--json"]);
    s.record("credential_schema", &["catalog", "credential-schema", "shopify"]);
    s.record("credential_schema_json", &["catalog", "credential-schema", "shopify", "--json"]);
    s.record("credential_schema_none", &["catalog", "credential-schema", "bigquery"]);
    s.finish();
}

#[tokio::test]
async fn dictionary_output() {
    let (_server, mut s) = Session::start("dictionary").await;
    s.record("list", &["dictionary", "list"]);
    s.record("list_json", &["dictionary", "list", "--json"]);
    s.record("search", &["dictionary", "search", "customer", "--type", "column"]);
    s.record("search_json", &["dictionary", "search", "customer", "--type", "column", "--json"]);
    s.record("get", &["dictionary", "get", COLUMN_ID]);
    s.record("get_json", &["dictionary", "get", COLUMN_ID, "--json"]);
    s.finish();
}

#[tokio::test]
async fn metrics_output() {
    let (_server, mut s) = Session::start("metrics").await;
    s.record("list", &["metrics", "list"]);
    s.record("list_json", &["metrics", "list", "--json"]);
    s.record("get", &["metrics", "get", M1]);
    s.record("get_json", &["metrics", "get", M1, "--json"]);

    let definition = s.file("demo.query.json", r#"{"version":2,"reportType":"segmentation"}"#);
    s.record("create", &["metrics", "create", "--name", "Demo metric", "--definition", &definition]);
    s.record("update", &["metrics", "update", M1, "--description", "Return on ad spend"]);
    s.record("activate", &["metrics", "activate", M1]);
    s.record("delete", &["metrics", "delete", M1, "--yes"]);
    s.finish();
}

#[tokio::test]
async fn models_output() {
    let (_server, mut s) = Session::start("models").await;
    s.record("list", &["models", "list"]);
    s.record("list_json", &["models", "list", "--json"]);
    s.record("get", &["models", "get", "11111111-2222-4333-8444-555555555555"]);
    s.record("get_json", &["models", "get", "11111111-2222-4333-8444-555555555555", "--json"]);
    s.finish();
}

#[tokio::test]
async fn measurement_output() {
    let (_server, mut s) = Session::start("measurement").await;
    let window = ["--from", "2026-09-01", "--to", "2026-09-30"];
    for (name, args) in [
        ("methodologies_list", vec!["measurement", "methodologies", "list"]),
        ("methodologies_get", vec!["measurement", "methodologies", "get", "m-account-2-with-a-long-id"]),
        ("rules_preview", [&["measurement", "rules", "preview"][..], &window[..]].concat()),
        ("ltv_list", vec!["measurement", "ltv", "list"]),
        ("ltv_cohort", vec!["measurement", "ltv", "cohort", "2026-08-01"]),
        ("ltv_customer", vec!["measurement", "ltv", "customer", "cust_demo_001"]),
        ("signals_list", vec!["measurement", "signals", "list"]),
        ("signals_click_path", vec!["measurement", "signals", "click-path"]),
    ] {
        s.record(name, &args);
        s.record(&format!("{name}_json"), &[&args[..], &["--json"][..]].concat());
    }
    s.finish();
}

#[tokio::test]
async fn completions_output() {
    // The scripts the installer saves, recorded whole: a change to what TAB offers shows in the diff
    // (VE-3830 added the values of fixed-list flags and changed nothing else).
    let (_server, mut s) = Session::start("completions").await;
    for shell in ["bash", "zsh", "fish"] {
        s.record(shell, &["completions", shell]);
    }
    // Bare, it says what it does, whether completions are set up for the shell `$SHELL` names, and how.
    let shell = |value: &'static str| move |cmd: &mut Command| _ = cmd.env("SHELL", value);
    s.record_with("bare_zsh", &["completions"], shell("/bin/zsh"));
    s.record_with("bare_bash", &["completions"], shell("/bin/bash"));
    s.record_with("bare_fish", &["completions"], shell("/opt/homebrew/bin/fish"));
    s.record_with("bare_unknown_shell", &["completions"], |cmd| _ = cmd.env_remove("SHELL"));
    s.record_with("bare_zsh_json", &["completions", "--json"], shell("/bin/zsh"));
    fn bare(s: &Session, shell: Option<&str>) -> (Option<i32>, Vec<u8>, Vec<u8>) {
        let mut cmd = s.sandbox.command(&["completions"]);
        match shell {
            Some(value) => cmd.env("SHELL", value),
            None => cmd.env_remove("SHELL"),
        };
        let out = cmd.output().unwrap();
        (out.status.code(), out.stdout, out.stderr)
    }
    // An empty `$SHELL`, or a shell other than bash, zsh and fish, reads as no shell.
    for value in ["", "/bin/tcsh"] {
        assert!(bare(&s, Some(value)) == bare(&s, None), "SHELL={value:?}");
    }
    // The lines it gives for zsh, added to ~/.zshrc by hand, count as set up.
    s.file(".zshrc", "autoload -Uz compinit && compinit\neval \"$(vendo completions zsh)\"\n");
    s.record_with("bare_zsh_by_hand", &["completions"], shell("/bin/zsh"));
    // What install.sh leaves for zsh: the saved script and the block in ~/.zshrc that loads it.
    std::fs::create_dir_all(s.home().join(".local/share/vendo/completions")).unwrap();
    s.file(".local/share/vendo/completions/vendo.zsh", "# the script\n");
    s.file(".zshrc", "\n# >>> vendo completions >>>\n# <<< vendo completions <<<\n");
    s.record_with("bare_zsh_installed", &["completions"], shell("/bin/zsh"));
    // The explanation goes to stderr and stdout stays empty, whatever the state: its indented lines are commands
    // (the installer's among them), so `eval "$(vendo completions $shell)"` or `vendo completions > <file>` with the
    // shell left out must get nothing, as before it could run bare.
    s.file(".bashrc", "eval \"$(vendo completions bash)\"\n");
    for value in [Some("/bin/zsh"), Some("/bin/bash"), Some("/opt/homebrew/bin/fish"), None] {
        let (code, stdout, stderr) = bare(&s, value);
        assert_eq!((code, text(&stdout)), (Some(0), String::new()), "SHELL={value:?}");
        assert!(text(&stderr).starts_with("`vendo completions <shell>` prints"), "SHELL={value:?}");
    }
    s.finish();
}

// ── vendo commands (VE-3831) ────────────────────────────────────────────────

#[tokio::test]
async fn commands_output() {
    let (_server, mut s) = Session::start("commands").await;
    s.record("list", &["commands"]);
    s.record("json", &["commands", "--json"]);
    s.finish();
}

/// What a help screen says about one command: its description, arguments, options (without
/// `-h, --help`), the options its usage line requires, the commands it lists and its global options.
#[derive(Debug, Default, PartialEq)]
struct HelpScreen {
    description: String,
    /// `(name, required)`: `<appId>` or `[shell]`.
    arguments: Vec<(String, bool, Vec<String>)>,
    options: Vec<HelpOption>,
    required: Vec<String>,
    commands: Vec<String>,
    global: Vec<HelpOption>,
}

#[derive(Debug, Clone, PartialEq)]
struct HelpOption {
    name: String,
    short: Option<String>,
    value_name: Option<String>,
    description: String,
    default: Option<String>,
    possible: Vec<String>,
}

/// `text [default: x] [possible values: a, b]` → `(text, default, values)`.
fn help_suffixes(text: &str) -> (String, Option<String>, Vec<String>) {
    let mut text = text.trim().to_string();
    let mut possible = Vec::new();
    if let Some(at) = text.find("[possible values: ") {
        possible = text[at + 18..].trim_end_matches(']').split(", ").map(str::to_string).collect();
        text = text[..at].trim_end().to_string();
    }
    let mut default = None;
    if let Some(at) = text.find(" [default: ").or_else(|| text.starts_with("[default: ").then_some(0)) {
        let rest = text[at..].trim_start();
        default = Some(rest["[default: ".len()..].trim_end_matches(']').to_string());
        text = text[..at].trim_end().to_string();
    }
    (text, default, possible)
}

/// `  -y, --yes  Skip …` or `      --type <appType>  App type …`.
fn help_option(line: &str) -> HelpOption {
    let line = line.trim_start();
    let (short, rest) = match line.strip_prefix('-').filter(|_| !line.starts_with("--")) {
        Some(_) => (Some(line[..2].to_string()), line[2..].trim_start_matches(", ")),
        None => (None, line),
    };
    let (spec, description) = rest.split_once("  ").unwrap_or((rest, ""));
    let (name, value_name) = match spec.split_once(' ') {
        Some((name, value)) => (name, Some(value.trim_matches(['<', '>']).to_string())),
        None => (spec, None),
    };
    let (description, default, possible) = help_suffixes(description);
    HelpOption { name: name.to_string(), short, value_name, description, default, possible }
}

fn read_help_screen(screen: &str) -> HelpScreen {
    let mut help =
        HelpScreen { description: screen.lines().next().unwrap_or_default().to_string(), ..Default::default() };
    let mut section = "";
    for line in screen.lines() {
        if let Some(usage) = line.strip_prefix("Usage: ") {
            // `vendo apps create [OPTIONS] --type <appType> --name <displayName>`: the options after [OPTIONS].
            let after = usage.split_once("[OPTIONS]").map_or("", |(_, after)| after);
            help.required =
                after.split_whitespace().filter(|word| word.starts_with("--")).map(str::to_string).collect();
            continue;
        }
        if line.ends_with(':') && !line.starts_with(' ') {
            section = line;
            continue;
        }
        if line.is_empty() || !line.starts_with("  ") {
            continue;
        }
        match section {
            "Arguments:" => {
                let (spec, rest) = line.trim().split_once("  ").unwrap_or((line.trim(), ""));
                let (_, _, possible) = help_suffixes(rest);
                help.arguments.push((
                    spec.trim_matches(['<', '>', '[', ']']).to_string(),
                    spec.starts_with('<'),
                    possible,
                ));
            }
            "Options:" | "Global options:" => {
                let option = help_option(line);
                if option.name == "--help" {
                    continue;
                }
                if section == "Options:" { help.options.push(option) } else { help.global.push(option) }
            }
            _ => {}
        }
    }
    help.commands = subcommands(screen);
    help
}

/// The same facts, from a node of `vendo commands --json`.
fn json_screen(node: &Value, root_globals: &[HelpOption]) -> HelpScreen {
    let strings = |values: &Value| -> Vec<String> {
        values.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
    };
    let text = |value: &Value| value.as_str().map(str::to_string);
    let option = |o: &Value| HelpOption {
        name: o["name"].as_str().unwrap().to_string(),
        short: text(&o["short"]),
        value_name: text(&o["valueName"]),
        description: text(&o["description"]).unwrap_or_default(),
        default: text(&o["default"]),
        possible: strings(&o["possibleValues"]),
    };
    let options: Vec<&Value> = node["options"].as_array().unwrap().iter().collect();
    let is_root = node["path"] == "";
    HelpScreen {
        description: node["description"].as_str().unwrap().to_string(),
        arguments: node["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a["name"].as_str().unwrap().to_string(),
                    a["required"].as_bool().unwrap(),
                    strings(&a["possibleValues"]),
                )
            })
            .collect(),
        options: options.iter().map(|o| option(o)).collect(),
        required: options
            .iter()
            .filter(|o| o["required"] == true)
            .map(|o| o["name"].as_str().unwrap().to_string())
            .collect(),
        commands: node["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect(),
        // Every screen but the root's lists --profile and --debug apart (VE-3827).
        global: if is_root { Vec::new() } else { root_globals.to_vec() },
    }
}

#[test]
fn the_command_tree_matches_the_help_screens() {
    // `vendo commands --json` reads the clap tree; this walks the help screens the CLI prints,
    // as the help snapshots do, and checks each command against its own screen: description,
    // arguments, options with value names, defaults and possible values, required options and
    // the commands it lists. Suggested values never show in help (VE-3830).
    let sandbox = Sandbox::new(CLOSED);
    let out = sandbox.run(&["commands", "--json"]);
    assert_eq!((out.status.code(), text(&out.stderr)), (Some(0), String::new()));
    let tree: Value = serde_json::from_str(&text(&out.stdout)).unwrap();
    let root_globals: Vec<HelpOption> = tree["options"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["global"] == true)
        .map(|o| {
            help_option(&format!(
                "      {}{}  {}",
                o["name"].as_str().unwrap(),
                o["valueName"].as_str().map(|v| format!(" <{v}>")).unwrap_or_default(),
                o["description"].as_str().unwrap()
            ))
        })
        .collect();
    let mut checked = 0;
    let mut pending = vec![(Vec::<String>::new(), &tree)];
    while let Some((path, node)) = pending.pop() {
        assert_eq!(node["path"], path.join(" "));
        let args: Vec<&str> = path.iter().map(String::as_str).chain(["--help"]).collect();
        let screen = read_help_screen(&text(&sandbox.run(&args).stdout));
        assert_eq!(json_screen(node, &root_globals), screen, "vendo {}", args.join(" "));
        checked += 1;
        for child in node["commands"].as_array().unwrap() {
            let child_path = path.iter().cloned().chain([child["name"].as_str().unwrap().to_string()]).collect();
            pending.push((child_path, child));
        }
    }
    // Every help screen the snapshot walk records, less the hidden `catalog credential-schema`.
    let help_screens = std::fs::read_dir(Path::new(SNAPSHOTS).join("help"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".snap"))
        .count();
    assert_eq!(checked, help_screens - HIDDEN_COMMANDS.len());
}

#[test]
fn the_help_screen_reader_reads_clap_screens() {
    let screen = "Create a new app\n\nUsage: vendo apps create [OPTIONS] --type <appType>\n\nArguments:\n  [shell]  [possible values: bash, zsh]\n\nOptions:\n      --type <appType>  App type\n      --role <role>     Role [default: source]\n  -y, --yes             Skip\n  -h, --help            Print help\n\nGlobal options:\n      --debug  Debug\n";
    let help = read_help_screen(screen);
    assert_eq!(help.description, "Create a new app");
    assert_eq!(help.arguments, [("shell".to_string(), false, vec!["bash".to_string(), "zsh".to_string()])]);
    assert_eq!(help.required, ["--type"]);
    let option =
        |name: &str, short: Option<&str>, value: Option<&str>, description: &str, default: Option<&str>| HelpOption {
            name: name.to_string(),
            short: short.map(str::to_string),
            value_name: value.map(str::to_string),
            description: description.to_string(),
            default: default.map(str::to_string),
            possible: Vec::new(),
        };
    assert_eq!(
        help.options,
        [
            option("--type", None, Some("appType"), "App type", None),
            option("--role", None, Some("role"), "Role", Some("source")),
            option("--yes", Some("-y"), None, "Skip", None),
        ]
    );
    assert_eq!(help.global[0].name, "--debug");
}

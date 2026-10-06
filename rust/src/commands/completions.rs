//! `vendo completions` (VE-3830). With a shell it prints the TAB-completion script that the
//! installer (`install.sh`, `install_completions`) saves and loads. With none it says what the
//! command does, whether completions are set up for the shell `$SHELL` names, and how to set them
//! up. Doctor's "Shell completions" check reads the same [`setup`].
//!
//! With `--json` (VE-3831) it prints `{ "shell", "script" }`, or bare, on stdout, what the
//! explanation says of the set-up: `{ "shell", "installed" }`.

use std::path::Path;

use serde_json::{Value, json};

use crate::{
    cli::{self, Shell},
    output,
    update_check::INSTALL_COMMAND,
};

pub fn run(ctx: &crate::context::Ctx, shell: Option<Shell>, json: bool) {
    match (shell, json) {
        (Some(shell), false) => output::write_stdout_bytes(&script(shell)),
        (Some(shell), true) => output::print_json(&json!({
            "shell": name(shell),
            "script": String::from_utf8_lossy(&script(shell)),
        })),
        // On stderr, so stdout only ever carries a script: the explanation's indented lines are
        // commands (the installer's among them), and `eval "$(vendo completions $shell)"` or a
        // redirect with the shell left out must get nothing, as before it could run bare.
        (None, false) => eprint!("{}", explain(login_shell().as_deref(), &ctx.home)),
        (None, true) => output::print_json(&setup_json(setup(login_shell().as_deref(), &ctx.home))),
    }
}

/// `bash`, `zsh` or `fish`, as the command takes it.
fn name(shell: Shell) -> String {
    clap_complete::Shell::from(shell).to_string()
}

/// Bare `completions --json`: the shell `$SHELL` names and whether completions are set up for it,
/// both null when it names none of bash, zsh and fish (the explanation's "could not be detected").
fn setup_json(setup: Setup) -> Value {
    match setup {
        Setup::Installed(shell) => json!({ "shell": name(shell), "installed": true }),
        Setup::Missing(shell) => json!({ "shell": name(shell), "installed": false }),
        Setup::Unknown => json!({ "shell": null, "installed": null }),
    }
}

/// The completion script for `shell`. Rendered to a buffer: clap_complete panics when its writer fails.
fn script(shell: Shell) -> Vec<u8> {
    // `vendo completions` without a shell only explains itself, so the scripts still complete it
    // with a shell to name, as they did before it could run bare. (`mut_subcommands` keeps the
    // commands in order; `mut_subcommand` would move this one last.)
    let mut command = cli::command().mut_subcommands(|cmd| match cmd.get_name() {
        "completions" => cmd.mut_arg("shell", |arg| arg.required(true)),
        _ => cmd,
    });
    let mut script = Vec::new();
    cli::suggesting(|| clap_complete::generate(clap_complete::Shell::from(shell), &mut command, "vendo", &mut script));
    script
}

/// The shell `$SHELL` names (its last path segment), which is the shell the installer sets up.
pub fn login_shell() -> Option<String> {
    std::env::var("SHELL").ok().and_then(|s| s.rsplit('/').next().map(str::to_string)).filter(|s| !s.is_empty())
}

/// Whether completions are set up for the shell `$SHELL` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    Installed(Shell),
    Missing(Shell),
    /// `$SHELL` is unset or names a shell other than bash, zsh and fish.
    Unknown,
}

impl Setup {
    /// One line on the state, as doctor's check prints it.
    pub fn detail(self) -> String {
        match self {
            Setup::Installed(Shell::Bash) => "Bash completions are installed in ~/.bashrc".into(),
            Setup::Installed(Shell::Zsh) => "Zsh completions are installed in ~/.zshrc".into(),
            Setup::Installed(Shell::Fish) => "Fish completions are installed".into(),
            Setup::Missing(shell) => format!("{} completions are not installed yet", label(shell)),
            Setup::Unknown => "Current shell could not be detected automatically".into(),
        }
    }
}

/// Whether completions for `shell` load from what is under `home`: the script the installer
/// saves (not empty), with, for bash and zsh, the `# >>> vendo completions >>>` block it adds to
/// the startup file (fish loads its completions folder itself); or, for bash and zsh, a line in
/// the startup file that runs `vendo completions <shell>`, as [`by_hand`] and the docs set it up.
pub fn setup(shell: Option<&str>, home: &Path) -> Setup {
    let shell = match shell {
        Some("bash") => Shell::Bash,
        Some("zsh") => Shell::Zsh,
        Some("fish") => Shell::Fish,
        _ => return Setup::Unknown,
    };
    let (script, rc) = match shell {
        Shell::Bash => (".local/share/vendo/completions/vendo.bash", Some(".bashrc")),
        Shell::Zsh => (".local/share/vendo/completions/vendo.zsh", Some(".zshrc")),
        Shell::Fish => (".config/fish/completions/vendo.fish", None),
    };
    let saved = std::fs::metadata(home.join(script)).is_ok_and(|file| file.len() > 0);
    let loaded = |rc: &str| {
        let text = std::fs::read_to_string(home.join(rc)).unwrap_or_default();
        let by_hand = format!("vendo completions {}", name(shell));
        (saved && text.contains("# >>> vendo completions >>>"))
            || text.lines().any(|line| !line.trim_start().starts_with('#') && line.contains(&by_hand))
    };
    if rc.map_or(saved, loaded) { Setup::Installed(shell) } else { Setup::Missing(shell) }
}

fn label(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "Bash",
        Shell::Zsh => "Zsh",
        Shell::Fish => "Fish",
    }
}

/// How to load completions without the installer: what to do, and the lines it takes. Zsh needs
/// `compinit` before the script, which registers itself with `compdef`, and a stock `~/.zshrc`
/// does not run it. Fish's line saves the script where the installer does.
fn by_hand(shell: Shell) -> (&'static str, &'static [&'static str]) {
    match shell {
        Shell::Bash => ("add this line to ~/.bashrc:", &[r#"eval "$(vendo completions bash)""#]),
        Shell::Zsh => (
            "add these lines to ~/.zshrc:",
            &["autoload -Uz compinit && compinit", r#"eval "$(vendo completions zsh)""#],
        ),
        Shell::Fish => (
            "save the script where fish loads it:",
            &["vendo completions fish > ~/.config/fish/completions/vendo.fish"],
        ),
    }
}

/// What bare `vendo completions` prints for `shell` (the name `$SHELL` gives) and `home`.
fn explain(shell: Option<&str>, home: &Path) -> String {
    let setup = setup(shell, home);
    let mut text = String::from(
        "`vendo completions <shell>` prints the TAB-completion script for bash, zsh or fish.\n\
         The installer saves it and sets up your shell to load it, so you rarely need to run it yourself.\n\n",
    );
    text.push_str(&format!("{}.\n\n", setup.detail()));
    let mut steps = |intro: String, lines: &[&str]| {
        text.push_str(&intro);
        text.push('\n');
        for line in lines {
            text.push_str(&format!("  {line}\n"));
        }
    };
    match setup {
        Setup::Installed(_) => steps("To reinstall them, run the installer:".into(), &[INSTALL_COMMAND]),
        Setup::Missing(shell) => {
            steps("To install them, run the installer:".into(), &[INSTALL_COMMAND]);
            let (intro, lines) = by_hand(shell);
            steps(format!("or {intro}"), lines);
        }
        Setup::Unknown => {
            steps("Completions are available for bash, zsh and fish. To install them:".into(), &[]);
            for (name, shell) in [("bash", Shell::Bash), ("zsh", Shell::Zsh), ("fish", Shell::Fish)] {
                let (intro, lines) = by_hand(shell);
                steps(format!("For {name}, {intro}"), lines);
            }
        }
    }
    text.push_str("Then open a new terminal.\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(home: &Path, name: &str, contents: &[u8]) {
        std::fs::create_dir_all(home.join(name).parent().unwrap()).unwrap();
        std::fs::write(home.join(name), contents).unwrap();
    }

    /// A HOME with what install.sh's `install_completions` leaves for `shell`.
    fn installed(shell: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let (script, rc) = match shell {
            "bash" => (".local/share/vendo/completions/vendo.bash", Some(".bashrc")),
            "zsh" => (".local/share/vendo/completions/vendo.zsh", Some(".zshrc")),
            _ => (".config/fish/completions/vendo.fish", None),
        };
        write(h, script, b"# the script\n");
        if let Some(rc) = rc {
            write(h, rc, b"\n# >>> vendo completions >>>\n# <<< vendo completions <<<\n");
        }
        home
    }

    /// A HOME where someone followed [`by_hand`] for `shell`: its lines in the startup file, or for
    /// fish the script saved where its line saves it.
    fn set_up_by_hand(shell: Shell) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let lines = by_hand(shell).1.join("\n") + "\n";
        match shell {
            Shell::Bash => write(home.path(), ".bashrc", lines.as_bytes()),
            Shell::Zsh => write(home.path(), ".zshrc", lines.as_bytes()),
            Shell::Fish => write(home.path(), ".config/fish/completions/vendo.fish", &script(shell)),
        }
        home
    }

    #[test]
    fn the_installers_files_or_the_manual_steps_count_as_installed() {
        for (name, shell) in [("bash", Shell::Bash), ("zsh", Shell::Zsh), ("fish", Shell::Fish)] {
            assert_eq!(setup(Some(name), installed(name).path()), Setup::Installed(shell), "{name}");
            assert_eq!(setup(Some(name), set_up_by_hand(shell).path()), Setup::Installed(shell), "{name} by hand");
            assert_eq!(setup(Some(name), tempfile::tempdir().unwrap().path()), Setup::Missing(shell), "{name}");
        }
        // The saved script without the startup-file block that loads it, or the block without the script.
        let home = installed("zsh");
        write(home.path(), ".zshrc", b"export EDITOR=vi\n");
        assert_eq!(setup(Some("zsh"), home.path()), Setup::Missing(Shell::Zsh));
        let home = installed("bash");
        std::fs::remove_file(home.path().join(".local/share/vendo/completions/vendo.bash")).unwrap();
        assert_eq!(setup(Some("bash"), home.path()), Setup::Missing(Shell::Bash));
        // An empty script loads nothing: what `vendo completions > <file>` leaves with the shell left out.
        for name in ["zsh", "fish"] {
            let home = installed(name);
            let script = if name == "zsh" {
                ".local/share/vendo/completions/vendo.zsh"
            } else {
                ".config/fish/completions/vendo.fish"
            };
            write(home.path(), script, b"");
            assert!(matches!(setup(Some(name), home.path()), Setup::Missing(_)), "{name}");
        }
        // A commented-out line, or another shell's, loads nothing.
        for rc in [
            "# eval \"$(vendo completions zsh)\"\n",
            "  #eval \"$(vendo completions zsh)\"\n",
            "eval \"$(vendo completions bash)\"\n",
        ] {
            let home = tempfile::tempdir().unwrap();
            write(home.path(), ".zshrc", rc.as_bytes());
            assert_eq!(setup(Some("zsh"), home.path()), Setup::Missing(Shell::Zsh), "{rc}");
        }
        for shell in [None, Some(""), Some("tcsh"), Some("Zsh")] {
            assert_eq!(setup(shell, installed("zsh").path()), Setup::Unknown, "{shell:?}");
        }
    }

    #[test]
    fn bare_json_says_the_shell_and_whether_it_is_set_up() {
        assert_eq!(
            setup_json(setup(Some("zsh"), installed("zsh").path())),
            json!({ "shell": "zsh", "installed": true })
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(setup_json(setup(Some("fish"), empty.path())), json!({ "shell": "fish", "installed": false }));
        assert_eq!(setup_json(setup(Some("tcsh"), empty.path())), json!({ "shell": null, "installed": null }));
    }

    #[test]
    fn the_explanation_says_what_to_do_for_each_state() {
        let missing = explain(Some("bash"), tempfile::tempdir().unwrap().path());
        assert!(missing.contains("Bash completions are not installed yet.\n"), "{missing}");
        assert!(missing.contains(&format!("  {INSTALL_COMMAND}\nor add this line to ~/.bashrc:\n")), "{missing}");
        let home = installed("bash");
        let done = explain(Some("bash"), home.path());
        assert!(done.contains("Bash completions are installed in ~/.bashrc.\n\nTo reinstall them"), "{done}");
        assert!(!done.contains("vendo completions bash"), "no second way to load them: {done}");
        // After its own manual steps it says they are installed, not the same steps again.
        let done = explain(Some("zsh"), set_up_by_hand(Shell::Zsh).path());
        assert!(done.contains("Zsh completions are installed in ~/.zshrc.\n"), "{done}");
        let unknown = explain(None, home.path());
        for line in ["  eval \"$(vendo completions bash)\"", "  autoload -Uz compinit && compinit", "fish loads it:"] {
            assert!(unknown.contains(line), "{line}: {unknown}");
        }
        assert!(!unknown.contains(INSTALL_COMMAND), "the installer skips an unknown shell: {unknown}");
    }
}

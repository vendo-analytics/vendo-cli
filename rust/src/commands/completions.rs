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
    let os = Os::current();
    match (shell, json) {
        (Some(shell), false) => output::write_stdout_bytes(&script(shell)),
        (Some(shell), true) => output::print_json(&json!({
            "shell": name(shell),
            "script": String::from_utf8_lossy(&script(shell)),
        })),
        // On stderr, so without --json stdout only ever carries a script: the explanation's indented lines are
        // commands (the installer's among them), and `eval "$(vendo completions $shell)"` or a
        // redirect with the shell left out must get nothing, as before it could run bare.
        (None, false) => eprint!("{}", explain(login_shell().as_deref(), &ctx.home, os)),
        (None, true) => output::print_json(&setup_json(setup(login_shell().as_deref(), &ctx.home, os))),
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
        Setup::Installed(shell, _) => json!({ "shell": name(shell), "installed": true }),
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

/// Where bash reads its startup files depends on the system (VE-3830, Yalcin 2026-10-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Terminal opens each window as a login shell, which reads its login file ([`bash_login_file`]),
    /// not `~/.bashrc`, so install.sh adds its block to both (`uname -s` is `Darwin`).
    MacOs,
    Other,
}

impl Os {
    /// The system this binary runs on: a macOS build runs only there, as install.sh's `uname -s` reads it.
    pub fn current() -> Os {
        if cfg!(target_os = "macos") { Os::MacOs } else { Os::Other }
    }
}

/// The files a bash login shell looks for, in the order it reads the first that exists.
const BASH_LOGIN_FILES: [&str; 3] = [".bash_profile", ".bash_login", ".profile"];

/// The file under `home` a bash login shell reads, as install.sh's `bash_login_file` picks it: the first
/// of [`BASH_LOGIN_FILES`] that exists, else `.bash_profile`, which the installer creates then (never
/// beside an existing `.profile`, which bash would stop reading).
pub fn bash_login_file(home: &Path) -> &'static str {
    BASH_LOGIN_FILES.into_iter().find(|file| home.join(file).exists()).unwrap_or(".bash_profile")
}

/// Whether completions are set up for the shell `$SHELL` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// Set up, loaded from these startup files under HOME (none for fish, which loads its
    /// completions folder itself).
    Installed(Shell, Vec<&'static str>),
    Missing(Shell),
    /// `$SHELL` is unset or names a shell other than bash, zsh and fish.
    Unknown,
}

impl Setup {
    /// One line on the state, as doctor's check prints it.
    pub fn detail(&self) -> String {
        match self {
            Setup::Installed(Shell::Fish, _) => "Fish completions are installed".into(),
            Setup::Installed(shell, files) => {
                let files: Vec<String> = files.iter().map(|file| format!("~/{file}")).collect();
                let files = match files.split_last() {
                    Some((last, [])) => last.clone(),
                    Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
                    None => String::new(),
                };
                format!("{} completions are installed in {files}", label(*shell))
            }
            Setup::Missing(shell) => format!("{} completions are not installed yet", label(*shell)),
            Setup::Unknown => "Current shell could not be detected automatically".into(),
        }
    }
}

/// Whether completions for `shell` load from what is under `home` on `os`: the script the installer
/// saves (not empty), with, for bash and zsh, the `# >>> vendo completions >>>` block it adds to a
/// startup file (fish loads its completions folder itself); or, for bash and zsh, a line in a startup
/// file that runs `vendo completions <shell>`, as [`by_hand`] and the docs set it up. For zsh that
/// line counts only after compinit ([`zsh_loads_by_hand`]). Bash's startup files are `~/.bashrc` and,
/// on macOS, its login files too ([`BASH_LOGIN_FILES`]).
pub fn setup(shell: Option<&str>, home: &Path, os: Os) -> Setup {
    let shell = match shell {
        Some("bash") => Shell::Bash,
        Some("zsh") => Shell::Zsh,
        Some("fish") => Shell::Fish,
        _ => return Setup::Unknown,
    };
    let (script, startup_files): (&str, Vec<&'static str>) = match shell {
        Shell::Bash if os == Os::MacOs => {
            (".local/share/vendo/completions/vendo.bash", [".bashrc"].into_iter().chain(BASH_LOGIN_FILES).collect())
        }
        Shell::Bash => (".local/share/vendo/completions/vendo.bash", vec![".bashrc"]),
        Shell::Zsh => (".local/share/vendo/completions/vendo.zsh", vec![".zshrc"]),
        Shell::Fish => (".config/fish/completions/vendo.fish", vec![]),
    };
    let saved = std::fs::metadata(home.join(script)).is_ok_and(|file| file.len() > 0);
    if shell == Shell::Fish {
        return if saved { Setup::Installed(shell, vec![]) } else { Setup::Missing(shell) };
    }
    let loads = |file: &&str| {
        let text = std::fs::read_to_string(home.join(file)).unwrap_or_default();
        (saved && text.contains("# >>> vendo completions >>>"))
            || match shell {
                Shell::Zsh => zsh_loads_by_hand(&text),
                _ => uncommented(&text).any(|line| line.contains("vendo completions bash")),
            }
    };
    let found: Vec<&'static str> = startup_files.into_iter().filter(loads).collect();
    if found.is_empty() { Setup::Missing(shell) } else { Setup::Installed(shell, found) }
}

/// The lines of a startup file that are not comments.
fn uncommented(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|line| !line.trim_start().starts_with('#'))
}

/// The zsh frameworks whose start-up runs compinit, by the file a `~/.zshrc` sources to start
/// them: oh-my-zsh (`$ZSH/oh-my-zsh.sh`), Prezto (`.zprezto/init.zsh`) and Zim (`${ZIM_HOME}/init.zsh`,
/// `~/.zim/init.zsh`).
const ZSH_FRAMEWORKS: [&str; 5] =
    ["/oh-my-zsh.sh", "/.zprezto/init.zsh", "ZIM_HOME}/init.zsh", "ZIM_HOME/init.zsh", "/.zim/init.zsh"];

/// Whether `zshrc` runs `vendo completions zsh` after compinit has run: the script registers itself
/// with `compdef`, which compinit defines, so on a stock zsh the line alone fails (Yalcin,
/// 2026-10-06). compinit runs on an earlier line that is not a comment (or earlier on the same line)
/// when a command there is `compinit`, or sources one of [`ZSH_FRAMEWORKS`].
fn zsh_loads_by_hand(zshrc: &str) -> bool {
    let mut compinit = false;
    for command in uncommented(zshrc).flat_map(|line| line.split([';', '&', '|'])) {
        if command.contains("vendo completions zsh") && compinit {
            return true;
        }
        compinit = compinit || runs_compinit(command);
    }
    false
}

/// Whether one shell command runs compinit: `compinit …`, or `source`/`.` of a framework that does.
fn runs_compinit(command: &str) -> bool {
    let mut words = command.split_whitespace().skip_while(|word| matches!(*word, "then" | "else" | "do" | "{" | "("));
    match words.next() {
        Some("compinit") => true,
        Some("source" | ".") => words.next().is_some_and(|file| {
            let file = file.trim_matches(['"', '\'']);
            ZSH_FRAMEWORKS.iter().any(|framework| file.ends_with(framework))
        }),
        _ => false,
    }
}

fn label(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "Bash",
        Shell::Zsh => "Zsh",
        Shell::Fish => "Fish",
    }
}

/// The line that loads bash completions by hand.
const BASH_LINE: &[&str] = &[r#"eval "$(vendo completions bash)""#];

/// How to load completions without the installer on `os`: what to do, and the lines it takes. Bash's
/// line goes in the file bash reads: `~/.bashrc`, or on macOS the login file ([`bash_login_file`]).
/// Zsh needs `compinit` before the script, which registers itself with `compdef`, and a stock
/// `~/.zshrc` does not run it; at the end of the file, a framework's later compinit cannot drop it.
/// Fish's line saves the script where the installer does.
fn by_hand(shell: Shell, home: &Path, os: Os) -> (String, &'static [&'static str]) {
    match shell {
        Shell::Bash if os == Os::MacOs => (format!("add this line to ~/{}:", bash_login_file(home)), BASH_LINE),
        Shell::Bash => ("add this line to ~/.bashrc:".into(), BASH_LINE),
        Shell::Zsh => (
            "add these lines to the end of ~/.zshrc:".into(),
            &["autoload -Uz compinit && compinit", r#"eval "$(vendo completions zsh)""#],
        ),
        Shell::Fish => (
            "save the script where fish loads it:".into(),
            &["vendo completions fish > ~/.config/fish/completions/vendo.fish"],
        ),
    }
}

/// What bare `vendo completions` prints for `shell` (the name `$SHELL` gives), `home` and `os`.
fn explain(shell: Option<&str>, home: &Path, os: Os) -> String {
    let setup = setup(shell, home, os);
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
        Setup::Installed(..) => steps("To reinstall them, run the installer:".into(), &[INSTALL_COMMAND]),
        Setup::Missing(shell) => {
            steps("To install them, run the installer:".into(), &[INSTALL_COMMAND]);
            let (intro, lines) = by_hand(shell, home, os);
            steps(format!("or {intro}"), lines);
        }
        Setup::Unknown => {
            steps("Completions are available for bash, zsh and fish. To install them:".into(), &[]);
            for (name, shell) in [("bash", Shell::Bash), ("zsh", Shell::Zsh), ("fish", Shell::Fish)] {
                let (intro, lines) = by_hand(shell, home, os);
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

    const ZSH_SCRIPT: &str = ".local/share/vendo/completions/vendo.zsh";
    const BASH_SCRIPT: &str = ".local/share/vendo/completions/vendo.bash";
    const BLOCK: &str = "\n# >>> vendo completions >>>\n# <<< vendo completions <<<\n";

    fn write(home: &Path, name: &str, contents: &[u8]) {
        std::fs::create_dir_all(home.join(name).parent().unwrap()).unwrap();
        std::fs::write(home.join(name), contents).unwrap();
    }

    /// A HOME with what install.sh's `install_completions` leaves for `shell` on Linux.
    fn installed(shell: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let (script, rc) = match shell {
            "bash" => (BASH_SCRIPT, Some(".bashrc")),
            "zsh" => (ZSH_SCRIPT, Some(".zshrc")),
            _ => (".config/fish/completions/vendo.fish", None),
        };
        write(h, script, b"# the script\n");
        if let Some(rc) = rc {
            write(h, rc, BLOCK.as_bytes());
        }
        home
    }

    /// A HOME where someone followed [`by_hand`] for `shell` on `os`: its lines in the file it names, or for
    /// fish the script saved where its line saves it.
    fn set_up_by_hand(shell: Shell, os: Os) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let lines = by_hand(shell, home.path(), os).1.join("\n") + "\n";
        match shell {
            Shell::Bash if os == Os::MacOs => write(home.path(), ".bash_profile", lines.as_bytes()),
            Shell::Bash => write(home.path(), ".bashrc", lines.as_bytes()),
            Shell::Zsh => write(home.path(), ".zshrc", lines.as_bytes()),
            Shell::Fish => write(home.path(), ".config/fish/completions/vendo.fish", &script(shell)),
        }
        home
    }

    fn zshrc(contents: &str) -> Setup {
        let home = tempfile::tempdir().unwrap();
        write(home.path(), ".zshrc", contents.as_bytes());
        setup(Some("zsh"), home.path(), Os::Other)
    }

    #[test]
    fn the_installers_files_or_the_manual_steps_count_as_installed() {
        for os in [Os::MacOs, Os::Other] {
            for (name, shell) in [("bash", Shell::Bash), ("zsh", Shell::Zsh), ("fish", Shell::Fish)] {
                assert!(matches!(setup(Some(name), installed(name).path(), os), Setup::Installed(s, _) if s == shell));
                let by_hand = set_up_by_hand(shell, os);
                assert!(
                    matches!(setup(Some(name), by_hand.path(), os), Setup::Installed(s, _) if s == shell),
                    "{name} by hand on {os:?}"
                );
                let empty = tempfile::tempdir().unwrap();
                assert_eq!(setup(Some(name), empty.path(), os), Setup::Missing(shell), "{name} on {os:?}");
            }
        }
        // The saved script without the startup-file block that loads it, or the block without the script.
        let home = installed("zsh");
        write(home.path(), ".zshrc", b"export EDITOR=vi\n");
        assert_eq!(setup(Some("zsh"), home.path(), Os::Other), Setup::Missing(Shell::Zsh));
        let home = installed("bash");
        std::fs::remove_file(home.path().join(BASH_SCRIPT)).unwrap();
        assert_eq!(setup(Some("bash"), home.path(), Os::Other), Setup::Missing(Shell::Bash));
        // An empty script loads nothing: what `vendo completions > <file>` leaves with the shell left out.
        for name in ["zsh", "fish"] {
            let home = installed(name);
            let script = if name == "zsh" { ZSH_SCRIPT } else { ".config/fish/completions/vendo.fish" };
            write(home.path(), script, b"");
            assert!(matches!(setup(Some(name), home.path(), Os::Other), Setup::Missing(_)), "{name}");
        }
        // A commented-out line, or another shell's, loads nothing.
        for rc in [
            "autoload -Uz compinit && compinit\n# eval \"$(vendo completions zsh)\"\n",
            "autoload -Uz compinit && compinit\n  #eval \"$(vendo completions zsh)\"\n",
            "autoload -Uz compinit && compinit\neval \"$(vendo completions bash)\"\n",
        ] {
            assert_eq!(zshrc(rc), Setup::Missing(Shell::Zsh), "{rc}");
        }
        for shell in [None, Some(""), Some("tcsh"), Some("Zsh")] {
            assert_eq!(setup(shell, installed("zsh").path(), Os::Other), Setup::Unknown, "{shell:?}");
        }
    }

    #[test]
    fn a_zsh_line_counts_only_after_compinit() {
        // `vendo completions zsh` registers with `compdef`, which compinit defines (Yalcin, 2026-10-06).
        let loaded = Setup::Installed(Shell::Zsh, vec![".zshrc"]);
        let line = "eval \"$(vendo completions zsh)\"\n";
        for before in [
            "autoload -Uz compinit && compinit\n",
            "autoload -U compinit; compinit -i\n",
            "autoload -Uz compinit\nif [[ -n ${ZDOTDIR}/.zcompdump(#qN.mh+24) ]]; then\n  compinit\nelse\n  compinit -C\nfi\n",
            "zstyle ':completion:*' menu select\nautoload -Uz compinit\ncompinit\n",
            // The frameworks whose start-up runs compinit: oh-my-zsh, Prezto and Zim.
            "export ZSH=\"$HOME/.oh-my-zsh\"\nplugins=(git)\nsource $ZSH/oh-my-zsh.sh\n",
            ". \"$ZSH/oh-my-zsh.sh\"\n",
            "if [[ -s \"${ZDOTDIR:-$HOME}/.zprezto/init.zsh\" ]]; then\n  source \"${ZDOTDIR:-$HOME}/.zprezto/init.zsh\"\nfi\n",
            "source ${ZIM_HOME}/init.zsh\n",
            "source ~/.zim/init.zsh\n",
        ] {
            assert_eq!(zshrc(&format!("{before}{line}")), loaded, "{before}");
        }
        // compinit and the line on one line, compinit first.
        assert_eq!(zshrc("autoload -Uz compinit && compinit && eval \"$(vendo completions zsh)\"\n"), loaded);
        for rc in [
            // No compinit at all, as on a stock ~/.zshrc.
            line.to_string(),
            // compinit only loaded, never run.
            format!("autoload -Uz compinit\n{line}"),
            // compinit after the line, where the line has already failed.
            format!("{line}autoload -Uz compinit && compinit\n"),
            format!("export ZSH=\"$HOME/.oh-my-zsh\"\n{line}source $ZSH/oh-my-zsh.sh\n"),
            // compinit commented out, or another file sourced.
            format!("# autoload -Uz compinit && compinit\n{line}"),
            format!("source ~/.aliases.zsh\n{line}"),
            format!("echo compinit\n{line}"),
        ] {
            assert_eq!(zshrc(&rc), Setup::Missing(Shell::Zsh), "{rc}");
        }
        // The installer's block counts as before, wherever it sits.
        let home = installed("zsh");
        write(home.path(), ".zshrc", format!("{BLOCK}source $ZSH/oh-my-zsh.sh\n").as_bytes());
        assert_eq!(setup(Some("zsh"), home.path(), Os::Other), loaded);
    }

    #[test]
    fn on_macos_bash_also_loads_from_the_login_files() {
        // Terminal opens each window as a login shell, which reads ~/.bash_profile, ~/.bash_login or ~/.profile.
        for file in [".bash_profile", ".bash_login", ".profile"] {
            let home = tempfile::tempdir().unwrap();
            write(home.path(), BASH_SCRIPT, b"# the script\n");
            write(home.path(), file, BLOCK.as_bytes());
            assert_eq!(setup(Some("bash"), home.path(), Os::MacOs), Setup::Installed(Shell::Bash, vec![file]));
            assert_eq!(setup(Some("bash"), home.path(), Os::Other), Setup::Missing(Shell::Bash), "{file}");
            let home = tempfile::tempdir().unwrap();
            write(home.path(), file, b"eval \"$(vendo completions bash)\"\n");
            assert_eq!(setup(Some("bash"), home.path(), Os::MacOs), Setup::Installed(Shell::Bash, vec![file]));
            assert_eq!(setup(Some("bash"), home.path(), Os::Other), Setup::Missing(Shell::Bash), "{file}");
        }
        // What the installer leaves on macOS: the block in ~/.bashrc and in the login file.
        let home = installed("bash");
        write(home.path(), ".bash_profile", BLOCK.as_bytes());
        let both = setup(Some("bash"), home.path(), Os::MacOs);
        assert_eq!(both, Setup::Installed(Shell::Bash, vec![".bashrc", ".bash_profile"]));
        assert_eq!(both.detail(), "Bash completions are installed in ~/.bashrc and ~/.bash_profile");
        // ~/.bashrc counts there too.
        let only_bashrc = setup(Some("bash"), installed("bash").path(), Os::MacOs);
        assert_eq!(only_bashrc.detail(), "Bash completions are installed in ~/.bashrc");
        assert_eq!(
            setup(Some("zsh"), installed("zsh").path(), Os::MacOs).detail(),
            "Zsh completions are installed in ~/.zshrc"
        );
        assert_eq!(setup(Some("fish"), installed("fish").path(), Os::MacOs).detail(), "Fish completions are installed");
    }

    #[test]
    fn the_bash_login_file_is_the_first_that_exists_or_bash_profile() {
        // As install.sh picks it: never ~/.bash_profile beside an existing ~/.profile, which bash would stop reading.
        for (present, expected) in [
            (&[][..], ".bash_profile"),
            (&[".profile"][..], ".profile"),
            (&[".bash_login"][..], ".bash_login"),
            (&[".bash_login", ".profile"][..], ".bash_login"),
            (&[".bash_profile", ".profile"][..], ".bash_profile"),
            (&[".bash_profile", ".bash_login", ".profile"][..], ".bash_profile"),
        ] {
            let home = tempfile::tempdir().unwrap();
            for file in present {
                write(home.path(), file, b"");
            }
            assert_eq!(bash_login_file(home.path()), expected, "{present:?}");
            let (intro, lines) = by_hand(Shell::Bash, home.path(), Os::MacOs);
            assert_eq!(
                (intro.as_str(), lines),
                (&*format!("add this line to ~/{expected}:"), BASH_LINE),
                "{present:?}"
            );
            let (intro, _) = by_hand(Shell::Bash, home.path(), Os::Other);
            assert_eq!(intro, "add this line to ~/.bashrc:", "{present:?}");
        }
    }

    #[test]
    fn bare_json_says_the_shell_and_whether_it_is_set_up() {
        assert_eq!(
            setup_json(setup(Some("zsh"), installed("zsh").path(), Os::Other)),
            json!({ "shell": "zsh", "installed": true })
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            setup_json(setup(Some("fish"), empty.path(), Os::Other)),
            json!({ "shell": "fish", "installed": false })
        );
        assert_eq!(
            setup_json(setup(Some("tcsh"), empty.path(), Os::Other)),
            json!({ "shell": null, "installed": null })
        );
    }

    const INTRO: &str = "`vendo completions <shell>` prints the TAB-completion script for bash, zsh or fish.\n\
        The installer saves it and sets up your shell to load it, so you rarely need to run it yourself.\n\n";

    #[test]
    fn the_explanation_says_what_to_do_for_each_state() {
        let empty = tempfile::tempdir().unwrap();
        // zsh: the lines go at the end of ~/.zshrc, after any compinit a framework runs (Yalcin, 2026-10-06).
        for os in [Os::MacOs, Os::Other] {
            assert_eq!(
                explain(Some("zsh"), empty.path(), os),
                format!(
                    "{INTRO}Zsh completions are not installed yet.\n\n\
                     To install them, run the installer:\n  {INSTALL_COMMAND}\n\
                     or add these lines to the end of ~/.zshrc:\n  autoload -Uz compinit && compinit\n  \
                     eval \"$(vendo completions zsh)\"\nThen open a new terminal.\n"
                )
            );
        }
        // bash: the file bash reads, which on macOS is the login file.
        for (os, file) in [(Os::MacOs, "~/.bash_profile"), (Os::Other, "~/.bashrc")] {
            assert_eq!(
                explain(Some("bash"), empty.path(), os),
                format!(
                    "{INTRO}Bash completions are not installed yet.\n\n\
                     To install them, run the installer:\n  {INSTALL_COMMAND}\n\
                     or add this line to {file}:\n  eval \"$(vendo completions bash)\"\nThen open a new terminal.\n"
                ),
                "{os:?}"
            );
        }
        let home = installed("bash");
        let done = explain(Some("bash"), home.path(), Os::Other);
        assert!(done.contains("Bash completions are installed in ~/.bashrc.\n\nTo reinstall them"), "{done}");
        assert!(!done.contains("vendo completions bash"), "no second way to load them: {done}");
        // After its own manual steps it says they are installed, not the same steps again.
        let done = explain(Some("zsh"), set_up_by_hand(Shell::Zsh, Os::Other).path(), Os::Other);
        assert!(done.contains("Zsh completions are installed in ~/.zshrc.\n"), "{done}");
        let done = explain(Some("bash"), set_up_by_hand(Shell::Bash, Os::MacOs).path(), Os::MacOs);
        assert!(done.contains("Bash completions are installed in ~/.bash_profile.\n"), "{done}");
        for (os, bash) in [
            (Os::MacOs, "For bash, add this line to ~/.bash_profile:"),
            (Os::Other, "For bash, add this line to ~/.bashrc:"),
        ] {
            let unknown = explain(None, home.path(), os);
            for line in [
                bash,
                "  eval \"$(vendo completions bash)\"",
                "For zsh, add these lines to the end of ~/.zshrc:",
                "  autoload -Uz compinit && compinit",
                "fish loads it:",
            ] {
                assert!(unknown.contains(line), "{line}: {unknown}");
            }
            assert!(!unknown.contains(INSTALL_COMMAND), "the installer skips an unknown shell: {unknown}");
        }
    }
}

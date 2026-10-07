# vendo-cli

Agent guide for this repo, shared by every coding agent (Codex reads it directly;
Claude Code reads it through `CLAUDE.md` = `@AGENTS.md`). Edit this file, not `CLAUDE.md`.

## Purpose
Standalone TypeScript CLI — "manage your data pipeline from the terminal". Published as the
`vendo` binary. Talks to the pipelines/web API (`/api/v1/*`). **This repo is the canonical and
only active Vendo CLI** — the former in-monorepo `vendo-web-v2/apps/cli` was removed (2026-06-02).

## Stack
- Node ≥ 22, TypeScript 5.9, `commander` 13, `chalk` 5, `cli-table3`, `ora`.
- Build: `tsup` (ESM → `dist/cli.js`) + Node SEA single-executable for standalone binaries.
- Tests: `vitest`. Lint: `eslint` 9. Version: see `package.json`.

## Rust port (`rust/`, Linear project "Vendo CLI in Rust")
The CLI is being ported to Rust one command group at a time (VE-3664 to VE-3669); the TypeScript
CLI in `src/` stays the shipped binary and takes bug fixes only until the switch-over (VE-3669).
- Goal: works exactly like the TypeScript CLI, just faster. `pnpm parity --rust rust/target/debug/vendo`
  compares the two on staging; allowed differences live in `parity/intended-differences.json`.
- `--json` prints the API response verbatim (no key rewriting), so nested `config`/`schedule`/
  `metrics` keys print snake_case where the TS client camelCased them (decided 2026-10-05). Where a TS command
  built its own JSON (whoami's `config`, now in `workspace --json`; the `metrics` `{ data }` envelope), Rust keeps that shape (Yalcin,
  2026-10-05, VE-3668). Numbers print as the API sent them (serde_json `arbitrary_precision`), and keys take
  JavaScript's order, array-index keys first, as `JSON.parse` gave the TS CLI (VE-3728).
- Locale (VE-3728): dates, numbers, the rate-limit time and measurement money follow `LC_ALL`/`LC_MESSAGES`/
  `LANG` as Node's ICU does (ICU4X, `rust/src/output/locale.rs`). Node-generated tables fill ICU4X's gaps
  (`scripts/gen-ymd-patterns.mjs`, `scripts/gen-usd-patterns.mjs`), and `scripts/gen-locale-fixture.mjs`
  writes the Node values the tests check. Regenerate all three when Node's ICU changes.
- Confirmation (VE-3823, decided by Yalcin, 2026-10-05): delete, cancel, reset and `logout --all` ask y/N only when
  stdin and stdout are both terminals and prompts are not off (`CI`/`VENDO_NO_INPUT`, VE-3826). Otherwise they need
  `--yes` and stop with exit 1 before any request, where the TS CLI went ahead; `--json` no longer implies `--yes`.
  `output::confirm` owns this.
- Stack: clap 4, reqwest (rustls), tokio, serde_json (`preserve_order`, `arbitrary_precision`), ICU4X,
  comfy-table, indicatif, inquire and crossterm (the group menus, the `menu` feature).
  Toolchain: `rustup` stable (`~/.cargo/bin`); `pnpm rust:test`, `pnpm rust:build`,
  `cargo clippy --all-targets` and `cargo fmt` (120 columns, `rust/rustfmt.toml`) from `rust/`.
- Tests: unit tests sit next to the code; `rust/tests/cli.rs` runs the built binary end to end with an isolated
  HOME, fake keys, a local stub server and a fresh update-check cache, so nothing leaves the machine (VE-3727). The
  caller's `CI`, `VENDO_NO_INPUT` and `TERM` are removed, so prompts behave as on a person's terminal on CI runners
  too (VE-3826). A test that starts many stubs in one loop should drop each with `release(server).await`: dropping
  a wiremock `MockServer` blocks on its state outside tokio, which hangs for good once the test's task has spent
  tokio's cooperative budget (it hung the VE-3831 refusal test).
- Snapshots (VE-3824), the regression net once the parity harness goes at 1.0.0: `rust/tests/cli/snapshots.rs`
  records every `--help` screen, found by walking the real command tree, in `rust/tests/snapshots/help/` (clap's
  `help`, which the root help lists, takes no `--help`: its own screen is `vendo help help`, recorded as
  `help/help.snap`; VE-3893), the table, `--json` and confirmation output of each command against
  the stub's synthetic account in `rust/tests/snapshots/output/`, and the usage error of every command that requires
  a value (37 with the hidden `catalog credential-schema`), as text and with `--json`, in
  `rust/tests/snapshots/usage/` (VE-3881), with that of `--profile` typed last with no name (`usage/profile_flag`,
  VE-3892). Any change to them fails `cargo test`. After a deliberate change run
  `INSTA_UPDATE=always cargo test --test cli` from `rust/` (it also deletes stale snapshots), review
  `git diff rust/tests/snapshots` and commit the snapshots with the change. Bare `vendo completions` for bash and
  for an unknown shell is recorded per system (VE-3830): `completions__bare_bash_macos`/`_linux` and
  `completions__bare_unknown_shell_macos`/`_linux`. The update command rewrites only this system's two, and CI,
  which tests on Linux only, checks only the `_linux` ones. So after a change to what they show, on a Mac also force
  the Linux rules and run the update command again, from `rust/`:
  `sed -i '' 's/cfg!(target_os = "macos")/cfg!(any())/' src/commands/completions.rs tests/cli.rs tests/cli/snapshots.rs`
  (`cfg!(any())` is false; the three places are `Os::current`, `Session::record_per_system` and
  `the_installer_adds_bash_completions_where_bash_reads_them`), then
  `INSTA_UPDATE=always cargo test --locked --test cli`, then undo with the same `sed` the other way round
  (`s/cfg!(any())/cfg!(target_os = "macos")/`), check that `grep -rnF 'cfg!(any())' src tests` finds nothing, and
  review the `_linux` diff. Editing both systems' files by hand the same way also works.
- Customer words follow vendo-web-v2's glossary (`apps/web/CONTEXT.md`; VE-3828, CLI 1.1): `vendo destinations`
  (hidden aliases `integrations`, `int`), "app" not "app connection", "platform" not "integration type". Flag names
  (`jobs … --integration <integrationId>`), API paths, JSON fields, `--json` output and code identifiers keep the
  API's "integration"; renaming a flag needs Yalcin's approval.
- Help layout (VE-3827, CLI 1.1): `vendo --help` is one sectioned list built at runtime from the command tree;
  `HELP_SECTIONS` in `rust/src/cli.rs` places each command, and a test fails when a visible command is in none.
  Group screens have no `help` row; `--profile`/`--debug` sit under "Global options". "Getting started" ends with
  `help` and `version` (VE-3893, decided by Yalcin 2026-10-07). `help` is clap's own command, which the tree
  `cli::command` returns has only once clap builds it, so its row is `CLAP_HELP` in `cli.rs` (clap's words, which a
  test checks) with nothing under it; `vendo help <command>` prints that command's screen (`vendo help help` the
  `help` command's own), and `vendo help --help` stays clap's usage error (exit 2). `vendo version` prints exactly
  what `--version` and `-V` print, which `preprocess` still reads anywhere before `--` (`commands/version.rs`
  prints both), and with `--json`
  `{"version":"<v>"}`. ❓ Open for Yalcin, built with cautious defaults: the two rows last in "Getting started", their
  descriptions in clap's words for `-h` and `-V` ("Print this message or the help of the given subcommand(s)",
  "Print version"), `vendo commands` leaving `help` out, and `vendo help --help` left as the usage error. `workspace`
  is the one identity and setup command (VE-3891, below; `whoami` until then) and `config` moved under `profile`;
  `profile set` is described as "Set values on the active profile" (Yalcin, 2026-10-06). Old paths keep working,
  hidden, printing exactly what their command prints: clap hidden aliases where clap allows (`config` = `profile`,
  `use` = `profile switch`, `whoami` and `doctor` = `workspace`), `MOVED` in `cli.rs` where it does not
  (`profile current`/`config show` → `workspace`, `config reset` → `logout --all`). Give a new hidden path a
  byte-identity test.
- Login (VE-3825, CLI 1.1): `vendo login` does what `init` did, and `init` is its hidden clap alias. It signs in
  through the browser when there is no working key, checks the key with `/me` and prints the setup summary. A key it
  has (profile or `VENDO_API_KEY`) is checked and kept; `--force`, `--env`/`--base-url` naming another instance, or a
  401/403 from `/me` sign in again, and any other failed check exits 1 without creating a key. With `VENDO_API_KEY`
  set it never opens a browser. Tests act as the browser (`login_at_browser` in `rust/tests/cli.rs` visits the printed
  sign-in URL on the stub); stdin stays closed, so no real browser opens. The one test of a piped stdin (VE-3826)
  writes no line end and keeps the pipe open until `vendo` has exited (`OnTerminal` stops it first on a failure).
- Catalog (VE-3829, CLI 1.1): `vendo catalog list` ("List the platforms ready to connect", Yalcin 2026-10-06) shows
  what the API lists by default, the platforms ready to connect, and ends with a footer built from the response's
  `meta` counts (`35 ready · 560 more on request (vendo catalog list --all)`; only `35 ready` when none is on
  request; the plain count line when the API sends no counts). The API counts within `--category` and `--role`, so
  the hint repeats them, `--category` first and the values as typed (`(vendo catalog list --category advertising
  --all)`; Yalcin, 2026-10-06); an empty value, which the API ignores, is left out. `--all` sends
  `include_request_access=true` (the route has no pagination) and ends with the count line. The Availability column
  says the API's `availability` in plain words: `self_serve` "ready", `request_access` "on request", anything else
  as sent. `catalog get` shows the same words and colours on an `Availability:` line in place of `Self-Serve:
  yes/no`, its labels widened to line up with it (Yalcin, 2026-10-06); an API that sends no `availability` (before
  VE-2436) keeps the old view, Self-Serve line and all. `--json` prints the response as sent, list and get alike;
  without `--all` the request is unchanged.
- Models and metrics (CLI 1.1): `models list`'s Type column and `models get`'s title (`orders_clean (sql)`) and
  `Type:` line, which replaces `Data Type:`, show the API's `modelType` (`sql`, `bqml`, `grouping`, …; VE-3840,
  Yalcin 2026-10-06). The metrics routes send no type, so `metrics list` has no Type column and `metrics get` no
  type after the name and no `Type:` line (VE-3856, Yalcin 2026-10-06); Format has its own column. The TS CLI read
  fields the API does not send (`dataType`, `metric_type`) and showed `undefined` or a blank, which `pnpm parity`
  reports.
- Errored apps (VE-3841, CLI 1.1): `status` counts an app as errored when its `consecutiveFailureCount` is above 0.
  Only the single-app response had it until vendo-web-v2 PR #2147 adds it to the apps list (Yalcin, 2026-10-06), so
  against an API without that change the count is 0. The snapshot stub's list sends it as the PR builds it, Demo Ads
  with 2, so `account__status` records one errored app.
- Completions (VE-3830, CLI 1.1): `vendo completions <shell>` prints the script the installer saves. Bare, it exits 0
  and says on stderr what it does, whether completions are set up for the shell `$SHELL` names, and how to set them
  up, and stdout stays empty: without `--json`, stdout carries nothing but a script, so an `eval` or redirect that
  leaves the shell out gets nothing. With `--json` (VE-3831) stdout carries JSON instead and stderr stays quiet: bare,
  `{"shell","installed"}`; with a shell, the script wrapped in `{"shell","script"}`.
  `rust/src/commands/completions.rs` owns that detection (the installer's saved script and startup-file block, or a
  line in a startup file that runs `vendo completions <shell>`), and the workspace's check uses it. Bash's startup file is
  `~/.bashrc` and, on macOS, whose Terminal opens login shells, also its login file: the first of `~/.bash_profile`,
  `~/.bash_login` and `~/.profile` that exists (Yalcin, 2026-10-06). There install.sh (`uname -s` Darwin) adds its
  block to that file too, creating `~/.bash_profile` when none exists (never beside `~/.profile`, which bash would
  stop reading), and its summary names each file (`bash, loaded from ~/.bashrc and ~/.bash_profile`); bare
  `vendo completions` and `vendo workspace` look in `~/.bashrc` and that one login file on macOS (`bash_login_file`), not in
  the login files bash skips, and the bare text's bash step names the login file.
  A zsh line counts only after compinit, which the script's `compdef` needs: an earlier line, not a comment (or a
  command earlier on its line), that runs `compinit` or sources oh-my-zsh (`oh-my-zsh.sh`), Prezto
  (`.zprezto/init.zsh`) or Zim (`${ZIM_HOME}/init.zsh`, `~/.zim/init.zsh`); the installer's block counts as before.
  The bare text tells zsh users to add its lines at the end of `~/.zshrc`, after a framework's compinit. What it
  says for bash is recorded per system (`completions__…_macos`/`…_linux`; see Snapshots for refreshing both), and
  `the_installer_adds_bash_completions_where_bash_reads_them` in
  `rust/tests/cli.rs` runs install.sh's `install_completions` (the script without its last line, `main "$@"`) in
  scratch HOMEs. A flag with a fixed set of values offers them on TAB through `Suggest` in `cli.rs`, each list citing
  its source: parsing still takes any string (the API decides), and clap sees the values only while a script is
  generated (`cli::suggesting`), so help screens and parse errors are as they were. `jobs list`'s `--status` and
  `--type` help lists the values TAB offers (`JOB_STATUSES`, `JOB_TYPES`; Yalcin, 2026-10-06). The scripts are
  generated with the shell argument required, so they complete `completions` as before;
  `rust/tests/snapshots/output/completions__*` record them whole. `completions --help` shows `[shell]` where the TS
  CLI shows `<shell>`, which `pnpm parity:help` reports.
- Agents (VE-3831, decided by Yalcin 2026-10-05, CLI 1.1):
  - Errors with `--json`: one line of JSON on stderr, the last line, in one fixed shape (document it in cli.mdx at
    release): `{"error":{"message":"…","code":"NOT_FOUND"|null,"status":404|null,"requestId":"…"|null}}`. `message`
    is what the text error says after `Error: `; `code` the API's `error.code` (v1 routes; null for the web-app
    routes' string errors); `status` the HTTP status the API answered (null when no response came back: timeout,
    network); `requestId` the ID the text's `Request ID:` line shows (the server's `X-Request-Id`, else the CLI's
    `cli-<uuid>`). An error the CLI raises itself (no key, a refused `--yes`, a bad flag value) has the message only.
    Exit codes are unchanged: 1, and 2 for a clap usage error, which is JSON too when the words include `--json`
    (clap's first paragraph, without `error: `, then each of clap's tips on a line of its own, such as
    `tip: a similar subcommand exists: 'list'`; Yalcin, 2026-10-06); help and a group run without its command print
    as before. The API's `details` are not in it. `output::error_json` owns the shape; `--debug`, warnings and the
    update notice may come before it on stderr. `destinations refresh-source --json` keeps the response on stdout
    when it fails and adds the error.
  - `vendo commands` lists every command the help shows but clap's `help`, which only the root help lists (VE-3893;
    ❓ open, see Help layout), with its description (in the root help's order);
    `--json` prints the tree, read at runtime from the clap tree (`commands/tree.rs`): per command `name`, `path`,
    `description`, visible `aliases`, `arguments` and `options` (name, short, valueName, description, required,
    default, `possibleValues` that parsing enforces, `suggestedValues` that TAB offers, `global`), `commands`.
    Hidden paths stay out; `version` is in. `the_command_tree_matches_the_help_screens` checks it against every
    help screen.
  - `--json` on the commands that lacked it, built from what their text shows and never printing the API key (the
    shapes are the build's choice, for Yalcin's review with CLI 1.1): `profile list`
    (`{"profiles":[{name, active, accountId, baseUrl}]}`), `profile switch` (`{"profile":…}`, null when nothing was
    switched; never opens the profile list), `profile set` (`{"profile","configPath"}`), `logout` (`{"removed":[names]}`;
    not logged in, the JSON error on stderr, nothing on stdout and exit 0 like the text, Yalcin 2026-10-06; an
    unknown `VENDO_PROFILE` is its error, exit 1, see below),
    `login`/`init` (`{"profile","baseUrl","accountId","auth":"verified"|"unverified"|"incomplete","accountName"}`;
    what it says on the way, the sign-in URL among it, goes to stderr), `jobs tail` (nothing while polling, then the
    last `GET /jobs/<id>` response as `jobs get --json` prints it; a failed job, or a wait that times out, also
    prints the JSON error, still exit 0),
    `jobs watch` (NDJSON: each poll's `GET /jobs` response on one line when it changed, a failed poll as the JSON
    error on stderr), `completions <shell>` (`{"shell","script"}`; bare, `{"shell","installed"}`, null for a shell
    it does not know) and `self-update` (the installer's output on stderr, then
    `{"previousVersion","version","installPath","binaryPath"}`; a failed installer is the JSON error with its code).
    `rust/src/watch.rs`'s `JsonScreen` owns the two job shapes.
  - Short IDs: wherever a command takes the full ID of an app, source, destination, job, metric, model or
    methodology (arguments and flags such as `--app`, `--source`), it takes the 8 characters tables show, with or
    without the `...` tables print after them (`1a2b3c4d...`; Yalcin, 2026-10-06). Exactly 8 hex digits, alone or
    followed by `...`, are looked up in that resource's list just before the request they go into (`short_ids.rs`:
    pages of 100, at most 5, an ID read on two pages counted once; for metrics, the `status=archived` list too when
    the default list, which leaves archived metrics out, has no match). One match is used. Several stop the command
    with exit 1 before its request (Yalcin, 2026-10-06): the error names the short ID as typed, how many match, each
    match's full ID with what its table names it by (apps and sources: name and type; destinations: the two apps
    and data type; jobs: type, platform and status; models, metrics, methodologies: name), at most 10 and then how
    many more, and says to use the full ID; with `--json` it is the JSON error (message only). None, or a list that
    fails, sends the argument as typed, so the API answers as before. Never on a full ID, a dry run that sends
    nothing, or before a delete/cancel's consent.
  - `VENDO_PROFILE` selects the profile like `--profile`: `--profile` > `VENDO_PROFILE` > `activeProfile`; empty is
    unset. `--profile`'s help says "(or set VENDO_PROFILE)". It never changes the saved `activeProfile` (Yalcin,
    2026-10-06; `ConfigStore::vendo_profile`): `profile set` writes to its profile, `logout` removes its profile and
    `login` saves its profile, each leaving `activeProfile` as saved; login then says the profile was not made
    active and that VENDO_PROFILE overrides the active profile in this shell, on stderr with `--json` (a profile
    that already is the saved active one: that VENDO_PROFILE overrides it, or nothing when VENDO_PROFILE names it
    too). `profile switch` is an explicit request: it changes `activeProfile`, then notes that VENDO_PROFILE still
    overrides it in this shell (not with `--json`, whose `active` says so). A name no profile has fails with an
    error that names the profile and VENDO_PROFILE and says to run `vendo profile list` or unset VENDO_PROFILE
    (`config::unknown_vendo_profile`), exit 1, where the CLI would otherwise say "No API key configured"
    (`config::require_api_key`) and in `logout`, which would say "Not currently logged in."; `mcp`, which prints its
    config either way, gives it in place of its no-key hint. A key in `VENDO_API_KEY` is still used, as for an
    unknown `--profile`. Hints about switching profiles or checking with `vendo workspace` (its profile list, its
    missing-profile check and its API-auth fixes, a new login key that cannot be checked) say that VENDO_PROFILE
    overrides the active profile (`config::vendo_profile_overrides`). The workspace screen says it once (VE-3891
    review): under its profile list when it lists the profiles, and then its fixes leave it out
    (`DoctorCheck::fix_without_override`; `--json`'s `remediation` keeps it). The fix for a rejected key (401/403)
    says to check the profile login saved with `vendo --profile <profile> workspace`. `vendo workspace` otherwise
    names the profile as it does for `--profile`, and with a VENDO_PROFILE no profile has it shows its screen instead
    of the error: the profile's check, a warning, with that fix, and exit 1 only when a check fails, as the key's and
    the account's do when that leaves no API key or account ID (with `VENDO_API_KEY` and `VENDO_ACCOUNT_ID` it can
    exit 0). `--profile`, which wins over VENDO_PROFILE, works as before.
- Workspace (VE-3891, decided by Yalcin 2026-10-07, CLI 1.1): `vendo workspace` is `whoami` and `doctor` in one
  command and one screen, each fact once (`rust/src/commands/workspace.rs`; the checks stay doctor's,
  `health::local_checks` and `health::auth_check`). First the account: the title is `/me`'s name and slug
  (`T101 · t101`), then `Account ID:`, `Profile:`, `Base URL:` and `API key:` (the key masked as doctor masked it,
  `vend...eKuE`, with `(key ID <apiKeyId>, scopes <scopes>)` once `/me` answers, `full access` for none); whoami's
  `Account:` line is the title's slug and its `API Key:` line, which showed the key ID, is gone. Labels are today's.
  Then whoami's `Env overrides active:` line, the saved profiles when there are two or more (`Profiles`, `*` on the
  active one, the name padded, the account ID's first 8 characters and `…`, the base URL without its scheme, left
  out for the default one) with the VENDO_PROFILE note under them, which the fixes then leave out, and `Checks`:
  each check in a few words after `[ok]`, `[warn]` or `[fail]`, its fix under it (`DoctorCheck::line` and
  `listed`). `CLI <version> at <path>, on
  PATH` joins the binary's and PATH's checks with the worse status and both fixes; `Config <path>`, `Zsh completions
  installed`, `Signed in as <name>` are the agreed words, and a check with a problem keeps doctor's words
  (`API key: Missing`, `API auth: HTTP 401: Unauthorized`). Paths under HOME show as `~`. The profile, key, base URL
  and account checks show only when they do not pass, as the lines above show their values, and doctor's title,
  `Summary:` line and `Suggested next steps` list (the fixes again) are gone. `/me` is asked only with a key and an
  account, as doctor asked it; signed out, with no key, offline or refused (401/403) it shows what the config has (no
  title, no key ID; a value the CLI lacks has no line, its check says so) and the checks with their fixes, and like
  doctor it exits 1 when a check fails, so `whoami` now does too (a binary off PATH fails a check). It prints
  whoami's update notice. `--json` keeps every key both had, whoami's first: `/me`'s response as sent (`data`),
  `config` (`selectedProfile`, `apiKeySource`, `baseUrl`, `baseUrlSource`, `accountId`, `accountIdSource`), then
  doctor's `summary`, `checks` (`name`, `status`, `detail`, `remediation`, doctor's words), `suggestions`,
  `identity` and `shell`; `data` and `identity` only when `/me` answered, and no JSON error on stderr when it did not
  (doctor's way; whoami printed one). `whoami` and `doctor` are its hidden clap aliases, `profile current` and
  `config show` reach it through `MOVED`, and all print exactly what it prints, text, `--json`, `--help` and usage
  errors (`whoami_doctor_and_their_old_paths_print_exactly_what_workspace_prints`). Hints that named `vendo whoami`
  or `vendo doctor` (login's and status's next steps, profile switch's "Verify with", login's errors, the API-auth
  fixes) name `vendo workspace`. ❓ Open for Yalcin, built with cautious defaults: the description ("Show the current
  account, your profiles and setup checks"), the title's second part read as the slug, the account ID and key ID
  shown whole (the preview elided them), the profile list's `…` IDs and hosts read from the preview, the profile list
  only from two profiles (whoami's rule) and without whoami's "Switch with `vendo profile switch` …" hint, no title
  without an answer from `/me`, a value the CLI lacks left out rather than shown as missing, the completions line
  without the file it loads from, doctor's words for a check with a problem, no `profiles` key in the JSON, and the
  VENDO_PROFILE note said under the profile list rather than in the fixes (VE-3891 review), whose fix for a missing
  profile still says to run `vendo profile list`.
  Renaming "Account" to "Workspace" in the labels is not decided.
- Group menus (VE-3826, decided by Yalcin 2026-10-05, CLI 1.1): a group run without its command (`vendo apps`,
  `vendo measurement ltv`, the hidden `config`; bare `vendo` is unchanged) opens an arrow-key menu of its visible
  commands and their descriptions where `output::can_show_menu` holds: `can_prompt` (stdin and stdout terminals and
  prompts not off, the rule `confirm` asks by), stderr a terminal too, as inquire draws the menu there (`vendo apps
  2>err.log` keeps the usage error), and `TERM` not `dumb` (Yalcin, 2026-10-06). ↑↓ move, typing filters by name
  and description, and Enter puts the chosen name where it would have been typed and parses again
  (`cli::chosen_command`), so global options, `MOVED` and confirmations apply as typed, and a chosen command missing a
  value asks for it as when typed (VE-3881, below); a chosen group opens its own menu. Esc, Ctrl-C and Ctrl-D exit 0
  (`output::quit_quietly`) and leave the title and `<canceled>`: inquire leaves the menu standing on Ctrl-C, so
  `output::clear_menu` redraws it from the line `output::run_prompt` saved (the setup the menu and the questions for a
  missing value share). A terminal of the menu's that hangs up while it is open (stdin's or stderr's: its
  window closed, or the program that opened the pseudo-terminal dropped it) ends the CLI as Ctrl-D does, exit 0 with
  nothing run, within a quarter of a second (`output::HangUpWatch`, a thread that polls stdin and stderr for POLLHUP,
  re-polling every 250 ms because macOS does not wake a poll that began with a key waiting): where it is not
  `vendo`'s controlling terminal no SIGHUP comes, and crossterm's read loop, which gets the end of the input (or an
  I/O error) at once from such a terminal, every time, spun at a whole core for hours (2026-10-06). The y/N
  questions read that once and exit 0 as for Ctrl-D; login's ENTER read ends with no browser
  opened, and login waits for the sign-in as with stdin closed
  (`the_questions_and_login_do_not_spin_when_their_terminal_hangs_up`). A menu left in the background of its
  terminal for good ends the CLI the same way, leaving that terminal, the shell's now, as it is
  (`output::reads_fail_in_background`): a program that ran `vendo <group>` from a shell exited while the menu was
  open, the shell took the terminal back, and each key typed at the shell made the read fail (EIO, the process group
  orphaned) and the loop spin. A menu that job control can bring back (`fg`) stays: `tcdrain`, which tells the two
  apart, stops it with SIGTTOU as a read would. A stdin opened write-only (`vendo apps 0>/dev/ttys004`), which no key
  can be read from, keeps the usage error (`output::stdin_reads`); the y/N questions read it once and
  exit 0 as for Ctrl-D. The menu takes the screen's height less one line at most, and on a short screen its list
  scrolls (`output::menu_page`). Its rows, its hint and the answered line leave the screen's last column free
  (`output::fitted`, VE-3881 review): inquire draws a line that changed again and then erases to the line's end, and
  where the terminal follows xterm (xterm, Ghostty, `screen` in the tests) that erase took off a character drawn in
  the last column, from the first key on. Without a terminal the usage error stays: the group's help on stderr, exit 2.
  The menu is inquire with its crossterm backend and no fuzzy matching (`output::choose_command`); crossterm,
  inquire's version, is a direct dependency for the screen size and the Ctrl-C redraw. Both are the default-on
  `menu` cargo feature: built with `--no-default-features` the CLI has no menu (`cli::menu_choice`), so a bare group
  is the usage error at a terminal too. Yalcin accepted the menu's size (+132,496 bytes, +2.09%, on the macOS arm64
  release binary when it came in), and going over 150 KB is fine (2026-10-06): `scripts/menu-size.sh` builds the
  release binary without and with the feature, reports the difference, and only warns at 150,000 bytes or more
  (linux-x64 was 152,488 at 1.1.0-rc.1). On macOS arm64 the code segment grows in 16 KB
  pages, so there the difference moves in steps of 16,384 bytes. Tests drive the menu on a
  pseudo-terminal that is `vendo`'s controlling terminal (`OnTerminal` in `rust/tests/cli.rs`; `screen` there replays
  what the terminal shows). `OnTerminal` keeps its own copy of the terminal end until `vendo` exits: macOS drops what
  the reader has not read yet when the last copy closes. `OnTerminal::start_detached` runs `vendo` on a terminal that
  is not its controlling terminal, and `hang_up` closes every copy of the controller and the test's terminal end;
  `exit_within` then reaps `vendo` with the processor time it used (`wait4`). `OnTerminal::launch_on` takes a
  pseudo-terminal the test made, to open its terminal end by name first. The orphaned-menu test drives job control
  with `/bin/sh -c 'set -m; …'` (bash on macOS, dash on Ubuntu) and kills what it leaves with `KillGroup`.
- Prompts off (VE-3826, decided by Yalcin 2026-10-06, CLI 1.1): `CI` or `VENDO_NO_INPUT` set to anything but empty,
  `0` or `false` (any case) turns every prompt off, also at a terminal; there is no `--no-input` flag.
  `output::prompts_off` owns the rule, and every prompt asks by it: the y/N questions refuse without `--yes`, a bare
  group, a command missing a value and `--profile` with no name are the usage error (exit 2), and the profile list
  does not open (`profile switch` prints "Cancelled."), each as without a terminal, through `can_prompt`; at a terminal
  login does not read its "Press ENTER to open in the browser" (`commands/login.rs`), so it does what it does without
  one, stdin closed on a CI runner: it prints the sign-in URL and that line, opens no browser and waits for the
  sign-in. A stdin that is not a terminal (a pipe, a file) is no prompt: login reads it as before, so
  `echo | CI=true vendo login` opens the browser as `echo | vendo login` does. The profile list opens where the group
  menu does (`can_show_menu`; VE-3892, below), so `echo 1 | vendo profile switch` reads no answer from the pipe, and so
  do the selectable lists (VE-3894, below): with prompts off a list command prints its table. On `TERM=dumb` the group
  menu, the questions for a missing value, the profile list and the selectable lists are off; the y/N questions still
  ask.
- Missing values (VE-3881, decided by Yalcin 2026-10-07, CLI 1.1): a command typed without a value it requires asks for
  it where the group menu opens (`output::can_show_menu`, the same rule, the hang-up watch included) instead of stopping
  with clap's usage error; optional values are not asked. `rust/src/ask.rs` (the `menu` feature) owns it: `VALUES` says
  how each value is asked for, and a value not there keeps the usage error. A value with choices opens an arrow-key list
  with type-to-filter (`output::choose_value`: the menu's inquire Select and hint, plain-text rows padded per column in
  the screen's columns, a wide character such as 東 taking two as inquire and the terminal count it (`unicode-width`;
  `output::menu_page` fits the list to the screen by them too), filtered by substring in any case): an app, source,
  destination, job, model or metric as `apps list`, `sources list`, `destinations list`, `jobs list`, `models list` and
  `metrics list` list them (newest first, pages of 100, at most 5 like a short-ID lookup; with more, the hint ends
  `· newest 500 shown`; metrics without the archived ones, as `metrics list` leaves them out; rows in the table's
  columns as plain text, a job's platform and start time `—` when it has none), a platform ready to connect as
  `catalog list` lists them by default (VE-3829) for `apps create --type`, `catalog get` and the hidden
  `catalog credential-schema`, a methodology as `measurement methodologies list` lists them (the system's and the
  account's, one response; short ID, name, `system` or `account`, click-path model), a cohort period for
  `measurement ltv cohort` from the cohorts `measurement ltv list` lists for the typed `--granularity` and `--segment`
  or their defaults (newest first, as many as the route sends, 500 at most, without predictions; period, segment, size;
  with 500 the hint ends `· newest 500 shown`, the route sending no total), for `dictionary get` first a subject type
  (`dictionary::SUBJECT_TYPES`, event first; titled `vendo dictionary get · subject type`), then that type's entries as
  `dictionary list --type` lists them (the route's order, pages of 100, at most 5, with more the hint ends
  `· first 500 shown`; name, the table's dash for none, and subject ID), `sources create --sync-type` the one type the
  API takes, the type of the app chosen for `--app` or typed (read as `apps get` reads it, a short ID looked up first;
  an app that cannot be read is that error, exit 1), in a list of that one row, and `destinations create --data-type`
  the 13 data types vendo-web-v2's `DataTypeSchema` (`lib/vendo/data-model.ts`) takes, in its order, the deprecated
  legacy ones too (`ask::DATA_TYPES`). Free text (`apps create --name`, `metrics create --name`,
  `measurement ltv customer`, `dictionary search`, an answer starting with `-` going in after `--` and `help` staying
  the query), a date (`measurement rules preview --from` and `--to`, not checked: the API decides) and a file's path
  (`destinations create --config-file`, `metrics create --definition`, taken as typed, relative to the current
  directory, `~` not expanded) are a one-line question (`output::ask_text`): the answer is taken without the spaces and
  line ends around it, as a shell takes a word (a pasted line brought its line end to the API), and one that is empty
  without them is refused. Keys typed before a list or question opens (while the list loads, or with the Enter that
  answered the menu or the list before) are thrown away (`output::discard_typed_ahead`), so a stray Enter chooses
  nothing unseen. Each is titled with the command as the tree names it (`vendo apps get`, `vendo apps create --type`,
  `vendo destinations get` for `vendo int get`), and the answered line reads like the command so far,
  `vendo apps get a1b2c3d4... (Menu Shop)`,
  `vendo destinations get 9c0d1e2f... (Analytics BQ → Demo Pixel)`, a job, which has no name, by its short ID alone
  (`vendo jobs cancel 1f2e3d4c...`), a cohort by its period, a dictionary entry by its whole subject ID and name (the
  dictionary's IDs are no short IDs), after inquire's mark for an answer: a green `>`, or `?` with `NO_COLOR` (as the
  tests run); the title of an open list or question and the `<canceled>` line start with `?` either way. The values go
  into the words where they would have been typed (an option as `--type=shopify`, an ID whole) and `cli::parse` parses
  again, once, so global options, `MOVED`, the y/N of a delete or cancel, `--dry-run`, `--json` and `--output` apply as
  typed, and an `update` given no change (Q13's default) fails after the choice as when typed with the ID:
  `Nothing to update — pass at least one flag.` for apps, sources and destinations, `No updates provided` for metrics,
  exit 1. Before anything is asked: no API key, or an unknown `VENDO_PROFILE`, is that error, and without an account a
  command whose requests go to the account (apps, sources, destinations, jobs, models, the dictionary;
  `ask::needs_account`) is the client's "No account configured" error (`Client::require_account`), whichever of its
  values is missing (`destinations create --dest-app <id>` too, before its data type and path; `dictionary search`
  before its question), exit 1 with nothing sent; the platforms, the metrics and measurement are the key's (the catalog
  route, the web app's `/api/metrics` and `/api/measurement/*`), so `apps create`, `catalog get`, the metrics and the
  measurement commands ask without an account. A list that fails is its error (exit 1, the JSON error with `--json`; a
  `--granularity` the LTV route does not know is its 400); an empty one says `No apps to choose from.` (`sources`,
  `destinations`, `jobs`, `models`, `metrics`, `platforms`, `methodologies`, `cohorts`, `event entries` and so on for
  the type chosen) and is the usage error (exit 2). Esc, Ctrl-C, Ctrl-D and a hang-up exit 0 with nothing run. ❓ Open
  for Yalcin, built with the spec's cautious defaults: lists not narrowed beyond what the list command shows (Q2:
  `sources create`'s apps not narrowed to active source apps, nor by a typed `--sync-type`), the sync type as a one-row
  list to confirm (Q3), the empty-list line (Q4), the cut-off notes' wording (`newest 500 shown`, `first 500 shown`;
  Q6), all 13 data types (Q7), a path taken as typed (Q8), `apps create`'s platforms not narrowed by `--role` (Q9),
  `catalog get` listing only the ready platforms and the hidden `credential-schema` asking as it does (Q10),
  `dictionary get`'s subject-type step and its title (Q11), the cohort periods as a list and the dates as unchecked
  questions (Q12), an `update` with no change asking first (Q13), and `--dry-run` sending the list request (Q14); not
  narrowed either: `jobs cancel` to running and queued jobs, `metrics activate` to drafts (Q2). Without a terminal, with
  prompts off, on `TERM=dumb`, with stderr redirected or a write-only stdin, the usage error stays byte for byte and
  nothing is sent (`rust/tests/snapshots/usage/`). Every required value of every command is asked for (43 values of 37
  commands, the hidden `catalog credential-schema` and the `integrations`/`int` aliases too); `ask.rs`'s coverage test
  fails when a required value has no row in `VALUES`. An agent that runs `vendo` on a pseudo-terminal without `CI` or
  `VENDO_NO_INPUT` now waits at the question where it got exit 2, as at the group menu and the y/N questions;
  `VENDO_NO_INPUT=1` turns it off. Since VE-3894 it also waits at `vendo apps list`, `sources list`, `destinations list`,
  `jobs list`, `catalog list`, `dictionary list`, `metrics list` and `models list`, where it got the table and exit 0,
  a worse change than this one's (a run that succeeded now waits);
  `VENDO_NO_INPUT=1`, `--json` or `--output` give the table back.
- Profile list (VE-3892, decided by Yalcin 2026-10-07, CLI 1.1): `--profile` typed with no name opens an arrow-key list
  of the saved profiles with type-to-filter where the group menu opens (`output::can_show_menu`, the same rule, the
  hang-up watch included): `ask::choose_profile`, a `choose_value` list (VE-3881) titled with the command as the tree
  names it (`vendo --profile`, `vendo apps list --profile`, `vendo destinations list --profile` for `vendo int list
  --profile`), each row `*` on the profile commands use without the flag (VENDO_PROFILE's, else the active one, as
  `profile list` marks it), the name, the account ID (`no account` for none) and the base URL's host
  (`profile_display::shown_host`, which the workspace's profile list uses too; none for the default one), answered as
  the name (`? vendo --profile beta`). On its own (global options only: `vendo --profile`, `vendo --debug --profile`)
  the words become `vendo profile switch <name>` (`cli::with_profile`), so the chosen profile becomes the active one
  and it prints exactly what that prints, VENDO_PROFILE's note included. At the end of a command (`vendo apps list
  --profile`) `--profile=<name>` takes its place and the words are parsed again (`cli::chosen_profile`): the command
  runs as if `--profile <name>` had been typed, for that command alone (the saved active profile stays, the flag wins
  over VENDO_PROFILE, and a group then opens its menu or a missing value is asked for, as typed). Only the last word
  can be a `--profile` with no name: clap takes the word after it as its name, `--` and words starting with `-` too,
  so `vendo --profile apps list` still takes `apps` as the name (known limit, unchanged). `vendo profile switch` with
  no name opens the same list, titled `vendo profile switch`, in place of its numbered picker (decision sheet item
  71), which is gone (`output::search_select_option`); with `--json` it still opens nothing. With no profile saved,
  `--profile` with no name says `No profiles to choose from.` and is the usage error (exit 2); `profile switch` says
  what it said (`No profiles yet. …`). Esc, Ctrl-C, Ctrl-D and a hang-up exit 0 with nothing run or switched. Without a
  terminal, with prompts off, on `TERM=dumb`, with stdout or stderr elsewhere or a write-only stdin, `--profile` with no
  name stays clap's usage error byte for byte (`usage/profile_flag`, recorded before the change) with nothing read, so a
  legacy flat config is not migrated and saved (`ask::profile` checks `can_show_menu` first; VE-3892 review), and
  `profile switch` prints `Cancelled.`; built without the `menu` feature there is no list either. ❓ Open for Yalcin,
  built with cautious defaults: the account ID whole, as `profile list` shows it (the workspace's list shows 8
  characters and `…`), no host for the default base URL (as `profile list` and the workspace leave it out), the marker
  on VENDO_PROFILE's profile when it is set (as `profile list` marks it), the cursor starting on the first row rather
  than the active one, the empty-list line and exit 2 (VE-3881's Q4), `profile switch` printing `Cancelled.` on
  `TERM=dumb` and with stderr redirected, where the numbered picker asked, and with a write-only stdin, where the picker
  showed its title and search line and then ended quietly (exit 0, nothing switched; VE-3892 review), and `--profile`'s
  help unchanged (it does not say that a bare `--profile` opens the list).
- Selectable lists (VE-3894, decided by Yalcin 2026-10-07, CLI 1.1: "items first then actions, but still keep the
  actions so we can go directly to the action without the list too. The change should only apply to list: make the
  list selectable"): where the group menu opens (`output::can_show_menu`, the same rule, the hang-up watch included), a
  list command that would print its table (no `--json`, no `--output`, an empty one included) shows the same rows, from
  the same requests and flags (short IDs looked up as typed, and for sources and destinations the active-jobs request
  that feeds Progress), as a `choose_value` list (VE-3881's) instead. `rust/src/browse.rs` owns it: the list
  command builds the table's cells once into a `browse::Table`, which prints the table, footer and all, where the list
  does not open (`Table::print`, the code that printed it before), and `browse::shown` decides (`browse::opens`; a stub
  that is false without the `menu` feature). Each row is the table's own cells as plain text (`output::strip_ansi`) on
  one line (line ends and other control characters a space, `browse::one_line`, the answered line too), padded per
  column (`ask::padded`), with no header row; the list is titled with the command as the tree names it
  (`vendo apps list`; `vendo destinations list` for `vendo int list` and `vendo integrations list`), the table's footer
  follows the hint (`· 57 apps`), and an item is answered as VE-3881 answers it (`? vendo apps list a1b2c3d4... (Menu
  Shop)`, `? vendo destinations list 9c0d1e2f... (Analytics BQ → Demo Pixel)`, a job by its short ID alone). Enter
  shows exactly what the group's `get` shows, from its code, spinner and requests (`apps::show`, `sources::show`,
  `integrations::show`, `jobs::show`, `catalog::show`, `dictionary::show`, `metrics::show`, `models::show`, which
  `get` calls after its short-ID lookup; a source or destination with its active job, as `get` reads it), then the
  item's action menu: titled with the group (`vendo apps`), the actions that
  apply to the item as that request returned it, each with its description from the tree, then `back   Back to the
  list`, the cursor on the first and the keys typed while the item loaded thrown away. Apps: `pause` when active,
  `resume` when inactive, neither for another state, then `update` and `delete`. Sources and destinations: first
  `sync` when active (vendo-web-v2 `sources/sync.ts` and `integrations/sync.ts` refuse it otherwise), for a destination
  then `refresh-source` when `get` returned a `sourceAppId` (`lib/server/source-refresh.ts` refuses it without one,
  `no_source_app`; it runs with its default window, the last 7 days), then pause or resume, `update` and `delete` as
  for apps. Jobs: `tail` and `cancel` while queued, pending or running (`jobs/cancel.ts` cancels only those), nothing
  else, so a finished job shows its details and `back` only. Metrics: `activate` for a draft (the help's 'Activate a
  draft metric'), then `update` and `delete` (its y/N, then `Cancelled` on n, as typed). Platforms, dictionary entries
  and models: nothing, so `back` only (never the hidden `catalog credential-schema`) (`browse::Group::actions`; only
  visible commands of the
  group that take the item's ID, never another group's or one such as `jobs tail --source`, which a unit test checks).
  A chosen action is answered as the command (`? vendo apps pause a1b2c3d4... (Menu Shop)`) and runs exactly as typed:
  `browse` keeps its words (`apps pause <full ID>`, the group as the tree names it, so `destinations pause <full ID>`
  from `vendo int list`; `browse::chosen`) and `main` parses them again after `--profile=<name>` and `--debug` as given
  (`action_args`; VENDO_PROFILE and VENDO_DEBUG carry over in the environment), so a delete's or cancel's y/N
  (VE-3823) asks, the full ID needs no short-ID lookup, `update` with no flags fails as typed (`Nothing to update —
  pass at least one flag.`, exit 1), a source's `sync` while its job runs says `Sync already in progress`, `tail`
  follows the job (clearing the screen as typed), and the CLI ends with the action's exit code. Back opens the list
  again with no request, the cursor on the item just viewed and the filter cleared; nothing above is
  erased (the answered lines and the details stay, the list opens below them). Esc, Ctrl-C, Ctrl-D and a hang-up on
  the list or the action menu exit 0 with nothing run. A details request that fails (an item deleted since the list
  loaded, a 429, the network) is `get`'s error, exit 1. An empty response prints the table and its count, exit 0, as
  does a first list the terminal refuses; a later list or action menu that cannot run exits 0 quietly. Without a
  terminal, with prompts off, on `TERM=dumb`, with stderr redirected, stdout piped or a write-only stdin, with
  `--json` or `--output`, and built without the `menu` feature, the list command prints exactly what it printed (the
  `output/` snapshots are unchanged; `where_the_list_cannot_open_and_with_json_or_output_apps_list_prints_what_it_printed`,
  `…_sources_destinations_and_jobs_list_print_what_they_printed`,
  `…_catalog_dictionary_metrics_and_models_list_print_what_they_printed`). `get`, the bare group's menu and every
  action command work as before. So far `vendo apps list`, `sources list`,
  `destinations list` (`integrations list`, `int list`), `jobs list`, `catalog list` (titled `vendo catalog list`,
  a platform answered by its app type, the footer `2 ready · 1 more on request (vendo catalog list --all)` with the
  typed filters, or `N platforms` with `--all`), `dictionary list` (an entry answered by its full subject ID and
  display name, `· 57 events`; a description's line break a space, so a row stays one line; `dictionary search`, which
  prints the same table, is unchanged), `metrics list` and `models list` (VE-3894's first three groups); the
  measurement and profile lists follow. ❓ Open for Yalcin, built
  with cautious defaults: `get` kept everywhere (Yalcin's "we don't really need get" in the runs that built this,
  against the decision's "keep the actions";
  options: hide it from the group menu only, hide it everywhere as a hidden path, or remove it, which breaks scripts,
  agents and VE-3881's questions), the table's own cells in a row (all columns, wider rows that wrap on 80 columns,
  no header; VE-3881's narrower cells would drop Status and Last Sync), the CLI ending after an action rather than
  returning to the list, `update` offered though it fails without flags, the action menu's title, rows, answers and
  order with the cursor on the first action (a double Enter after it opens runs pause, resume, sync, refresh-source or
  tail, which ask no y/N; option: start on `back`), the footer as today's words in the hint (`· 57 apps` while 20 rows
  show; option `20 of 57 apps`), an empty list printing the table and count (option: VE-3881's `No apps to choose
  from.`, exit 2), Back's cursor on the item with the filter cleared and nothing erased, `delete` offered though the
  API refuses an app still in use (409, VE-3756) and for a source or destination in any state, neither pause nor
  resume (nor sync) for a state other than active and inactive, `tail` offered only for a queued, pending or running
  job (a finished one's would repeat what `get` showed), `refresh-source` run with its default window rather than
  asking for `--from` and `--to`, no actions of another group (`jobs tail --source`, `jobs list --source` from a
  source), no 'Create an app with this platform' on a catalog platform (it is `apps create`, another group's command:
  without `--credentials-file` it starts the browser sign-in, which cannot work for a credential platform such as
  BigQuery, its `--role` defaults to source, wrong for a destination-only platform, and the API refuses a platform on
  request; proposed, not built), `activate` offered only for a draft metric (not an archived one), `update` offered
  for a metric though it fails without flags (`No updates provided`, exit 1), `dictionary search` left as a table,
  a failed details request ending with `get`'s error rather than returning to the list, agents on a
  pseudo-terminal now waiting at the list (see Missing values), `--output ""` keeping the table, and the list
  commands' `--help` unchanged.
- Ported so far: login, init, logout, whoami, config, profile, status, doctor, mcp, completions,
  self-update (VE-3665; whoami and doctor are `workspace` since VE-3891); jobs list/get/cancel/watch/tail and the shared watcher (VE-3666); apps, sources,
  integrations (`int`) and catalog (VE-3667); metrics, models and measurement (VE-3668); dictionary (VE-3713).
  `rust/src/web_app.rs` is the one place that knows the web-app routes (`/api/metrics`, `/api/measurement/*`),
  which go out as raw paths with no account prefix. `rust/src/dictionary.rs` pins the dictionary field and
  param names to vendo-web-v2's `route-handlers/dictionary/serialize.ts`, like `dictionary-contract.test.ts`.
- Versions: `rust/Cargo.toml` carries the Rust CLI's release version, and release-candidate tags must match
  it. `package.json` stays the TypeScript CLI's version until the switch-over (VE-3669).

## Layout (`src/`)
`cli.ts`, `client.ts`, `config.ts`, `identity.ts`, plus `commands/` (21 modules on 2026-10-05 — `ls src/commands` to recount: apps, sources,
integrations, jobs, metrics, models, catalog, dictionary, measurement, pipeline-resource, mcp, login,
logout, init, doctor, status, whoami, profile, config, completions, self-update). Tests in `src/__tests__/`.

## Key commands
- `pnpm run typecheck` — `tsc --noEmit`
- `pnpm run test` — `vitest run`
- `pnpm run lint` / `pnpm run lint:fix`
- `pnpm run build` — `tsup` (dev bundle); `pnpm run build:standalone` — SEA binary
- `pnpm parity --profile <staging profile> [--rust <binary>] [--only <prefix>]` — runs every read-only command
  against staging with the TypeScript CLI (and the Rust CLI when given) and reports differences and
  commands that fail today. Needs `pnpm build`. Refuses non-staging URLs. When you add or rename a
  command, classify it in `parity/commands.json` or the run fails. Tests: `pnpm test:parity` (VE-3664).
- `pnpm parity:help [--rust <binary>] [--only <prefix>]` — compares `--help` of every command (root, groups and
  leaves) between the two CLIs: description, arguments, options with defaults, subcommands with aliases, and
  examples. Offline, no profile; needs `pnpm build` and a Rust binary. clap's layout is accepted (VE-3713).
- `pnpm parity:writes --profile <staging profile> --rust <binary> [--only pipeline|metrics]` — runs the write
  commands with both CLIs on throwaway resources, compares output, and deletes everything it created, also on
  failure: apps, sources and integration refusals on webhook apps (`pipeline`, VE-3667), and draft metrics
  named "CLI parity …" (`metrics`, VE-3668). Point it at a disposable staging workspace ("Vendo CLI test"),
  not one people use.
- Distribution: `install.sh` pulls per-platform binaries from GitHub Releases
  (`vendo-analytics/vendo-cli`); releases are tag-triggered, see below.

## CI and releases (VE-3731)
- CI (`.github/workflows/ci.yml`) runs on every pull request and push to `main`, one job per area:
  - TypeScript: `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm lint`, `pnpm test`, `pnpm test:parity`.
  - Rust, from `rust/`: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
    `cargo test --locked`.
  - `cargo deny check` with `rust/deny.toml` (advisories in one job; licences, bans and sources in another).
    Locally: `cargo install cargo-deny --locked`, then `cargo deny check` from `rust/`. Allow a new licence or
    ignore an advisory only with a reason in `deny.toml`.
- `cli-vX.Y.Z` tags run `release.yml`: the TypeScript binaries, published as a normal release that becomes
  "latest", which is what `install.sh`, `vendo self-update` and the update notice install. Stays until VE-3669.
- `cli-vX.Y.Z-rc.N` tags run `release-rc.yml`: the Rust binaries for linux-x64, linux-arm64, darwin-arm64 and
  darwin-x64 (cross-compiled on Apple silicon), same asset names and `.sha256` files, published as a GitHub
  pre-release that never becomes "latest". The workflow checks the tag matches `rust/Cargo.toml`, runs
  `cargo test`, then for each target checks the menu's size (`scripts/menu-size.sh`, which writes the sizes with and
  without the menu to the run's summary and warns, never fails, when the menu adds 150,000 bytes or more),
  builds, smoke-tests and uploads the binary.
- Cutting a release candidate: bump `version` in `rust/Cargo.toml` to `X.Y.Z-rc.N` and update `rust/Cargo.lock`
  (`cargo update --workspace` from `rust/`) in one commit, merged through a PR like any change. Then tag that
  commit `cli-vX.Y.Z-rc.N` and push the tag. Agents never push tags or create releases: Yalcin approves each one.
- Before tagging, run `pnpm build && pnpm parity:help --rust rust/target/release/vendo` on that commit. Every help
  screen must be the same, except the two accepted for VE-3823 (`logout`, `metrics delete`; Yalcin, 2026-10-06),
  `--profile`'s description on the root screen, which names VENDO_PROFILE (VE-3831, Yalcin 2026-10-06),
  `jobs list`'s `--status` and `--type`, which list the API's values (VE-3830, Yalcin 2026-10-06), and `config set`'s
  description, `profile set`'s "Set values on the active profile" (VE-3827, Yalcin 2026-10-06), and `workspace` in the
  root help where `whoami` and `doctor` were, whose screens are now `workspace`'s (VE-3891, Yalcin 2026-10-07).
  This is a manual step, not a CI job (decided by Yalcin, 2026-10-05): the TypeScript CLI
  it compares against is deleted at 1.0.0 (VE-3669).
- Installing a release candidate: `VENDO_VERSION=cli-vX.Y.Z-rc.N bash install.sh` from a checkout,
  `curl -fsSL https://app2.vendodata.com/install.sh | VENDO_VERSION=cli-vX.Y.Z-rc.N bash` from anywhere, or
  `vendo self-update --version cli-vX.Y.Z-rc.N`.

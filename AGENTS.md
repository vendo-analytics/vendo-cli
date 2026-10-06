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
  built its own JSON (whoami's `config`, the `metrics` `{ data }` envelope), Rust keeps that shape (Yalcin,
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
  records every `--help` screen, found by walking the real command tree, in `rust/tests/snapshots/help/`, and the
  table, `--json` and confirmation output of each command against the stub's synthetic account in
  `rust/tests/snapshots/output/`. Any change to them fails `cargo test`. After a deliberate change run
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
  Group screens have no `help` row; `--profile`/`--debug` sit under "Global options". `whoami` is the one identity
  command and `config` moved under `profile`; `profile set` is described as "Set values on the active profile"
  (Yalcin, 2026-10-06). Old paths keep working, hidden, printing exactly what their command prints: clap hidden
  aliases where clap allows (`config` = `profile`, `use` = `profile switch`), `MOVED` in `cli.rs` where it does not
  (`profile current`/`config show` → `whoami`, `config reset` → `logout --all`). Give a new hidden path a
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
- Models (VE-3840, CLI 1.1): `models list`'s Type column and `models get`'s title (`orders_clean (sql)`) and `Type:`
  line, which replaces `Data Type:`, show the API's `modelType` (`sql`, `bqml`, `grouping`, …; Yalcin, 2026-10-06).
  The TS CLI read a `dataType` the API does not send and showed `undefined`, which `pnpm parity` reports.
- Completions (VE-3830, CLI 1.1): `vendo completions <shell>` prints the script the installer saves. Bare, it exits 0
  and says on stderr what it does, whether completions are set up for the shell `$SHELL` names, and how to set them
  up, and stdout stays empty: without `--json`, stdout carries nothing but a script, so an `eval` or redirect that
  leaves the shell out gets nothing. With `--json` (VE-3831) stdout carries JSON instead and stderr stays quiet: bare,
  `{"shell","installed"}`; with a shell, the script wrapped in `{"shell","script"}`.
  `rust/src/commands/completions.rs` owns that detection (the installer's saved script and startup-file block, or a
  line in a startup file that runs `vendo completions <shell>`), and doctor's check uses it. Bash's startup file is
  `~/.bashrc` and, on macOS, whose Terminal opens login shells, also its login file: the first of `~/.bash_profile`,
  `~/.bash_login` and `~/.profile` that exists (Yalcin, 2026-10-06). There install.sh (`uname -s` Darwin) adds its
  block to that file too, creating `~/.bash_profile` when none exists (never beside `~/.profile`, which bash would
  stop reading), and its summary names each file (`bash, loaded from ~/.bashrc and ~/.bash_profile`); bare
  `vendo completions` and doctor look in `~/.bashrc` and that one login file on macOS (`bash_login_file`), not in
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
  - `vendo commands` lists every command the help shows with its description (in the root help's order);
    `--json` prints the tree, read at runtime from the clap tree (`commands/tree.rs`): per command `name`, `path`,
    `description`, visible `aliases`, `arguments` and `options` (name, short, valueName, description, required,
    default, `possibleValues` that parsing enforces, `suggestedValues` that TAB offers, `global`), `commands`.
    Hidden paths stay out. `the_command_tree_matches_the_help_screens` checks it against every help screen.
  - `--json` on the commands that lacked it, built from what their text shows and never printing the API key (the
    shapes are the build's choice, for Yalcin's review with CLI 1.1): `profile list`
    (`{"profiles":[{name, active, accountId, baseUrl}]}`), `profile switch` (`{"profile":…}`, null when nothing was
    switched; never opens the picker), `profile set` (`{"profile","configPath"}`), `logout` (`{"removed":[names]}`;
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
    unknown `--profile`. Hints about switching profiles or checking with whoami (whoami's profile list, doctor's
    missing profile and its API-auth fixes, a new login key that cannot be checked) say that VENDO_PROFILE
    overrides the active profile (`config::vendo_profile_overrides`); doctor's fix for a rejected key (401/403)
    says to check the profile login saved with `vendo --profile <profile> whoami`. whoami and doctor otherwise name
    the profile as they do for `--profile`. `--profile`, which wins over VENDO_PROFILE, works as before.
- Group menus (VE-3826, decided by Yalcin 2026-10-05, CLI 1.1): a group run without its command (`vendo apps`,
  `vendo measurement ltv`, the hidden `config`; bare `vendo` is unchanged) opens an arrow-key menu of its visible
  commands and their descriptions where `output::can_show_menu` holds: `can_prompt` (stdin and stdout terminals and
  prompts not off, the rule `confirm` asks by), stderr a terminal too, as inquire draws the menu there (`vendo apps
  2>err.log` keeps the usage error), and `TERM` not `dumb` (Yalcin, 2026-10-06). ↑↓ move, typing filters by name
  and description, and Enter puts the chosen name where it would have been typed and parses again
  (`cli::chosen_command`), so global options, `MOVED`, confirmations and usage errors (a missing `<appId>`) apply as
  typed; a chosen group opens its own menu. Esc, Ctrl-C and Ctrl-D exit 0 (`output::quit_quietly`) and leave the
  title and `<canceled>`: inquire leaves the menu standing on Ctrl-C, so `output::clear_menu` redraws it from the
  line `choose_command` saved. The menu takes the screen's height less one line at most, and on a short screen its
  list scrolls (`output::menu_page`). Without a terminal the usage error stays: the group's help on stderr, exit 2.
  The menu is inquire with its crossterm backend and no fuzzy matching (`output::choose_command`); crossterm,
  inquire's version, is a direct dependency for the screen size and the Ctrl-C redraw. Both are the default-on
  `menu` cargo feature: built with `--no-default-features` the CLI has no menu (`cli::menu_choice`), so a bare group
  is the usage error at a terminal too. Yalcin accepted the menu's size (+132,496 bytes, +2.09%, on the macOS arm64
  release binary when it came in) on condition that it stays under 150 KB at each release (2026-10-06):
  `scripts/menu-size.sh` builds the release binary without and with the feature and fails when the menu adds
  150,000 bytes or more (the decision sheet counts KB in thousands). On macOS arm64 the code segment grows in 16 KB
  pages, so there the difference moves in steps of 16,384 bytes. Tests drive the menu on a
  pseudo-terminal that is `vendo`'s controlling terminal (`OnTerminal` in `rust/tests/cli.rs`; `screen` there replays
  what the terminal shows). `OnTerminal` keeps its own copy of the terminal end until `vendo` exits: macOS drops what
  the reader has not read yet when the last copy closes.
- Prompts off (VE-3826, decided by Yalcin 2026-10-06, CLI 1.1): `CI` or `VENDO_NO_INPUT` set to anything but empty,
  `0` or `false` (any case) turns every prompt off, also at a terminal; there is no `--no-input` flag.
  `output::prompts_off` owns the rule, and every prompt asks by it: the y/N questions refuse without `--yes`, a bare
  group is the usage error (exit 2), and the profile picker does not ask (`profile switch` prints "Cancelled."),
  each as without a terminal, through `can_prompt`; at a terminal login does not read its "Press ENTER to open in
  the browser" (`commands/login.rs`), so it does what it does without one, stdin closed on a CI runner: it prints
  the sign-in URL and that line, opens no browser and waits for the sign-in. A stdin that is not a terminal (a pipe,
  a file) is no prompt: login reads it as before, so `echo | CI=true vendo login` opens the browser as
  `echo | vendo login` does. The profile picker (`output::search_select_option`) asks by `can_prompt`, so
  only when stdin and stdout are terminals (`echo 1 | vendo profile switch` reads no answer from the pipe). On
  `TERM=dumb` only the group menu is off; the questions and the picker still ask.
- Ported so far: login, init, logout, whoami, config, profile, status, doctor, mcp, completions,
  self-update (VE-3665); jobs list/get/cancel/watch/tail and the shared watcher (VE-3666); apps, sources,
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
  without the menu to the run's summary and fails the target when the menu adds 150,000 bytes (150 KB) or more),
  builds, smoke-tests and uploads the binary.
- Cutting a release candidate: bump `version` in `rust/Cargo.toml` to `X.Y.Z-rc.N` and update `rust/Cargo.lock`
  (`cargo update --workspace` from `rust/`) in one commit, merged through a PR like any change. Then tag that
  commit `cli-vX.Y.Z-rc.N` and push the tag. Agents never push tags or create releases: Yalcin approves each one.
- Before tagging, run `pnpm build && pnpm parity:help --rust rust/target/release/vendo` on that commit. Every help
  screen must be the same, except the two accepted for VE-3823 (`logout`, `metrics delete`; Yalcin, 2026-10-06),
  `--profile`'s description on the root screen, which names VENDO_PROFILE (VE-3831, Yalcin 2026-10-06),
  `jobs list`'s `--status` and `--type`, which list the API's values (VE-3830, Yalcin 2026-10-06), and `config set`'s
  description, `profile set`'s "Set values on the active profile" (VE-3827, Yalcin 2026-10-06).
  This is a manual step, not a CI job (decided by Yalcin, 2026-10-05): the TypeScript CLI
  it compares against is deleted at 1.0.0 (VE-3669).
- Installing a release candidate: `VENDO_VERSION=cli-vX.Y.Z-rc.N bash install.sh` from a checkout,
  `curl -fsSL https://app2.vendodata.com/install.sh | VENDO_VERSION=cli-vX.Y.Z-rc.N bash` from anywhere, or
  `vendo self-update --version cli-vX.Y.Z-rc.N`.

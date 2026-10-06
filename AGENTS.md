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
  stdin and stdout are both terminals. Otherwise they need `--yes` and stop with exit 1 before any request, where the
  TS CLI went ahead; `--json` no longer implies `--yes`. `output::confirm` owns this.
- Stack: clap 4, reqwest (rustls), tokio, serde_json (`preserve_order`, `arbitrary_precision`), ICU4X,
  comfy-table, indicatif.
  Toolchain: `rustup` stable (`~/.cargo/bin`); `pnpm rust:test`, `pnpm rust:build`,
  `cargo clippy --all-targets` and `cargo fmt` (120 columns, `rust/rustfmt.toml`) from `rust/`.
- Tests: unit tests sit next to the code; `rust/tests/cli.rs` runs the built binary end to end with an isolated
  HOME, fake keys, a local stub server and a fresh update-check cache, so nothing leaves the machine (VE-3727).
- Snapshots (VE-3824), the regression net once the parity harness goes at 1.0.0: `rust/tests/cli/snapshots.rs`
  records every `--help` screen, found by walking the real command tree, in `rust/tests/snapshots/help/`, and the
  table, `--json` and confirmation output of each command against the stub's synthetic account in
  `rust/tests/snapshots/output/`. Any change to them fails `cargo test`. After a deliberate change run
  `INSTA_UPDATE=always cargo test --test cli` from `rust/` (it also deletes stale snapshots), review
  `git diff rust/tests/snapshots` and commit the snapshots with the change.
- Customer words follow vendo-web-v2's glossary (`apps/web/CONTEXT.md`; VE-3828, CLI 1.1): `vendo destinations`
  (hidden aliases `integrations`, `int`), "app" not "app connection", "platform" not "integration type". Flag names
  (`jobs … --integration <integrationId>`), API paths, JSON fields, `--json` output and code identifiers keep the
  API's "integration"; renaming a flag needs Yalcin's approval.
- Help layout (VE-3827, CLI 1.1): `vendo --help` is one sectioned list built at runtime from the command tree;
  `HELP_SECTIONS` in `rust/src/cli.rs` places each command, and a test fails when a visible command is in none.
  Group screens have no `help` row; `--profile`/`--debug` sit under "Global options". `whoami` is the one identity
  command and `config` moved under `profile`. Old paths keep working, hidden, printing exactly what their command
  prints: clap hidden aliases where clap allows (`config` = `profile`, `use` = `profile switch`), `MOVED` in `cli.rs`
  where it does not (`profile current`/`config show` → `whoami`, `config reset` → `logout --all`). Give a new
  hidden path a byte-identity test.
- Login (VE-3825, CLI 1.1): `vendo login` does what `init` did, and `init` is its hidden clap alias. It signs in
  through the browser when there is no working key, checks the key with `/me` and prints the setup summary. A key it
  has (profile or `VENDO_API_KEY`) is checked and kept; `--force`, `--env`/`--base-url` naming another instance, or a
  401/403 from `/me` sign in again, and any other failed check exits 1 without creating a key. With `VENDO_API_KEY`
  set it never opens a browser. Tests act as the browser (`login_at_browser` in `rust/tests/cli.rs` visits the printed
  sign-in URL on the stub); stdin stays closed, so no real browser opens.
- Catalog (VE-3829, CLI 1.1): `vendo catalog list` shows what the API lists by default, the platforms ready to
  connect, and ends with a footer built from the response's `meta` counts (`35 ready · 560 more on request (vendo
  catalog list --all)`; only `35 ready` when none is on request; the plain count line when the API sends no counts).
  `--all` sends `include_request_access=true` (the route has no pagination) and ends with the count line. The
  Availability column says the API's `availability` in plain words: `self_serve` "ready", `request_access` "on
  request", anything else as sent. `--json` prints the response as sent; without `--all` the request is unchanged.
- Completions (VE-3830, CLI 1.1): `vendo completions <shell>` prints the script the installer saves. Bare, it exits 0
  and says on stderr what it does, whether completions are set up for the shell `$SHELL` names, and how to set them
  up; stdout only ever carries a script, so an `eval` or redirect that leaves the shell out gets nothing.
  `rust/src/commands/completions.rs` owns that detection (the installer's saved script and startup-file block, or a
  line in `~/.bashrc`/`~/.zshrc` that runs `vendo completions <shell>`), and doctor's check uses it. A flag with a
  fixed set of values offers them on TAB through `Suggest` in `cli.rs`, each list citing its source: parsing still
  takes any string (the API decides), and clap sees the values only while a script is generated
  (`cli::suggesting`), so help screens and parse errors are as they were. The scripts are generated with the shell
  argument required, so they complete `completions` as before; `rust/tests/snapshots/output/completions__*` record
  them whole. `completions --help` shows `[shell]` where the TS CLI shows `<shell>`, which `pnpm parity:help`
  reports.
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
  `cargo test`, then builds, smoke-tests and uploads each binary.
- Cutting a release candidate: bump `version` in `rust/Cargo.toml` to `X.Y.Z-rc.N` and update `rust/Cargo.lock`
  (`cargo update --workspace` from `rust/`) in one commit, merged through a PR like any change. Then tag that
  commit `cli-vX.Y.Z-rc.N` and push the tag. Agents never push tags or create releases: Yalcin approves each one.
- Before tagging, run `pnpm build && pnpm parity:help --rust rust/target/release/vendo` on that commit. Every help
  screen must be the same, except the two accepted for VE-3823 (`logout`, `metrics delete`; Yalcin, 2026-10-06).
  This is a manual step, not a CI job (decided by Yalcin, 2026-10-05): the TypeScript CLI
  it compares against is deleted at 1.0.0 (VE-3669).
- Installing a release candidate: `VENDO_VERSION=cli-vX.Y.Z-rc.N bash install.sh` from a checkout,
  `curl -fsSL https://app2.vendodata.com/install.sh | VENDO_VERSION=cli-vX.Y.Z-rc.N bash` from anywhere, or
  `vendo self-update --version cli-vX.Y.Z-rc.N`.

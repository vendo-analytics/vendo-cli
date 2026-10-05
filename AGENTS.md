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
  `metrics` keys print snake_case where the TS client camelCased them (decided 2026-10-05).
- Stack: clap 4, reqwest (rustls), tokio, serde_json (`preserve_order`), comfy-table, indicatif.
  Toolchain: `rustup` stable (`~/.cargo/bin`); `pnpm rust:test`, `pnpm rust:build`,
  `cargo clippy --all-targets` and `cargo fmt` (120 columns, `rust/rustfmt.toml`) from `rust/`.
- Tests: unit tests sit next to the code; `rust/tests/cli.rs` runs the built binary end to end with an isolated
  HOME, fake keys, a local stub server and a fresh update-check cache, so nothing leaves the machine (VE-3727).
- Ported so far: login, init, logout, whoami, config, profile, status, doctor, mcp, completions,
  self-update (VE-3665); jobs list/get/cancel/watch/tail and the shared watcher (VE-3666); apps, sources,
  integrations (`int`) and catalog (VE-3667); metrics, models and measurement (VE-3668). Not yet: dictionary
  (VE-3713). `rust/src/web_app.rs` is the one place that knows the web-app routes (`/api/metrics`,
  `/api/measurement/*`), which go out as raw paths with no account prefix.
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
- Installing a release candidate: `VENDO_VERSION=cli-vX.Y.Z-rc.N bash install.sh` from a checkout,
  `curl -fsSL https://app2.vendodata.com/install.sh | VENDO_VERSION=cli-vX.Y.Z-rc.N bash` from anywhere, or
  `vendo self-update --version cli-vX.Y.Z-rc.N`.

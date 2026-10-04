# PROTOTYPE — throwaway Rust port of the vendo CLI

**Question:** Is rewriting the `vendo` CLI in Rust worth it, and what would it take to
stay compatible with the TypeScript CLI and its release/install contract?

**Scope:** `login` (browser + headless), `whoami`, `apps list`, `completions`, global
`--profile`/`--debug`, the `VENDO_*` env vars and the daily update check. It shares
`~/.config/vendo/config.json` with the TypeScript CLI.

**Run:** `pnpm proto:rust -- whoami` (or `cargo build --release` here and run
`target/release/vendo`).

**Answer (2026-10-04):** Yes, it's feasible.
- **Size:** a 4.3 MB binary, against 106 MB today.
- **Startup:** about 7 ms, against about 55 ms.
- **JSON:** `whoami`/`apps list` output is byte-identical to the TypeScript CLI against staging.
- **Login:** the `/cli-auth` contract holds.
- **Cost:** slower builds and 136 crates to keep up to date.

Full results, measurements and the proposed plan:
`vendo_knowledgebase/Plans/research/vendo-cli-rust-rewrite-20261004.md`.

Delete this folder (and the `proto:rust` script) once the decision is made.

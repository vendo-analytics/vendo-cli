# CLAUDE.md — vendo-cli

## Purpose
Standalone TypeScript CLI — "manage your data pipeline from the terminal". Published as the
`vendo` binary. Talks to the pipelines/web API (`/api/v1/*`). **This repo is the canonical and
only active Vendo CLI** — the former in-monorepo `vendo-web-v2/apps/cli` was removed (2026-06-02).

## Stack
- Node ≥ 22, TypeScript 5.9, `commander` 13, `chalk` 5, `cli-table3`, `ora`.
- Build: `tsup` (ESM → `dist/cli.js`) + Node SEA single-executable for standalone binaries.
- Tests: `vitest`. Lint: `eslint` 9. Version: see `package.json`.

## Layout (`src/`)
`cli.ts`, `client.ts`, `config.ts`, `identity.ts`, plus `commands/` (~22 modules: apps, sources,
integrations, jobs, metrics, models, catalog, measurement, login, logout, init, doctor, status,
whoami, profile, config, completions, self-update). Tests in `src/__tests__/`.

## Key commands
- `pnpm run typecheck` — `tsc --noEmit`
- `pnpm run test` — `vitest run`
- `pnpm run lint` / `pnpm run lint:fix`
- `pnpm run build` — `tsup` (dev bundle); `pnpm run build:standalone` — SEA binary
- Distribution: `install.sh` pulls per-platform binaries from GitHub Releases
  (`vendo-analytics/vendo-cli`); release CI is tag-triggered (`cli-v*`, `.github/workflows/release.yml`).

## Canonical-status note
Do all CLI work here. The apps `role` → granular `permissions[]` migration landed on `main`
(PRs #1/#3), as did the API-drift fixes (PR #4: catalog route, POST pause/resume, headless
OAuth polling).

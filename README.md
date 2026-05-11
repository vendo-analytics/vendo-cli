# Vendo CLI

Manage your [Vendo](https://vendodata.com) data pipeline from the terminal.

## Install

**Production** (uses `https://app2.vendodata.com`):

```bash
curl -fsSL https://app2.vendodata.com/install.sh | bash
```

**Staging** (uses `https://stg.vendodata.com`):

```bash
curl -fsSL https://stg.vendodata.com/install.sh | bash
```

The installer downloads the right standalone binary for your platform from this repo's [GitHub Releases](https://github.com/vendo-analytics/vendo-cli/releases), verifies its SHA-256, and installs `vendo` into `~/.local/bin`. Shell completions are installed when possible.

Supported platforms: `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64`.

## Quickstart

```bash
vendo init       # browser-based first-time setup
vendo doctor     # verify config + auth
vendo whoami     # show the active account
vendo status     # check service health
```

Log in once and every subsequent command works with no flags:

```bash
vendo login                # opens a browser window
# or, for CI / scripts:
vendo login --api-key vendo_sk_… --account <account-id>
```

## Common commands

```bash
vendo apps list                          # list installed apps
vendo sources list                       # list data sources
vendo int list                           # list export integrations
vendo jobs watch                         # live view of running jobs
vendo jobs tail <job-id>                 # tail one job
vendo catalog list                       # browse the connector catalog
vendo measurement methodologies list     # marketing measurement
vendo metrics list                       # custom metrics
```

Each command supports `--json` for machine-readable output and `--help` for full usage.

## Multi-account

```bash
vendo profile list                       # show saved profiles
vendo profile switch <account-slug>      # change the active profile
vendo profile current                    # what's active right now
vendo --profile <name> <command>         # one-off override
```

Env-var overrides (handy in CI):

```bash
VENDO_API_KEY=…           # Bearer key
VENDO_ACCOUNT_ID=…        # Account UUID
VENDO_API_URL=…           # API base URL (defaults to https://app2.vendodata.com)
```

## Self-update

```bash
vendo self-update         # re-run the hosted installer
```

Updates are checked once every 24 hours and surfaced after `login` and `status`.

## Build from source

Requires Node ≥ 22.

```bash
pnpm install
pnpm build                # ESM bundle at dist/cli.js
pnpm build:standalone     # standalone binary at dist/vendo-<platform>
pnpm test                 # unit tests
pnpm typecheck            # tsc --noEmit
```

The standalone build uses Node's [single-executable-applications](https://nodejs.org/api/single-executable-applications.html) feature and requires an **official** Node 22 binary (the Homebrew build strips the SEA fuse). Set `SEA_NODE_BINARY` to the path of an unpacked nodejs.org binary if your `node` doesn't carry the fuse.

## License

MIT — see [LICENSE](./LICENSE).

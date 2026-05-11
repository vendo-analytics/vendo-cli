# Vendo CLI

Manage your [Vendo](https://vendodata.com) data pipeline from the terminal.

## Install

```bash
curl -fsSL https://app2.vendodata.com/install.sh | bash
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

## Authentication

The CLI stores credentials in a profile at `~/.config/vendo/config.json`. **Log in once and every subsequent command works with no flags.**

**Browser login (recommended).** Opens a browser window where you confirm the account; the CLI saves the resulting profile and you're done:

```bash
vendo login
```

If you have access to multiple Vendo accounts the browser flow lets you pick which one to authenticate against, and saves the profile keyed by the account slug.

**Headless login.** For CI, automation, or any environment without a browser, mint an account-scoped API key from the Vendo dashboard (Settings → API Keys) and pass both the key and the account id:

```bash
vendo login --api-key vendo_sk_… --account <account-id>
```

This validates the credentials against `/api/v1/me` and writes the same profile that the browser flow would, so future commands need no flags.

**Inspecting the active session.**

```bash
vendo whoami            # which account you're authenticated as
vendo profile current   # active profile + where each effective value comes from
vendo doctor            # full health check
```

**Logging out.**

```bash
vendo logout
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

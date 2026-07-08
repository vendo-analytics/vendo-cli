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

## What you can do

The CLI is a full management surface — not just read commands. Every resource (`apps`, `sources`, `integrations`, `metrics`) supports the standard set: `list`, `get`, `create`, `update`, `delete`, plus operational verbs like `pause`, `resume`, and `sync`. Jobs add `watch`, `tail`, and `cancel`.

### Browse

```bash
vendo apps list                          # installed apps
vendo sources list                       # data sources
vendo int list                           # export integrations
vendo models list                        # data models
vendo metrics list                       # custom metrics
vendo catalog list                       # connector catalog (apps you can install)
vendo measurement methodologies list     # marketing measurement methodologies
vendo measurement ltv list               # LTV cohorts
vendo measurement signals list           # per-signal availability
vendo jobs list                          # recent sync jobs
```

### Inspect one thing

```bash
vendo apps get <app-id>
vendo sources get <source-id>
vendo int get <integration-id>
vendo jobs get <job-id>
vendo models get <model-id>
vendo metrics get <metric-id>
```

### Create / update / delete

```bash
vendo apps create --type <type> --credentials-file creds.json
vendo sources create --app <app-id> --config-file config.json
vendo int create  --source <id> --dest <id> --config-file config.json
vendo metrics create --name "<name>" --formula "<sql>"

vendo apps update    <app-id>    --credentials-file creds.json
vendo sources update <source-id> --config-file config.json
vendo int update     <int-id>    --config-file config.json
vendo metrics update <metric-id> --formula "<sql>"

vendo apps delete    <app-id>    -y     # -y skips the confirmation prompt
vendo sources delete <source-id> -y
vendo int delete     <int-id>    -y
vendo metrics delete <metric-id> -y
```

### Run + watch jobs

```bash
vendo sources sync <source-id>           # trigger a manual import
vendo sources sync <source-id> --watch   # …and wait for it
vendo int sync     <int-id> --watch      # same for export integrations
vendo jobs watch                         # live polling view of running jobs
vendo jobs tail <job-id>                 # tail one job's events live
vendo jobs tail --source <source-id> --next   # wait for the next new job, then tail it
vendo jobs cancel <job-id> -y            # cancel a queued/running job
```

### Pause + resume

```bash
vendo apps pause <app-id>      # stop everything tied to this app
vendo apps resume <app-id>
vendo sources pause <source-id>
vendo sources resume <source-id>
vendo int pause <int-id>
vendo int resume <int-id>
```

### Discover everything

`vendo --help` lists every command group. `vendo <group> --help` (e.g. `vendo int --help`, `vendo jobs --help`) lists the verbs under that group. `vendo <group> <verb> --help` shows the full options for that verb. `vendo doctor` is a one-shot health check of your local setup + API auth.

Every command supports `--json` for machine-readable output (so you can pipe through `jq`), and `--debug` for verbose request diagnostics.

## Connect an MCP client (Claude, Cursor, …)

Vendo runs a [Model Context Protocol](https://modelcontextprotocol.io) server, so AI assistants can read your account, sources, models, metrics, and warehouse. `vendo mcp` prints a ready-to-paste client config:

```bash
vendo mcp                  # human-readable config + connection details
vendo mcp --json           # just the mcpServers JSON block
vendo mcp --show-key       # embed your API key instead of a ${VENDO_API_KEY} placeholder
```

Paste the block into your client (`claude_desktop_config.json`, `.cursor/mcp.json`, etc.):

```json
{
  "mcpServers": {
    "vendo": {
      "type": "http",
      "url": "https://app2.vendodata.com/api/mcp",
      "headers": { "Authorization": "Bearer ${VENDO_API_KEY}" }
    }
  }
}
```

The transport is stateless streamable-HTTP; auth is the same `vendo_sk_*` key the CLI uses (or OAuth in claude.ai). Use **app2.vendodata.com** — `app.vendodata.com` does not serve `/api/mcp`.

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

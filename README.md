# Vendo CLI

Manage your [Vendo](https://vendodata.com) data pipeline from the terminal: apps, sources, destinations, sync jobs,
the platform catalog, the data dictionary, metrics, models and Marketing Measurement.

Full documentation: [docs.vendodata.com/cli](https://docs.vendodata.com/cli).

## Install

```bash
curl -fsSL https://app2.vendodata.com/install.sh | bash
```

The installer downloads the binary for your system from this repo's
[GitHub Releases](https://github.com/vendo-analytics/vendo-cli/releases), checks its SHA-256, installs it as
`~/.local/bin/vendo` and sets up TAB completion for your shell (see [Shell completions](#shell-completions)).
If `~/.local/bin` is not on your `PATH`, the installer says so; add it, then open a new terminal.

Supported systems: macOS on Apple silicon and Intel (`darwin-arm64`, `darwin-x64`) and Linux on x64 and arm64
(`linux-x64`, `linux-arm64`).

| Installer setting | Effect |
| --- | --- |
| `VENDO_VERSION=1.1.0` | Install that release instead of the latest |
| `VENDO_INSTALL_COMPLETIONS=0` | Skip shell completions |

```bash
curl -fsSL https://app2.vendodata.com/install.sh | VENDO_VERSION=1.1.0 bash
```

To update later, run `vendo self-update` (`--version <version>` installs a specific release). `vendo status`,
`vendo whoami` and the browser sign-in of `vendo login` check for a newer release once a day and print a notice
when there is one.

## Sign in

```bash
vendo login
```

Without a working API key, `vendo login` prints a sign-in link, opens it in your browser when you press Enter,
and waits while you confirm the account there. It then checks the new key and prints a setup summary. The key is
saved in a profile named after the account, in `~/.config/vendo/config.json`, and every later command uses it with
no flags.

Run `vendo login` again and it checks the key you already have and keeps it; `vendo login --force` signs in again.

```bash
vendo whoami    # the account and profile you are using
vendo doctor    # check the install, the settings and the API key
vendo status    # account health: apps, sources, destinations and recent failures
```

**Without a browser** (servers, CI), create an API key in the Vendo web app under Settings → API keys and pass it
with the account ID:

```bash
vendo login --api-key vendo_sk_... --account <account-id>
```

This checks the key and saves the same profile the browser sign-in does. To save nothing at all, set
`VENDO_API_KEY` and `VENDO_ACCOUNT_ID` instead; they take precedence over the saved profile's values (see
[Environment variables](#environment-variables)).

**Several accounts.** Each sign-in saves a profile; one of them is active.

```bash
vendo profile list                  # saved profiles, the active one marked
vendo profile switch <profile>      # make another profile active (no name: pick from a list)
vendo --profile <profile> status    # use a profile for one command
vendo profile set --account <id>    # change a value on the active profile
```

**Sign out.** `vendo logout` removes the active profile; `vendo logout --all` removes every profile (it asks
first; pass `--yes` in scripts).

## Commands

`vendo --help` lists every command in four sections:

| Section | Commands |
| --- | --- |
| Getting started | `login`, `logout`, `whoami`, `status`, `doctor`, `commands` |
| Data pipeline | `apps`, `sources`, `destinations`, `jobs` |
| Data catalog | `catalog`, `dictionary`, `metrics`, `models`, `measurement` |
| Account | `profile`, `mcp`, `completions`, `self-update` |

The commands in each group:

| Group | Commands |
| --- | --- |
| `apps` | `list`, `diagnose`, `get`, `pause`, `resume`, `delete`, `create`, `update` |
| `sources` | `list`, `get`, `sync`, `pause`, `resume`, `delete`, `create`, `update` |
| `destinations` | `list`, `get`, `sync`, `refresh-source`, `pause`, `resume`, `delete`, `create`, `update` |
| `jobs` | `list`, `get`, `cancel`, `watch`, `tail` |
| `catalog` | `list`, `get` |
| `dictionary` | `list`, `search`, `get` |
| `metrics` | `list`, `get`, `create`, `update`, `activate`, `delete` |
| `models` | `list`, `get` |
| `measurement` | `methodologies list`, `methodologies get`, `rules preview`, `ltv list`, `ltv cohort`, `ltv customer`, `signals list`, `signals click-path` |
| `profile` | `list`, `switch`, `set` |

`vendo <command> --help` shows a command's flags and examples, and `vendo commands` lists every command on one line
each. `--profile <name>` and `--debug` work with every command. At a terminal, a group run without its command
(`vendo apps`) opens a menu of its commands: arrow keys move, typing filters, Enter runs, Esc leaves. The menu
needs stdin, stdout and stderr to be terminals, `TERM` not `dumb` and prompts on (see
[Non-interactive runs](#non-interactive-runs-ci-and-vendo_no_input)); otherwise the group prints its help and
exits 2.

### Apps, sources and destinations

An **app** is your connection to a platform (Shopify, Google Ads, BigQuery, ...) and holds its credentials. A
**source** imports data from an app into your warehouse. A **destination** sends your data out to an app.

```bash
vendo apps list
vendo apps list --role destination
vendo apps diagnose                      # apps that need attention
vendo apps get <app-id>

vendo sources list --app <app-id>
vendo sources sync <source-id> --watch   # start an import and follow it until it ends
vendo destinations list --state active
vendo destinations sync <destination-id>
vendo destinations refresh-source <destination-id> --from 2026-06-29 --to 2026-07-02
```

Create and change them from JSON files:

```bash
vendo apps create --type onesignal --name "OneSignal Prod" --role destination --credentials-file onesignal.json
vendo apps update <app-id> --credentials-file rotated.json

vendo sources create --app <app-id> --sync-type shopify --import-tasks orders,customers --run-now
vendo sources update <source-id> --frequency 6 --unit hours

vendo destinations create --source-app <app-id> --dest-app <app-id> --data-type user_properties --config-file tasks.json
vendo destinations update <destination-id> --config-file new-tasks.json
```

`vendo catalog list` lists the platform names `apps create --type` takes. Pause, resume and delete work the same
way on apps, sources and destinations:

```bash
vendo sources pause <source-id>
vendo sources resume <source-id>
vendo destinations delete <destination-id> --dry-run   # say what it would do, send nothing
vendo destinations delete <destination-id> --yes
```

`vendo integrations` and `vendo int`, the old names of `vendo destinations`, still work.

### Jobs

```bash
vendo jobs list --status failed
vendo jobs list --source <source-id>
vendo jobs list --integration <destination-id>
vendo jobs get <job-id>
vendo jobs watch                         # live view of running and pending jobs
vendo jobs tail <job-id>                 # follow one job until it ends
vendo jobs tail --source <source-id> --next   # wait for the source's next job, then follow it
vendo jobs cancel <job-id>
```

`--integration` takes a destination ID.

### Catalog

`vendo catalog list` shows the platforms ready to connect, and its last line counts the rest:

```text
35 ready · 560 more on request (vendo catalog list --all)
```

```bash
vendo catalog list --all                       # every platform, the ones on request too
vendo catalog list --category advertising      # the counts and the hint keep your filters
vendo catalog list --role destination
vendo catalog get shopify                      # details, availability and docs link
```

The Availability column says `ready` for a platform you can connect yourself and `on request` for one you ask Vendo
for access to.

### Data dictionary

The dictionary describes your events, properties, groups, columns, metrics, models and audiences.
`vendo dictionary list` shows events unless you pass `--type` (`event`, `prop`, `group`, `column`, `metric`,
`model` or `audience`).

```bash
vendo dictionary list --type prop
vendo dictionary list --type event --query checkout
vendo dictionary search checkout
vendo dictionary get <subject-id>
vendo dictionary get event:checkout_completed
```

`search` and `--query` (`-q`) match text in the subject ID, name and description, ignoring case, within one subject
type. They are not the ranked search behind the MCP `dictionary_search` tool.

`get` takes the subject ID that `list` and `search` print, or a name such as `event:checkout_completed` when only
one entry has that name (write a space as `%20`: `event:Order%20Placed`). When several entries share it, the error
lists their subject IDs.

### Metrics, models and Marketing Measurement

```bash
vendo metrics list --status active
vendo metrics create --name "ROAS" --definition roas.query.json
vendo metrics activate <metric-id>
vendo models list --valid
vendo models get <model-id>
vendo measurement methodologies list
vendo measurement ltv list --granularity weekly
vendo measurement signals list
```

### Connect an MCP client (Claude, Cursor, ...)

Vendo runs a [Model Context Protocol](https://modelcontextprotocol.io) server, so AI assistants can work with your
account. `vendo mcp` prints a client configuration ready to paste:

```bash
vendo mcp              # the configuration and how to connect
vendo mcp --json       # only the mcpServers block
vendo mcp --show-key   # put your API key in place of the ${VENDO_API_KEY} placeholder
```

```json
{
  "mcpServers": {
    "vendo": {
      "type": "http",
      "url": "https://app2.vendodata.com/api/mcp",
      "headers": {
        "Authorization": "Bearer ${VENDO_API_KEY}"
      }
    }
  }
}
```

Paste it into your client's configuration (`claude_desktop_config.json`, `.cursor/mcp.json`, ...). The server
takes the same `vendo_sk_...` key as the CLI. Use `app2.vendodata.com`: `app.vendodata.com` does not serve
`/api/mcp`.

## Shell completions

The installer sets up TAB completion for bash, zsh and fish. TAB completes commands and flags, and offers the
values of flags that take one of a fixed list, such as `jobs list --status`. Run `vendo completions` to see
whether completions are set up for your shell and how to set them up by hand.

**zsh.** Add these lines at the end of `~/.zshrc`, after any framework such as oh-my-zsh, Prezto or Zim:

```zsh
autoload -Uz compinit && compinit
eval "$(vendo completions zsh)"
```

The completions need `compinit` to run before them, and a `compinit` that runs after them, as frameworks do, drops
vendo's completions. That is why the lines go last.

**bash.** On Linux, add this line to `~/.bashrc`:

```bash
eval "$(vendo completions bash)"
```

On macOS, Terminal opens bash as a login shell, which reads its login file and not `~/.bashrc`. Add the line to
the first of `~/.bash_profile`, `~/.bash_login` and `~/.profile` that exists, or create `~/.bash_profile` when
none does. The installer writes to `~/.bashrc` and to that file.

**fish.**

```fish
vendo completions fish > ~/.config/fish/completions/vendo.fish
```

Open a new terminal after any of these.

## Scripts and AI agents

### JSON output

Every command takes `--json`. The result goes to stdout as JSON, and messages, progress and the update notice go
to stderr, so stdout stays parseable.

- The commands for your data (`apps`, `sources`, `destinations`, `jobs`, `catalog`, `dictionary`, `metrics`,
  `models`, `measurement`) print the API's response as it came, mostly `{ "data": ... }`, with `meta` beside the
  data of a list. These build their own JSON instead:
  - The `metrics` commands put the metric under `data`. `metrics list` puts the metrics under `data` and the
    total in `meta.pagination.total`; `metrics delete` puts the API's response under `data`.
  - `apps diagnose`: `{"broken":[...],"orphaned":[...]}`, where `missing` on each orphaned app says what it
    lacks, `source` or `destination`.
  - `measurement methodologies get`: the methodology alone, with no `data` around it.
  - `sources sync` and `destinations sync`, when a job is already running:
    `{"data":{"jobId","status","message":"Sync already in progress"}}`.
  - `apps create` without `--credentials-file`, which connects the app through OAuth in the browser:
    `{"data":{"id"}}`.
- The other commands print objects of their own, for example `vendo profile list --json`:
  `{"profiles":[{"name","active","accountId","baseUrl"}]}`, and `vendo login --json`:
  `{"profile","baseUrl","accountId","auth","accountName"}`.
- `vendo jobs watch --json` prints one line of JSON each time the job list changes; `vendo jobs tail --json` prints
  the job when it ends, as `vendo jobs get --json` does.
- `vendo mcp --json` prints the `mcpServers` block.
- No JSON output contains your API key unless you ask for it with `vendo mcp --show-key`.
- `--dry-run` prints text, with `--json` too: one line for delete, pause, resume and cancel, and a few lines (the
  resource and its active job) for `sources sync` and `destinations sync`.

The list commands, except `profile list` and `measurement signals list`, also take `--output <field>`, which
prints one field per row, and so does `dictionary search`:

```bash
vendo sources list --state active --output id
```

### Errors and exit codes

A command exits with 0 when it succeeds, 1 when it fails and 2 when it was called wrongly (an unknown command, a
missing argument). Two report a failure and still exit 0: `vendo logout` when you are not logged in, and
`vendo jobs tail` when the job fails or the wait times out. With `--json` both print the JSON error, and
`jobs tail` still prints the last job it read, if any, on stdout.

With `--json`, a failure is one line of JSON on stderr, its last line:

```json
{"error":{"message":"App not found","code":"NOT_FOUND","status":404,"requestId":"req_1a2b3c4d5e6f7a8b"}}
```

| Field | Value |
| --- | --- |
| `message` | What the text error says after `Error: ` |
| `code` | The API's error code, or `null` |
| `status` | The HTTP status the API answered, or `null` when no answer came (a timeout, no network) |
| `requestId` | The request's ID, or `null`; quote it when you contact support |

An error the CLI finds itself, such as a missing `--yes` or a wrong argument, has the message only, the other
fields `null`. A usage error's message includes the tips that point to the right command, such as
`tip: a similar subcommand exists: 'list'`. With `--debug`, debug lines come before the error.

### Discover commands

`vendo commands --json` prints the command tree, so an agent can find every command and flag without reading help
screens. Each command has `name`, `path`, `description`, `aliases`, `arguments`, `options` and its subcommands in
`commands`. Each option has `name`, `short`, `valueName`, `description`, `required`, `default`,
`possibleValues` (the values it accepts), `suggestedValues` (the values TAB offers) and `global`.

### Short IDs

Tables show IDs by their first 8 characters, such as `1a2b3c4d...`. Wherever a command takes the full ID of an app,
source, destination, job, metric, model or methodology, as an argument or in a flag such as `--app`, `--source`,
`--integration`, `--source-app` or `--dest-app`, it also takes those 8 characters, with or without the `...`:

```bash
vendo jobs get 1a2b3c4d
vendo apps pause 1a2b3c4d...
```

- The CLI looks the short ID up in that resource's list, reading up to its newest 500 items. An older item needs
  its full ID.
- When several items start with it, the command stops with exit 1 before it sends its request, and the error
  lists up to 10 of them by full ID. Use the full ID.
- When none does, the short ID is sent as typed and the API answers as it would for any unknown ID.
- A `--dry-run` of delete, pause, resume or cancel sends nothing, so it does not look the ID up. A `--dry-run` of
  `sources sync` or `destinations sync` looks it up, then reads the resource and its active job (GET requests
  only).

### Choosing a profile: `VENDO_PROFILE`

`VENDO_PROFILE=<profile>` selects a profile like `--profile`, for every command in that shell. `--profile` wins
over `VENDO_PROFILE`, which wins over the active profile. `VENDO_PROFILE` never changes which profile is saved as
active; `vendo profile switch` does. With a name no profile has, the commands that need an API key, and
`vendo logout`, stop with exit 1 and an error that names it, unless `VENDO_API_KEY` is set. Other commands carry
on, and `vendo doctor` shows it as a warning.

### Non-interactive runs: `CI` and `VENDO_NO_INPUT`

The CLI asks a question only when stdin and stdout are terminals. Set `CI` or `VENDO_NO_INPUT` to anything but
empty, `0` or `false` and it asks none, at a terminal too:

- `delete`, `jobs cancel` and `logout --all` need `--yes`. Without it they stop with exit 1 before they send
  anything. `--json` does not imply `--yes`.
- A group run without its command (`vendo apps`) prints its help and exits 2 instead of opening the menu.
- `vendo profile switch` without a profile name prints `Cancelled.` instead of a list to pick from.
- `vendo login` without a key prints the sign-in link and waits; it opens no browser. In CI, use
  `vendo login --api-key ... --account ...` or the environment variables below.

`TERM=dumb` turns off only the group menu; the y/N questions and the profile picker still ask.

### Environment variables

| Variable | Effect |
| --- | --- |
| `VENDO_API_KEY` | API key to use, over the profile's |
| `VENDO_ACCOUNT_ID` | Account to use, over the profile's |
| `VENDO_API_URL` | API base URL, over the profile's (default `https://app2.vendodata.com`) |
| `VENDO_PROFILE` | Profile to use instead of the active one |
| `VENDO_NO_INPUT`, `CI` | Ask no questions (see above) |
| `VENDO_DEBUG=1` | Print request diagnostics on stderr, like `--debug` |
| `NO_COLOR` | Print without colours |

## Coming from an earlier version

Old command names keep working but are no longer in the help:

| Old | Now |
| --- | --- |
| `vendo init` | `vendo login` |
| `vendo integrations`, `vendo int` | `vendo destinations` |
| `vendo config ...` | `vendo profile ...` |
| `vendo profile current`, `vendo config show` | `vendo whoami` |
| `vendo config reset` | `vendo logout --all` |
| `vendo config use` | `vendo profile switch` |

`vendo catalog credential-schema <platform>` also still works but is no longer in the help;
`vendo catalog get <platform>` shows the same credential fields.

Delete, cancel and `logout --all` no longer go ahead without `--yes` when no one can answer the question, and
`--json` no longer implies `--yes`.

## Build from source

The CLI is written in Rust. With a stable toolchain from [rustup](https://rustup.rs):

```bash
cd rust
cargo build --release   # the binary is rust/target/release/vendo
cargo test
```

## License

MIT. See [LICENSE](./LICENSE).

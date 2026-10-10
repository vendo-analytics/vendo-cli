# Changelog

Each release's notes. The release workflow publishes the section of a `cli-vX.Y.Z` tag's version as that GitHub
release's notes, so every release needs a `## X.Y.Z` section here before it is tagged.

## Unreleased

- `vendo status` counts an app as errored when its status needs attention (reconnect required or disconnected),
  the apps `vendo apps diagnose` lists as broken, instead of always showing 0.

## 1.1.0

The Vendo CLI is rewritten in Rust and replaces the TypeScript CLI (0.3.1). Your commands, flags, profiles and
config file keep working: everything not listed below works as it did. Update with `vendo self-update` from 0.3.x,
or install with `curl -fsSL https://app2.vendodata.com/install.sh | bash`.

### Smaller and faster

- One native binary with no bundled Node.js: a 7 to 9 MB download instead of 110 to 125 MB, and it starts faster.
- HTTPS uses your system's certificates (the macOS keychain, the Linux CA store) instead of Node's built-in list.
  `NODE_EXTRA_CA_CERTS` is no longer read: add a company certificate to the system store instead.

### New commands and names

- `vendo destinations` is the new name of `vendo integrations`.
- `vendo login` signs in through the browser, checks the key and shows your workspace. Run again, it keeps a key
  that works; `--force` signs in again. It replaces `vendo init`.
- `vendo workspace` shows the account, the API key, your profiles and setup checks on one screen. It replaces
  `vendo whoami` and `vendo doctor`. It exits 1 only when you are not signed in, so scripts can use it as a sign-in
  check; `vendo doctor` still exits 1 when any check fails.
- `vendo profile` holds what `vendo config` did: `profile list`, `profile switch`, `profile set`.
- `vendo update` replaces `vendo self-update`. `vendo update --version <version>` installs that version (the old
  CLI printed its own version instead).
- `vendo version` prints the version, like `vendo --version`.
- `vendo help` lists every command in three sections (Account, Data pipeline, Data catalog), each group with its
  commands; `vendo help <command>` shows a command's help.
- `vendo catalog list` shows the platforms ready to connect and counts the ones available on request;
  `vendo catalog list --all` lists every platform. `vendo catalog get` shows an Availability line.
- `vendo models list` and `vendo models get` show the model's type (`sql`, `bqml`, `grouping`, …).
  `vendo metrics list` no longer has a Type column, which the API stopped filling.
- `vendo status` counts apps whose last runs failed as errored.

### At a terminal

- The installer sets up TAB completion for bash, zsh and fish. TAB also offers the values of flags that take one of
  a fixed list, such as `jobs list --status`. `vendo completions` says whether completions are set up and how.
- A group run without its command (`vendo apps`) opens a menu of its commands.
- A command missing a value it needs (`vendo apps get`) asks for it, with a list to pick from where there is one.
- The list commands (`vendo apps list`, `jobs list`, `metrics list`, …) open a list you can pick from; Enter shows
  the item, then the actions that apply to it, such as pause, sync or delete.
- `--profile` with no name, and `vendo profile switch` with none, open a list of your profiles.
- Arrow keys move, typing filters, Enter chooses, and Esc or Ctrl-C leaves with nothing run. Scripts, pipes and CI
  get the tables and errors they got before. Set `VENDO_NO_INPUT=1` (or `CI`) to turn every question and list off.

### For scripts and AI agents

- Every command takes `--json`. With `--json`, a failure is one line of JSON on stderr:
  `{"error":{"message","code","status","requestId"}}`.
- `vendo commands --json` prints every command, argument and option, so an agent can discover them.
- Wherever a command takes a full ID, it also takes the 8 characters tables show (`1a2b3c4d` or `1a2b3c4d...`).
  When several items match, the command stops and lists them.
- `VENDO_PROFILE=<profile>` selects a profile for every command in that shell, like `--profile`. It never changes
  the saved active profile.
- `CI` or `VENDO_NO_INPUT` turns every question off, also at a terminal.

### Changed behaviour

- `delete`, `jobs cancel`, `config reset` and `logout --all` need `--yes` when no one can answer the question (no
  terminal, or questions turned off). Without it they stop with exit 1 before sending anything. `--json` no longer
  implies `--yes`, and `--dry-run` needs no `--yes`. At a terminal they still ask y/N.
- `vendo logout --all` asks before removing every profile, and takes `-y, --yes`.
- The update notice goes to stderr, so `--json | jq` keeps working when an update is available. It only announces
  a newer stable release, never a release candidate or an older version.
- `--json` prints the API's response as it came. Nested keys keep the API's snake_case where the old CLI
  camelCased them: in `status`, `jobs list`, `jobs get`, `apps get`, `destinations list`, `destinations get`,
  `models list` and `models get` (config, schedule, metrics, output config, columns, definition). Numbers print as
  the API sent them (`1.50` stays `1.50`; integers past 2^53 stay exact).
- Help screens have a new layout, and a wrong command or flag exits with 2 instead of 1.
- Tables have different borders and column gaps; the cells are the same.
- Where the old CLI crashed with a stack trace (an unreadable file, "Nothing to update", a broken value in
  `config.json` such as `"profiles": null`), the CLI prints `Error: <message>` and exits 1, or treats the broken
  value as unset.
- `--debug` shows the full cause of a failed request (DNS, refused connection, TLS) instead of "fetch failed".
- Ctrl-C stops `jobs watch` at once, and exits while a spinner runs (the old CLI could hang).
  `vendo apps create` with a browser sign-in finishes as soon as the browser returns.
- The spinner looks different, and prints nothing when stderr is not a terminal.
- `Error:` is coloured when stderr is a terminal. `CI` and an unknown `TERM` no longer turn colours off;
  `NO_COLOR` and `FORCE_COLOR` still apply.
- Dates, numbers and money follow your locale as before. In a few locales without data in the new formatting
  library (about 130 of Node's 943, such as `se`, `gsw` and `ln`), numbers and money use the default format.
- `vendo -- workspace --json` runs the command instead of refusing.
- `vendo completions --help` shows `[shell]` instead of `<shell>`; bare `vendo completions` explains itself.
- `vendo profile set` is described as "Set values on the active profile".

### Removed

- `vendo mcp`. How to connect an MCP client (Claude, Cursor, …) is in the docs at
  [docs.vendodata.com](https://docs.vendodata.com).

### Old names that still work

They are no longer in the help, and run the new command (`vendo doctor` keeps its stricter exit code, see above).

| Old | Now |
| --- | --- |
| `vendo init` | `vendo login` |
| `vendo integrations`, `vendo int` | `vendo destinations` |
| `vendo config ...` | `vendo profile ...` |
| `vendo config use` | `vendo profile switch` |
| `vendo whoami`, `vendo doctor` | `vendo workspace` |
| `vendo profile current`, `vendo config show` | `vendo workspace` |
| `vendo config reset` | `vendo logout --all` |
| `vendo self-update` | `vendo update` |

`vendo catalog credential-schema <platform>` also still works, hidden; `vendo catalog get <platform>` shows the same
credential fields.

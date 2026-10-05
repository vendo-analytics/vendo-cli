// Write-command parity (VE-3667, VE-3668): run the same create/update/pause/
// resume/sync/activate/delete commands with the TypeScript CLI and the Rust CLI
// on staging, each CLI on its own throwaway resources, and compare output and
// exit codes after masking what must differ (IDs, timestamps, the names).
//
//   pnpm build && pnpm parity:writes --profile <staging profile> [--rust <binary>] [--only pipeline|metrics] [--keep]
//
// Use a disposable staging workspace (e.g. "Vendo CLI test"): the run creates
// webhook apps, sources and draft metrics there. At the end of each scenario,
// also when a step fails, it deletes every app named "CLI parity …" with its
// sources, and every metric named "CLI parity …". Same safety as
// parity/run.mjs: staging or localhost only, isolated HOME.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';

import { MISSING_ID, camelCaseKeysDeep, diffCells, diffErrors, diffJson, failureLine } from './lib.mjs';
import { createSession, fail, root } from './session.mjs';

const { values: opts } = parseArgs({
  options: {
    profile: { type: 'string' },
    rust: { type: 'string' },
    keep: { type: 'boolean', default: false },
    only: { type: 'string' },
  },
});
const SCENARIOS = ['pipeline', 'metrics'];
if (opts.only && !SCENARIOS.includes(opts.only)) fail(`--only must be one of: ${SCENARIOS.join(', ')}.`);

// `sync --watch` follows a job to the end, so allow more than the read harness.
const session = createSession({ ...opts, timeoutMs: 600_000 });
if (!session.hasRust) fail(`Rust binary not found at ${session.rustPath}. Pass --rust <binary>.`);

const CLIS = ['ts', 'rust'];
const label = (cli) => (cli === 'ts' ? 'TS' : 'Rust');
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const outDir = join(root, 'parity', 'out', `${stamp}-writes`);
mkdirSync(outDir, { recursive: true });
const emptyCredentials = join(outDir, 'empty-credentials.json');
writeFileSync(emptyCredentials, '{}\n');
const integrationConfig = join(outDir, 'integration-config.json');
writeFileSync(integrationConfig, JSON.stringify({ tasks: [{ name: 'parity' }] }));
// A valid QuerySpec v2 segmentation definition (VE-2637). A workspace without
// this event can't compile it, so the metric stays a draft.
const metricDefinition = join(outDir, 'metric.query.json');
writeFileSync(
  metricDefinition,
  JSON.stringify({
    version: 2,
    id: 'cli-parity-metric',
    reportType: 'segmentation',
    entities: [{ id: 'events', kind: 'events', label: 'Events' }],
    measures: [{ id: 'A', kind: 'event', entityId: 'events', eventName: 'cli_parity_event', aggregation: 'count', filters: [] }],
    dimensions: [],
    filters: [],
    timeRange: { preset: 'last_30_days' },
    config: {},
    metricOutput: { kind: 'measure', measureId: 'A' },
  }),
);
const invalidMetricDefinition = join(outDir, 'invalid-metric.query.json');
writeFileSync(invalidMetricDefinition, JSON.stringify({ version: 2, reportType: 'segmentation' }));

// ── Masking: what legitimately differs between the two CLIs' resources ─────
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;
const SHORT_ID = /\b[0-9a-f]{8}\.\.\./g;
const ISO_TIME = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?/g;
const NAME = /CLI parity (TS|Rust)/g;

function mask(text) {
  return text
    .replace(UUID, '<id>')
    .replace(SHORT_ID, '<id>...')
    .replace(ISO_TIME, '<time>')
    .replace(NAME, 'CLI parity <cli>');
}

function maskJson(value) {
  if (Array.isArray(value)) return value.map(maskJson);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, val]) => [key, maskJson(val)]));
  }
  return typeof value === 'string' ? mask(value) : value;
}

function parseJson(text) {
  try {
    return { value: JSON.parse(text) };
  } catch (err) {
    return { error: `not JSON: ${err.message}` };
  }
}

// ── Steps ───────────────────────────────────────────────────────────────────
const results = [];

function compare(ts, rust, { json, keyCasing }) {
  // Error lines can name a resource or job ID of their own ("Sync already in progress: <id>").
  const diffs = diffErrors({ ...ts, stderr: mask(ts.stderr) }, { ...rust, stderr: mask(rust.stderr) });
  if (ts.code === rust.code) {
    if (json && ts.code === 0) {
      const a = parseJson(ts.stdout);
      const b = parseJson(rust.stdout);
      if (a.error || b.error) diffs.push({ path: 'stdout', kind: 'changed', a: a.error ?? 'JSON', b: b.error ?? 'JSON' });
      else diffs.push(...diffJson(maskJson(a.value), maskJson(keyCasing ? camelCaseKeysDeep(b.value) : b.value)));
    } else {
      diffs.push(...diffCells(mask(ts.stdout), mask(rust.stdout)));
    }
  }
  return diffs;
}

const TRANSIENT = /timed out|fetch failed|rate limit|error sending request/i;
const RATE_LIMITED = /rate limit/i;

/**
 * Run one CLI command. The API refuses a rate-limited call before the command
 * runs, so wait out the key's minute window and run it again. The test key
 * allows 60 requests a minute, shared by both CLIs.
 */
function runCli(cli, args) {
  let res = session.run(cli, args);
  for (let i = 0; i < 2 && RATE_LIMITED.test(failureLine(res.stderr) ?? ''); i++) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 61_000);
    res = session.run(cli, args);
  }
  return res;
}

/**
 * Staging flakes (timeouts, rate limits, a swallowed catalog hiccup) make one
 * CLI fail where the other worked. Rerun the side that failed once: a real
 * difference fails again. Both failing differently is retried only for a
 * transient-looking error.
 */
function flakySides(res) {
  const failed = CLIS.filter((cli) => res[cli].code !== 0);
  if (failed.length === 1) return failed;
  return failed.filter((cli) => TRANSIENT.test(failureLine(res[cli].stderr) ?? ''));
}

/**
 * Run `argsFor(cli)` with both CLIs and compare. `json` compares the parsed
 * output (Rust camelCased first when `keyCasing`), otherwise table cells.
 * `must` aborts the run (after cleanup) unless both CLIs exit 0.
 */
function step(name, argsFor, { json = false, keyCasing = false, must = false } = {}) {
  const res = Object.fromEntries(CLIS.map((cli) => [cli, runCli(cli, argsFor(cli))]));
  let diffs = compare(res.ts, res.rust, { json, keyCasing });
  const retried = [];
  if (diffs.length) {
    for (const cli of flakySides(res)) {
      retried.push(`${label(cli)} after: ${failureLine(res[cli].stderr)}`);
      res[cli] = runCli(cli, argsFor(cli));
    }
    if (retried.length) diffs = compare(res.ts, res.rust, { json, keyCasing });
  }
  const { ts, rust } = res;
  const outcome = diffs.length ? 'DIFFERS' : 'same';
  const exit = ts.code === rust.code ? `exit ${ts.code}` : `exit TS ${ts.code} / Rust ${rust.code}`;
  console.log(`[${outcome}] ${name} (${exit})${retried.length ? ` (retried ${retried.join('; ')})` : ''}`);
  for (const diff of diffs.slice(0, 6)) {
    console.log(`    ${diff.kind} ${diff.path}: TS ${JSON.stringify(diff.a)} → Rust ${JSON.stringify(diff.b)}`);
  }
  results.push({ name, outcome, diffs, retried, args: CLIS.map((cli) => argsFor(cli)), ts, rust });
  if (must && (ts.code !== 0 || rust.code !== 0)) {
    const why = CLIS.filter((cli) => res[cli].code !== 0).map((cli) => `${label(cli)}: ${failureLine(res[cli].stderr)}`);
    throw new Error(`"${name}" must succeed with both CLIs. ${why.join('; ')}`);
  }
  return res;
}

/** A check on staging state, e.g. that a dry run left the resource unchanged. */
function check(name, ok, detail) {
  console.log(`[${ok ? 'ok' : 'FAILED'}] check: ${name}${ok ? '' : ` (${detail})`}`);
  results.push({ name: `check: ${name}`, outcome: ok ? 'same' : 'DIFFERS', diffs: ok ? [] : [{ path: name, kind: 'check', a: detail }] });
}

function idsFrom(res, read = (r) => JSON.parse(r.stdout).data.id) {
  return Object.fromEntries(CLIS.map((cli) => [cli, read(res[cli])]));
}

/** Retry a cleanup command: staging sometimes times out or 500s for a moment. */
function runWithRetry(args, attempts = 3) {
  let res;
  for (let i = 0; i < attempts; i++) {
    res = runCli('rust', args);
    if (res.code === 0) return res;
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 3000);
  }
  return res;
}

/**
 * Delete every app this script names "CLI parity …" and its sources, sources
 * first (an app with active sources can't be deleted). Sweeping by name also
 * catches what a timed-out create made on the server without telling us.
 */
function sweep() {
  const listed = runWithRetry(['apps', 'list', '--limit', '100', '--json']);
  if (listed.code !== 0) return console.log(`cleanup: could not list apps: ${failureLine(listed.stderr)}`);
  const apps = JSON.parse(listed.stdout).data.filter((a) => /^CLI parity /.test(a.displayName));
  for (const a of apps) {
    const sources = runWithRetry(['sources', 'list', '--app', a.id, '--limit', '100', '--json']);
    for (const s of sources.code === 0 ? JSON.parse(sources.stdout).data : []) {
      const res = runWithRetry(['sources', 'delete', s.id, '--yes', '--json']);
      console.log(`cleanup: source ${s.id} ${res.code === 0 ? 'deleted' : `NOT deleted: ${failureLine(res.stderr)}`}`);
    }
    const res = runWithRetry(['apps', 'delete', a.id, '--yes', '--json']);
    console.log(`cleanup: app "${a.displayName}" ${res.code === 0 ? 'deleted' : `NOT deleted: ${failureLine(res.stderr)}`}`);
  }
}

function getJson(cli, args) {
  const res = runCli(cli, [...args, '--json']);
  return res.code === 0 ? JSON.parse(res.stdout) : null;
}

/**
 * `delete` without `--yes` on a TTY must ask and, on "n", delete nothing.
 * Runs under macOS/Linux `expect` to get a real terminal.
 */
function confirmOnTty(cli, kind, id) {
  const [cmd, ...prefix] = cli === 'ts' ? [process.execPath, join(root, 'dist', 'cli.js')] : [session.rustPath];
  const argv = [cmd, ...prefix, kind, 'delete', id].map((a) => `{${a}}`).join(' ');
  const script = `set timeout 60; spawn ${argv}; expect {
    "(y/N)" { send "n\\r"; expect eof; exit 0 }
    timeout { exit 3 }
    eof { exit 4 }
  }`;
  const res = spawnSync('expect', ['-c', script], { env: session.env, encoding: 'utf8', timeout: 90_000 });
  return { asked: res.status === 0, output: res.stdout ?? '', error: res.error?.message };
}

// ── Scenario: apps, sources and integration refusals (VE-3667) ─────────────
const name = (cli, suffix = '') => `CLI parity ${label(cli)}${suffix}`;

function pipelineScenario() {
  // Apps: one webhook app per CLI per output mode (webhook needs no credentials).
  let res = step(
    'apps create --json',
    (cli) => ['apps', 'create', '--type', 'webhook', '--name', name(cli), '--role', 'source', '--credentials-file', emptyCredentials, '--json'],
    { json: true, must: true },
  );
  const app = idsFrom(res);
  step(
    'apps create',
    (cli) => ['apps', 'create', '--type', 'webhook', '--name', name(cli, ' sources'), '--credentials-file', emptyCredentials],
    { must: true },
  );
  res = step(
    'apps create --output id',
    (cli) => ['apps', 'create', '--type', 'webhook', '--name', name(cli, ' field'), '--permissions', 'read_warehouse', '--credentials-file', emptyCredentials, '--output', 'id'],
    { must: true },
  );
  const fieldApp = idsFrom(res, (r) => r.stdout.trim());
  // The plain create prints a short ID only; find the app by name.
  const sourceApp = Object.fromEntries(
    CLIS.map((cli) => [cli, getJson('rust', ['apps', 'list', '--type', 'webhook', '--limit', '100']).data.find((a) => a.displayName === name(cli, ' sources'))?.id]),
  );
  if (!sourceApp.ts || !sourceApp.rust) throw new Error('could not find the apps created without --json');

  step('apps get', (cli) => ['apps', 'get', app[cli]]);
  step('apps get --json', (cli) => ['apps', 'get', app[cli], '--json'], { json: true, keyCasing: true });
  step('apps update --name --json', (cli) => ['apps', 'update', app[cli], '--name', name(cli, ' renamed'), '--json'], { json: true, must: true });
  step('apps update --role', (cli) => ['apps', 'update', app[cli], '--role', 'source'], { must: true });
  step('apps update (no flags)', (cli) => ['apps', 'update', app[cli]]);
  step('apps update --credentials-file (missing file)', (cli) => ['apps', 'update', app[cli], '--credentials-file', '/nonexistent/parity.json']);
  step('apps pause --dry-run', (cli) => ['apps', 'pause', app[cli], '--dry-run']);
  for (const cli of CLIS) {
    const state = getJson(cli, ['apps', 'get', app[cli]])?.data?.state;
    check(`apps pause --dry-run left the ${label(cli)} app active`, state === 'active', `state ${state}`);
  }
  step('apps pause', (cli) => ['apps', 'pause', app[cli]], { must: true });
  step('apps get (paused)', (cli) => ['apps', 'get', app[cli]]);
  // A webhook app resumes with no provider check (VE-3701). Pause it again for
  // the paused-app refusal under sources below.
  step('apps resume --json', (cli) => ['apps', 'resume', app[cli], '--json'], { json: true, must: true });
  step('apps pause (again)', (cli) => ['apps', 'pause', app[cli]], { must: true });
  step('apps pause --output id', (cli) => ['apps', 'pause', fieldApp[cli], '--output', 'id'], { must: true });
  step('apps resume', (cli) => ['apps', 'resume', fieldApp[cli]]);
  step('apps diagnose', () => ['apps', 'diagnose']);
  step('apps diagnose --json', () => ['apps', 'diagnose', '--json'], { json: true, keyCasing: true });
  step('apps create (unknown type)', () => ['apps', 'create', '--type', 'parity_missing_type', '--name', 'x', '--credentials-file', emptyCredentials]);
  step('apps create (missing credentials file)', () => ['apps', 'create', '--type', 'webhook', '--name', 'x', '--credentials-file', '/nonexistent/parity.json']);

  // Sources on each CLI's active webhook app.
  res = step(
    'sources create --json',
    (cli) => ['sources', 'create', '--app', sourceApp[cli], '--sync-type', 'webhook', '--json'],
    { json: true, must: true },
  );
  const source = idsFrom(res);
  res = step(
    'sources create --output id',
    (cli) => ['sources', 'create', '--app', sourceApp[cli], '--sync-type', 'webhook', '--output', 'id'],
    { must: true },
  );
  const fieldSource = idsFrom(res, (r) => r.stdout.trim());
  // A webhook source has no schedule, so create ignores the frequency and skips
  // the plan check (VE-3703). The plan refusal (VE-3702) needs a batch source.
  res = step(
    'sources create --frequency (ignored for a webhook source)',
    (cli) => ['sources', 'create', '--app', sourceApp[cli], '--sync-type', 'webhook', '--frequency', '1', '--output', 'id'],
    { must: true },
  );
  const ignoredFrequencySource = idsFrom(res, (r) => r.stdout.trim());
  step('sources create (sync type does not match the app)', (cli) => ['sources', 'create', '--app', sourceApp[cli], '--sync-type', 'shopify']);
  step('sources create (paused app)', (cli) => ['sources', 'create', '--app', app[cli], '--sync-type', 'webhook']);
  step('sources list --app', (cli) => ['sources', 'list', '--app', sourceApp[cli]]);
  step('sources list --app --json', (cli) => ['sources', 'list', '--app', sourceApp[cli], '--json'], { json: true, keyCasing: true });
  step('sources get', (cli) => ['sources', 'get', source[cli]]);
  step('sources get --json', (cli) => ['sources', 'get', source[cli], '--json'], { json: true, keyCasing: true });
  step('sources update --json', (cli) => ['sources', 'update', source[cli], '--import-tasks', 'events', '--json'], { json: true, keyCasing: true, must: true });
  step('sources update --import-tasks', (cli) => ['sources', 'update', source[cli], '--import-tasks', 'events']);
  // A webhook source refuses a schedule with a 400 (VE-3761).
  step('sources update --frequency (webhook source)', (cli) => ['sources', 'update', source[cli], '--frequency', '2', '--unit', 'days']);
  step('sources update --frequency --json (webhook source)', (cli) => ['sources', 'update', source[cli], '--frequency', '2', '--unit', 'days', '--json'], { json: true });
  step('sources update (no flags)', (cli) => ['sources', 'update', source[cli]]);
  // A dry run must start no job, so compare job counts around it.
  const jobCount = (cli) => getJson(cli, ['jobs', 'list', '--source', source[cli], '--limit', '100'])?.data?.length;
  const jobsBefore = Object.fromEntries(CLIS.map((cli) => [cli, jobCount(cli)]));
  step('sources sync --dry-run', (cli) => ['sources', 'sync', source[cli], '--dry-run']);
  for (const cli of CLIS) {
    const after = jobCount(cli);
    check(`sources sync --dry-run started no ${label(cli)} job`, after !== undefined && after === jobsBefore[cli], `${jobsBefore[cli]} jobs before, ${after} after`);
  }
  // A webhook source has no import, so sync is refused with a 400 (VE-3703): this compares the refusal, not a watched job.
  step('sources sync --watch', (cli) => ['sources', 'sync', source[cli], '--watch']);
  step('sources sync --json', (cli) => ['sources', 'sync', fieldSource[cli], '--json'], { json: true, keyCasing: true });
  step('sources pause --dry-run', (cli) => ['sources', 'pause', source[cli], '--dry-run']);
  step('sources pause', (cli) => ['sources', 'pause', source[cli]], { must: true });
  step('sources get (paused)', (cli) => ['sources', 'get', source[cli]]);
  step('sources resume --json', (cli) => ['sources', 'resume', source[cli], '--json'], { json: true, keyCasing: true });
  step('sources pause --output id', (cli) => ['sources', 'pause', fieldSource[cli], '--output', 'id']);
  step('sources resume', (cli) => ['sources', 'resume', fieldSource[cli]]);

  // Integrations: a disposable workspace has no destination app (every
  // destination type needs real credentials or OAuth), so only the refusals.
  step('integrations create (destination app has no destination role)', (cli) => [
    'integrations', 'create', '--dest-app', sourceApp[cli], '--data-type', 'events', '--config-file', integrationConfig,
  ]);
  step('integrations create (missing config file)', (cli) => [
    'int', 'create', '--dest-app', sourceApp[cli], '--data-type', 'events', '--config-file', '/nonexistent/parity.json',
  ]);
  step('integrations get (missing)', () => ['integrations', 'get', MISSING_ID]);
  step('integrations sync --dry-run (missing)', () => ['integrations', 'sync', MISSING_ID, '--dry-run']);
  step('integrations refresh-source (missing)', () => ['integrations', 'refresh-source', MISSING_ID, '--from', '2026-09-01']);
  step('integrations refresh-source (bad date)', () => ['integrations', 'refresh-source', MISSING_ID, '--from', 'not-a-date']);
  step('integrations update (no flags)', () => ['integrations', 'update', MISSING_ID]);

  // Deletes: confirmation on a TTY, dry run, then each output mode.
  for (const cli of CLIS) {
    const tty = confirmOnTty(cli, 'sources', fieldSource[cli]);
    const still = getJson(cli, ['sources', 'get', fieldSource[cli]])?.data?.id === fieldSource[cli];
    check(`${label(cli)} sources delete asks on a TTY and "n" keeps the source`, tty.asked && still, tty.error ?? `asked=${tty.asked} kept=${still} ${tty.output.slice(-200)}`);
  }
  step('sources delete --dry-run', (cli) => ['sources', 'delete', source[cli], '--dry-run']);
  for (const cli of CLIS) {
    const still = getJson(cli, ['sources', 'get', source[cli]])?.data?.id === source[cli];
    check(`sources delete --dry-run kept the ${label(cli)} source`, still, 'source is gone');
  }
  step('sources delete --yes --json', (cli) => ['sources', 'delete', source[cli], '--yes', '--json'], { json: true, keyCasing: true });
  step('sources delete --yes', (cli) => ['sources', 'delete', fieldSource[cli], '--yes']);
  // The app delete below needs no active source left (VE-3739).
  step('sources delete --yes --output id', (cli) => ['sources', 'delete', ignoredFrequencySource[cli], '--yes', '--output', 'id']);
  step('sources get (deleted)', (cli) => ['sources', 'get', source[cli]]);

  step('apps delete --dry-run', (cli) => ['apps', 'delete', app[cli], '--dry-run']);
  step('apps delete --yes --json', (cli) => ['apps', 'delete', app[cli], '--yes', '--json'], { json: true, keyCasing: true });
  step('apps delete --yes', (cli) => ['apps', 'delete', sourceApp[cli], '--yes']);
  step('apps delete --yes --output id', (cli) => ['apps', 'delete', fieldApp[cli], '--yes', '--output', 'id']);
}

// ── Scenario: metrics (VE-3668) ─────────────────────────────────────────────
// Each CLI creates its own draft metrics, then lists, reads, updates,
// activates and deletes them. A workspace that can't compile the definition
// refuses activation; both CLIs must report the refusal the same way.
function metricsScenario() {
  const res = step(
    'metrics create --json',
    (cli) => ['metrics', 'create', '--name', name(cli), '--definition', metricDefinition, '--json'],
    { json: true, must: true },
  );
  const metric = idsFrom(res);
  step(
    'metrics create --format --unit --description',
    (cli) => [
      'metrics', 'create', '--name', name(cli, ' plain'), '--definition', metricDefinition,
      '--format', 'currency', '--unit', '$', '--description', 'CLI parity check',
    ],
    { must: true },
  );
  const listed = getJson('rust', ['metrics', 'list', '--limit', '100']);
  const plainMetric = Object.fromEntries(CLIS.map((cli) => [cli, listed?.data?.find((m) => m.name === name(cli, ' plain'))?.id]));
  if (!plainMetric.ts || !plainMetric.rust) throw new Error('could not find the metrics created without --json');

  step('metrics create (invalid definition)', (cli) => ['metrics', 'create', '--name', name(cli, ' invalid'), '--definition', invalidMetricDefinition]);
  step('metrics create (missing definition file)', () => ['metrics', 'create', '--name', 'x', '--definition', '/nonexistent/parity.query.json']);
  step('metrics list', () => ['metrics', 'list', '--limit', '100']);
  step('metrics list --json', () => ['metrics', 'list', '--limit', '100', '--json'], { json: true });
  step('metrics list --status draft --output id', () => ['metrics', 'list', '--status', 'draft', '--limit', '100', '--output', 'id']);
  step('metrics get', (cli) => ['metrics', 'get', metric[cli]]);
  step('metrics get --json', (cli) => ['metrics', 'get', metric[cli], '--json'], { json: true });
  step('metrics get (missing)', () => ['metrics', 'get', MISSING_ID]);
  step(
    'metrics update --name --description --json',
    (cli) => ['metrics', 'update', metric[cli], '--name', name(cli, ' renamed'), '--description', 'updated by parity', '--json'],
    { json: true, must: true },
  );
  step(
    'metrics update --definition --format',
    (cli) => ['metrics', 'update', metric[cli], '--definition', metricDefinition, '--format', 'percentage'],
    { must: true },
  );
  step('metrics update (no flags)', (cli) => ['metrics', 'update', metric[cli]]);
  step('metrics update (missing definition file)', (cli) => ['metrics', 'update', metric[cli], '--definition', '/nonexistent/parity.query.json']);
  step('metrics activate', (cli) => ['metrics', 'activate', metric[cli]]);
  step('metrics activate --json', (cli) => ['metrics', 'activate', plainMetric[cli], '--json'], { json: true });
  step('metrics activate (missing)', () => ['metrics', 'activate', MISSING_ID]);
  step('metrics update --status archived --json', (cli) => ['metrics', 'update', plainMetric[cli], '--status', 'archived', '--json'], { json: true });
  step('metrics list --status archived', () => ['metrics', 'list', '--status', 'archived', '--limit', '100']);
  for (const cli of CLIS) {
    const tty = confirmOnTty(cli, 'metrics', metric[cli]);
    const still = getJson(cli, ['metrics', 'get', metric[cli]])?.data?.id === metric[cli];
    const cancelled = /Cancelled/.test(tty.output);
    check(
      `${label(cli)} metrics delete asks on a TTY and "n" keeps the metric`,
      tty.asked && still && cancelled,
      tty.error ?? `asked=${tty.asked} kept=${still} cancelled=${cancelled} ${tty.output.slice(-200)}`,
    );
  }
  step('metrics delete --yes', (cli) => ['metrics', 'delete', metric[cli], '--yes']);
  step('metrics delete --json', (cli) => ['metrics', 'delete', plainMetric[cli], '--json'], { json: true });
  step('metrics get (deleted)', (cli) => ['metrics', 'get', metric[cli]]);
  step('metrics delete (missing)', () => ['metrics', 'delete', MISSING_ID, '--yes']);
}

/** Delete every metric named "CLI parity …", archived ones included. */
function sweepMetrics() {
  for (const status of [[], ['--status', 'archived']]) {
    const listed = runWithRetry(['metrics', 'list', '--limit', '100', ...status, '--json']);
    if (listed.code !== 0) {
      console.log(`cleanup: could not list metrics: ${failureLine(listed.stderr)}`);
      continue;
    }
    for (const m of JSON.parse(listed.stdout).data.filter((m) => /^CLI parity /.test(m.name))) {
      const res = runWithRetry(['metrics', 'delete', m.id, '--yes', '--json']);
      console.log(`cleanup: metric "${m.name}" ${res.code === 0 ? 'deleted' : `NOT deleted: ${failureLine(res.stderr)}`}`);
    }
  }
}

// ── Run: each scenario cleans up after itself, also when a step fails ──────
const scenarios = [
  { name: 'pipeline', run: pipelineScenario, cleanup: sweep },
  { name: 'metrics', run: metricsScenario, cleanup: sweepMetrics },
];
const failures = [];
try {
  for (const scenario of scenarios.filter((s) => !opts.only || s.name === opts.only)) {
    try {
      scenario.run();
    } catch (err) {
      failures.push(`${scenario.name}: ${err.message}`);
      console.error(`\nStopped ${scenario.name}: ${err.message}`);
    } finally {
      scenario.cleanup();
    }
  }
} finally {
  session.cleanup();
}

// ── Report ──────────────────────────────────────────────────────────────────
const differs = results.filter((r) => r.outcome !== 'same');
writeFileSync(join(outDir, 'results.json'), JSON.stringify({ profile: opts.profile, baseUrl: session.baseUrl, results }, null, 2));
const lines = [
  `# CLI write parity, ${new Date().toISOString().slice(0, 16).replace('T', ' ')} UTC`,
  '',
  `- Profile \`${opts.profile}\` on ${session.baseUrl}`,
  `- ${results.length} steps and checks; ${differs.length} differ or failed${failures.length ? `; stopped early: ${failures.join('; ')}` : ''}`,
  '',
  '| Step | Result |',
  '|---|---|',
  ...results.map((r) => `| ${r.name} | ${r.outcome}${r.diffs.length ? `: ${r.diffs.map((d) => d.path).join(', ')}` : ''}${r.retried?.length ? ` (retried ${r.retried.join('; ')})` : ''} |`),
  '',
];
writeFileSync(join(outDir, 'report.md'), lines.join('\n'));
console.log(`\n${results.length - differs.length}/${results.length} same${failures.length ? ', STOPPED EARLY' : ''}. Report: ${join(outDir, 'report.md')}`);
process.exit(failures.length || differs.length ? 1 : 0);

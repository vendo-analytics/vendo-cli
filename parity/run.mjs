// Parity harness (VE-3664): run every read-only command against staging with
// the TypeScript CLI and the Rust CLI, and report where their output differs
// plus which commands already fail on staging today.
//
//   pnpm build && pnpm parity --profile <staging profile> [--rust <binary>] [--only <prefix>] [--keep]
//
// Safety: only commands classified `read` in parity/commands.json run; the
// chosen profile is copied into a throwaway HOME (VENDO_* env cleared); any
// base URL other than staging/localhost is refused.
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';

import {
  MISSING_ID,
  camelCaseKeysDeep,
  diffCells,
  diffErrors,
  diffFlags,
  diffJson,
  failureLine,
  filterIntended,
  getPath,
  isNotPorted,
  validateClassification,
} from './lib.mjs';
import { createSession, fail, root } from './session.mjs';

const { values: opts } = parseArgs({
  options: {
    profile: { type: 'string' },
    rust: { type: 'string' },
    only: { type: 'string' },
    keep: { type: 'boolean', default: false },
  },
});

// ── Setup: profile, guard, binaries, classification ─────────────────────────
const { baseUrl, rustPath, hasRust, home, run, cleanup } = createSession(opts);

const discovery = spawnSync(process.execPath, [join(root, 'parity', 'discover.mjs'), root], { encoding: 'utf8' });
if (discovery.status !== 0) fail(`command discovery failed:\n${discovery.stderr}`);
const discovered = JSON.parse(discovery.stdout);
const { commands } = JSON.parse(readFileSync(join(root, 'parity', 'commands.json'), 'utf8'));
const intended = JSON.parse(readFileSync(join(root, 'parity', 'intended-differences.json'), 'utf8'));
const intendedPaths = intended.paths;
const intendedRows = intended.rows ?? [];
const intendedKeyCasing = intended.keyCasing ?? [];
const problems = validateClassification(discovered.map((leaf) => leaf.path), commands);
if (problems.length > 0) fail(`parity/commands.json is out of date:\n  ${problems.join('\n  ')}`);
process.on('SIGINT', () => {
  cleanup();
  process.exit(130);
});

// ── Arguments and variants ──────────────────────────────────────────────────
const sources = new Map();
function resolveArgs(entry) {
  const args = [];
  for (const spec of entry.args ?? []) {
    if ('value' in spec) {
      args.push(spec.value);
      continue;
    }
    if (!sources.has(spec.from)) {
      const res = run('ts', [...spec.from.split(' '), '--json']);
      let body = null;
      try {
        body = res.code === 0 ? JSON.parse(res.stdout) : null;
      } catch {
        // Not JSON: treated as a failed source below.
      }
      sources.set(spec.from, { body, error: failureLine(res.stderr) ?? `exit ${res.code}` });
    }
    const source = sources.get(spec.from);
    const value = source.body ? getPath(source.body, spec.path) : undefined;
    if (value != null) args.push(String(value));
    else if (spec.fallback) args.push(spec.fallback);
    else {
      return {
        skip: source.body
          ? `no staging data: \`${spec.from}\` has nothing at ${spec.path}`
          : `\`${spec.from}\` fails: ${source.error}`,
      };
    }
  }
  return { args };
}

function variantsFor(leaf, entry, args) {
  const base = leaf.path.split(' ');
  const flags = entry.flags ?? [];
  if (entry.argSets) {
    return entry.argSets.map((set) => ({ name: set.join(' '), argv: [...base, ...set], compare: entry.compare ?? 'cells' }));
  }
  const variants = [];
  if (leaf.options.includes('--json')) variants.push({ name: 'json', argv: [...base, ...args, ...flags, '--json'], compare: 'json' });
  if (leaf.options.includes('--output')) {
    variants.push({ name: 'output', argv: [...base, ...args, ...flags, '--output', entry.outputField ?? 'id'], compare: 'cells' });
  }
  variants.push({ name: 'plain', argv: [...base, ...args, ...flags], compare: entry.compare ?? 'cells' });
  if (entry.notFound) {
    const missing = entry.notFound === true ? MISSING_ID : entry.notFound;
    variants.push({ name: 'not-found', argv: [...base, missing, ...args.slice(1), ...flags], compare: 'cells' });
  }
  return variants;
}

function compare(variant, ts, rust, command) {
  if (variant.compare === 'exit') {
    const same = ts.code === rust.code && Boolean(ts.stdout.trim()) === Boolean(rust.stdout.trim());
    return same ? [] : [{ path: 'exit code / output', kind: 'changed', a: ts.code, b: rust.code }];
  }
  if (ts.code !== 0 || rust.code !== 0) {
    const diffs = diffErrors(ts, rust);
    // Same failing exit code with output (doctor exits 1 on a failed check):
    // the output still has to match.
    if (diffs.length > 0 || !(ts.stdout.trim() || rust.stdout.trim())) return diffs;
  }
  if (variant.compare === 'json') {
    let a;
    let b;
    try {
      a = JSON.parse(ts.stdout);
    } catch {
      return [{ path: '(stdout)', kind: 'changed', a: 'TypeScript printed non-JSON' }];
    }
    try {
      b = JSON.parse(rust.stdout);
    } catch {
      return [{ path: '(stdout)', kind: 'changed', b: 'Rust printed non-JSON' }];
    }
    const casing = intendedKeyCasing.some((rule) => rule.command === command && (!rule.variant || rule.variant === variant.name));
    return filterIntended(diffJson(a, casing ? camelCaseKeysDeep(b) : b), intendedPaths, command, variant.name);
  }
  const rowRules = intendedRows.filter((rule) => rule.command === command && (!rule.variant || rule.variant === variant.name));
  return diffCells(ts.stdout, rust.stdout, rowRules);
}

// ── Run ─────────────────────────────────────────────────────────────────────
const results = [];
const selected = discovered.filter((leaf) => !opts.only || leaf.path.startsWith(opts.only));
try {
  for (const [index, leaf] of selected.entries()) {
    const entry = commands[leaf.path];
    const result = {
      command: leaf.path,
      class: entry.class,
      envDependent: entry.envDependent,
      ported: false,
      flagDiffs: [],
      variants: [],
    };

    if (hasRust) {
      const help = run('rust', [...leaf.path.split(' '), '--help']);
      result.ported = !isNotPorted(help);
      if (result.ported) result.flagDiffs = diffFlags(leaf.options, help.stdout);
    }

    if (entry.class === 'read') {
      const resolved = resolveArgs(entry);
      if (resolved.skip) result.skipped = resolved.skip;
      else {
        for (const variant of variantsFor(leaf, entry, resolved.args)) {
          const expectSuccess = variant.name !== 'not-found';
          let ts = run('ts', variant.argv);
          let rust = result.ported ? run('rust', variant.argv) : null;
          let diffs = rust ? compare(variant, ts, rust, leaf.path) : [];
          if (diffs.length > 0 || (expectSuccess && ts.code !== 0)) {
            // Data can move between two runs ("5m ago") and staging can time
            // out: re-run once before reporting a difference or a failure.
            ts = run('ts', variant.argv);
            rust = result.ported ? run('rust', variant.argv) : null;
            diffs = rust ? compare(variant, ts, rust, leaf.path) : [];
          }
          result.variants.push({
            name: variant.name,
            argv: variant.argv.join(' '),
            ts: { code: ts.code, error: failureLine(ts.stderr) },
            rust: rust && { code: rust.code, error: failureLine(rust.stderr) },
            diffs,
          });
        }
      }
    }

    results.push(result);
    process.stderr.write(`[${index + 1}/${selected.length}] ${leaf.path}: ${statusOf(result)}\n`);
  }
} finally {
  cleanup();
}

function statusOf(result) {
  if (result.class !== 'read') return `${result.class} (not run)${result.flagDiffs.length ? ', flag differences' : ''}`;
  if (result.skipped) return `skipped (${result.skipped})`;
  const tsFails = !result.envDependent && result.variants.some((v) => v.name !== 'not-found' && v.ts.code !== 0);
  const rust = !hasRust ? 'no Rust binary' : !result.ported ? 'not ported' : hasDefect(result) ? 'DIFFERS' : 'same';
  return `${tsFails ? 'fails on staging, ' : ''}${rust}`;
}

function hasDefect(result) {
  return result.flagDiffs.length > 0 || result.variants.some((v) => v.diffs.length > 0);
}

// ── Report ──────────────────────────────────────────────────────────────────
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const outDir = join(root, 'parity', 'out', stamp);
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, 'results.json'), JSON.stringify({ profile: opts.profile, baseUrl, rustPath: hasRust ? rustPath : null, results }, null, 2));

const tsVersion = run('ts', ['--version']).stdout.trim();
const rustVersion = hasRust ? run('rust', ['--version']).stdout.trim() : null;
const reads = results.filter((r) => r.class === 'read');
const failing = reads.filter((r) => !r.envDependent).flatMap((r) =>
  r.variants.filter((v) => v.name !== 'not-found' && v.ts.code !== 0).map((v) => ({ command: r.command, ...v })),
);
const defects = results.filter(hasDefect);
const same = reads.filter((r) => r.ported && !r.skipped && !hasDefect(r));
const notPorted = results.filter((r) => hasRust && !r.ported);
const skipped = reads.filter((r) => r.skipped);
const cell = (value) => String(value ?? '').replace(/\|/g, '\\|').replace(/\n/g, ' ').slice(0, 160);
const show = (value) => (value === undefined ? '∅' : JSON.stringify(value));

const lines = [
  `# CLI parity report, ${new Date().toISOString().slice(0, 16).replace('T', ' ')} UTC`,
  '',
  `- Profile \`${opts.profile}\` on ${baseUrl}`,
  `- TypeScript: \`node dist/cli.js\` ${tsVersion}`,
  `- Rust: ${hasRust ? `\`${rustPath}\` ${rustVersion}` : 'no binary, so this is a TypeScript-only run'}`,
  `- Commands: ${results.length} checked (${reads.length} read, ${results.length - reads.length} write or interactive and not run)`,
  '',
  `## Fails on staging today with TypeScript (${failing.length})`,
  '',
  ...(failing.length
    ? ['| Command | Run | Exit | Error |', '|---|---|---|---|', ...failing.map((f) => `| ${f.command} | \`${cell(f.argv)}\` | ${f.ts.code} | ${cell(f.ts.error)} |`)]
    : ['None.']),
  '',
  ...reads
    .filter((r) => r.envDependent)
    .map((r) => `Not counted as failing: \`${r.command}\`. ${r.envDependent}`),
  '',
  `## Rust differences (${defects.length})`,
  '',
  ...(defects.length
    ? defects.flatMap((r) => [
        `### ${r.command}`,
        ...r.flagDiffs.map((d) => `- flag ${d.path} is ${d.kind} in Rust`),
        ...r.variants
          .filter((v) => v.diffs.length)
          .flatMap((v) => [
            `- \`${v.argv}\``,
            ...v.diffs.slice(0, 8).map((d) => `  - ${d.kind} \`${d.path}\`: TS ${cell(show(d.a))} → Rust ${cell(show(d.b))}`),
            ...(v.diffs.length > 8 ? [`  - …and ${v.diffs.length - 8} more (results.json)`] : []),
          ]),
      ])
    : ['None.']),
  '',
  `## Same in both (${same.length})`,
  '',
  same.length ? same.map((r) => `\`${r.command}\``).join(', ') : 'None.',
  '',
  `## Not ported to Rust yet (${notPorted.length})`,
  '',
  notPorted.length ? notPorted.map((r) => `\`${r.command}\``).join(', ') : hasRust ? 'None.' : 'No Rust binary.',
  '',
  `## Skipped (${skipped.length})`,
  '',
  ...(skipped.length ? skipped.map((r) => `- \`${r.command}\`: ${r.skipped}`) : ['None.']),
  '',
];
writeFileSync(join(outDir, 'report.md'), lines.join('\n'));
console.log(`Report: ${join(outDir, 'report.md')}`);
if (opts.keep) console.log(`Kept isolated HOME (contains a copy of the API key): ${home}`);
process.exit(defects.length > 0 ? 1 : 0);

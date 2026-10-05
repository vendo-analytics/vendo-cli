// Pure helpers for the parity harness (VE-3664). No I/O here, so every rule
// that decides "same or different" is covered by parity/lib.test.mjs.

export const CLASSES = new Set(['read', 'write', 'interactive']);

/** Hosts the harness may talk to. There is deliberately no override. */
const SAFE_HOSTS = new Set(['stg.vendodata.com', 'localhost', '127.0.0.1']);

/** A well-formed UUID that never exists, for the not-found variant. */
export const MISSING_ID = '00000000-0000-4000-8000-000000000000';

/** Root options every command accepts; never counted as a flag difference. */
const GLOBAL_FLAGS = new Set(['--profile', '--debug', '--help', '--version']);

// The TS CLI falls back to the prod URL when a profile has no baseUrl.
const DEFAULT_BASE_URL = 'https://app2.vendodata.com';

export function assertSafeBaseUrl(baseUrl) {
  const raw = baseUrl ?? DEFAULT_BASE_URL;
  let host;
  try {
    host = new URL(raw).hostname;
  } catch {
    throw new Error(`Refusing to run: "${raw}" is not a valid URL.`);
  }
  if (!SAFE_HOSTS.has(host)) {
    throw new Error(
      `Refusing to run against ${raw}. The parity harness only talks to ` +
        `staging or localhost (${[...SAFE_HOSTS].join(', ')}).`,
    );
  }
  return raw;
}

/**
 * Every discovered command must be classified, and every classified command
 * must still exist. Returns a list of problems; empty means valid.
 */
export function validateClassification(discoveredPaths, commands) {
  const problems = [];
  const discovered = new Set(discoveredPaths);
  for (const path of discoveredPaths) {
    if (!commands[path]) problems.push(`unclassified command: "${path}"`);
  }
  for (const [path, entry] of Object.entries(commands)) {
    if (!discovered.has(path)) {
      problems.push(`classified command no longer exists: "${path}"`);
    } else if (!CLASSES.has(entry.class)) {
      problems.push(`"${path}" has unknown class "${entry.class}"`);
    }
  }
  return problems;
}

/** Read `data[0].id` / `data.methodologies[0].id` style paths. */
export function getPath(value, path) {
  let current = value;
  for (const part of path.match(/[^.[\]]+/g) ?? []) {
    if (current == null) return undefined;
    current = current[/^\d+$/.test(part) ? Number(part) : part];
  }
  return current;
}

/** Deep JSON diff; key order is ignored, array order is not. */
export function diffJson(a, b, path = '') {
  if (Object.is(a, b)) return [];
  const isObj = (v) => v !== null && typeof v === 'object';
  if (Array.isArray(a) && Array.isArray(b)) {
    const diffs = [];
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      if (i >= a.length) diffs.push({ path: `${path}[${i}]`, kind: 'extra', b: b[i] });
      else if (i >= b.length) diffs.push({ path: `${path}[${i}]`, kind: 'missing', a: a[i] });
      else diffs.push(...diffJson(a[i], b[i], `${path}[${i}]`));
    }
    return diffs;
  }
  if (isObj(a) && isObj(b) && !Array.isArray(a) && !Array.isArray(b)) {
    const diffs = [];
    for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
      const child = path ? `${path}.${key}` : key;
      if (!(key in b)) diffs.push({ path: child, kind: 'missing', a: a[key] });
      else if (!(key in a)) diffs.push({ path: child, kind: 'extra', b: b[key] });
      else diffs.push(...diffJson(a[key], b[key], child));
    }
    return diffs;
  }
  return [{ path: path || '(root)', kind: 'changed', a, b }];
}

/**
 * The TypeScript client's `toCamelCaseDeep`. An intended `keyCasing` rule
 * applies it to the Rust output before comparing, so the only difference
 * allowed is snake_case vs camelCase keys (values must still match).
 */
export function camelCaseKeysDeep(value) {
  if (Array.isArray(value)) return value.map(camelCaseKeysDeep);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, val]) => [
        key.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase()),
        camelCaseKeysDeep(val),
      ]),
    );
  }
  return value;
}

/** `data[*].category` matches `data[3].category`. */
export function pathMatches(pattern, path) {
  const escaped = pattern
    .split('[*]')
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('\\[\\d+\\]');
  return new RegExp(`^${escaped}$`).test(path);
}

export function filterIntended(diffs, intendedPaths, command, variant) {
  const rules = intendedPaths.filter(
    (rule) => rule.command === command && (!rule.variant || rule.variant === variant),
  );
  return diffs.filter((diff) => !rules.some((rule) => pathMatches(rule.path, diff.path)));
}

/** Table/text output as rows of cells; column gap width does not matter. */
export function toCells(text) {
  return stripAnsi(text)
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => line.split(/\s{2,}/));
}

/**
 * Rows whose text matches an intended `rowPattern` for this command/variant
 * (e.g. doctor's "CLI binary" line, which names the running executable) are
 * dropped from both sides before comparing.
 */
export function diffCells(a, b, rowRules = []) {
  const patterns = rowRules.map((rule) => new RegExp(rule.rowPattern));
  const keep = (row) => !patterns.some((pattern) => pattern.test(row.join('  ')));
  const rowsA = toCells(a).filter(keep);
  const rowsB = toCells(b).filter(keep);
  const diffs = [];
  for (let i = 0; i < Math.max(rowsA.length, rowsB.length); i++) {
    const left = rowsA[i];
    const right = rowsB[i];
    const same =
      left && right && left.length === right.length && left.every((cell, c) => sameCell(cell, right[c]));
    if (!same) diffs.push({ path: `row ${i + 1}`, kind: 'changed', a: left, b: right });
  }
  return diffs;
}

/**
 * Cells match exactly, or are clock readings one tick apart: the two CLIs
 * run a second or so apart, so "24m ago"/"25m ago" or a running job's
 * "6s"/"7s" are the same answer. Formatting itself is unit-tested.
 */
export function sameCell(a, b) {
  if (a === b) return true;
  const ago = (s) => {
    if (s === 'just now') return { n: 0, unit: 'm' };
    const m = s.match(/^(\d+)([mhd]) ago$/);
    return m ? { n: Number(m[1]), unit: m[2] } : null;
  };
  const [x, y] = [ago(a), ago(b)];
  if (x && y) return (x.unit === y.unit || x.n === 0 || y.n === 0) && Math.abs(x.n - y.n) <= 1;
  const duration = (s) => {
    const m = s.match(/^(?:(\d+)h)? ?(?:(\d+)m)? ?(?:(\d+)s)?$/);
    if (!m || s === '' || !(m[1] || m[2] || m[3])) return null;
    const step = m[3] !== undefined ? 1 : m[2] !== undefined ? 60 : 3600;
    return { seconds: Number(m[1] ?? 0) * 3600 + Number(m[2] ?? 0) * 60 + Number(m[3] ?? 0), step };
  };
  const [d1, d2] = [duration(a), duration(b)];
  return Boolean(d1 && d2) && Math.abs(d1.seconds - d2.seconds) <= Math.max(d1.step, d2.step);
}

/** The user-facing error line, with request IDs removed. */
export function errorLine(stderr) {
  const line = stripAnsi(stderr)
    .split('\n')
    .find((l) => /^error:/i.test(l.trim()));
  return line?.trim().replace(/\bcli-[0-9a-f-]{36}\b/g, '<request-id>');
}

/** For the report: the error line, else a crash line (TypeError…), else the last line. */
export function failureLine(stderr) {
  const lines = stripAnsi(stderr)
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
  return errorLine(stderr) ?? lines.find((l) => /^\w*Error\b/.test(l)) ?? lines.at(-1);
}

export function diffErrors(a, b) {
  const diffs = [];
  if (a.code !== b.code) diffs.push({ path: 'exit code', kind: 'changed', a: a.code, b: b.code });
  if (a.code !== 0 || b.code !== 0) {
    const left = errorLine(a.stderr);
    const right = errorLine(b.stderr);
    if (left !== right) diffs.push({ path: 'error', kind: 'changed', a: left, b: right });
  }
  return diffs;
}

/** clap reports an unknown subcommand with exit 2; that means "not ported yet". */
export function isNotPorted(result) {
  return result.code === 2 && /unrecognized subcommand/i.test(result.stderr);
}

/**
 * Long flags listed in a help screen's `Options:` section (not flags quoted in
 * descriptions or examples), minus the root options every command has.
 */
export function helpFlags(helpText) {
  const flags = new Set();
  let inOptions = false;
  for (const line of stripAnsi(helpText).split('\n')) {
    if (/^Options:/.test(line)) {
      inOptions = true;
      continue;
    }
    if (inOptions && line.trim() && !/^\s/.test(line)) inOptions = false;
    const match = inOptions && line.match(/^\s+(?:-\w,\s+)?(--[a-z][a-z0-9-]*)/);
    if (match && !GLOBAL_FLAGS.has(match[1])) flags.add(match[1]);
  }
  return flags;
}

export function diffFlags(tsOptions, rustHelp) {
  const ts = new Set(tsOptions.filter((flag) => !GLOBAL_FLAGS.has(flag)));
  const rust = helpFlags(rustHelp);
  const diffs = [];
  for (const flag of ts) if (!rust.has(flag)) diffs.push({ path: flag, kind: 'missing' });
  for (const flag of rust) if (!ts.has(flag)) diffs.push({ path: flag, kind: 'extra' });
  return diffs;
}

export function stripAnsi(text) {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\u001b\[[0-9;?]*[A-Za-z]/g, '');
}

// ── Help parity (VE-3713) ───────────────────────────────────────────────────
// `--help` text from commander (TS) and clap (Rust) parsed into what users rely
// on: description, positional arguments, options (short, long, value name,
// description, default), subcommands (with aliases) and examples. Layout and
// wording of the standard parts (`-h, --help`, `-V, --version`) are accepted
// differences; the root's --profile and --debug are compared, and on other
// commands they are clap's global options that commander lists only on the root.

const HELP_SECTIONS = /^(Usage|Options|Commands|Examples|Arguments):\s*(.*)$/;
const ALWAYS_IGNORED = new Set(['--help', '--version']);
const ROOT_ONLY = new Set(['--profile', '--debug']);

function helpSections(text) {
  const out = { head: [] };
  let current = 'head';
  for (const line of stripAnsi(text).split('\n')) {
    const match = line.match(HELP_SECTIONS);
    if (match) {
      current = match[1];
      out[current] = match[2] ? [match[2]] : [];
      continue;
    }
    (out[current] ??= []).push(line);
  }
  return out;
}

/** Items of an Options/Commands section; wrapped continuation lines are joined. */
function helpEntries(lines = []) {
  const items = [];
  let itemIndent = null;
  for (const line of lines) {
    if (!line.trim()) continue;
    const indent = line.match(/^ */)[0].length;
    if (itemIndent === null || indent <= itemIndent + 4) {
      itemIndent ??= indent;
      items.push(line.trim());
    } else {
      items[items.length - 1] += ` ${line.trim()}`;
    }
  }
  return items;
}

function parseOption(entry) {
  const match = entry.match(/^(?:(-\w),\s+)?(--[\w-]+)(?:\s+<([^>]+)>)?(?:\s+(.*))?$/);
  if (!match) return null;
  const [, short, long, value, rest] = match;
  let description = (rest ?? '').trim();
  let defaultValue = null;
  const commander = description.match(/\s*\(default: (.*)\)$/);
  const clap = description.match(/\s*\[default: ([^\]]*)\]$/);
  if (commander) {
    defaultValue = commander[1].replace(/^"(.*)"$/, '$1');
    description = description.slice(0, commander.index);
  } else if (clap) {
    defaultValue = clap[1];
    description = description.slice(0, clap.index);
  }
  return { long, short: short ?? null, value: value ?? null, description: description.trim(), default: defaultValue };
}

function parseUsageArgs(usage) {
  // Drop `vendo <path…>`, then option placeholders and `--flag <value>` pairs.
  const tokens = usage.trim().split(/\s+/).slice(1);
  const args = [];
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i];
    if (token.startsWith('-')) {
      if (tokens[i + 1]?.startsWith('<')) i++;
      continue;
    }
    const match = token.match(/^(<|\[)([^>\]]+)[>\]](\.\.\.)?$/);
    if (!match || /^(options|command)$/i.test(match[2])) continue;
    args.push(`${match[1] === '<' ? '<' : '['}${match[2]}${match[1] === '<' ? '>' : ']'}`);
  }
  return args;
}

function parseCommand(entry, cli) {
  if (cli === 'ts') {
    const match = entry.match(/^([\w-]+)((?:\|[\w-]+)*)((?:\s+(?:\[[^\]]+\]|<[^>]+>))*)\s+(.*)$/);
    if (!match) return null;
    const aliases = match[2] ? match[2].split('|').filter(Boolean) : [];
    return { name: match[1], aliases, description: match[4].trim() };
  }
  const match = entry.match(/^([\w-]+)\s+(.*)$/);
  if (!match) return null;
  let description = match[2].trim();
  let aliases = [];
  const alias = description.match(/\s*\[alias(?:es)?: ([^\]]*)\]$/);
  if (alias) {
    aliases = alias[1].split(',').map((a) => a.trim());
    description = description.slice(0, alias.index);
  }
  return { name: match[1], aliases, description: description.trim() };
}

/**
 * One `--help` screen as data. `cli` is `ts` (commander: Usage first, then the
 * description) or `rust` (clap: description first).
 */
export function parseHelp(text, cli, { root = false } = {}) {
  const sections = helpSections(text);
  const usage = (sections.Usage?.[0] ?? '').trim();
  const lines = (list) => (list ?? []).map((l) => l.trim()).filter(Boolean);
  const description = (cli === 'ts' ? lines(sections.Usage).slice(1) : lines(sections.head)).join(' ');
  const ignored = (long) => ALWAYS_IGNORED.has(long) || (!root && ROOT_ONLY.has(long));
  const options = helpEntries(sections.Options)
    .map(parseOption)
    .filter((option) => option && !ignored(option.long));
  const commands = helpEntries(sections.Commands)
    .map((entry) => parseCommand(entry, cli))
    .filter((command) => command && command.name !== 'help');
  return { description, arguments: parseUsageArgs(usage), options, commands, examples: lines(sections.Examples) };
}

/** What differs between two parsed help screens; empty means the same. */
export function diffHelp(ts, rust) {
  const diffs = [];
  const show = (value) => JSON.stringify(value);
  if (ts.description !== rust.description) diffs.push({ path: 'description', a: ts.description, b: rust.description });
  if (show(ts.arguments) !== show(rust.arguments)) diffs.push({ path: 'arguments', a: ts.arguments, b: rust.arguments });
  const byLong = (options) => new Map(options.map((option) => [option.long, option]));
  const [a, b] = [byLong(ts.options), byLong(rust.options)];
  for (const long of new Set([...a.keys(), ...b.keys()])) {
    if (show(a.get(long)) !== show(b.get(long))) diffs.push({ path: `option ${long}`, a: a.get(long), b: b.get(long) });
  }
  const order = (options) => options.map((option) => option.long);
  if (diffs.every((d) => !d.path.startsWith('option')) && show(order(ts.options)) !== show(order(rust.options))) {
    diffs.push({ path: 'option order', a: order(ts.options), b: order(rust.options) });
  }
  if (show(ts.commands) !== show(rust.commands)) diffs.push({ path: 'subcommands', a: ts.commands, b: rust.commands });
  if (show(ts.examples) !== show(rust.examples)) diffs.push({ path: 'examples', a: ts.examples, b: rust.examples });
  return diffs;
}

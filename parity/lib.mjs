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

export function diffCells(a, b) {
  const rowsA = toCells(a);
  const rowsB = toCells(b);
  const diffs = [];
  for (let i = 0; i < Math.max(rowsA.length, rowsB.length); i++) {
    const left = JSON.stringify(rowsA[i] ?? null);
    const right = JSON.stringify(rowsB[i] ?? null);
    if (left !== right) diffs.push({ path: `row ${i + 1}`, kind: 'changed', a: rowsA[i], b: rowsB[i] });
  }
  return diffs;
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

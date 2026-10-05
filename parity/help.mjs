// Help parity (VE-3713): compare `--help` for every command, groups and the
// root included, between the TypeScript CLI and the Rust CLI: description,
// arguments, options (short, long, value name, description, default),
// subcommands with their aliases, and examples. clap's layout and its wording
// for -h/--help and -V/--version are accepted (Yalcin, 2026-10-04).
//
//   pnpm build && pnpm parity:help [--rust <binary>] [--only <prefix>]
//
// Offline: no profile and no network. Both CLIs run in an empty throwaway
// HOME with VENDO_* cleared, so nothing reads or writes the real config.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

import { diffHelp, isNotPorted, parseHelp } from './lib.mjs';
import { fail, root } from './session.mjs';

const { values: opts } = parseArgs({
  options: {
    rust: { type: 'string' },
    only: { type: 'string' },
  },
});

if (!existsSync(join(root, 'dist', 'cli.js'))) fail('dist/cli.js is missing. Run `pnpm build` first.');
const rustPath = opts.rust ? resolve(opts.rust) : join(root, 'rust', 'target', 'release', 'vendo');
if (!existsSync(rustPath)) fail(`Rust binary not found at ${rustPath}. Pass --rust <binary>.`);

const discovery = spawnSync(process.execPath, [join(root, 'parity', 'discover.mjs'), root, '--all'], { encoding: 'utf8' });
if (discovery.status !== 0) fail(`command discovery failed:\n${discovery.stderr}`);
const paths = JSON.parse(discovery.stdout).filter((path) => !opts.only || path.startsWith(opts.only));

const home = mkdtempSync(join(tmpdir(), 'vendo-help-'));
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('VENDO_')));
Object.assign(env, { HOME: home, NO_COLOR: '1' });

function help(cli, path) {
  const [cmd, ...prefix] = cli === 'ts' ? [process.execPath, join(root, 'dist', 'cli.js')] : [rustPath];
  const args = [...(path ? path.split(' ') : []), '--help'];
  const res = spawnSync(cmd, [...prefix, ...args], { env, encoding: 'utf8', timeout: 30_000 });
  return { code: res.status ?? 1, stdout: res.stdout ?? '', stderr: res.stderr ?? '' };
}

const results = [];
try {
  for (const path of paths) {
    const name = `vendo ${path}`.trim();
    const ts = help('ts', path);
    const rust = help('rust', path);
    if (isNotPorted(rust)) {
      results.push({ name, status: 'not ported', diffs: [] });
    } else if (ts.code !== 0 || rust.code !== 0) {
      results.push({ name, status: 'DIFFERS', diffs: [{ path: 'exit code', a: ts.code, b: rust.code }] });
    } else {
      const parsed = (cli, out) => parseHelp(out.stdout, cli, { root: path === '' });
      const diffs = diffHelp(parsed('ts', ts), parsed('rust', rust));
      results.push({ name, status: diffs.length ? 'DIFFERS' : 'same', diffs });
    }
    const last = results.at(-1);
    console.log(`${last.status === 'same' ? 'same' : last.status.padEnd(4)}  ${name} --help`);
    for (const diff of last.diffs) console.log(`      ${diff.path}: TS ${JSON.stringify(diff.a)} | Rust ${JSON.stringify(diff.b)}`);
  }
} finally {
  rmSync(home, { recursive: true, force: true });
}

const differs = results.filter((r) => r.status !== 'same');
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const outDir = join(root, 'parity', 'out', `${stamp}-help`);
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, 'results.json'), JSON.stringify({ rustPath, results }, null, 2));
const lines = [
  `# CLI help parity, ${new Date().toISOString().slice(0, 16).replace('T', ' ')} UTC`,
  '',
  `- Rust: \`${rustPath}\``,
  `- ${results.length} help screens; ${results.length - differs.length} the same, ${differs.length} differ or not ported`,
  '',
  ...differs.flatMap((r) => [`## ${r.name} (${r.status})`, ...r.diffs.map((d) => `- ${d.path}: TS \`${JSON.stringify(d.a)}\` → Rust \`${JSON.stringify(d.b)}\``), '']),
];
writeFileSync(join(outDir, 'report.md'), lines.join('\n'));
console.log(`\n${results.length - differs.length}/${results.length} help screens the same. Report: ${join(outDir, 'report.md')}`);
process.exit(differs.length ? 1 : 0);

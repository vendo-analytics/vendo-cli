// Shared setup for the parity scripts (run.mjs, writes.mjs): load a staging
// profile, refuse anything that is not staging or localhost, and run both CLIs
// in a throwaway HOME that holds only that profile (VENDO_* env cleared), so
// neither CLI can touch the real config or update cache.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { assertSafeBaseUrl } from './lib.mjs';

export const root = resolve(import.meta.dirname, '..');

export function fail(message) {
  console.error(`parity: ${message}`);
  process.exit(2);
}

/**
 * @param {{ profile?: string, rust?: string, keep?: boolean, timeoutMs?: number }} opts
 */
export function createSession(opts) {
  if (!opts.profile) fail('--profile <staging profile> is required.');
  const realConfigPath = join(homedir(), '.config', 'vendo', 'config.json');
  const realConfig = existsSync(realConfigPath) ? JSON.parse(readFileSync(realConfigPath, 'utf8')) : {};
  const profile = realConfig.profiles?.[opts.profile];
  if (!profile?.apiKey) fail(`profile "${opts.profile}" not found in ${realConfigPath}, or it has no API key.`);
  let baseUrl;
  try {
    baseUrl = assertSafeBaseUrl(profile.baseUrl);
  } catch (err) {
    fail(err.message);
  }

  if (!existsSync(join(root, 'dist', 'cli.js'))) fail('dist/cli.js is missing. Run `pnpm build` first.');
  const rustPath = opts.rust ? resolve(opts.rust) : join(root, 'rust', 'target', 'release', 'vendo');
  const hasRust = existsSync(rustPath);

  const home = mkdtempSync(join(tmpdir(), 'vendo-parity-'));
  mkdirSync(join(home, '.config', 'vendo'), { recursive: true });
  writeFileSync(
    join(home, '.config', 'vendo', 'config.json'),
    JSON.stringify({ activeProfile: opts.profile, profiles: { [opts.profile]: profile } }, null, 2),
  );
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('VENDO_')));
  env.HOME = home;
  const cleanup = () => {
    if (!opts.keep) rmSync(home, { recursive: true, force: true });
  };
  const timeoutMs = opts.timeoutMs ?? 90_000;

  /** Run one CLI (`ts` or `rust`) with `args`; never throws. */
  function run(cli, args) {
    const [cmd, ...prefix] = cli === 'ts' ? [process.execPath, join(root, 'dist', 'cli.js')] : [rustPath];
    const res = spawnSync(cmd, [...prefix, ...args], {
      cwd: root,
      env,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: timeoutMs,
    });
    const timedOut = res.error?.code === 'ETIMEDOUT';
    return {
      code: res.status ?? 1,
      stdout: res.stdout ?? '',
      stderr: (res.stderr ?? '') + (timedOut ? `\nError: timed out after ${timeoutMs / 1000}s` : ''),
    };
  }

  return { baseUrl, rustPath, hasRust, home, env, run, cleanup };
}

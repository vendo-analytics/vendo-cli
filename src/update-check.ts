import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { getConfigPath } from './config.js';
import { c } from './output.js';

declare const __CLI_VERSION__: string;

const CHECK_INTERVAL = 24 * 60 * 60 * 1000; // 24 hours
const CACHE_FILE = join(getConfigPath(), '..', '.update-check');
const RELEASES_URL =
  'https://api.github.com/repos/vendo-analytics/vendo-cli/releases/latest';
const INSTALL_COMMAND =
  'curl -fsSL https://app2.vendodata.com/install.sh | bash';

interface UpdateCache {
  lastCheck: number;
  latestVersion?: string;
}

function loadCache(): UpdateCache {
  try {
    return JSON.parse(readFileSync(CACHE_FILE, 'utf-8')) as UpdateCache;
  } catch {
    return { lastCheck: 0 };
  }
}

function saveCache(cache: UpdateCache): void {
  try {
    writeFileSync(CACHE_FILE, JSON.stringify(cache), 'utf-8');
  } catch {
    // Silently ignore write errors
  }
}

/**
 * Check GitHub releases for a newer version. Non-blocking, never throws.
 * Shows a one-line notice if outdated.
 */
export async function checkForUpdates(): Promise<void> {
  try {
    const cache = loadCache();

    // Skip if checked recently
    if (Date.now() - cache.lastCheck < CHECK_INTERVAL) {
      if (cache.latestVersion && cache.latestVersion !== __CLI_VERSION__) {
        printUpdateNotice(cache.latestVersion);
      }
      return;
    }

    // Fetch latest version from GitHub releases (with short timeout)
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 3000);

    const res = await fetch(RELEASES_URL, {
      signal: controller.signal,
      headers: {
        Accept: 'application/vnd.github+json',
      },
    });
    clearTimeout(timer);

    if (!res.ok) return;

    const data = (await res.json()) as { tag_name?: string };
    const latest = normalizeReleaseVersion(data.tag_name);

    saveCache({ lastCheck: Date.now(), latestVersion: latest });

    if (latest && latest !== __CLI_VERSION__) {
      printUpdateNotice(latest);
    }
  } catch {
    // Never block CLI on update check failures
  }
}

function printUpdateNotice(latest: string): void {
  console.log(
    c.yellow(`Update available: ${__CLI_VERSION__} → ${latest}`),
    c.dim(`— run \`${INSTALL_COMMAND}\` to update`),
  );
  console.log();
}

function normalizeReleaseVersion(tagName?: string): string | undefined {
  if (!tagName) return undefined;

  return tagName.startsWith('cli-v') ? tagName.slice(5) : tagName;
}

import { readFileSync, writeFileSync } from 'node:fs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Import after mocks
import { checkForUpdates } from '../update-check.js';

// Mock filesystem
vi.mock('node:fs', () => ({
  readFileSync: vi.fn(),
  writeFileSync: vi.fn(),
}));

// Mock config to control the config path
vi.mock('../config.js', () => ({
  getConfigPath: () => '/mock-home/.config/vendo/config.json',
}));

// Mock output to capture printed messages
vi.mock('../output.js', () => ({
  c: {
    yellow: (s: string) => s,
    dim: (s: string) => s,
  },
}));

const mockReadFileSync = vi.mocked(readFileSync);
const mockWriteFileSync = vi.mocked(writeFileSync);

describe('update-check', () => {
  let consoleLogSpy: ReturnType<typeof vi.spyOn>;
  const latestReleaseUrl =
    'https://api.github.com/repos/vendo-analytics/vendo-cli/releases/latest';

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal('fetch', vi.fn());
    consoleLogSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    consoleLogSpy.mockRestore();
    vi.clearAllMocks();
  });

  describe('cache logic', () => {
    it('skips network check when cache is fresh', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      // Cache was checked 1 hour ago (within 24h window)
      const cache = { lastCheck: now - 3600 * 1000, latestVersion: '0.2.0' };
      mockReadFileSync.mockReturnValue(JSON.stringify(cache));

      await checkForUpdates();

      // Should NOT have called fetch — cache is fresh
      expect(vi.mocked(globalThis.fetch)).not.toHaveBeenCalled();
    });

    it('shows update notice from cache when version differs', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      // Cache was checked recently, but has a newer version than current
      const cache = { lastCheck: now - 3600 * 1000, latestVersion: '0.4.0' };
      mockReadFileSync.mockReturnValue(JSON.stringify(cache));

      await checkForUpdates();

      // Should print an update notice
      expect(consoleLogSpy).toHaveBeenCalledWith(
        expect.stringContaining('Update available'),
        expect.any(String),
      );
    });

    it('does not show notice from cache when version matches', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      // __CLI_VERSION__ is read from package.json (currently 0.3.0)
      const cache = { lastCheck: now - 3600 * 1000, latestVersion: '0.3.0' };
      mockReadFileSync.mockReturnValue(JSON.stringify(cache));

      await checkForUpdates();

      // No update notice since versions match
      expect(consoleLogSpy).not.toHaveBeenCalledWith(
        expect.stringContaining('Update available'),
        expect.any(String),
      );
    });

    it('fetches from GitHub releases when cache is stale (>24h)', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      // Cache is 25 hours old
      const cache = { lastCheck: now - 25 * 3600 * 1000 };
      mockReadFileSync.mockReturnValue(JSON.stringify(cache));

      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify({ tag_name: 'cli-v0.3.0' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      await checkForUpdates();

      expect(mockFetch).toHaveBeenCalledWith(
        latestReleaseUrl,
        expect.objectContaining({
          headers: { Accept: 'application/vnd.github+json' },
          signal: expect.any(AbortSignal),
        }),
      );
    });

    it('starts fresh when no cache file exists', async () => {
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify({ tag_name: 'cli-v0.2.0' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      await checkForUpdates();

      // Should fetch from GitHub releases since lastCheck defaults to 0
      expect(mockFetch).toHaveBeenCalled();
    });
  });

  describe('version comparison', () => {
    it('shows update notice when the latest release is newer', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      // Stale cache to force fetch
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ tag_name: 'cli-v1.0.0' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      await checkForUpdates();

      expect(consoleLogSpy).toHaveBeenCalledWith(
        expect.stringContaining('Update available'),
        expect.any(String),
      );
    });

    it('does not show notice when versions match', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      // Same version as __CLI_VERSION__ (read from package.json)
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ tag_name: 'cli-v0.3.0' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      await checkForUpdates();

      expect(consoleLogSpy).not.toHaveBeenCalledWith(
        expect.stringContaining('Update available'),
        expect.any(String),
      );
    });

    it('saves cache after successful fetch', async () => {
      const now = Date.now();
      vi.setSystemTime(now);

      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ tag_name: 'cli-v0.5.0' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      await checkForUpdates();

      expect(mockWriteFileSync).toHaveBeenCalledWith(
        expect.stringContaining('.update-check'),
        expect.any(String),
        'utf-8',
      );

      // Verify the saved cache content
      const writtenContent = mockWriteFileSync.mock.calls[0]![1] as string;
      const savedCache = JSON.parse(writtenContent);
      expect(savedCache.lastCheck).toBe(now);
      expect(savedCache.latestVersion).toBe('0.5.0');
    });
  });

  describe('error resilience', () => {
    it('does not throw on fetch failure', async () => {
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      vi.mocked(globalThis.fetch).mockRejectedValue(new Error('Network error'));

      // Should not throw
      await expect(checkForUpdates()).resolves.toBeUndefined();
    });

    it('does not throw on non-200 response', async () => {
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response('Server Error', {
          status: 500,
          headers: new Headers(),
        }),
      );

      await expect(checkForUpdates()).resolves.toBeUndefined();
      // Should not save cache on error
      expect(mockWriteFileSync).not.toHaveBeenCalled();
    });

    it('does not throw when cache write fails', async () => {
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });
      mockWriteFileSync.mockImplementation(() => {
        throw new Error('EACCES');
      });

      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ tag_name: 'cli-v0.5.0' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      // Should not throw even if write fails
      await expect(checkForUpdates()).resolves.toBeUndefined();
    });
  });
});

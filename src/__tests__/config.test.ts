import {
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Import after mocks are set up
import {
  clearActiveProfile,
  deleteConfig,
  findProfilesByAccountId,
  getAccountId,
  getApiKey,
  getBaseUrl,
  getConfigPath,
  getEffectiveConfig,
  getSelectedProfileName,
  listProfileSummaries,
  listProfiles,
  loadConfig,
  maskApiKey,
  migrateLegacyConfig,
  requireAccountId,
  requireApiKey,
  saveConfig,
  saveProfile,
  setProfileOverride,
} from '../config.js';

// Mock the filesystem and os modules before importing config
vi.mock('node:fs', () => ({
  existsSync: vi.fn(),
  readFileSync: vi.fn(),
  writeFileSync: vi.fn(),
  mkdirSync: vi.fn(),
  unlinkSync: vi.fn(),
}));

vi.mock('node:os', () => ({
  homedir: () => '/mock-home',
}));

const mockExistsSync = vi.mocked(existsSync);
const mockReadFileSync = vi.mocked(readFileSync);
const mockWriteFileSync = vi.mocked(writeFileSync);
const mockMkdirSync = vi.mocked(mkdirSync);
const mockUnlinkSync = vi.mocked(unlinkSync);

describe('config', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockExistsSync.mockReturnValue(true);
    // Reset profile override between tests
    setProfileOverride(undefined);
    // Clear env vars
    delete process.env.VENDO_API_KEY;
    delete process.env.VENDO_API_URL;
    delete process.env.VENDO_ACCOUNT_ID;
  });

  afterEach(() => {
    delete process.env.VENDO_API_KEY;
    delete process.env.VENDO_API_URL;
    delete process.env.VENDO_ACCOUNT_ID;
  });

  describe('getConfigPath', () => {
    it('returns the path under ~/.config/vendo', () => {
      const path = getConfigPath();
      expect(path).toBe('/mock-home/.config/vendo/config.json');
    });
  });

  describe('loadConfig', () => {
    it('returns parsed config when file exists', () => {
      const config = {
        apiKey: 'vendo_sk_test123',
        baseUrl: 'https://example.com',
      };
      mockReadFileSync.mockReturnValue(JSON.stringify(config));

      const result = loadConfig();
      expect(result).toEqual(config);
      expect(mockReadFileSync).toHaveBeenCalledWith(
        '/mock-home/.config/vendo/config.json',
        'utf-8',
      );
    });

    it('returns empty object when file does not exist', () => {
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      const result = loadConfig();
      expect(result).toEqual({});
    });

    it('returns empty object when file contains invalid JSON', () => {
      mockReadFileSync.mockReturnValue('not valid json {{{');

      const result = loadConfig();
      expect(result).toEqual({});
    });
  });

  describe('saveConfig', () => {
    it('merges updates with existing config and writes to disk', () => {
      const existing = { apiKey: 'old_key', baseUrl: 'https://old.com' };
      mockReadFileSync.mockReturnValue(JSON.stringify(existing));

      saveConfig({ baseUrl: 'https://new.com', accountId: 'acc-123' });

      expect(mockMkdirSync).toHaveBeenCalledWith('/mock-home/.config/vendo', {
        recursive: true,
      });
      expect(mockWriteFileSync).toHaveBeenCalledWith(
        '/mock-home/.config/vendo/config.json',
        JSON.stringify(
          {
            apiKey: 'old_key',
            baseUrl: 'https://new.com',
            accountId: 'acc-123',
          },
          null,
          2,
        ) + '\n',
        'utf-8',
      );
    });

    it('creates fresh config when no existing file', () => {
      mockReadFileSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });

      saveConfig({ apiKey: 'new_key' });

      const written = mockWriteFileSync.mock.calls[0]![1] as string;
      const parsed = JSON.parse(written);
      expect(parsed).toEqual({ apiKey: 'new_key' });
    });
  });

  describe('saveProfile', () => {
    it('saves a named profile and sets it as active', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({}));

      saveProfile('my-team', {
        apiKey: 'vendo_sk_teamkey',
        accountId: 'team-id',
      });

      const written = mockWriteFileSync.mock.calls[0]![1] as string;
      const parsed = JSON.parse(written);

      expect(parsed.activeProfile).toBe('my-team');
      expect(parsed.profiles['my-team']).toEqual({
        apiKey: 'vendo_sk_teamkey',
        accountId: 'team-id',
      });
    });

    it('preserves existing profiles when adding a new one', () => {
      const existing = {
        profiles: {
          existing: { apiKey: 'vendo_sk_existing', accountId: 'old-id' },
        },
        activeProfile: 'existing',
      };
      mockReadFileSync.mockReturnValue(JSON.stringify(existing));

      saveProfile('new-team', { apiKey: 'vendo_sk_new' });

      const written = mockWriteFileSync.mock.calls[0]![1] as string;
      const parsed = JSON.parse(written);

      expect(parsed.profiles.existing).toEqual({
        apiKey: 'vendo_sk_existing',
        accountId: 'old-id',
      });
      expect(parsed.profiles['new-team']).toEqual({ apiKey: 'vendo_sk_new' });
      expect(parsed.activeProfile).toBe('new-team');
    });
  });

  describe('listProfiles', () => {
    it('returns empty array when no profiles exist', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({}));
      expect(listProfiles()).toEqual([]);
    });

    it('returns profiles with active flag', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            alpha: { apiKey: 'a' },
            beta: { apiKey: 'b' },
          },
          activeProfile: 'beta',
        }),
      );

      const result = listProfiles();
      expect(result).toEqual([
        { name: 'alpha', active: false },
        { name: 'beta', active: true },
      ]);
    });
  });

  describe('listProfileSummaries', () => {
    it('returns profile account IDs and base URLs', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            alpha: { apiKey: 'a', accountId: 'acct-alpha' },
            beta: {
              apiKey: 'b',
              accountId: 'acct-beta',
              baseUrl: 'https://beta.example.com',
            },
          },
          activeProfile: 'beta',
        }),
      );

      expect(listProfileSummaries()).toEqual([
        {
          name: 'alpha',
          active: false,
          accountId: 'acct-alpha',
          baseUrl: 'https://app2.vendodata.com',
        },
        {
          name: 'beta',
          active: true,
          accountId: 'acct-beta',
          baseUrl: 'https://beta.example.com',
        },
      ]);
    });
  });

  describe('findProfilesByAccountId', () => {
    it('returns matching profiles for an account ID', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            alpha: { apiKey: 'a', accountId: 'acct-shared' },
            beta: { apiKey: 'b', accountId: 'acct-other' },
            gamma: { apiKey: 'c', accountId: 'acct-shared' },
          },
          activeProfile: 'alpha',
        }),
      );

      expect(findProfilesByAccountId('acct-shared')).toEqual([
        {
          name: 'alpha',
          active: true,
          accountId: 'acct-shared',
          baseUrl: 'https://app2.vendodata.com',
        },
        {
          name: 'gamma',
          active: false,
          accountId: 'acct-shared',
          baseUrl: 'https://app2.vendodata.com',
        },
      ]);
    });
  });

  describe('getApiKey / getBaseUrl / getAccountId', () => {
    it('prefers env var over config file', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          apiKey: 'config_key',
          baseUrl: 'https://config.com',
          accountId: 'cfg-id',
        }),
      );

      process.env.VENDO_API_KEY = 'env_key';
      process.env.VENDO_API_URL = 'https://env.com';
      process.env.VENDO_ACCOUNT_ID = 'env-id';

      expect(getApiKey()).toBe('env_key');
      expect(getBaseUrl()).toBe('https://env.com');
      expect(getAccountId()).toBe('env-id');
    });

    it('falls back to config file values', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          apiKey: 'config_key',
          baseUrl: 'https://config.com',
          accountId: 'cfg-id',
        }),
      );

      expect(getApiKey()).toBe('config_key');
      expect(getBaseUrl()).toBe('https://config.com');
      expect(getAccountId()).toBe('cfg-id');
    });

    it('returns default base URL when nothing is configured', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({}));
      expect(getBaseUrl()).toBe('https://app2.vendodata.com');
    });

    it('resolves active profile values', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          apiKey: 'legacy_key',
          profiles: {
            team: {
              apiKey: 'profile_key',
              accountId: 'profile-id',
              baseUrl: 'https://profile.com',
            },
          },
          activeProfile: 'team',
        }),
      );

      expect(getApiKey()).toBe('profile_key');
      expect(getBaseUrl()).toBe('https://profile.com');
      expect(getAccountId()).toBe('profile-id');
    });

    it('falls back to legacy flat config when active profile not found', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          apiKey: 'legacy_key',
          accountId: 'legacy-id',
          activeProfile: 'nonexistent',
          profiles: {},
        }),
      );

      expect(getApiKey()).toBe('legacy_key');
      expect(getAccountId()).toBe('legacy-id');
    });
  });

  describe('profile override', () => {
    it('uses override profile when set', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            default: { apiKey: 'default_key', accountId: 'default-id' },
            override: { apiKey: 'override_key', accountId: 'override-id' },
          },
          activeProfile: 'default',
        }),
      );

      setProfileOverride('override');

      expect(getApiKey()).toBe('override_key');
      expect(getAccountId()).toBe('override-id');
    });

    it('returns the selected profile name from override or config', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          activeProfile: 'default',
        }),
      );

      expect(getSelectedProfileName()).toBe('default');

      setProfileOverride('override');
      expect(getSelectedProfileName()).toBe('override');
    });
  });

  describe('getEffectiveConfig', () => {
    it('reports config sources and file presence', () => {
      mockExistsSync.mockReturnValue(false);
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            team: {
              apiKey: 'profile_key',
              accountId: 'acct-1',
              baseUrl: 'https://profile.com',
            },
          },
          activeProfile: 'team',
        }),
      );

      const result = getEffectiveConfig();

      expect(result.configExists).toBe(false);
      expect(result.selectedProfile).toBe('team');
      expect(result.selectedProfileExists).toBe(true);
      expect(result.apiKeySource).toBe('profile');
      expect(result.baseUrlSource).toBe('profile');
      expect(result.accountIdSource).toBe('profile');
      expect(result.profiles).toEqual([{ name: 'team', active: true }]);
    });

    it('prefers env vars and falls back to default base URL', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({}));
      process.env.VENDO_API_KEY = 'env_key';
      process.env.VENDO_ACCOUNT_ID = 'env-account';

      const result = getEffectiveConfig();

      expect(result.apiKey).toBe('env_key');
      expect(result.apiKeySource).toBe('env');
      expect(result.accountId).toBe('env-account');
      expect(result.accountIdSource).toBe('env');
      expect(result.baseUrl).toBe('https://app2.vendodata.com');
      expect(result.baseUrlSource).toBe('default');
    });

    it('marks missing override profiles as unresolved', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            alpha: { apiKey: 'a' },
          },
          activeProfile: 'alpha',
        }),
      );
      setProfileOverride('missing');

      const result = getEffectiveConfig();

      expect(result.selectedProfile).toBe('missing');
      expect(result.selectedProfileExists).toBe(false);
      expect(result.apiKeySource).toBe('missing');
      expect(result.accountIdSource).toBe('missing');
    });
  });

  describe('requireApiKey', () => {
    it('returns key when configured', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({ apiKey: 'test_key' }));
      expect(requireApiKey()).toBe('test_key');
    });

    it('throws when no key is configured', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({}));
      expect(() => requireApiKey()).toThrow('No API key configured');
    });
  });

  describe('requireAccountId', () => {
    it('returns account ID when configured', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({ accountId: 'abc-123' }),
      );
      expect(requireAccountId()).toBe('abc-123');
    });

    it('throws when no account ID is configured', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({}));
      expect(() => requireAccountId()).toThrow('No account configured');
    });
  });

  describe('deleteConfig', () => {
    it('returns true when file is successfully deleted', () => {
      mockUnlinkSync.mockImplementation(() => {});
      expect(deleteConfig()).toBe(true);
      expect(mockUnlinkSync).toHaveBeenCalledWith(
        '/mock-home/.config/vendo/config.json',
      );
    });

    it('returns false when file does not exist', () => {
      mockUnlinkSync.mockImplementation(() => {
        throw new Error('ENOENT');
      });
      expect(deleteConfig()).toBe(false);
    });
  });

  describe('maskApiKey', () => {
    it('masks the middle of a long key', () => {
      expect(maskApiKey('vendo_sk_1234567890abcdef')).toBe('vend...cdef');
    });

    it('returns **** for short keys', () => {
      expect(maskApiKey('short')).toBe('****');
      expect(maskApiKey('12345678')).toBe('****');
    });

    it('masks keys exactly 9 characters', () => {
      expect(maskApiKey('123456789')).toBe('1234...6789');
    });
  });

  describe('migrateLegacyConfig', () => {
    it('leaves a profiles-only config untouched', () => {
      const config = {
        profiles: { team: { apiKey: 'k', accountId: 'a' } },
        activeProfile: 'team',
      };
      const result = migrateLegacyConfig(config);
      expect(result.migrated).toBe(false);
      expect(result.config).toBe(config);
    });

    it('folds flat-only config into a default profile', () => {
      const result = migrateLegacyConfig({
        apiKey: 'flat_key',
        accountId: 'flat-id',
        baseUrl: 'https://flat.com',
      });

      expect(result.migrated).toBe(true);
      expect(result.config).toEqual({
        profiles: {
          default: {
            apiKey: 'flat_key',
            accountId: 'flat-id',
            baseUrl: 'https://flat.com',
          },
        },
        activeProfile: 'default',
      });
      // Flat fields are gone from the top level.
      expect(result.config.apiKey).toBeUndefined();
    });

    it('backfills the active profile from legacy fields (profile wins)', () => {
      const result = migrateLegacyConfig({
        apiKey: 'flat_key',
        accountId: 'flat-id',
        profiles: { team: { apiKey: 'profile_key' } },
        activeProfile: 'team',
      });

      expect(result.migrated).toBe(true);
      // profile_key kept; accountId backfilled from legacy.
      expect(result.config.profiles?.team).toEqual({
        apiKey: 'profile_key',
        accountId: 'flat-id',
      });
      expect(result.config.apiKey).toBeUndefined();
    });

    it('creates default when activeProfile points to a missing profile', () => {
      const result = migrateLegacyConfig({
        apiKey: 'flat_key',
        accountId: 'flat-id',
        activeProfile: 'nonexistent',
        profiles: {},
      });

      expect(result.config.profiles?.default).toEqual({
        apiKey: 'flat_key',
        accountId: 'flat-id',
      });
      expect(result.config.activeProfile).toBe('default');
    });

    it('migrates an account-only config (no api key)', () => {
      const result = migrateLegacyConfig({ accountId: 'flat-id' });
      expect(result.config.profiles?.default).toEqual({ accountId: 'flat-id' });
      expect(result.config.activeProfile).toBe('default');
    });

    it('avoids clobbering an existing default profile', () => {
      const result = migrateLegacyConfig({
        apiKey: 'flat_key',
        profiles: { default: { apiKey: 'existing' } },
        // no activeProfile, so legacy needs a fresh slot
      });

      expect(result.config.profiles?.default).toEqual({ apiKey: 'existing' });
      expect(result.config.profiles?.['default-2']).toEqual({
        apiKey: 'flat_key',
      });
      expect(result.config.activeProfile).toBe('default-2');
    });
  });

  describe('legacy config auto-migration on read', () => {
    it('persists the migrated profiles-only config to disk', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({ apiKey: 'flat_key', accountId: 'flat-id' }),
      );

      // Any resolution read triggers migration.
      expect(getApiKey()).toBe('flat_key');

      const written = mockWriteFileSync.mock.calls.at(-1)![1] as string;
      const parsed = JSON.parse(written);
      expect(parsed).toEqual({
        profiles: { default: { apiKey: 'flat_key', accountId: 'flat-id' } },
        activeProfile: 'default',
      });
    });
  });

  describe('clearActiveProfile', () => {
    it('removes the active profile and unsets activeProfile', () => {
      mockReadFileSync.mockReturnValue(
        JSON.stringify({
          profiles: {
            team: { apiKey: 'team_key', accountId: 'team-id' },
            other: { apiKey: 'other_key' },
          },
          activeProfile: 'team',
        }),
      );

      const cleared = clearActiveProfile();
      expect(cleared).toBe('team');

      const written = mockWriteFileSync.mock.calls.at(-1)![1] as string;
      const parsed = JSON.parse(written);
      expect(parsed.profiles.team).toBeUndefined();
      expect(parsed.profiles.other).toEqual({ apiKey: 'other_key' });
      expect(parsed.activeProfile).toBeUndefined();
    });

    it('returns undefined when there is no active profile', () => {
      mockReadFileSync.mockReturnValue(JSON.stringify({ profiles: {} }));
      expect(clearActiveProfile()).toBeUndefined();
    });
  });
});

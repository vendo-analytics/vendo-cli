import {
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';

export interface VendoProfile {
  // Optional so a legacy config carrying only an accountId/baseUrl (no key)
  // still migrates into a profile without losing those values.
  apiKey?: string;
  baseUrl?: string;
  accountId?: string;
}

export interface VendoConfig {
  // Legacy flat fields (pre-profiles). Read once by migrateLegacyConfig, then
  // folded into a profile and removed from disk — never read for resolution.
  apiKey?: string;
  baseUrl?: string;
  accountId?: string;
  // Multi-account
  activeProfile?: string;
  profiles?: Record<string, VendoProfile>;
}

const CONFIG_DIR = join(homedir(), '.config', 'vendo');
const CONFIG_PATH = join(CONFIG_DIR, 'config.json');
export const DEFAULT_BASE_URL = 'https://app2.vendodata.com';
export const STAGING_BASE_URL = 'https://stg.vendodata.com';

/**
 * Resolve the base URL for a login flow from explicit flags (VE-1563):
 * `--base-url` wins, then `--env` (staging | prod), then the normal chain
 * (VENDO_API_URL → active profile → prod default). Throws on an invalid
 * URL or unknown environment so `vendo login` fails before opening a
 * browser at the wrong instance. Explicitly-passed EMPTY values (an unset
 * shell variable: `--base-url "$VENDO_STAGING_URL"`) are errors too — falling
 * through to the prod default would be the exact silent wrong-instance login
 * this flag exists to prevent (VE-1603).
 */
export function resolveLoginBaseUrl(
  opts: { env?: string; baseUrl?: string } = {},
): string {
  if (opts.baseUrl !== undefined) {
    let parsed: URL;
    try {
      parsed = new URL(opts.baseUrl);
    } catch {
      throw new Error(
        `Invalid --base-url: "${opts.baseUrl}". Expected a full URL like ${STAGING_BASE_URL}.`,
      );
    }
    if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
      throw new Error(
        `Invalid --base-url protocol: "${opts.baseUrl}". Use http(s).`,
      );
    }
    // Origin only — login paths (/cli-auth) are appended by the flow.
    return parsed.origin;
  }

  if (opts.env !== undefined) {
    switch (opts.env.toLowerCase()) {
      case 'staging':
      case 'stg':
        return STAGING_BASE_URL;
      case 'prod':
      case 'production':
        return DEFAULT_BASE_URL;
      default:
        throw new Error(
          `Unknown --env "${opts.env}". Use "staging" or "prod".`,
        );
    }
  }

  return getBaseUrl();
}

// Global override set by --profile flag
let profileOverride: string | undefined;

export type ConfigValueSource = 'env' | 'profile' | 'default' | 'missing';

export interface EffectiveConfig {
  configExists: boolean;
  configPath: string;
  selectedProfile?: string;
  selectedProfileExists: boolean;
  profiles: Array<{ name: string; active: boolean }>;
  apiKey?: string;
  apiKeySource: ConfigValueSource;
  baseUrl: string;
  baseUrlSource: ConfigValueSource;
  accountId?: string;
  accountIdSource: ConfigValueSource;
}

export interface VendoProfileSummary {
  name: string;
  active: boolean;
  accountId?: string;
  baseUrl: string;
}

export interface ConfigValueUpdates {
  apiKey?: string;
  baseUrl?: string;
  accountId?: string;
}

export function setProfileOverride(name?: string): void {
  profileOverride = name;
}

export function getConfigPath(): string {
  return CONFIG_PATH;
}

export function loadConfig(): VendoConfig {
  try {
    const raw = readFileSync(CONFIG_PATH, 'utf-8');
    return JSON.parse(raw) as VendoConfig;
  } catch {
    return {};
  }
}

export function saveConfig(updates: Partial<VendoConfig>): void {
  mkdirSync(dirname(CONFIG_PATH), { recursive: true });
  const existing = loadConfig();
  const merged = { ...existing, ...updates };
  writeFileSync(CONFIG_PATH, JSON.stringify(merged, null, 2) + '\n', 'utf-8');
}

/**
 * Fold any legacy flat fields (`apiKey`/`baseUrl`/`accountId`) into a profile,
 * dropping them from the top level. Pure — returns the rewritten config and
 * whether anything changed.
 *
 * Resolution order used to prefer the active profile then fall back per-field
 * to the flat fields, so migration replicates that exactly: gaps in the active
 * profile are backfilled from legacy; otherwise legacy becomes a `default`
 * profile. The result is profiles-only.
 */
export function migrateLegacyConfig(config: VendoConfig): {
  config: VendoConfig;
  migrated: boolean;
} {
  const hasLegacy =
    config.apiKey !== undefined ||
    config.baseUrl !== undefined ||
    config.accountId !== undefined;
  if (!hasLegacy) return { config, migrated: false };

  const profiles: Record<string, VendoProfile> = { ...(config.profiles ?? {}) };
  const legacy: VendoProfile = removeUndefinedValues({
    apiKey: config.apiKey,
    baseUrl: config.baseUrl,
    accountId: config.accountId,
  });

  let activeProfile = config.activeProfile;

  if (activeProfile && profiles[activeProfile]) {
    // Backfill only the fields the active profile is missing (profile wins).
    profiles[activeProfile] = { ...legacy, ...profiles[activeProfile] };
  } else {
    // No usable active profile — fold legacy into a fresh `default` profile.
    let name = 'default';
    let suffix = 2;
    while (profiles[name]) name = `default-${suffix++}`;
    profiles[name] = legacy;
    activeProfile = name;
  }

  const migrated: VendoConfig = { profiles };
  if (activeProfile) migrated.activeProfile = activeProfile;
  return { config: migrated, migrated: true };
}

/**
 * Read config with legacy flat fields migrated into profiles. The migration is
 * persisted once (the next read sees the rewritten, profiles-only file). All
 * resolution and profile-aware writers go through this, never `loadConfig`
 * directly, so the legacy shape is invisible above this layer.
 *
 * Persisting is best-effort: this is a read path, so a read-only or
 * permission-restricted config dir must not make every command fail. If the
 * write fails we fall back to the in-memory migration (the file stays in its
 * legacy shape and is re-migrated on the next read).
 */
function readConfig(): VendoConfig {
  const { config, migrated } = migrateLegacyConfig(loadConfig());
  if (migrated) {
    try {
      mkdirSync(dirname(CONFIG_PATH), { recursive: true });
      writeFileSync(
        CONFIG_PATH,
        JSON.stringify(config, null, 2) + '\n',
        'utf-8',
      );
    } catch {
      // Best-effort — degrade to the in-memory migration.
    }
  }
  return config;
}

/**
 * Apply config-value updates to the active profile, creating a `default`
 * profile when none is selected. Always writes to a profile — there is no
 * legacy flat-field target.
 */
export function saveResolvedConfigValues(updates: ConfigValueUpdates): {
  target: 'profile';
  profileName: string;
} {
  const config = readConfig();
  const profileName = getSelectedProfileName(config) ?? 'default';
  const existing = config.profiles?.[profileName] ?? {};
  const definedUpdates = removeUndefinedValues(updates);

  saveConfig({
    profiles: {
      ...config.profiles,
      [profileName]: { ...existing, ...definedUpdates },
    },
    activeProfile: profileName,
  });
  return { target: 'profile', profileName };
}

/**
 * Save a named profile (used by login)
 */
export function saveProfile(name: string, profile: VendoProfile): void {
  const config = readConfig();
  const profiles = config.profiles ?? {};
  profiles[name] = profile;
  saveConfig({ profiles, activeProfile: name });
}

/**
 * Remove the active profile's stored credentials (used by logout). Returns the
 * name of the profile that was cleared, or undefined if none was active.
 */
export function clearActiveProfile(): string | undefined {
  const config = readConfig();
  const name = getSelectedProfileName(config);
  if (!name || !config.profiles?.[name]) return undefined;

  const profiles = { ...config.profiles };
  delete profiles[name];
  saveConfig({ profiles, activeProfile: undefined });
  return name;
}

/**
 * Get all profile names
 */
export function listProfiles(): Array<{ name: string; active: boolean }> {
  return listProfileSummaries().map(({ name, active }) => ({ name, active }));
}

export function listProfileSummaries(): VendoProfileSummary[] {
  const config = readConfig();
  const active = getSelectedProfileName(config);
  if (!config.profiles) return [];
  return Object.entries(config.profiles).map(([name, profile]) => ({
    name,
    active: name === active,
    accountId: profile.accountId,
    baseUrl: profile.baseUrl ?? DEFAULT_BASE_URL,
  }));
}

export function findProfilesByAccountId(
  accountId: string,
): VendoProfileSummary[] {
  return listProfileSummaries().filter(
    (profile) => profile.accountId === accountId,
  );
}

/**
 * Get the name of the selected profile, including --profile override
 */
export function getSelectedProfileName(
  config = readConfig(),
): string | undefined {
  if (profileOverride) return profileOverride;
  return config.activeProfile;
}

export function getEffectiveConfig(): EffectiveConfig {
  const config = readConfig();
  const selectedProfile = getSelectedProfileName(config);
  const selectedProfileExists = Boolean(
    selectedProfile && config.profiles?.[selectedProfile],
  );
  const profile = selectedProfileExists
    ? config.profiles?.[selectedProfile!]
    : undefined;
  const profiles = listProfiles();

  const apiKey = resolveValue(process.env.VENDO_API_KEY, profile?.apiKey);
  const baseUrl = resolveValue(
    process.env.VENDO_API_URL,
    profile?.baseUrl,
    DEFAULT_BASE_URL,
  );
  const accountId = resolveValue(
    process.env.VENDO_ACCOUNT_ID,
    profile?.accountId,
  );

  return {
    configExists: existsSync(CONFIG_PATH),
    configPath: CONFIG_PATH,
    selectedProfile,
    selectedProfileExists,
    profiles,
    apiKey: apiKey.value,
    apiKeySource: apiKey.source,
    baseUrl: baseUrl.value ?? DEFAULT_BASE_URL,
    baseUrlSource: baseUrl.source,
    accountId: accountId.value,
    accountIdSource: accountId.source,
  };
}

export function getApiKey(): string | undefined {
  return getEffectiveConfig().apiKey;
}

export function getBaseUrl(): string {
  return getEffectiveConfig().baseUrl;
}

export function requireApiKey(): string {
  const key = getApiKey();
  if (!key) {
    throw new Error(
      'No API key configured. Run `vendo login` or `vendo config set --api-key <key>` or set VENDO_API_KEY.',
    );
  }
  return key;
}

export function getAccountId(): string | undefined {
  return getEffectiveConfig().accountId;
}

export function requireAccountId(): string {
  const id = getAccountId();
  if (!id) {
    throw new Error(
      'No account configured. Run `vendo config set --account <account-id>` or set VENDO_ACCOUNT_ID.',
    );
  }
  return id;
}

export function deleteConfig(): boolean {
  try {
    unlinkSync(CONFIG_PATH);
    return true;
  } catch {
    return false;
  }
}

export function maskApiKey(key: string): string {
  if (key.length <= 8) return '****';
  return key.slice(0, 4) + '...' + key.slice(-4);
}

function resolveValue(
  envValue: string | undefined,
  profileValue: string | undefined,
  defaultValue?: string,
): { value?: string; source: ConfigValueSource } {
  if (envValue) return { value: envValue, source: 'env' };
  if (profileValue) return { value: profileValue, source: 'profile' };
  if (defaultValue) return { value: defaultValue, source: 'default' };
  return { value: undefined, source: 'missing' };
}

function removeUndefinedValues<T extends object>(values: T): Partial<T> {
  const definedEntries = Object.entries(
    values as Record<string, string | undefined>,
  ).filter(([, value]) => value !== undefined);
  return Object.fromEntries(definedEntries) as Partial<T>;
}

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
  apiKey: string;
  baseUrl?: string;
  accountId?: string;
}

export interface VendoConfig {
  // Legacy flat fields (pre-profiles migration)
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

// Global override set by --profile flag
let profileOverride: string | undefined;

export type ConfigValueSource =
  | 'env'
  | 'profile'
  | 'legacy'
  | 'default'
  | 'missing';

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

export function saveResolvedConfigValues(
  updates: ConfigValueUpdates,
): { target: 'profile'; profileName: string } | { target: 'legacy' } {
  const config = loadConfig();
  const selectedProfile = getSelectedProfileName(config);
  const profile =
    selectedProfile && config.profiles?.[selectedProfile]
      ? config.profiles[selectedProfile]
      : undefined;
  const definedUpdates = removeUndefinedValues(updates);

  if (profile && selectedProfile) {
    saveConfig({
      profiles: {
        ...config.profiles,
        [selectedProfile]: {
          ...profile,
          ...definedUpdates,
        },
      },
    });
    return { target: 'profile', profileName: selectedProfile };
  }

  saveConfig(definedUpdates);
  return { target: 'legacy' };
}

/**
 * Save a named profile (used by login)
 */
export function saveProfile(name: string, profile: VendoProfile): void {
  const config = loadConfig();
  const profiles = config.profiles ?? {};
  profiles[name] = profile;
  saveConfig({ profiles, activeProfile: name });
}

/**
 * Get all profile names
 */
export function listProfiles(): Array<{ name: string; active: boolean }> {
  return listProfileSummaries().map(({ name, active }) => ({ name, active }));
}

export function listProfileSummaries(): VendoProfileSummary[] {
  const config = loadConfig();
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
  config = loadConfig(),
): string | undefined {
  if (profileOverride) return profileOverride;
  return config.activeProfile;
}

export function getEffectiveConfig(): EffectiveConfig {
  const config = loadConfig();
  const selectedProfile = getSelectedProfileName(config);
  const selectedProfileExists = Boolean(
    selectedProfile && config.profiles?.[selectedProfile],
  );
  const profile = selectedProfileExists
    ? config.profiles?.[selectedProfile!]
    : undefined;
  const profiles = listProfiles();

  const apiKey = resolveValue(
    process.env.VENDO_API_KEY,
    profile?.apiKey,
    config.apiKey,
  );
  const baseUrl = resolveValue(
    process.env.VENDO_API_URL,
    profile?.baseUrl,
    config.baseUrl,
    DEFAULT_BASE_URL,
  );
  const accountId = resolveValue(
    process.env.VENDO_ACCOUNT_ID,
    profile?.accountId,
    config.accountId,
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
  legacyValue: string | undefined,
  defaultValue?: string,
): { value?: string; source: ConfigValueSource } {
  if (envValue) return { value: envValue, source: 'env' };
  if (profileValue) return { value: profileValue, source: 'profile' };
  if (legacyValue) return { value: legacyValue, source: 'legacy' };
  if (defaultValue) return { value: defaultValue, source: 'default' };
  return { value: undefined, source: 'missing' };
}

function removeUndefinedValues<T extends object>(values: T): Partial<T> {
  const definedEntries = Object.entries(
    values as Record<string, string | undefined>,
  ).filter(([, value]) => value !== undefined);
  return Object.fromEntries(definedEntries) as Partial<T>;
}

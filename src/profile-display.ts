import type { EffectiveConfig, VendoProfileSummary } from './config.js';
import {
  DEFAULT_BASE_URL,
  findProfilesByAccountId,
  saveConfig,
} from './config.js';
import {
  c,
  exitWithError,
  printSuccess,
  searchSelectOption,
} from './output.js';

export function formatProfileLabel(
  profile: VendoProfileSummary,
  options: { annotateActive?: boolean } = {},
): string {
  const parts = [
    options.annotateActive && profile.active
      ? `${profile.name} (active)`
      : profile.name,
    profile.accountId ?? 'no account',
  ];

  if (profile.baseUrl !== DEFAULT_BASE_URL) {
    parts.push(profile.baseUrl);
  }

  return parts.join('  ');
}

export function formatProfileListLine(
  profile: VendoProfileSummary,
  options: { annotateActive?: boolean; indent?: string } = {},
): string {
  const marker = profile.active ? c.green('*') : ' ';
  return `${options.indent ?? ''}${marker} ${formatProfileLabel(profile, options)}`;
}

export function printProfileList(
  profiles: VendoProfileSummary[],
  options: {
    annotateActive?: boolean;
    indent?: string;
    emptyMessage: string;
  },
): boolean {
  if (profiles.length === 0) {
    console.log(c.dim(options.emptyMessage));
    return false;
  }

  for (const profile of profiles) {
    console.log(
      formatProfileListLine(profile, {
        annotateActive: options.annotateActive,
        indent: options.indent,
      }),
    );
  }

  return true;
}

export async function promptForProfileSelection(
  profiles: VendoProfileSummary[],
): Promise<string | undefined> {
  return searchSelectOption(
    'Search and select a profile',
    profiles.map((profile) => ({
      value: profile.name,
      label: formatProfileLabel(profile, { annotateActive: true }),
      searchText: `${profile.name} ${profile.accountId ?? ''} ${profile.baseUrl}`,
    })),
  );
}

export function printProfileSwitchSuccess(
  profile: VendoProfileSummary,
  verifyHint: string,
): void {
  printSuccess(`Switched to profile ${c.bold(profile.name)}.`);
  console.log(
    c.dim(
      `  Account ID: ${profile.accountId ?? 'not set'}  Base URL: ${profile.baseUrl}`,
    ),
  );
  console.log(c.dim(`  Verify with ${verifyHint}.`));
}

export function printCurrentProfileSummary(
  config: Pick<
    EffectiveConfig,
    | 'selectedProfile'
    | 'accountId'
    | 'baseUrl'
    | 'apiKeySource'
    | 'baseUrlSource'
    | 'accountIdSource'
  >,
  currentProfile?: VendoProfileSummary,
): void {
  console.log(c.bold('Current Profile'));
  console.log();
  console.log(
    `  Profile:     ${config.selectedProfile ?? c.dim('none selected')}`,
  );
  console.log(`  Account ID:  ${config.accountId ?? c.dim('not set')}`);
  console.log(`  Base URL:    ${config.baseUrl}`);

  const overrides = getEnvOverrideNames(config);
  if (overrides.length > 0) {
    console.log();
    console.log(c.dim(`  Env overrides active: ${overrides.join(', ')}`));
  }

  if (currentProfile) {
    console.log();
    console.log(
      c.dim(
        `  Saved profile: ${formatProfileLabel(currentProfile, { annotateActive: true })}`,
      ),
    );
  }
}

export async function switchProfileSelection(
  profiles: VendoProfileSummary[],
  options: {
    profileName?: string;
    accountId?: string;
    emptyMessage: string;
    listCommand: string;
    profileCommand: string;
    verifyHint: string;
  },
): Promise<void> {
  if (profiles.length === 0) {
    console.log(c.dim(options.emptyMessage));
    return;
  }

  let profileName = options.profileName;

  if (profileName && options.accountId) {
    exitWithError(
      'Choose either a profile name or `--account <accountId>`, not both.',
    );
  }

  if (options.accountId) {
    const matches = findProfilesByAccountId(options.accountId);
    if (matches.length === 0) {
      exitWithError(
        `No profile found for account ID "${options.accountId}".\n${c.dim(`  Run \`${options.listCommand}\` to inspect configured profiles.`)}`,
      );
    }

    if (matches.length > 1) {
      exitWithError(
        `Multiple profiles use account ID "${options.accountId}".\n${matches
          .map((match) => c.dim(`  ${match.name}`))
          .join(
            '\n',
          )}\n${c.dim(`  Switch by profile name with \`${options.profileCommand} <profile>\`.`)}`,
      );
    }

    profileName = matches[0]!.name;
  }

  if (!profileName) {
    profileName = await promptForProfileSelection(profiles);
  }

  if (!profileName) {
    console.log(c.dim('Cancelled.'));
    return;
  }

  const targetProfile = profiles.find(
    (profile) => profile.name === profileName,
  );
  if (!targetProfile) {
    exitWithError(
      `Profile "${profileName}" not found.\n${c.dim(`  Run \`${options.listCommand}\` to inspect configured profiles.`)}`,
    );
  }

  saveConfig({ activeProfile: targetProfile.name });
  printProfileSwitchSuccess(targetProfile, options.verifyHint);
}

export function getEnvOverrideNames(
  config: Pick<
    EffectiveConfig,
    'apiKeySource' | 'baseUrlSource' | 'accountIdSource'
  >,
): string[] {
  const overrides = [];
  if (config.apiKeySource === 'env') overrides.push('VENDO_API_KEY');
  if (config.baseUrlSource === 'env') overrides.push('VENDO_API_URL');
  if (config.accountIdSource === 'env') overrides.push('VENDO_ACCOUNT_ID');
  return overrides;
}

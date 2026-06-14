import { Command } from 'commander';

import { type MeResponse, getClient } from '../client.js';
import { getEffectiveConfig, listProfileSummaries } from '../config.js';
import { addExamples, c, printJson, runAction } from '../output.js';
import {
  formatProfileListLine,
  getEnvOverrideNames,
} from '../profile-display.js';
import { checkForUpdates } from '../update-check.js';

export function registerWhoamiCommand(program: Command): void {
  const cmd = program
    .command('whoami')
    .description('Show the current authenticated account')
    .option('--json', 'Output raw JSON')
    .action(async (opts: { json?: boolean }) => {
      await checkForUpdates();

      const res = await runAction('Checking identity...', () =>
        getClient().get<MeResponse>('/me'),
      );
      const config = getEffectiveConfig();

      if (opts.json) {
        printJson({
          ...res,
          config: {
            selectedProfile: config.selectedProfile,
            apiKeySource: config.apiKeySource,
            baseUrl: config.baseUrl,
            baseUrlSource: config.baseUrlSource,
            accountId: config.accountId,
            accountIdSource: config.accountIdSource,
          },
        });
        return;
      }

      const me = res.data;
      console.log();
      console.log(c.bold(me.accountName ?? me.accountSlug ?? me.accountId));
      console.log();
      console.log(`  Account:     ${me.accountSlug ?? me.accountId}`);
      console.log(`  Account ID:  ${me.accountId}`);
      console.log(
        `  Profile:     ${config.selectedProfile ?? c.dim('none selected')}`,
      );
      console.log(`  Base URL:    ${config.baseUrl}`);
      console.log(`  API Key:     ${c.dim(me.apiKeyId ?? 'unknown')}`);
      if (me.scopes && me.scopes.length > 0) {
        console.log(`  Scopes:      ${me.scopes.join(', ')}`);
      } else {
        console.log(`  Scopes:      ${c.dim('full access')}`);
      }

      const overrides = getEnvOverrideNames(config);

      if (overrides.length > 0) {
        console.log();
        console.log(c.dim(`  Env overrides active: ${overrides.join(', ')}`));
      }

      const profiles = listProfileSummaries();
      if (profiles.length > 1) {
        console.log();
        console.log(c.bold('  Profiles'));
        for (const profile of profiles) {
          console.log(formatProfileListLine(profile, { indent: '    ' }));
        }

        console.log();
        console.log(
          c.dim(
            '  Switch with `vendo profile switch`, target one command with `vendo --profile <name> ...`, or use `vendo config use` as a compatibility alias.',
          ),
        );
      }
    });

  addExamples(cmd, ['vendo whoami', 'vendo whoami --json']);
}

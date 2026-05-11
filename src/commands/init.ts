import { Command } from 'commander';

import { getEffectiveConfig } from '../config.js';
import { type MeResponse, fetchIdentity } from '../identity.js';
import { addExamples, c, exitWithError, printSuccess } from '../output.js';
import { printLoginSuccess, runBrowserLogin } from './login.js';

export function registerInitCommand(program: Command): void {
  const cmd = program
    .command('init')
    .description('Guide first-time Vendo CLI setup')
    .action(async () => {
      console.log(c.bold('Vendo CLI Setup'));
      console.log();

      const initialConfig = getEffectiveConfig();

      if (!initialConfig.apiKey) {
        console.log(c.dim('No API key found. Starting browser login...'));

        try {
          const loginResult = await runBrowserLogin();
          printLoginSuccess(loginResult);
        } catch (err) {
          exitWithError(err);
        }
      } else {
        console.log(
          c.dim(
            `Using existing profile ${initialConfig.selectedProfile ?? '(legacy config)'}.`,
          ),
        );
      }

      const config = getEffectiveConfig();
      const identity = await probeIdentity(
        config.apiKey,
        config.accountId,
        config.baseUrl,
      );

      console.log();
      console.log(c.bold('Setup summary'));
      console.log(
        `  Profile:     ${config.selectedProfile ?? c.dim('legacy config')}`,
      );
      console.log(`  Base URL:    ${config.baseUrl}`);
      console.log(`  Account ID:  ${config.accountId ?? c.dim('missing')}`);

      if (identity) {
        console.log(
          `  Auth:        ${c.green('verified')} as ${identity.accountName ?? identity.accountSlug ?? identity.accountId}`,
        );
      } else if (config.apiKey && config.accountId) {
        console.log(
          `  Auth:        ${c.yellow('not verified')} (API check failed)`,
        );
      } else {
        console.log(
          `  Auth:        ${c.yellow('incomplete')} (account ID still required)`,
        );
      }

      console.log();
      console.log(c.bold('Next steps'));
      console.log('  vendo doctor');
      console.log('  vendo whoami');
      console.log('  vendo status');

      if (!config.accountId) {
        console.log();
        console.log(
          c.dim(
            'Set an account explicitly with `vendo config set --account <account-id>` if your login flow did not provide one.',
          ),
        );
      }

      printSuccess('Vendo CLI setup complete.');
    });

  addExamples(cmd, ['vendo init']);
}

async function probeIdentity(
  apiKey: string | undefined,
  accountId: string | undefined,
  baseUrl: string,
): Promise<MeResponse | null> {
  if (!apiKey || !accountId) {
    return null;
  }
  try {
    return await fetchIdentity(apiKey, accountId, baseUrl);
  } catch {
    return null;
  }
}

import { Command } from 'commander';

import {
  deleteConfig,
  getConfigPath,
  getEffectiveConfig,
  listProfileSummaries,
  maskApiKey,
  saveResolvedConfigValues,
} from '../config.js';
import {
  addExamples,
  c,
  confirm,
  exitWithError,
  printSuccess,
} from '../output.js';
import {
  printProfileList,
  switchProfileSelection,
} from '../profile-display.js';

export function registerConfigCommand(program: Command): void {
  const cmd = program.command('config').description('Manage CLI configuration');

  const setCmd = cmd
    .command('set')
    .description('Set configuration values')
    .option('--api-key <key>', 'API key for authentication')
    .option('--base-url <url>', 'Base URL for the Vendo API')
    .option('--account <id>', 'Account ID to operate on')
    .action((opts: { apiKey?: string; baseUrl?: string; account?: string }) => {
      if (!opts.apiKey && !opts.baseUrl && !opts.account) {
        exitWithError(
          'Provide at least one option: --api-key <key>, --base-url <url>, or --account <id>',
        );
      }

      saveResolvedConfigValues({
        apiKey: opts.apiKey,
        baseUrl: opts.baseUrl,
        accountId: opts.account,
      });
      console.log(c.green('Configuration saved'), c.dim(getConfigPath()));
    });

  addExamples(setCmd, [
    'vendo config set --api-key <key>',
    'vendo config set --account <id>',
    'vendo config set --api-key <key> --account <id>',
  ]);

  const showCmd = cmd
    .command('show')
    .description('Inspect low-level CLI configuration')
    .action(() => {
      const effective = getEffectiveConfig();

      console.log(c.bold('Vendo CLI Config Inspect'));
      console.log();
      console.log(
        c.dim(
          '  Use `vendo profile current` for the day-to-day effective account view.',
        ),
      );

      console.log();
      console.log(c.bold('Effective Values'));
      console.log();
      console.log(
        `  API Key:        ${effective.apiKey ? maskApiKey(effective.apiKey) : c.dim('not set')} ${c.dim(`(${effective.apiKeySource})`)}`,
      );
      console.log(
        `  Base URL:       ${effective.baseUrl} ${c.dim(`(${effective.baseUrlSource})`)}`,
      );
      console.log(
        `  Account ID:     ${effective.accountId ?? c.dim('not set')} ${c.dim(`(${effective.accountIdSource})`)}`,
      );
      console.log(
        `  Active Profile: ${effective.selectedProfile ?? c.dim('none selected')}`,
      );
      console.log(`  Config Path:    ${c.dim(getConfigPath())}`);

      console.log();
      console.log(c.bold('Saved Profiles'));
      console.log();
      printProfileList(listProfileSummaries(), {
        annotateActive: true,
        indent: '  ',
        emptyMessage:
          'No profiles configured. Run `vendo login` to create one.',
      });
    });

  addExamples(showCmd, ['vendo config show']);

  const useCmd = cmd
    .command('use [profile]')
    .description('Alias for `vendo profile switch`')
    .option(
      '--account <accountId>',
      'Switch by account ID instead of profile name',
    )
    .action(
      async (profileName: string | undefined, opts: { account?: string }) => {
        console.log(
          c.dim(
            'Tip: prefer `vendo profile switch` for interactive profile changes.',
          ),
        );
        console.log();
        await switchProfileSelection(listProfileSummaries(), {
          profileName,
          accountId: opts.account,
          emptyMessage: 'No profiles yet. Run `vendo login` to create one.',
          listCommand: 'vendo profile list',
          profileCommand: 'vendo profile switch',
          verifyHint: '`vendo whoami`',
        });
      },
    );

  addExamples(useCmd, [
    'vendo config use <profile>',
    'vendo config use --account <accountId>',
  ]);

  const listAliasCmd = cmd
    .command('list')
    .description('Alias for `vendo profile list`')
    .action(() => {
      console.log(
        c.dim('Tip: prefer `vendo profile list` for saved profile management.'),
      );
      console.log();
      printProfileList(listProfileSummaries(), {
        annotateActive: false,
        emptyMessage:
          'No profiles configured. Run `vendo login` to create one.',
      });
    });

  addExamples(listAliasCmd, ['vendo config list']);

  const resetCmd = cmd
    .command('reset')
    .description('Delete all CLI configuration')
    .option('-y, --yes', 'Skip confirmation prompt')
    .action(async (opts: { yes?: boolean }) => {
      if (!opts.yes) {
        const ok = await confirm('Delete all CLI configuration?');
        if (!ok) {
          console.log(c.dim('Cancelled.'));
          return;
        }
      }

      const deleted = deleteConfig();
      if (deleted) {
        printSuccess('Configuration deleted.');
      } else {
        console.log(c.dim('No configuration file found.'));
      }
    });

  addExamples(resetCmd, ['vendo config reset', 'vendo config reset --yes']);
}

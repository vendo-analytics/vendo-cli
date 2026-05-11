import { Command } from 'commander';

import { getEffectiveConfig, listProfileSummaries } from '../config.js';
import { addExamples } from '../output.js';
import {
  printCurrentProfileSummary,
  printProfileList,
  switchProfileSelection,
} from '../profile-display.js';

export function registerProfileCommand(program: Command): void {
  const cmd = program.command('profile').description('Manage account profiles');

  const listCmd = cmd
    .command('list')
    .description('List all configured profiles')
    .action(() => {
      printProfileList(listProfileSummaries(), {
        annotateActive: true,
        emptyMessage:
          'No profiles configured. Run `vendo login` to create one.',
      });
    });

  addExamples(listCmd, ['vendo profile list']);

  const currentCmd = cmd
    .command('current')
    .description('Show the current effective profile')
    .action(() => {
      const config = getEffectiveConfig();
      const profiles = listProfileSummaries();
      const currentProfile = profiles.find((profile) => profile.active);
      printCurrentProfileSummary(config, currentProfile);
    });

  addExamples(currentCmd, ['vendo profile current']);

  const switchCmd = cmd
    .command('switch [profile]')
    .description('Switch to a different profile')
    .option(
      '--account <accountId>',
      'Switch by account ID instead of profile name',
    )
    .action(async (profileName?: string, opts?: { account?: string }) => {
      await switchProfileSelection(listProfileSummaries(), {
        profileName,
        accountId: opts?.account,
        emptyMessage: 'No profiles yet. Run `vendo login` to create one.',
        listCommand: 'vendo profile list',
        profileCommand: 'vendo profile switch',
        verifyHint: '`vendo profile current` or `vendo whoami`',
      });
    });

  addExamples(switchCmd, [
    'vendo profile switch',
    'vendo profile switch myprofile',
    'vendo profile switch --account <accountId>',
  ]);
}

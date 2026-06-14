import { Command } from 'commander';

import { clearActiveProfile, deleteConfig, getApiKey } from '../config.js';
import { addExamples, c, printError, printSuccess } from '../output.js';

export function registerLogoutCommand(program: Command): void {
  const cmd = program
    .command('logout')
    .description('Remove stored credentials')
    .option('--all', 'Remove every saved profile, not just the active one')
    .action((opts: { all?: boolean }) => {
      if (opts.all) {
        const deleted = deleteConfig();
        if (deleted) {
          printSuccess('Logged out. All profiles removed.');
        } else {
          console.log(c.dim('No configuration file found.'));
        }
        return;
      }

      if (!getApiKey()) {
        printError('Not currently logged in.');
        return;
      }

      const cleared = clearActiveProfile();
      if (cleared) {
        printSuccess(`Logged out of profile "${cleared}".`);
      } else {
        printSuccess('Logged out.');
      }
    });

  addExamples(cmd, ['vendo logout', 'vendo logout --all']);
}

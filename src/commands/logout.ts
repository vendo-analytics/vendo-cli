import { Command } from 'commander';

import { getApiKey, saveConfig } from '../config.js';
import { addExamples, printError, printSuccess } from '../output.js';

export function registerLogoutCommand(program: Command): void {
  const cmd = program
    .command('logout')
    .description('Remove stored credentials')
    .action(() => {
      const existing = getApiKey();

      if (!existing) {
        printError('Not currently logged in.');
        return;
      }

      saveConfig({ apiKey: undefined });
      printSuccess('Logged out. API key removed from config.');
    });

  addExamples(cmd, ['vendo logout']);
}

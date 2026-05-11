import { Command } from 'commander';
import { spawnSync } from 'node:child_process';
import { homedir } from 'node:os';
import { join } from 'node:path';

import { addExamples, c, exitWithError, printSuccess } from '../output.js';

const INSTALL_URL = 'https://app2.vendodata.com/install.sh';

export function registerSelfUpdateCommand(program: Command): void {
  const cmd = program
    .command('self-update')
    .description('Update the Vendo CLI using the hosted installer')
    .option('--version <version>', 'Install a specific version')
    .action((opts: { version?: string }) => {
      const env = { ...process.env };
      if (opts.version) {
        env.VENDO_VERSION = opts.version;
      }

      const result = spawnSync(
        'bash',
        ['-lc', `curl -fsSL ${INSTALL_URL} | bash`],
        {
          env,
          stdio: 'inherit',
        },
      );

      if (result.error) {
        exitWithError(result.error);
      }

      if (result.status !== 0) {
        process.exit(result.status ?? 1);
      }

      printSuccess('Vendo CLI updated.');

      const standardInstallPath = join(homedir(), '.local', 'bin', 'vendo');
      if (process.execPath !== standardInstallPath) {
        console.log();
        console.log(
          c.dim(
            `The installer writes to ${standardInstallPath}. If you use a custom path, switch to that binary after update.`,
          ),
        );
      }
    });

  addExamples(cmd, ['vendo self-update', 'vendo self-update --version 0.3.0']);
}

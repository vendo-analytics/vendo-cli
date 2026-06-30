import { Command } from 'commander';

import { registerAppsCommand } from './commands/apps.js';
import { registerCatalogCommand } from './commands/catalog.js';
import { registerCompletionsCommand } from './commands/completions.js';
import { registerConfigCommand } from './commands/config.js';
import { registerDoctorCommand } from './commands/doctor.js';
import { registerInitCommand } from './commands/init.js';
import { registerIntegrationsCommand } from './commands/integrations.js';
import { registerJobsCommand } from './commands/jobs.js';
import { registerLoginCommand } from './commands/login.js';
import { registerLogoutCommand } from './commands/logout.js';
import { registerMcpCommand } from './commands/mcp.js';
import { registerMeasurementCommand } from './commands/measurement.js';
import { registerMetricsCommand } from './commands/metrics.js';
import { registerModelsCommand } from './commands/models.js';
import { registerProfileCommand } from './commands/profile.js';
import { registerSelfUpdateCommand } from './commands/self-update.js';
import { registerSourcesCommand } from './commands/sources.js';
import { registerStatusCommand } from './commands/status.js';
import { registerWhoamiCommand } from './commands/whoami.js';
import { setProfileOverride } from './config.js';
import { setDebugEnabled } from './debug.js';
import { c } from './output.js';

declare const __CLI_VERSION__: string;

const program = new Command();
const registerCommands = [
  registerLoginCommand,
  registerInitCommand,
  registerLogoutCommand,
  registerConfigCommand,
  registerProfileCommand,
  registerStatusCommand,
  registerWhoamiCommand,
  registerAppsCommand,
  registerSourcesCommand,
  registerIntegrationsCommand,
  registerJobsCommand,
  registerCatalogCommand,
  registerMetricsCommand,
  registerModelsCommand,
  registerMeasurementCommand,
  registerMcpCommand,
  registerCompletionsCommand,
  registerDoctorCommand,
  registerSelfUpdateCommand,
];

program
  .name('vendo')
  .description('Vendo CLI — manage your data pipeline from the terminal')
  .version(__CLI_VERSION__)
  .option('--profile <name>', 'Use a specific account profile')
  .option('--debug', 'Enable verbose request diagnostics')
  .configureOutput({
    outputError: (str, _write) => {
      const cleaned = str.replace(/^error:\s*/i, '').trim();
      console.error(c.red('Error:'), cleaned);

      // Suggest --help for the failing command (skip flags and their values)
      const rawArgs = process.argv.slice(2);
      const args = rawArgs.reduce<string[]>((acc, arg, i) => {
        if (arg.startsWith('-')) return acc;
        if (
          i > 0 &&
          rawArgs[i - 1]?.startsWith('-') &&
          !rawArgs[i - 1]?.startsWith('--no-')
        )
          return acc;
        acc.push(arg);
        return acc;
      }, []);
      if (args.length > 0) {
        console.error('');
        console.error(
          c.dim(`Run 'vendo ${args.join(' ')} --help' for usage and examples.`),
        );
      }
    },
  })
  .hook('preAction', (thisCommand) => {
    const opts = thisCommand.opts();
    if (opts.profile) {
      setProfileOverride(opts.profile);
    }
    if (opts.debug) {
      setDebugEnabled(true);
    }
  });

for (const registerCommand of registerCommands) {
  registerCommand(program);
}

program.parse();

import { Command } from 'commander';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname } from 'node:path';

import { getEffectiveConfig, maskApiKey } from '../config.js';
import {
  type MeResponse,
  IdentityFetchError,
  fetchIdentity,
} from '../identity.js';
import { addExamples, c, printJson } from '../output.js';

type CheckStatus = 'ok' | 'warn' | 'fail';

interface DoctorCheck {
  name: string;
  status: CheckStatus;
  detail: string;
  remediation?: string;
}

export function registerDoctorCommand(program: Command): void {
  const cmd = program
    .command('doctor')
    .description('Run local configuration and connectivity checks')
    .option('--json', 'Output raw JSON')
    .action(async (opts: { json?: boolean }) => {
      const config = getEffectiveConfig();
      const checks: DoctorCheck[] = [];
      const standardBinaryPath = `${homedir()}/.local/bin/vendo`;
      const currentBinaryDir = dirname(process.execPath);
      const pathSegments = (process.env.PATH ?? '').split(':').filter(Boolean);
      const shellName = (process.env.SHELL ?? '').split('/').pop();
      const shellLabel = shellName || 'unknown shell';

      if (process.execPath === standardBinaryPath) {
        checks.push({
          name: 'CLI binary',
          status: 'ok',
          detail: process.execPath,
        });
      } else {
        checks.push({
          name: 'CLI binary',
          status: 'warn',
          detail: `${process.execPath} (standard install path is ${standardBinaryPath})`,
          remediation:
            'Reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash` if you want the managed install path.',
        });
      }

      if (
        pathSegments.includes(currentBinaryDir) ||
        pathSegments.includes(dirname(standardBinaryPath))
      ) {
        checks.push({
          name: 'PATH',
          status: 'ok',
          detail: `${currentBinaryDir} is available in PATH`,
        });
      } else {
        checks.push({
          name: 'PATH',
          status: 'fail',
          detail: `${currentBinaryDir} is not available in PATH`,
          remediation: getPathRemediation(shellName),
        });
      }

      checks.push({
        name: 'Config file',
        status: config.configExists ? 'ok' : 'warn',
        detail: config.configExists
          ? config.configPath
          : `${config.configPath} (not found yet)`,
        remediation: config.configExists
          ? undefined
          : 'Run `vendo init` to create and populate CLI config.',
      });

      if (config.selectedProfile) {
        checks.push({
          name: 'Selected profile',
          status: config.selectedProfileExists ? 'ok' : 'warn',
          detail: config.selectedProfileExists
            ? config.selectedProfile
            : `${config.selectedProfile} (not found in config)`,
          remediation: config.selectedProfileExists
            ? undefined
            : 'Run `vendo config use` to switch profiles, or `vendo init` to create one.',
        });
      } else {
        checks.push({
          name: 'Selected profile',
          status: 'warn',
          detail: 'No active profile selected',
          remediation: 'Run `vendo init` or `vendo config use <profile>`.',
        });
      }

      if (config.apiKey) {
        checks.push({
          name: 'API key',
          status: 'ok',
          detail: `${maskApiKey(config.apiKey)} (${formatSource(config.apiKeySource)})`,
        });
      } else {
        checks.push({
          name: 'API key',
          status: 'fail',
          detail: 'Missing',
          remediation:
            'Run `vendo login` or `vendo config set --api-key <key>`.',
        });
      }

      checks.push({
        name: 'Base URL',
        status: 'ok',
        detail: `${config.baseUrl} (${formatSource(config.baseUrlSource)})`,
      });

      if (config.accountId) {
        checks.push({
          name: 'Account ID',
          status: 'ok',
          detail: `${config.accountId} (${formatSource(config.accountIdSource)})`,
        });
      } else {
        checks.push({
          name: 'Account ID',
          status: 'fail',
          detail: 'Missing',
          remediation:
            'Run `vendo config set --account <account-id>` or set `VENDO_ACCOUNT_ID`.',
        });
      }

      const completionCheck = getCompletionCheck(shellName);
      checks.push(completionCheck);

      let identity: MeResponse | undefined;

      if (config.apiKey && config.accountId) {
        try {
          identity = await fetchIdentity(
            config.apiKey,
            config.accountId,
            config.baseUrl,
          );
          checks.push({
            name: 'API auth',
            status: 'ok',
            detail: `Authenticated as ${identity.accountName ?? identity.accountSlug ?? identity.accountId}`,
          });
        } catch (err) {
          if (err instanceof IdentityFetchError) {
            checks.push({
              name: 'API auth',
              status: 'fail',
              detail: `HTTP ${err.status}: ${err.statusText}`,
              remediation: getApiAuthRemediation(err.status),
            });
          } else {
            checks.push({
              name: 'API auth',
              status: 'fail',
              detail: err instanceof Error ? err.message : String(err),
              remediation:
                'Check network access and run `vendo whoami --debug` to inspect the failing request.',
            });
          }
        }
      } else {
        checks.push({
          name: 'API auth',
          status: 'warn',
          detail: 'Skipped because API key or account ID is missing',
        });
      }

      const summary = summarizeChecks(checks);
      const suggestions = Array.from(
        new Set(
          checks
            .filter((check) => check.remediation)
            .map((check) => check.remediation!),
        ),
      );

      if (opts.json) {
        printJson({
          summary,
          checks,
          suggestions,
          identity,
          shell: shellLabel,
        });
        process.exit(summary.fail > 0 ? 1 : 0);
      }

      console.log(c.bold('Vendo CLI Doctor'));
      console.log();

      for (const check of checks) {
        console.log(
          `${formatMarker(check.status)} ${check.name}: ${check.detail}`,
        );
        if (check.remediation && check.status !== 'ok') {
          console.log(`       ${c.dim(`Fix: ${check.remediation}`)}`);
        }
      }

      console.log();
      console.log(
        c.dim(
          `Summary: ${summary.ok} ok, ${summary.warn} warnings, ${summary.fail} failures`,
        ),
      );

      if (suggestions.length > 0) {
        console.log();
        console.log(c.bold('Suggested next steps'));
        for (const suggestion of Array.from(new Set(suggestions))) {
          console.log(`  - ${suggestion}`);
        }
      }

      process.exit(summary.fail > 0 ? 1 : 0);
    });

  addExamples(cmd, ['vendo doctor', 'vendo doctor --json']);
}

function getCompletionCheck(shellName?: string): DoctorCheck {
  const home = homedir();

  switch (shellName) {
    case 'bash':
      if (
        existsSync(`${home}/.local/share/vendo/completions/vendo.bash`) &&
        rcFileContains(`${home}/.bashrc`, '# >>> vendo completions >>>')
      ) {
        return {
          name: 'Shell completions',
          status: 'ok',
          detail: 'Bash completions are installed in ~/.bashrc',
        };
      }
      return {
        name: 'Shell completions',
        status: 'warn',
        detail: 'Bash completions are not installed yet',
        remediation:
          'Run `vendo completions bash` or reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash`.',
      };
    case 'zsh':
      if (
        existsSync(`${home}/.local/share/vendo/completions/vendo.zsh`) &&
        rcFileContains(`${home}/.zshrc`, '# >>> vendo completions >>>')
      ) {
        return {
          name: 'Shell completions',
          status: 'ok',
          detail: 'Zsh completions are installed in ~/.zshrc',
        };
      }
      return {
        name: 'Shell completions',
        status: 'warn',
        detail: 'Zsh completions are not installed yet',
        remediation:
          'Run `vendo completions zsh` or reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash`.',
      };
    case 'fish':
      if (existsSync(`${home}/.config/fish/completions/vendo.fish`)) {
        return {
          name: 'Shell completions',
          status: 'ok',
          detail: 'Fish completions are installed',
        };
      }
      return {
        name: 'Shell completions',
        status: 'warn',
        detail: 'Fish completions are not installed yet',
        remediation:
          'Run `vendo completions fish` or reinstall with `curl -fsSL https://app2.vendodata.com/install.sh | bash`.',
      };
    default:
      return {
        name: 'Shell completions',
        status: 'warn',
        detail: 'Current shell could not be detected automatically',
        remediation:
          'Run `vendo completions <shell>` manually after choosing your shell, or reinstall with the hosted installer.',
      };
  }
}

function getPathRemediation(shellName?: string): string {
  const exportLine = 'export PATH="$HOME/.local/bin:$PATH"';
  const rcFile = getShellRcFile(shellName);

  if (!rcFile) {
    return `Add \`${exportLine}\` to your shell profile, then restart your shell.`;
  }

  return `Add \`${exportLine}\` to \`${rcFile}\`, then restart your shell.`;
}

function getApiAuthRemediation(status: number): string {
  if (status === 401 || status === 403) {
    return 'Run `vendo login` to refresh credentials, then retry `vendo whoami`.';
  }

  if (status === 404) {
    return 'Verify the configured account and base URL with `vendo whoami --debug`.';
  }

  return 'Run `vendo whoami --debug` to inspect the failing request and response.';
}

function getShellRcFile(shellName?: string): string | undefined {
  switch (shellName) {
    case 'bash':
      return '~/.bashrc';
    case 'zsh':
      return '~/.zshrc';
    case 'fish':
      return '~/.config/fish/config.fish';
    default:
      return undefined;
  }
}

function summarizeChecks(checks: DoctorCheck[]): Record<CheckStatus, number> {
  return checks.reduce(
    (acc, check) => {
      acc[check.status] += 1;
      return acc;
    },
    { ok: 0, warn: 0, fail: 0 } satisfies Record<CheckStatus, number>,
  );
}

function formatSource(source: string): string {
  switch (source) {
    case 'env':
      return 'from env';
    case 'profile':
      return 'from profile';
    case 'default':
      return 'default';
    default:
      return source;
  }
}

function formatMarker(status: CheckStatus): string {
  switch (status) {
    case 'ok':
      return c.green('[ok]');
    case 'warn':
      return c.yellow('[warn]');
    case 'fail':
      return c.red('[fail]');
  }
}

function rcFileContains(path: string, marker: string): boolean {
  try {
    return readFileSync(path, 'utf-8').includes(marker);
  } catch {
    return false;
  }
}

import { Command } from 'commander';
import { exec } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { createServer } from 'node:http';
import { createInterface } from 'node:readline';

import {
  DEFAULT_BASE_URL,
  getBaseUrl,
  resolveLoginBaseUrl,
  saveProfile,
} from '../config.js';
import {
  type MeResponse,
  IdentityFetchError,
  fetchIdentity,
} from '../identity.js';
import {
  addExamples,
  c,
  exitWithError,
  printSuccess,
  runAction,
} from '../output.js';
import { checkForUpdates } from '../update-check.js';

interface LoginResult {
  account: string;
  accountId?: string;
  baseUrl: string;
}

export function registerLoginCommand(program: Command): void {
  const cmd = program
    .command('login')
    .description('Authenticate with your Vendo account')
    .option(
      '--api-key <key>',
      'API key for headless/CI login (requires --account)',
    )
    .option(
      '--account <id>',
      'Account ID for headless/CI login (requires --api-key)',
    )
    .option(
      '--env <environment>',
      'Target instance: "staging" or "prod" (default: VENDO_API_URL/profile, else prod)',
    )
    .option('--base-url <url>', 'Explicit API base URL (overrides --env)')
    .action(
      async (opts: {
        apiKey?: string;
        account?: string;
        env?: string;
        baseUrl?: string;
      }) => {
      // Resolve the target instance up front so a typo'd --env / --base-url
      // fails here instead of opening a browser at the wrong instance
      // (VE-1563: a plain login silently targeted prod).
      let baseUrl: string;
      try {
        baseUrl = resolveLoginBaseUrl(opts);
      } catch (err) {
        exitWithError(err);
      }

      // Headless path: both --api-key and --account provided
      if (opts.apiKey || opts.account) {
        if (!opts.apiKey || !opts.account) {
          exitWithError(
            'Both --api-key and --account are required for headless login.\n' +
              c.dim('  Example: vendo login --api-key <key> --account <id>'),
          );
        }

        const identity = await runAction('Validating credentials...', () =>
          validateCredentials(opts.apiKey!, opts.account!, baseUrl),
        );

        const profileName =
          identity.accountSlug ?? identity.accountName ?? opts.account;

        saveProfile(profileName, {
          apiKey: opts.apiKey,
          accountId: opts.account,
          ...(baseUrl !== DEFAULT_BASE_URL && { baseUrl }),
        });

        printLoginSuccess({
          account: profileName,
          accountId: opts.account,
          baseUrl,
        });
        process.exit(0);
      }

      // Interactive browser path (existing)
      try {
        const result = await runBrowserLogin(baseUrl);
        printLoginSuccess(result);
        process.exit(0);
      } catch (err) {
        exitWithError(err);
      }
    },
    );

  addExamples(cmd, [
    'vendo login',
    'vendo login --env staging',
    'vendo login --api-key vendo_sk_... --account <account-id>',
  ]);
}

async function validateCredentials(
  apiKey: string,
  accountId: string,
  baseUrl: string,
): Promise<MeResponse> {
  try {
    return await fetchIdentity(apiKey, accountId, baseUrl);
  } catch (err) {
    if (err instanceof IdentityFetchError) {
      throw new Error(
        `Credential validation failed (HTTP ${err.status}). Check your API key and account ID.`,
      );
    }
    throw err;
  }
}

export async function runBrowserLogin(
  baseUrlOverride?: string,
): Promise<LoginResult> {
  await checkForUpdates();

  const state = randomBytes(16).toString('hex');
  const baseUrl = baseUrlOverride ?? getBaseUrl();
  const { key, account, accountId } = await startAuthFlow(baseUrl, state);

  saveProfile(account, {
    apiKey: key,
    accountId,
    ...(baseUrl !== DEFAULT_BASE_URL && { baseUrl }),
  });

  return {
    account,
    accountId,
    baseUrl,
  };
}

export function printLoginSuccess(result: LoginResult): void {
  console.log();
  printSuccess(`Authenticated as ${c.bold(result.account)}`);
  console.log(c.dim(`Active profile: ${result.account}`));
  if (result.accountId) {
    console.log(c.dim(`Account ID saved: ${result.accountId}`));
  }
  console.log();
  console.log(c.bold('Next steps'));
  console.log('  vendo whoami');
  console.log('  vendo status');
  console.log('  vendo doctor');
}

function startAuthFlow(
  baseUrl: string,
  state: string,
): Promise<{ key: string; account: string; accountId?: string }> {
  return new Promise((resolve, reject) => {
    const server = createServer((req, res) => {
      const url = new URL(req.url ?? '/', `http://localhost`);

      if (url.pathname !== '/callback') {
        res.writeHead(404);
        res.end();
        return;
      }

      const receivedState = url.searchParams.get('state');
      const key = url.searchParams.get('key');
      const account = url.searchParams.get('account');
      const accountId = url.searchParams.get('account_id');
      const error = url.searchParams.get('error');

      if (error) {
        sendHtml(res, 'Authentication Failed', error);
        reject(new Error(error));
        setTimeout(cleanup, 1000);
        return;
      }

      if (receivedState !== state) {
        sendHtml(res, 'Invalid Request', 'State mismatch. Please try again.');
        reject(new Error('State mismatch — possible CSRF attack'));
        setTimeout(cleanup, 1000);
        return;
      }

      if (!key || !account) {
        sendHtml(res, 'Missing Credentials', 'Please try again.');
        reject(new Error('Missing key or account in callback'));
        setTimeout(cleanup, 1000);
        return;
      }

      sendHtml(
        res,
        'Authenticated',
        'You can close this window and return to the terminal.',
      );
      resolve({ key, account, accountId: accountId ?? undefined });

      // Delay cleanup so the browser receives the HTML response
      setTimeout(cleanup, 1000);
    });

    const timeout = setTimeout(
      () => {
        cleanup();
        reject(new Error('Authentication timed out after 5 minutes'));
      },
      5 * 60 * 1000,
    );

    function cleanup() {
      clearTimeout(timeout);
      server.close();
    }

    // Listen on random port on IPv4 localhost (browsers resolve localhost to 127.0.0.1)
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      if (!addr || typeof addr === 'string') {
        cleanup();
        reject(new Error('Failed to start local server'));
        return;
      }

      const port = addr.port;
      const authUrl = `${baseUrl}/cli-auth?port=${port}&state=${state}`;

      console.log('Login at:');
      console.log(authUrl);

      const rl = createInterface({
        input: process.stdin,
        output: process.stdout,
      });
      rl.question('Press ENTER to open in the browser...', () => {
        rl.close();
        openBrowser(authUrl);
      });

      console.log();
      console.log(c.dim('Waiting for authorization...'));
    });
  });
}

function openBrowser(url: string): void {
  const cmd =
    process.platform === 'darwin'
      ? 'open'
      : process.platform === 'win32'
        ? 'start'
        : 'xdg-open';

  exec(`${cmd} "${url}"`, (err) => {
    if (err) {
      console.log(
        c.dim(
          `Could not open browser automatically. Please visit the URL above.`,
        ),
      );
    }
  });
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function sendHtml(
  res: import('node:http').ServerResponse,
  title: string,
  message: string,
): void {
  const safeTitle = escapeHtml(title);
  const safeMessage = escapeHtml(message);
  res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
  res.end(`<!DOCTYPE html>
<html>
<head><title>Vendo CLI</title>
<style>
  body { font-family: system-ui, sans-serif; display: flex; align-items: center;
    justify-content: center; min-height: 100vh; margin: 0; background: #fafafa; }
  .card { background: white; border-radius: 12px; padding: 2rem 3rem;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1); text-align: center; }
  h1 { margin: 0 0 0.5rem; font-size: 1.5rem; }
  p { color: #666; margin: 0; }
</style>
</head>
<body><div class="card"><h1>${safeTitle}</h1><p>${safeMessage}</p></div></body>
</html>`);
}

import { Command } from 'commander';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';

import { ClientError, getClient } from '../client.js';
import {
  addExamples,
  c,
  colorStatus,
  confirm,
  createTable,
  printCount,
  printDryRun,
  printField,
  printJson,
  printLabel,
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
  timeAgo,
} from '../output.js';

interface AppItem {
  id: string;
  appType: string;
  displayName: string;
  permissions: string[];
  state: string;
  errorMessage?: string | null;
  lastSyncAt?: string | null;
  createdAt: string;
}

interface AppDetail extends AppItem {
  config?: unknown;
  consecutiveFailureCount?: number;
  updatedAt: string;
}

// ── role ↔ permissions bridge ────────────────────────────────────────────────
// The API migrated from a `role` field to granular `permissions`. The CLI keeps
// the familiar `--role source,destination` UX and derives permissions from it
// client-side (conservative — never grants campaign_changes by default).
//
// Detection-set vs grant-set divergence is intentional:
//   - DESTINATION_PERMISSIONS below is the *detection* set used by
//     capabilityLabel to recognise a destination app (so it must include
//     campaign_changes and the other write scopes the API may return).
//   - permissionsFromRoles is the *grant* set requested by `--role`. It must
//     stay conservative and NEVER grant campaign_changes (or other write
//     scopes) implicitly from a role alone.
const SOURCE_PERMISSIONS = ['performance_data'];
const DESTINATION_PERMISSIONS = [
  'send_conversions',
  'sync_audiences',
  'campaign_changes',
  'campaign_adjust_bids',
  'campaign_adjust_spend',
  'campaign_toggle',
];

// `--role` derives a conservative superset of permissions: it asks for what the
// role *might* need without knowing the per-platform capability matrix (e.g.
// not every destination supports send_conversions). The CLI deliberately does
// NOT replicate that matrix — the API clamps the requested permissions down to
// the platform's actual allowed set (dropping unsupported scopes, erroring only
// if none remain), so over-requesting here is safe.
function permissionsFromRoles(roles: string[]): string[] {
  const perms: string[] = [];
  if (roles.includes('source') || roles.includes('both')) {
    perms.push(...SOURCE_PERMISSIONS);
  }
  if (roles.includes('destination') || roles.includes('both')) {
    perms.push('send_conversions', 'sync_audiences');
  }
  return perms.length > 0 ? perms : [...SOURCE_PERMISSIONS];
}

/** Human-readable capability label derived from permissions (for display). */
function capabilityLabel(permissions: string[] = []): string {
  const isSource = permissions.some((p) => SOURCE_PERMISSIONS.includes(p));
  const isDestination = permissions.some((p) =>
    DESTINATION_PERMISSIONS.includes(p),
  );
  if (isSource && isDestination) return 'source, destination';
  if (isDestination) return 'destination';
  if (isSource) return 'source';
  return '—';
}

export function registerAppsCommand(program: Command): void {
  const cmd = program.command('apps').description('Manage app connections');

  // apps list
  const listCmd = cmd
    .command('list')
    .description('List all app connections')
    .option('--state <state>', 'Filter by state (active, inactive)')
    .option('--type <type>', 'Filter by app type')
    .option(
      '--role <role>',
      'Filter by capability (source, destination) — derived from permissions',
    )
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const res = await runAction('Fetching apps...', () =>
        getClient().get<AppItem[]>('/apps', {
          state: opts.state,
          app_type: opts.type,
          capability: opts.role,
          limit: opts.limit,
          offset: opts.offset,
        }),
      );

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        printField(
          res.data as unknown as Record<string, unknown>[],
          opts.output,
        );
        return;
      }

      const table = createTable([
        'ID',
        'Name',
        'Type',
        'Role',
        'State',
        'Last Sync',
      ]);

      for (const app of res.data) {
        table.push([
          c.dim(shortId(app.id)),
          app.displayName,
          app.appType,
          capabilityLabel(app.permissions),
          colorStatus(app.state),
          timeAgo(app.lastSyncAt),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, 'app');
    });

  addExamples(listCmd, [
    'vendo apps list',
    'vendo apps list --role source',
    'vendo apps list --output id',
  ]);

  // apps get
  const getCmd = cmd
    .command('get <appId>')
    .description('Get app details')
    .option('--json', 'Output raw JSON')
    .action(async (appId: string, opts: { json?: boolean }) => {
      const res = await runAction('Fetching app...', () =>
        getClient().get<AppDetail>(`/apps/${appId}`),
      );

      if (opts.json) {
        printJson(res);
        return;
      }

      const app = res.data;
      console.log();
      console.log(c.bold(app.displayName), c.dim(`(${app.appType})`));
      console.log();
      console.log(`  ID:          ${app.id}`);
      console.log(`  Type:        ${app.appType}`);
      console.log(`  Capability:  ${capabilityLabel(app.permissions)}`);
      console.log(`  Permissions: ${(app.permissions ?? []).join(', ') || '—'}`);
      console.log(`  State:       ${colorStatus(app.state)}`);
      console.log(`  Last Sync:   ${timeAgo(app.lastSyncAt)}`);
      console.log(`  Created:     ${timeAgo(app.createdAt)}`);
      if (app.errorMessage) {
        console.log(`  Error:       ${c.red(app.errorMessage)}`);
      }
      if (
        typeof app.consecutiveFailureCount === 'number' &&
        app.consecutiveFailureCount > 0
      ) {
        console.log(
          `  Failures:    ${c.red(String(app.consecutiveFailureCount))} consecutive`,
        );
      }
    });

  addExamples(getCmd, [
    'vendo apps get <appId>',
    'vendo apps get <appId> --json',
  ]);

  // apps pause
  const pauseCmd = cmd
    .command('pause <appId>')
    .description('Pause an app connection')
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        appId: string,
        opts: { json?: boolean; dryRun?: boolean; output?: string },
      ) => {
        if (opts.dryRun) {
          printDryRun('pause', 'app', appId);
          return;
        }

        const res = await runAction('Pausing app...', () =>
          getClient().post(`/apps/${appId}/pause`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(appId);
          return;
        }

        printSuccess(`App ${shortId(appId)} paused.`);
      },
    );

  addExamples(pauseCmd, [
    'vendo apps pause <appId>',
    'vendo apps pause <appId> --dry-run',
  ]);

  // apps resume
  const resumeCmd = cmd
    .command('resume <appId>')
    .description('Resume a paused app connection')
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        appId: string,
        opts: { json?: boolean; dryRun?: boolean; output?: string },
      ) => {
        if (opts.dryRun) {
          printDryRun('resume', 'app', appId);
          return;
        }

        const res = await runAction('Resuming app...', () =>
          getClient().post(`/apps/${appId}/resume`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(appId);
          return;
        }

        printSuccess(`App ${shortId(appId)} resumed.`);
      },
    );

  addExamples(resumeCmd, [
    'vendo apps resume <appId>',
    'vendo apps resume <appId> --dry-run',
  ]);

  // apps delete
  const deleteCmd = cmd
    .command('delete <appId>')
    .description('Delete an app connection (soft delete)')
    .option('--json', 'Output raw JSON')
    .option('-y, --yes', 'Skip confirmation prompt')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        appId: string,
        opts: {
          json?: boolean;
          yes?: boolean;
          dryRun?: boolean;
          output?: string;
        },
      ) => {
        if (opts.dryRun) {
          printDryRun('delete', 'app', appId);
          return;
        }

        if (!opts.yes && !opts.json) {
          const ok = await confirm(`Delete app ${shortId(appId)}?`);
          if (!ok) return;
        }

        const res = await runAction('Deleting app...', () =>
          getClient().delete(`/apps/${appId}`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(appId);
          return;
        }

        printSuccess(`App ${shortId(appId)} deleted.`);
      },
    );

  addExamples(deleteCmd, [
    'vendo apps delete <appId>',
    'vendo apps delete <appId> --yes',
    'vendo apps delete <appId> --dry-run',
  ]);

  // apps create
  const createCmd = cmd
    .command('create')
    .description('Create a new app connection')
    .requiredOption(
      '--type <appType>',
      'App type (e.g. google_ads, onesignal). See: vendo catalog list',
    )
    .requiredOption(
      '--name <displayName>',
      'Human-readable name for this connection',
    )
    .option(
      '--role <role>',
      'Comma-separated capability: source, destination (derives default permissions)',
      'source',
    )
    .option(
      '--permissions <permissions>',
      'Comma-separated granular permissions (e.g. performance_data,send_conversions). Overrides --role.',
    )
    .option(
      '--credentials-file <path>',
      'Path to a JSON file with the credential payload',
    )
    .option(
      '--config-file <path>',
      'Path to a JSON file with app-specific config',
    )
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (opts) => {
      // Explicit --permissions wins; otherwise derive from --role capability.
      const permissions: string[] = opts.permissions
        ? String(opts.permissions)
            .split(',')
            .map((p: string) => p.trim())
            .filter(Boolean)
        : permissionsFromRoles(
            String(opts.role)
              .split(',')
              .map((r: string) => r.trim())
              .filter(Boolean),
          );

      // No credentials → request the browser-assisted OAuth flow. The API
      // bounces back 202 with an authUrl, we open the browser and wait for
      // the drawer to POST back to our local port.
      const needsOAuthAssist = !opts.credentialsFile;

      if (needsOAuthAssist) {
        const { appId } = await runBrowserOAuth({
          appType: opts.type,
          displayName: opts.name,
          permissions,
          config: opts.configFile ? readJsonFile(opts.configFile) : undefined,
        });

        const outputMode = resolveOutputMode(opts);
        if (outputMode === 'json') {
          printJson({ data: { id: appId } });
          return;
        }
        if (outputMode === 'field') {
          console.log(appId);
          return;
        }
        printSuccess(`App ${shortId(appId)} created via OAuth.`);
        return;
      }

      const body = {
        appType: opts.type,
        displayName: opts.name,
        permissions,
        credentials: readJsonFile(opts.credentialsFile),
        config: opts.configFile ? readJsonFile(opts.configFile) : undefined,
      };

      const res = await runAction('Creating app...', () =>
        getClient().post<AppDetail>('/apps', body),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      const app = res.data;
      if (!app) {
        printSuccess('App created.');
        return;
      }

      if (outputMode === 'field') {
        console.log(app.id);
        return;
      }

      printSuccess(
        `App ${c.bold(app.displayName)} (${shortId(app.id)}) created.`,
      );
      printLabel('Type', app.appType);
      printLabel('Capability', capabilityLabel(app.permissions));
      printLabel('State', colorStatus(app.state));
    });

  addExamples(createCmd, [
    'vendo apps create --type onesignal --name "OneSignal Prod" --role destination --credentials-file onesignal.json',
    'vendo apps create --type bigquery --name "Analytics BQ" --role source --credentials-file bq-sa.json',
  ]);

  // apps update
  const updateCmd = cmd
    .command('update <appId>')
    .description('Update an app connection')
    .option('--name <displayName>', 'New display name')
    .option(
      '--role <role>',
      'Comma-separated capability: source, destination (derives permissions)',
    )
    .option(
      '--permissions <permissions>',
      'Comma-separated granular permissions. Overrides --role.',
    )
    .option('--credentials-file <path>', 'Replace credentials from a JSON file')
    .option('--config-file <path>', 'Replace config from a JSON file')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (appId: string, opts) => {
      const body: Record<string, unknown> = {};
      if (opts.name) body.displayName = opts.name;
      if (opts.permissions) {
        body.permissions = String(opts.permissions)
          .split(',')
          .map((p: string) => p.trim())
          .filter(Boolean);
      } else if (opts.role) {
        body.permissions = permissionsFromRoles(
          String(opts.role)
            .split(',')
            .map((r: string) => r.trim())
            .filter(Boolean),
        );
      }
      if (opts.credentialsFile) {
        body.credentials = readJsonFile(opts.credentialsFile);
      }
      if (opts.configFile) body.config = readJsonFile(opts.configFile);

      if (Object.keys(body).length === 0) {
        throw new Error('Nothing to update — pass at least one flag.');
      }

      const res = await runAction('Updating app...', () =>
        getClient().patch<AppDetail>(`/apps/${appId}`, body),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        console.log(appId);
        return;
      }

      printSuccess(`App ${shortId(appId)} updated.`);
    });

  addExamples(updateCmd, [
    'vendo apps update <appId> --name "New name"',
    'vendo apps update <appId> --credentials-file rotated.json',
  ]);
}

function readJsonFile(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch (err) {
    const msg = err instanceof Error ? err.message : 'read error';
    throw new Error(`Failed to read ${path}: ${msg}`);
  }
}

interface OAuthAssistInput {
  appType: string;
  displayName: string;
  permissions: string[];
  config?: unknown;
}

interface OAuthAssistResponse {
  authRequired: true;
  sessionId: string;
  authUrl: string;
  expiresAt: string;
}

/**
 * Browser-assisted OAuth flow for `vendo apps create`.
 *
 * 1. POST /apps with oauthAssist.
 * 2. API returns 202 with an authUrl.
 * 3. We start a local HTTP server so the drawer can POST the outcome back.
 * 4. Open the browser.
 * 5. Wait for the callback (with a poll-based fallback in case POST fails).
 */
async function runBrowserOAuth(
  input: OAuthAssistInput,
): Promise<{ appId: string }> {
  const { port, waitForCallback, stop } = await startLocalCallbackServer();

  try {
    const client = getClient();
    const initResponse = await runAction('Opening OAuth session...', () =>
      client.post<OAuthAssistResponse>('/apps', {
        appType: input.appType,
        displayName: input.displayName,
        permissions: input.permissions,
        config: input.config,
        oauthAssist: { caller: 'cli', httpCallbackPort: port },
      }),
    );

    const oauth = initResponse.data;
    if (!oauth || !('authRequired' in oauth) || !oauth.sessionId) {
      throw new Error(
        'Server returned an unexpected response to the OAuth-assist request.',
      );
    }

    console.log(
      `\n${c.bold('Authorize in your browser:')}\n${oauth.authUrl}\n`,
    );

    await openInBrowser(oauth.authUrl);

    const settled = await runAction('Waiting for authorization...', () =>
      Promise.race([waitForCallback(), pollSession(client, oauth.sessionId)]),
    );

    if (settled.status !== 'completed' || !settled.appId) {
      throw new Error(settled.error ?? 'Authorization did not complete');
    }

    return { appId: settled.appId };
  } finally {
    stop();
  }
}

interface CallbackResult {
  status: 'completed' | 'failed' | 'cancelled';
  appId?: string;
  error?: string;
}

async function startLocalCallbackServer(): Promise<{
  port: number;
  waitForCallback: () => Promise<CallbackResult>;
  stop: () => void;
}> {
  return new Promise((resolve, reject) => {
    let resolveCallback: ((r: CallbackResult) => void) | null = null;
    let rejectCallback: ((e: Error) => void) | null = null;

    const server = createServer((req, res) => {
      const url = new URL(req.url ?? '/', 'http://127.0.0.1');
      if (url.pathname !== '/callback') {
        res.writeHead(404);
        res.end();
        return;
      }

      const sessionStatus = url.searchParams.get('status');
      let body = '';
      req.on('data', (chunk) => {
        body += chunk;
      });
      req.on('end', () => {
        let parsed: Partial<CallbackResult> = {};
        try {
          parsed = body ? (JSON.parse(body) as Partial<CallbackResult>) : {};
        } catch {
          // ignore — fall back to query params
        }

        const result: CallbackResult = {
          status:
            (parsed.status as CallbackResult['status']) ??
            (sessionStatus as CallbackResult['status']) ??
            'failed',
          appId: parsed.appId ?? undefined,
          error: parsed.error ?? undefined,
        };

        res.writeHead(200, { 'Content-Type': 'text/plain' });
        res.end('ok');

        if (resolveCallback) resolveCallback(result);
      });
    });

    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      if (!addr || typeof addr === 'string') {
        reject(new Error('Failed to start local callback server'));
        return;
      }

      resolve({
        port: addr.port,
        waitForCallback: () =>
          new Promise<CallbackResult>((res, rej) => {
            resolveCallback = res;
            rejectCallback = rej;
          }),
        stop: () => {
          server.close();
          if (rejectCallback) rejectCallback(new Error('server stopped'));
        },
      });
    });
  });
}

async function pollSession(
  client: ReturnType<typeof getClient>,
  sessionId: string,
): Promise<CallbackResult> {
  const deadline = Date.now() + 5 * 60 * 1000;
  let consecutiveNotFound = 0;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 2000));
    try {
      // GLOBAL route: GET /api/v1/apps/oauth-session/{sessionId}
      // (excluded from account-prefix injection in client.ts).
      const res = await client.get<{
        status: string;
        appId: string | null;
        error: string | null;
      }>(`/apps/oauth-session/${sessionId}`);
      consecutiveNotFound = 0;
      const data = res.data;
      if (!data) continue;
      if (data.status === 'completed') {
        return {
          status: 'completed',
          appId: data.appId ?? undefined,
        };
      }
      if (data.status === 'failed' || data.status === 'cancelled') {
        return {
          status: data.status as CallbackResult['status'],
          error: data.error ?? undefined,
        };
      }
    } catch (err) {
      // A 404 means the session does not exist server-side (expired,
      // deleted, or the polling route is unavailable). Tolerate a couple
      // in case of lag, then fail fast instead of silently polling for
      // 5 minutes. Other errors are treated as transient — keep polling.
      if (err instanceof ClientError && err.statusCode === 404) {
        consecutiveNotFound += 1;
        if (consecutiveNotFound >= 3) {
          return {
            status: 'failed',
            error:
              `OAuth session ${sessionId} was not found on the server ` +
              `(GET /apps/oauth-session/${sessionId} returned 404 ` +
              `${consecutiveNotFound} times). The session may have ` +
              'expired — run the command again.',
          };
        }
      }
    }
  }
  return { status: 'failed', error: 'Timed out after 5 minutes' };
}

async function openInBrowser(url: string): Promise<void> {
  const { exec } = await import('node:child_process');
  const cmd =
    process.platform === 'darwin'
      ? `open "${url}"`
      : process.platform === 'win32'
        ? `start "" "${url}"`
        : `xdg-open "${url}"`;
  await new Promise<void>((resolve) => {
    exec(cmd, () => resolve());
  });
}

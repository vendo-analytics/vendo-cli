import { Command } from 'commander';
import { connect } from 'node:net';
import { afterEach, describe, expect, it, vi } from 'vitest';

const { get, post, exec } = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  exec: vi.fn(),
}));

vi.mock('../client.js', async (original) => ({
  ...(await original<typeof import('../client.js')>()),
  getClient: () => ({ get, post }),
}));
vi.mock('../output.js', async (original) => ({
  ...(await original<typeof import('../output.js')>()),
  runAction: async (_message: string, action: () => Promise<unknown>) =>
    action(),
}));
// Opening the browser hands over to the scripted approval page below, so no
// real browser starts.
vi.mock('node:child_process', async (original) => ({
  ...(await original<typeof import('node:child_process')>()),
  exec,
}));

import { registerAppsCommand } from '../commands/apps.js';

const WEB_APP = 'https://stg.vendodata.com';
const AUTH_URL = `${WEB_APP}/oauth-drawer/s1`;

interface ApprovalPage {
  port: number;
  /** A request to the CLI's local `/callback?<query>`. */
  callback: (query: string, init?: RequestInit) => Promise<Response>;
  /** True while `vendo apps create` is still waiting for the outcome. */
  waiting: () => Promise<boolean>;
}

/** The approval page's POST, as `oauth-drawer-page.tsx` sends it. */
function drawerPost(payload: object): RequestInit {
  return {
    method: 'POST',
    headers: { Origin: WEB_APP, 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  };
}

/**
 * Runs `vendo apps create` without credentials (the OAuth-assist flow). When
 * the CLI opens the authorization URL, `approve` plays the approval page
 * against the CLI's local callback. The session poll's 2-second timer is
 * faked and never fires, so only the callback can settle the flow.
 */
async function createViaOAuth(
  approve: (page: ApprovalPage) => Promise<void>,
  flags: string[] = ['--json'],
): Promise<string[]> {
  vi.useFakeTimers({ toFake: ['setTimeout'] });
  let port = 0;
  post.mockImplementation(
    async (_path: string, body: { oauthAssist: { httpCallbackPort: number } }) => {
      port = body.oauthAssist.httpCallbackPort;
      return {
        data: {
          authRequired: true,
          sessionId: 's1',
          authUrl: AUTH_URL,
          expiresAt: '2026-10-05T12:05:00Z',
        },
      };
    },
  );
  let opened: () => void = () => {};
  const browserOpened = new Promise<void>((resolve) => {
    opened = resolve;
  });
  exec.mockImplementation((_command: string, done: () => void) => {
    opened();
    done();
  });

  let finished = false;
  const page = browserOpened.then(() =>
    approve({
      port,
      callback: (query, init) =>
        fetch(`http://127.0.0.1:${port}/callback?${query}`, init),
      waiting: async () => {
        await new Promise((resolve) => setImmediate(resolve));
        return !finished;
      },
    }),
  );
  const log = vi.spyOn(console, 'log').mockImplementation(() => {});
  const program = new Command();
  registerAppsCommand(program);
  const run = program.parseAsync(
    [
      'apps',
      'create',
      '--type',
      'hubspot',
      '--name',
      'HubSpot',
      '--permissions',
      'performance_data',
      ...flags,
    ],
    { from: 'user' },
  );
  void run.then(
    () => (finished = true),
    () => (finished = true),
  );

  await Promise.all([run, page]);
  expect(exec).toHaveBeenCalledWith(
    expect.stringContaining(AUTH_URL),
    expect.any(Function),
  );
  expect(post).toHaveBeenCalledWith(
    '/apps',
    expect.objectContaining({
      oauthAssist: { caller: 'cli', httpCallbackPort: port },
    }),
  );
  const lines = log.mock.calls.map((args) => args.join(' '));
  expect(lines[0]).toContain(`Authorize in your browser:`);
  expect(lines[0]).toContain(AUTH_URL);
  return lines.slice(1);
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('apps create OAuth callback', () => {
  it('answers the browser preflight and completes with the app id from the POST', async () => {
    const output = await createViaOAuth(async ({ callback, waiting }) => {
      const preflight = await callback('sessionId=s1&status=completed', {
        method: 'OPTIONS',
        headers: {
          Origin: WEB_APP,
          'Access-Control-Request-Method': 'POST',
          'Access-Control-Request-Headers': 'content-type',
          'Access-Control-Request-Private-Network': 'true',
        },
      });
      expect(preflight.status).toBe(204);
      const header = (name: string) => preflight.headers.get(name);
      expect(header('access-control-allow-origin')).toBe(WEB_APP);
      expect(header('access-control-allow-methods')).toBe('POST, GET, OPTIONS');
      expect(header('access-control-allow-headers')).toBe('Content-Type');
      expect(header('access-control-allow-private-network')).toBe('true');
      expect(header('access-control-max-age')).toBe('600');
      // The preflight is not the answer.
      expect(await waiting()).toBe(true);

      const res = await callback(
        'sessionId=s1&status=completed',
        drawerPost({ status: 'completed', appId: 'app-9' }),
      );
      expect(res.status).toBe(200);
      expect(res.headers.get('access-control-allow-origin')).toBe(WEB_APP);
      expect(await res.text()).toBe('ok');
    });
    expect(output.map((line) => JSON.parse(line))).toEqual([
      { data: { id: 'app-9' } },
    ]);
  });

  it('is not delayed by an idle connection', async () => {
    const output = await createViaOAuth(async ({ port, callback }) => {
      // A browser preconnect: connected, never sends a request.
      const idle = connect(port, '127.0.0.1');
      await new Promise((resolve) => idle.once('connect', resolve));
      try {
        const res = await callback(
          'sessionId=s1&status=completed',
          drawerPost({ status: 'completed', appId: 'app-9' }),
        );
        expect(res.status).toBe(200);
      } finally {
        idle.destroy();
      }
    }, []);
    expect(output).toEqual([expect.stringContaining('App app-9 created via OAuth.')]);
  });

  it('settles on a GET that carries a status', async () => {
    await expect(
      createViaOAuth(async ({ callback }) => {
        const res = await callback('status=failed&error=x');
        expect(res.status).toBe(200);
      }),
    ).rejects.toThrow(/^x$/);
  });

  it('keeps waiting through requests that are not the outcome', async () => {
    const output = await createViaOAuth(async ({ port, callback, waiting }) => {
      const other = await fetch(`http://127.0.0.1:${port}/favicon.ico`);
      expect(other.status).toBe(404);
      expect((await callback('sessionId=s1')).status).toBe(400);
      const put = await callback('status=completed', { method: 'PUT' });
      expect(put.status).toBe(405);
      expect(put.headers.get('allow')).toBe('POST, GET, OPTIONS');
      expect(await waiting()).toBe(true);

      await callback(
        'sessionId=s1&status=completed',
        drawerPost({ status: 'completed', appId: 'app-9' }),
      );
    });
    expect(output.map((line) => JSON.parse(line))).toEqual([
      { data: { id: 'app-9' } },
    ]);
  });
});

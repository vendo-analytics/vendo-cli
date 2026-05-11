import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ClientError } from '../client.js';
import { requireAccountId } from '../config.js';
import { setDebugEnabled } from '../debug.js';

// Mock config module
vi.mock('../config.js', () => ({
  requireApiKey: vi.fn(() => 'test-api-key'),
  getBaseUrl: vi.fn(() => 'https://api.test.com'),
  requireAccountId: vi.fn(() => 'acct-123'),
}));

// Get a reference to the only mocked function actually consulted in tests.
const mockRequireAccountId = vi.mocked(requireAccountId);

describe('client', () => {
  describe('ClientError', () => {
    it('has statusCode, code, and message', () => {
      const err = new ClientError('Not found', 404, 'RESOURCE_NOT_FOUND');
      expect(err.message).toBe('Not found');
      expect(err.statusCode).toBe(404);
      expect(err.code).toBe('RESOURCE_NOT_FOUND');
      expect(err.name).toBe('ClientError');
    });

    it('works without optional code', () => {
      const err = new ClientError('Timeout', 408);
      expect(err.statusCode).toBe(408);
      expect(err.code).toBeUndefined();
    });

    it('stores request metadata when provided', () => {
      const err = new ClientError('Bad request', 400, 'BAD_REQUEST', {
        requestId: 'cli-123',
        serverRequestId: 'req_456',
        details: { field: 'account_id' },
      });
      expect(err.requestId).toBe('cli-123');
      expect(err.serverRequestId).toBe('req_456');
      expect(err.details).toEqual({ field: 'account_id' });
    });

    it('is an instance of Error', () => {
      const err = new ClientError('test', 500);
      expect(err).toBeInstanceOf(Error);
      expect(err).toBeInstanceOf(ClientError);
    });
  });

  describe('friendlyHttpError (tested via client error handling)', () => {
    // friendlyHttpError is a private function, so we test it through the client's
    // error handling behavior by mocking fetch responses.

    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      // Reset the singleton client between tests
      // We can do this by clearing the module-level _client variable
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('throws ClientError with auth message for 401', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response('invalid', {
          status: 401,
          statusText: 'Unauthorized',
          headers: new Headers(),
        }),
      );

      // Reset singleton so constructor runs with our mocks
      const { getClient: freshGetClient } = await import('../client.js');

      // Need to force a new client (singleton cached from previous import)
      // We test by directly creating a request that triggers the error path
      try {
        const client = freshGetClient();
        await client.get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toContain('Authentication failed');
        expect((err as ClientError).statusCode).toBe(401);
      }
    });

    it('throws ClientError with permission message for 403', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response('forbidden', {
          status: 403,
          statusText: 'Forbidden',
          headers: new Headers(),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        const client = freshGetClient();
        await client.get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toContain('Permission denied');
      }
    });

    it('throws ClientError with not-found message for 404', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response('not found', {
          status: 404,
          statusText: 'Not Found',
          headers: new Headers(),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        const client = freshGetClient();
        await client.get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toContain('Resource not found');
      }
    });

    it('throws ClientError with rate-limit message for 429', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response('too many', {
          status: 429,
          statusText: 'Too Many Requests',
          headers: new Headers(),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        const client = freshGetClient();
        await client.get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toContain('Rate limit exceeded');
      }
    });

    it('uses error body message when API returns structured error', async () => {
      const errorBody = {
        error: {
          code: 'VALIDATION_ERROR',
          message: 'Invalid field: name is required',
        },
      };
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify(errorBody), {
          status: 422,
          statusText: 'Unprocessable Entity',
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        const client = freshGetClient();
        await client.get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toBe(
          'Invalid field: name is required',
        );
        expect((err as ClientError).code).toBe('VALIDATION_ERROR');
      }
    });

    it('falls back to HTTP status when error body has no message', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify({ error: {} }), {
          status: 500,
          statusText: 'Internal Server Error',
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        const client = freshGetClient();
        await client.get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toContain('HTTP 500');
      }
    });
  });

  describe('rate limit warning', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('warns when rate limit remaining is below 5', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify({ data: { ok: true } }), {
          status: 200,
          headers: new Headers({
            'Content-Type': 'application/json',
            'X-RateLimit-Remaining': '3',
            'X-RateLimit-Reset': String(Math.floor(Date.now() / 1000) + 60),
          }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const client = freshGetClient();
      await client.get('/me');

      expect(consoleWarnSpy).toHaveBeenCalledWith(
        expect.stringContaining('Rate limit low'),
      );
    });

    it('does not warn when rate limit remaining is 5 or more', async () => {
      const mockFetch = vi.mocked(globalThis.fetch);
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify({ data: { ok: true } }), {
          status: 200,
          headers: new Headers({
            'Content-Type': 'application/json',
            'X-RateLimit-Remaining': '50',
          }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const client = freshGetClient();
      await client.get('/me');

      expect(consoleWarnSpy).not.toHaveBeenCalled();
    });
  });

  describe('response normalization', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('passes through responses already in data envelope', async () => {
      const body = { data: { id: '1', name: 'Test' } };
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/me');
      expect(res.data).toEqual({ id: '1', name: 'Test' });
    });

    it('wraps list responses with pagination metadata', async () => {
      const body = {
        apps: [
          { id: '1', app_type_id: 'google_ads' },
          { id: '2', app_type_id: 'stripe' },
        ],
        total: 42,
      };
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/apps');
      expect(Array.isArray(res.data)).toBe(true);
      expect(res.meta?.pagination?.total).toBe(42);
    });

    it('converts snake_case keys to camelCase', async () => {
      const body = {
        data: { app_type_id: 'stripe', created_at: '2026-01-01' },
      };
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/me');
      const data = res.data as Record<string, unknown>;
      expect(data.appTypeId).toBe('stripe');
      expect(data.createdAt).toBe('2026-01-01');
    });

    it('handles 204 No Content', async () => {
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(null, {
          status: 204,
          headers: new Headers(),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().delete('/apps/123');
      expect(res.data).toEqual({});
    });

    it('does not treat array fields on a single item as a collection response', async () => {
      const body = {
        id: 'src_123',
        import_tasks: ['orders', 'customers'],
        created_at: '2026-01-01',
      };
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/sources/src_123');
      const data = res.data as Record<string, unknown>;

      expect(Array.isArray(res.data)).toBe(false);
      expect(data.id).toBe('src_123');
      expect(data.importTasks).toEqual(['orders', 'customers']);
      expect(res.meta).toBeUndefined();
    });
  });

  describe('path mapping', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ data: {} }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('maps /integrations to /connections with account prefix', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().get('/integrations');

      const url = vi.mocked(globalThis.fetch).mock.calls[0]![0] as string;
      expect(url).toContain('/api/v1/accounts/acct-123/connections');
    });

    it('maps /catalog to /connectors (no account prefix)', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().get('/catalog');

      const url = vi.mocked(globalThis.fetch).mock.calls[0]![0] as string;
      expect(url).toContain('/api/v1/connectors');
      expect(url).not.toContain('/accounts/');
    });

    it('does not require account ID for global catalog endpoints', async () => {
      mockRequireAccountId.mockImplementation(() => {
        throw new Error('No account configured');
      });

      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ connectors: [] }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      await expect(freshGetClient().get('/catalog')).resolves.toMatchObject({
        data: [],
      });

      const fetchCall = vi.mocked(globalThis.fetch).mock.calls[0]!;
      const options = fetchCall[1] as RequestInit;
      const headers = options.headers as Record<string, string>;
      expect(headers['X-Account-Id']).toBeUndefined();

      mockRequireAccountId.mockImplementation(() => 'acct-123');
    });

    it('maps /catalog/stripe to /connectors/stripe/catalog', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().get('/catalog/stripe');

      const url = vi.mocked(globalThis.fetch).mock.calls[0]![0] as string;
      expect(url).toContain('/api/v1/connectors/stripe/catalog');
    });

    it('adds account prefix for /apps', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().get('/apps');

      const url = vi.mocked(globalThis.fetch).mock.calls[0]![0] as string;
      expect(url).toContain('/api/v1/accounts/acct-123/apps');
    });

    it('adds account prefix for /jobs', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().get('/jobs');

      const url = vi.mocked(globalThis.fetch).mock.calls[0]![0] as string;
      expect(url).toContain('/api/v1/accounts/acct-123/jobs');
    });
  });

  describe('HTTP method override', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ data: {} }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('converts POST to PATCH for /pause actions', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().post('/integrations/123/pause');

      const fetchCall = vi.mocked(globalThis.fetch).mock.calls[0]!;
      const options = fetchCall[1] as RequestInit;
      expect(options.method).toBe('PATCH');
    });

    it('converts POST to PATCH for /resume actions', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().post('/integrations/123/resume');

      const fetchCall = vi.mocked(globalThis.fetch).mock.calls[0]!;
      const options = fetchCall[1] as RequestInit;
      expect(options.method).toBe('PATCH');
    });

    it('keeps POST as POST for non-action paths', async () => {
      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().post('/apps', { name: 'test' });

      const fetchCall = vi.mocked(globalThis.fetch).mock.calls[0]!;
      const options = fetchCall[1] as RequestInit;
      expect(options.method).toBe('POST');
    });
  });

  describe('field type fixes', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('converts role string to array', async () => {
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ id: '1', role: 'source' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/apps/1');
      const data = res.data as Record<string, unknown>;
      expect(data.role).toEqual(['source']);
    });

    it('adds appType alias from appTypeId', async () => {
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ id: '1', app_type_id: 'stripe' }), {
          status: 200,
          headers: new Headers({ 'Content-Type': 'application/json' }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/apps/1');
      const data = res.data as Record<string, unknown>;
      expect(data.appType).toBe('stripe');
    });

    it('adds supportedRoles default when missing', async () => {
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(
          JSON.stringify({ connection_type: 'google_ads', name: 'GA' }),
          {
            status: 200,
            headers: new Headers({ 'Content-Type': 'application/json' }),
          },
        ),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      const res = await freshGetClient().get('/catalog');
      const data = res.data as Record<string, unknown>;
      expect(data.supportedRoles).toEqual(['source']);
    });
  });

  describe('timeout handling', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
    });

    it('throws ClientError with timeout message on AbortError', async () => {
      vi.mocked(globalThis.fetch).mockRejectedValue(
        Object.assign(new Error('The operation was aborted'), {
          name: 'AbortError',
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        await freshGetClient().get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toBe('Request timed out');
        expect((err as ClientError).statusCode).toBe(408);
      }
    });

    it('wraps unknown errors in ClientError', async () => {
      vi.mocked(globalThis.fetch).mockRejectedValue(
        new Error('DNS resolution failed'),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      try {
        await freshGetClient().get('/me');
        expect.fail('Should have thrown');
      } catch (err) {
        expect(err).toBeInstanceOf(ClientError);
        expect((err as ClientError).message).toBe('DNS resolution failed');
        expect((err as ClientError).statusCode).toBe(0);
      }
    });
  });

  describe('debug logging', () => {
    let consoleWarnSpy: ReturnType<typeof vi.spyOn>;
    let consoleErrorSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.stubGlobal('fetch', vi.fn());
      consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
      consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
      setDebugEnabled(true);
    });

    afterEach(() => {
      vi.unstubAllGlobals();
      consoleWarnSpy.mockRestore();
      consoleErrorSpy.mockRestore();
      setDebugEnabled(false);
    });

    it('logs request and response metadata when debug is enabled', async () => {
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(JSON.stringify({ data: { ok: true } }), {
          status: 200,
          headers: new Headers({
            'Content-Type': 'application/json',
            'X-Request-Id': 'req_server_123',
          }),
        }),
      );

      const { getClient: freshGetClient } = await import('../client.js');
      await freshGetClient().get('/me');

      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('[debug] request'),
      );
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('[debug] response'),
      );
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('serverRequestId=req_server_123'),
      );
    });

    it('logs structured API failure details when debug is enabled', async () => {
      vi.mocked(globalThis.fetch).mockResolvedValue(
        new Response(
          JSON.stringify({
            error: {
              code: 'VALIDATION_ERROR',
              message: 'Invalid payload',
              details: { field: 'account_id' },
            },
          }),
          {
            status: 422,
            statusText: 'Unprocessable Entity',
            headers: new Headers({
              'Content-Type': 'application/json',
              'X-Request-Id': 'req_server_422',
            }),
          },
        ),
      );

      const { getClient: freshGetClient } = await import('../client.js');

      await expect(freshGetClient().get('/me')).rejects.toBeInstanceOf(
        ClientError,
      );

      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('[debug] response_error'),
      );
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('VALIDATION_ERROR'),
      );
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('serverRequestId=req_server_422'),
      );
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        expect.stringContaining('errorMessage="Invalid payload"'),
      );
    });
  });
});

import { afterEach, describe, expect, it, vi } from 'vitest';

import { ClientError, createClient } from '../client.js';
import { IdentityFetchError, fetchIdentity } from '../identity.js';

// Keep the real ClientError class; stub only the client factory so we control
// what verify() rejects with.
vi.mock('../client.js', async (importActual) => {
  const actual = await importActual<typeof import('../client.js')>();
  return { ...actual, createClient: vi.fn() };
});

const mockCreateClient = vi.mocked(createClient);

function clientThatRejectsWith(err: unknown) {
  return {
    verify: vi.fn().mockRejectedValue(err),
  } as unknown as ReturnType<typeof createClient>;
}

describe('fetchIdentity error mapping', () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it('returns the identity on success', async () => {
    const me = {
      accountId: 'acct-1',
      accountName: 'Acme',
      accountSlug: 'acme',
    };
    mockCreateClient.mockReturnValue({
      verify: vi.fn().mockResolvedValue(me),
    } as unknown as ReturnType<typeof createClient>);

    await expect(fetchIdentity('k', 'acct-1', 'https://x')).resolves.toEqual(
      me,
    );
  });

  it('maps an HTTP error response to IdentityFetchError', async () => {
    // A real HTTP response carries statusText.
    const httpErr = new ClientError('Unauthorized', 401, undefined, {
      statusText: 'Unauthorized',
    });
    mockCreateClient.mockReturnValue(clientThatRejectsWith(httpErr));

    await expect(
      fetchIdentity('k', 'acct-1', 'https://x'),
    ).rejects.toBeInstanceOf(IdentityFetchError);
  });

  it('propagates a client-side timeout (no statusText) as the underlying error', async () => {
    // The transport throws this for a 30s abort — statusText is absent.
    const timeout = new ClientError('Request timed out', 408);
    mockCreateClient.mockReturnValue(clientThatRejectsWith(timeout));

    await expect(fetchIdentity('k', 'acct-1', 'https://x')).rejects.toBe(
      timeout,
    );
  });

  it('propagates a network error (statusCode 0) as the underlying error', async () => {
    const network = new ClientError('fetch failed', 0);
    mockCreateClient.mockReturnValue(clientThatRejectsWith(network));

    await expect(fetchIdentity('k', 'acct-1', 'https://x')).rejects.toBe(
      network,
    );
  });
});

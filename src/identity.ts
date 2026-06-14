import { ClientError, type MeResponse, createClient } from './client.js';

export type { MeResponse };

export class IdentityFetchError extends Error {
  constructor(
    public readonly status: number,
    public readonly statusText: string,
  ) {
    super(`Credential validation failed (HTTP ${status})`);
    this.name = 'IdentityFetchError';
  }
}

/**
 * Validate credentials by fetching `/api/v1/me` with an explicit (not-yet-saved)
 * API key and account ID. A thin wrapper over the shared {@link createClient}
 * transport so login/doctor/init use the same request pipeline (timeouts,
 * debug logging, snake_case → camelCase normalization) as the rest of the CLI.
 *
 * HTTP error responses are surfaced as {@link IdentityFetchError} (preserving
 * the status/statusText callers format); network/timeout failures propagate as
 * the underlying error.
 *
 * The discriminator is `statusText`: the transport sets it (from
 * `res.statusText`) only when an actual HTTP response came back, so a missing
 * `statusText` marks a synthetic client-side failure — a 30s timeout
 * (`ClientError(..., 408)`) or a network error (`ClientError(..., 0)`) — which
 * must propagate rather than masquerade as a credential rejection.
 */
export async function fetchIdentity(
  apiKey: string,
  accountId: string,
  baseUrl: string,
): Promise<MeResponse> {
  try {
    return await createClient({ apiKey, accountId, baseUrl }).verify();
  } catch (err) {
    if (err instanceof ClientError && err.statusText !== undefined) {
      throw new IdentityFetchError(err.statusCode, err.statusText);
    }
    throw err;
  }
}

import { randomUUID } from 'node:crypto';

import { toCamelCaseDeep } from './client.js';

export interface MeResponse {
  accountId: string;
  accountName: string | null;
  accountSlug: string | null;
  apiKeyId?: string;
  scopes?: string[];
}

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
 * Fetch `/api/v1/me` and return the camelCased identity payload.
 *
 * The pipelines API returns snake_case; the rest of the CLI consumes
 * camelCase via the shared client normaliser. Without applying it here
 * `accountSlug` / `accountName` would be undefined at runtime even though
 * the TypeScript cast suggests otherwise — silently demoting saved
 * profile names to the raw account UUID.
 */
export async function fetchIdentity(
  apiKey: string,
  accountId: string,
  baseUrl: string,
): Promise<MeResponse> {
  const res = await fetch(new URL('/api/v1/me', baseUrl), {
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${apiKey}`,
      'X-API-Secret': apiKey,
      'X-Account-Id': accountId,
      'X-Request-Id': `cli-${randomUUID()}`,
      'X-Actor': 'vendo-cli',
    },
  });

  if (!res.ok) {
    throw new IdentityFetchError(res.status, res.statusText);
  }

  return toCamelCaseDeep(await res.json()) as MeResponse;
}

import { describe, expect, it } from 'vitest';

import { buildMcpClientConfig } from '../commands/mcp.js';

const PLACEHOLDER = 'Bearer ${VENDO_API_KEY}';

describe('buildMcpClientConfig', () => {
  it('derives the /api/mcp endpoint from the base URL', () => {
    const { endpoint, mcpServers } = buildMcpClientConfig({
      baseUrl: 'https://app2.vendodata.com',
    });
    expect(endpoint).toBe('https://app2.vendodata.com/api/mcp');
    expect(mcpServers.vendo).toEqual({
      type: 'http',
      url: 'https://app2.vendodata.com/api/mcp',
      headers: { Authorization: PLACEHOLDER },
    });
  });

  it('strips trailing slashes from the base URL (no //api/mcp)', () => {
    expect(
      buildMcpClientConfig({ baseUrl: 'https://stg.vendodata.com//' }).endpoint,
    ).toBe('https://stg.vendodata.com/api/mcp');
  });

  it('uses the placeholder when no key is given', () => {
    const res = buildMcpClientConfig({ baseUrl: 'https://app2.vendodata.com' });
    expect(res.keyEmbedded).toBe(false);
    expect(res.mcpServers.vendo.headers.Authorization).toBe(PLACEHOLDER);
  });

  it('does NOT embed the key unless --show-key is set (no accidental leak)', () => {
    const res = buildMcpClientConfig({
      baseUrl: 'https://app2.vendodata.com',
      apiKey: 'vendo_sk_secret',
    });
    expect(res.keyEmbedded).toBe(false);
    expect(res.mcpServers.vendo.headers.Authorization).toBe(PLACEHOLDER);
  });

  it('embeds the real key only when showKey AND a key are present', () => {
    const res = buildMcpClientConfig({
      baseUrl: 'https://app2.vendodata.com',
      apiKey: 'vendo_sk_secret',
      showKey: true,
    });
    expect(res.keyEmbedded).toBe(true);
    expect(res.mcpServers.vendo.headers.Authorization).toBe(
      'Bearer vendo_sk_secret',
    );
  });

  it('falls back to the placeholder when showKey is set but no key exists', () => {
    const res = buildMcpClientConfig({
      baseUrl: 'https://app2.vendodata.com',
      showKey: true,
    });
    expect(res.keyEmbedded).toBe(false);
    expect(res.mcpServers.vendo.headers.Authorization).toBe(PLACEHOLDER);
  });
});

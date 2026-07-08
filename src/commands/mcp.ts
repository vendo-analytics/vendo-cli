import { Command } from 'commander';

import { getEffectiveConfig } from '../config.js';
import { addExamples, c, printJson } from '../output.js';

const API_KEY_PLACEHOLDER = '${VENDO_API_KEY}';

export interface McpClientConfig {
  /** The MCP endpoint, derived from the active base URL. */
  endpoint: string;
  /** A `mcpServers` block ready to paste into an MCP client config. */
  mcpServers: {
    vendo: {
      type: 'http';
      url: string;
      headers: { Authorization: string };
    };
  };
  /** Whether a real key was embedded (vs the placeholder). */
  keyEmbedded: boolean;
}

/**
 * Build the MCP client configuration for a base URL + (optional) API key.
 *
 * Pure so it can be unit-tested without touching disk/env. The MCP server is
 * the web app's `/api/mcp` endpoint; the transport is stateless streamable-HTTP
 * (no `Mcp-Session-Id`), so the only thing a client needs is the URL + a
 * `Authorization: Bearer` header (an API key, or OAuth in claude.ai).
 */
export function buildMcpClientConfig(opts: {
  baseUrl: string;
  apiKey?: string;
  showKey?: boolean;
}): McpClientConfig {
  const endpoint = `${opts.baseUrl.replace(/\/+$/, '')}/api/mcp`;
  const keyEmbedded = Boolean(opts.showKey && opts.apiKey);
  const authorization = keyEmbedded
    ? `Bearer ${opts.apiKey}`
    : `Bearer ${API_KEY_PLACEHOLDER}`;

  return {
    endpoint,
    keyEmbedded,
    mcpServers: {
      vendo: {
        type: 'http',
        url: endpoint,
        headers: { Authorization: authorization },
      },
    },
  };
}

export function registerMcpCommand(program: Command): void {
  const cmd = program
    .command('mcp')
    .description(
      'Show how to connect an MCP client (Claude, Cursor, Windsurf) to Vendo',
    )
    .option('--json', 'Output only the mcpServers JSON block')
    .option(
      '--show-key',
      'Embed your actual API key instead of a ${VENDO_API_KEY} placeholder',
    )
    .action((opts: { json?: boolean; showKey?: boolean }) => {
      const config = getEffectiveConfig();
      const { endpoint, mcpServers, keyEmbedded } = buildMcpClientConfig({
        baseUrl: config.baseUrl,
        apiKey: config.apiKey,
        showKey: opts.showKey,
      });

      if (opts.json) {
        printJson({ mcpServers });
        return;
      }

      const block = JSON.stringify({ mcpServers }, null, 2)
        .split('\n')
        .map((line) => `  ${line}`)
        .join('\n');

      console.log();
      console.log(c.bold('Connect an MCP client to Vendo'));
      console.log();
      console.log(`  Endpoint:   ${endpoint}`);
      console.log(`  Transport:  streamable-http (stateless)`);
      console.log(
        `  Auth:       Authorization: Bearer <api key>   (or OAuth in claude.ai)`,
      );
      console.log();
      console.log(
        '  Add to your MCP client config (Claude Desktop, Cursor, Windsurf, …):',
      );
      console.log();
      console.log(block);
      console.log();

      if (!config.apiKey) {
        console.log(
          c.dim(
            '  No API key configured — run `vendo login` or set VENDO_API_KEY first.',
          ),
        );
      } else if (!keyEmbedded) {
        console.log(
          c.dim(
            '  Replace ${VENDO_API_KEY} with your key (or set it in the client env), or re-run with --show-key to embed it.',
          ),
        );
      }
      console.log(
        c.dim(
          '  Tip: the MCP server lives on app2.vendodata.com — app.vendodata.com does not serve /api/mcp.',
        ),
      );
    });

  addExamples(cmd, ['vendo mcp', 'vendo mcp --json', 'vendo mcp --show-key']);
}

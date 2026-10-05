import { Command } from 'commander';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  DictionaryItem,
  DictionaryListParams,
  DictionaryLookup,
} from '../api/dictionary.js';
import type { ApiResponse } from '../client.js';

vi.mock('../config.js', () => ({
  requireApiKey: vi.fn(() => 'test-api-key'),
  getBaseUrl: vi.fn(() => 'https://api.test.com'),
  requireAccountId: vi.fn(() => 'acct-123'),
}));

const { registerDictionaryCommand } = await import('../commands/dictionary.js');

/*
 * The dictionary wire contract, as vendo-web-v2 serves it
 * (apps/web/app/api/v1/_lib/route-handlers/dictionary/{collection,lookup,serialize}.ts,
 * read on origin/staging 4e3f294, 2026-10-05, and checked against stg.vendodata.com).
 * The CLI reads these routes with getCanonical, which keeps the server's keys
 * verbatim, so a renamed field on either side breaks the command silently.
 * Change these lists only together with the server.
 */
const SERVER_ITEM_FIELDS = [
  'subjectId',
  'subjectType',
  'displayName',
  'description',
  'dataType',
  'semanticType',
  'tags',
  'origin',
  'lastSeenAt',
  'status',
] as const;
const SERVER_LOOKUP_FIELDS = ['subjectId', 'found', 'definition'] as const;
const SERVER_LIST_PARAMS = ['type', 'q', 'limit', 'offset'] as const;

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2
    ? true
    : false;

// Each line fails `pnpm typecheck` when the CLI type gains, loses or renames a key.
const itemMatchesServer: Equal<
  keyof DictionaryItem,
  (typeof SERVER_ITEM_FIELDS)[number]
> = true;
const lookupMatchesServer: Equal<
  keyof DictionaryLookup,
  (typeof SERVER_LOOKUP_FIELDS)[number]
> = true;
const listParamsMatchServer: Equal<
  keyof DictionaryListParams,
  (typeof SERVER_LIST_PARAMS)[number]
> = true;

const DAY_MS = 86_400_000;

// Server-shaped fixtures with synthetic values. `satisfies` also checks the
// nested `meta.pagination` keys against ApiResponse.
const eventItem = {
  subjectId: '0123456789abcdef0123456789abcdef',
  subjectType: 'event',
  displayName: 'Checkout Completed',
  description: 'A customer placed an order.',
  dataType: null,
  semanticType: null,
  tags: [],
  origin: 'lexicon',
  lastSeenAt: null,
  status: 'active',
} satisfies DictionaryItem;

const columnItem = {
  subjectId:
    'source:11111111-2222-4333-8444-555555555555/table:customers/col:email',
  subjectType: 'column',
  displayName: 'Customer email',
  description: 'Lowercased email of the latest customer record',
  dataType: 'string',
  semanticType: 'email',
  tags: ['pii', 'crm'],
  origin: 'bq_schema',
  lastSeenAt: new Date(Date.now() - 3 * DAY_MS).toISOString(),
  status: 'deprecated',
} satisfies DictionaryItem;

const listBody = {
  data: [eventItem],
  meta: { pagination: { total: 6, limit: 20, offset: 0, hasMore: false } },
} satisfies ApiResponse<DictionaryItem[]>;

const lookupBody = {
  data: { subjectId: columnItem.subjectId, found: true, definition: columnItem },
} satisfies ApiResponse<DictionaryLookup>;

const missingBody = {
  data: { subjectId: 'event:nope', found: false },
} satisfies ApiResponse<DictionaryLookup>;

let stdout: string[];

function respondWith(body: unknown): void {
  vi.mocked(globalThis.fetch).mockResolvedValue(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: new Headers({ 'Content-Type': 'application/json' }),
    }),
  );
}

function requestedUrl(): string {
  return vi.mocked(globalThis.fetch).mock.calls[0]![0] as string;
}

async function run(...args: string[]): Promise<string> {
  const program = new Command().exitOverride();
  registerDictionaryCommand(program);
  await program.parseAsync(['node', 'vendo', 'dictionary', ...args]);
  // eslint-disable-next-line no-control-regex
  return stdout.join('\n').replace(/\u001b\[[0-9;]*m/g, '');
}

beforeEach(() => {
  stdout = [];
  vi.stubGlobal('fetch', vi.fn());
  vi.spyOn(console, 'log').mockImplementation((...args: unknown[]) => {
    stdout.push(args.join(' '));
  });
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(process, 'exit').mockImplementation(
    (code?: string | number | null) => {
      throw new Error(`process.exit(${code})`);
    },
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('dictionary contract with vendo-web-v2', () => {
  it('types carry exactly the server field and param names', () => {
    expect(itemMatchesServer).toBe(true);
    expect(lookupMatchesServer).toBe(true);
    expect(listParamsMatchServer).toBe(true);
    expect(Object.keys(columnItem)).toEqual([...SERVER_ITEM_FIELDS]);
    expect(Object.keys(lookupBody.data)).toEqual([...SERVER_LOOKUP_FIELDS]);
  });

  it('list sends type, q, limit and offset to the account-scoped collection route', async () => {
    respondWith(listBody);
    await run('list', '--query', 'checkout');
    expect(requestedUrl()).toBe(
      'https://api.test.com/api/v1/accounts/acct-123/dictionary?type=event&q=checkout&limit=20&offset=0',
    );
    expect([...new URL(requestedUrl()).searchParams.keys()]).toEqual([
      ...SERVER_LIST_PARAMS,
    ]);
  });

  it('search sends its argument as q with the chosen type', async () => {
    respondWith(listBody);
    await run('search', 'email', '--type', 'column', '--limit', '5');
    expect(requestedUrl()).toBe(
      'https://api.test.com/api/v1/accounts/acct-123/dictionary?type=column&q=email&limit=5&offset=0',
    );
  });

  it('list prints each row by subject ID with the server total', async () => {
    respondWith(listBody);
    const out = await run('list');
    expect(out).toContain('Subject ID');
    expect(out).toContain(eventItem.subjectId);
    expect(out).toContain(eventItem.displayName);
    expect(out).toContain(eventItem.description);
    expect(out).toContain('6 events');
  });

  it('--output reads the camelCase field names', async () => {
    respondWith(listBody);
    expect(await run('list', '--output', 'displayName')).toBe(
      eventItem.displayName,
    );
    stdout = [];
    respondWith(listBody);
    expect(await run('search', 'checkout', '--output', 'subjectId')).toBe(
      eventItem.subjectId,
    );
  });

  it('--json prints the server body unchanged', async () => {
    respondWith(listBody);
    expect(JSON.parse(await run('list', '--json'))).toEqual(listBody);
    stdout = [];
    respondWith(lookupBody);
    expect(JSON.parse(await run('get', columnItem.subjectId, '--json'))).toEqual(
      lookupBody,
    );
  });

  it('get sends subject_id to the lookup route and prints every field', async () => {
    respondWith(lookupBody);
    const out = await run('get', columnItem.subjectId);

    const url = new URL(requestedUrl());
    expect(url.pathname).toBe('/api/v1/accounts/acct-123/dictionary/lookup');
    expect([...url.searchParams.keys()]).toEqual(['subject_id']);
    expect(url.searchParams.get('subject_id')).toBe(columnItem.subjectId);

    expect(out).toContain(`Subject:      ${columnItem.subjectId}`);
    expect(out).toContain('Type:         column');
    expect(out).toContain('Display:      Customer email');
    expect(out).toContain('Data type:    string');
    expect(out).toContain('Semantic:     email');
    expect(out).toContain('Origin:       bq_schema');
    expect(out).toContain('Status:       deprecated');
    expect(out).toContain('Last seen:    3d ago');
    expect(out).toContain('Tags:         pii, crm');
    expect(out).toContain(columnItem.description);
  });

  it('get reports a subject the server does not know', async () => {
    respondWith(missingBody);
    expect(await run('get', 'event:nope')).toContain(
      'No dictionary entry for event:nope',
    );
  });
});

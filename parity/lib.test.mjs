import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  assertSafeBaseUrl,
  camelCaseKeysDeep,
  diffCells,
  diffErrors,
  diffFlags,
  diffJson,
  failureLine,
  sameCell,
  filterIntended,
  diffHelp,
  getPath,
  isNotPorted,
  parseHelp,
  validateClassification,
} from './lib.mjs';

describe('validateClassification', () => {
  it('fails on unclassified and stale commands before anything runs', () => {
    const problems = validateClassification(['apps list', 'apps get', 'jobs list'], {
      'apps list': { class: 'read' },
      'apps get': { class: 'read' },
      'retired list': { class: 'read' },
    });
    assert.deepEqual(problems, [
      'unclassified command: "jobs list"',
      'classified command no longer exists: "retired list"',
    ]);
  });

  it('rejects an unknown class', () => {
    const problems = validateClassification(['apps list'], { 'apps list': { class: 'safe' } });
    assert.deepEqual(problems, ['"apps list" has unknown class "safe"']);
  });

  it('accepts a complete classification', () => {
    assert.deepEqual(
      validateClassification(['whoami'], { whoami: { class: 'read' } }),
      [],
    );
  });
});

describe('assertSafeBaseUrl', () => {
  it('rejects prod, including the implicit prod default', () => {
    assert.throws(() => assertSafeBaseUrl('https://app2.vendodata.com'), /Refusing/);
    assert.throws(() => assertSafeBaseUrl(undefined), /Refusing/);
    assert.throws(() => assertSafeBaseUrl('not a url'), /Refusing/);
  });

  it('accepts staging and localhost', () => {
    assert.equal(assertSafeBaseUrl('https://stg.vendodata.com'), 'https://stg.vendodata.com');
    assert.equal(assertSafeBaseUrl('http://localhost:3000'), 'http://localhost:3000');
    assert.equal(assertSafeBaseUrl('http://127.0.0.1:3031'), 'http://127.0.0.1:3031');
  });
});

describe('diffJson', () => {
  const ts = { data: [{ id: 'a1', appType: 'stripe', state: 'active' }], meta: { total: 1 } };

  it('ignores key order', () => {
    const reordered = { meta: { total: 1 }, data: [{ state: 'active', appType: 'stripe', id: 'a1' }] };
    assert.deepEqual(diffJson(ts, reordered), []);
  });

  it('reports a renamed key as missing plus extra', () => {
    const renamed = { data: [{ id: 'a1', app_type: 'stripe', state: 'active' }], meta: { total: 1 } };
    assert.deepEqual(
      diffJson(ts, renamed).map((d) => `${d.kind} ${d.path}`),
      ['missing data[0].appType', 'extra data[0].app_type'],
    );
  });

  it('reports changed values and array length', () => {
    const changed = { data: [{ id: 'a1', appType: 'stripe', state: 'paused' }, { id: 'a2' }], meta: { total: 1 } };
    assert.deepEqual(
      diffJson(ts, changed).map((d) => `${d.kind} ${d.path}`),
      ['changed data[0].state', 'extra data[1]'],
    );
  });

  it('drops only differences listed as intended for that command', () => {
    const diffs = [
      { path: 'data[0].category', kind: 'missing' },
      { path: 'data[3].category', kind: 'missing' },
      { path: 'data[0].appType', kind: 'missing' },
    ];
    const intended = [{ command: 'catalog list', variant: 'json', path: 'data[*].category' }];
    assert.deepEqual(
      filterIntended(diffs, intended, 'catalog list', 'json').map((d) => d.path),
      ['data[0].appType'],
    );
    assert.equal(filterIntended(diffs, intended, 'apps list', 'json').length, 3);
  });
});

describe('camelCaseKeysDeep', () => {
  it('matches the TS client so only key casing may differ', () => {
    const raw = { data: [{ config: { custom_source: { field_mappings: [{ sync_null: true }] }, n2_value: 1 } }] };
    const ts = { data: [{ config: { customSource: { fieldMappings: [{ syncNull: true }] }, n2Value: 1 } }] };
    assert.deepEqual(diffJson(ts, camelCaseKeysDeep(raw)), []);
    const changedValue = { data: [{ config: { custom_source: { field_mappings: [{ sync_null: false }] }, n2_value: 1 } }] };
    assert.equal(diffJson(ts, camelCaseKeysDeep(changedValue)).length, 1);
  });
});

describe('diffCells', () => {
  const ts = ' ID             Name    State\n 39812d09...    Amp     active\n8 apps\n';

  it('accepts the narrower column gap', () => {
    const rust = ' ID           Name  State\n 39812d09...  Amp   active  \n8 apps\n';
    assert.deepEqual(diffCells(ts, rust), []);
  });

  it('drops rows matching an intended row pattern on both sides', () => {
    const tsDoctor = '[warn] CLI binary: /usr/bin/node (standard install path is /h/.local/bin/vendo)\n[ok] Base URL: https://stg\n';
    const rustDoctor = '[warn] CLI binary: /repo/rust/target/debug/vendo (standard install path is /h/.local/bin/vendo)\n[ok] Base URL: https://stg\n';
    assert.equal(diffCells(tsDoctor, rustDoctor).length, 1);
    assert.deepEqual(diffCells(tsDoctor, rustDoctor, [{ rowPattern: '^\\[\\w+\\] CLI binary:' }]), []);
  });

  it('reports a changed cell', () => {
    const rust = ' ID           Name  State\n 39812d09...  Amp   paused\n8 apps\n';
    assert.deepEqual(diffCells(ts, rust).map((d) => d.path), ['row 2']);
  });
});

describe('sameCell', () => {
  it('treats clock readings one tick apart as the same', () => {
    assert.equal(sameCell('24m ago', '25m ago'), true);
    assert.equal(sameCell('just now', '1m ago'), true);
    assert.equal(sameCell('23h ago', '1d ago'), false);
    assert.equal(sameCell('6s', '7s'), true);
    assert.equal(sameCell('5m 59s', '6m'), true);
    assert.equal(sameCell('3h 15m', '3h 16m'), true);
  });

  it('still catches real differences', () => {
    assert.equal(sameCell('24m ago', '27m ago'), false);
    assert.equal(sameCell('6s', '20s'), false);
    assert.equal(sameCell('completed', 'failed'), false);
    assert.equal(sameCell('2,400 rows', '2,401 rows'), false);
  });
});

describe('diffErrors', () => {
  it('ignores request IDs', () => {
    const a = { code: 1, stderr: 'Error: Resource not found.\nRequest ID: cli-fd0e818b-b44c-4655-9ec3-5fc42a7adacc\n' };
    const b = { code: 1, stderr: 'Error: Resource not found.\nRequest ID: cli-11111111-2222-4333-8444-555555555555\n' };
    assert.deepEqual(diffErrors(a, b), []);
  });

  it('reports a different message or exit code', () => {
    const a = { code: 1, stderr: 'Error: Resource not found. Check the ID and try again.\n' };
    const b = { code: 1, stderr: 'Error: HTTP 404\n' };
    assert.deepEqual(diffErrors(a, b).map((d) => d.path), ['error']);
    assert.deepEqual(diffErrors(a, { ...a, code: 2 }).map((d) => d.path), ['exit code']);
  });
});

describe('failureLine', () => {
  it('prefers the Error: line, then a crash line, then the last line', () => {
    assert.equal(failureLine('Error: Request timed out\n'), 'Error: Request timed out');
    const crash = 'file:///dist/cli.js:4247\n    for (const row of rows) {\n\nTypeError: rows is not iterable\n    at Command\n\nNode.js v22\n';
    assert.equal(failureLine(crash), 'TypeError: rows is not iterable');
    assert.equal(failureLine('something odd\nlast words\n'), 'last words');
  });
});

describe('help and porting status', () => {
  const clapHelp = [
    'List all app connections',
    '',
    'Usage: vendo apps list [OPTIONS]',
    '',
    'Options:',
    '      --state <STATE>    Filter by state (active, inactive)',
    '      --type <type>      Filter by app type',
    '      --json             Output raw JSON',
    '      --profile <name>   Use a specific account profile',
    '  -h, --help             Print help',
    '',
    'Examples:',
    '  $ vendo apps list --output id',
  ].join('\n');

  it('compares flags from the Options section only, ignoring root flags', () => {
    assert.deepEqual(diffFlags(['--state', '--type', '--json'], clapHelp), []);
    assert.deepEqual(
      diffFlags(['--state', '--type', '--json', '--limit'], clapHelp).map((d) => `${d.kind} ${d.path}`),
      ['missing --limit'],
    );
  });

  it('treats clap\'s unrecognized subcommand as not ported, not as a difference', () => {
    assert.equal(isNotPorted({ code: 2, stderr: "error: unrecognized subcommand 'sources'\n" }), true);
    assert.equal(isNotPorted({ code: 2, stderr: "error: unexpected argument '--limit' found\n" }), false);
  });
});

describe('getPath', () => {
  it('reads nested list paths', () => {
    const body = { data: { methodologies: [{ id: 'm1' }] } };
    assert.equal(getPath(body, 'data.methodologies[0].id'), 'm1');
    assert.equal(getPath(body, 'data.cohorts[0].cohort_period'), undefined);
  });
});

describe('help parity', () => {
  // Trimmed from the real `--help` of both CLIs.
  const tsList = `Usage: vendo dictionary list [options]

List catalog definitions (events by default)

Options:
  --type <type>       Filter by subject type (event, prop, group, column,
                      metric, model, audience) (default: "event")
  -q, --query <text>  Text search across subject ID, name and description
  --json              Output raw JSON
  -h, --help          display help for command

Examples:
  $ vendo dictionary list
  $ vendo dictionary list --type prop -q email --json
`;
  const rustList = `List catalog definitions (events by default)

Usage: vendo dictionary list [OPTIONS]

Options:
      --profile <name>  Use a specific account profile
      --type <type>     Filter by subject type (event, prop, group, column, metric, model, audience) [default: event]
      --debug           Enable verbose request diagnostics
  -q, --query <text>    Text search across subject ID, name and description
      --json            Output raw JSON
  -h, --help            Print help

Examples:
  $ vendo dictionary list
  $ vendo dictionary list --type prop -q email --json
`;

  it('reads wrapped descriptions, defaults and short flags from both formats the same way', () => {
    const ts = parseHelp(tsList, 'ts');
    assert.deepEqual(ts.options, [
      {
        long: '--type',
        short: null,
        value: 'type',
        description: 'Filter by subject type (event, prop, group, column, metric, model, audience)',
        default: 'event',
      },
      { long: '--query', short: '-q', value: 'text', description: 'Text search across subject ID, name and description', default: null },
      { long: '--json', short: null, value: null, description: 'Output raw JSON', default: null },
    ]);
    assert.equal(ts.description, 'List catalog definitions (events by default)');
    assert.deepEqual(diffHelp(ts, parseHelp(rustList, 'rust')), []);
  });

  it('reports a changed description, default, short flag or example', () => {
    const ts = parseHelp(tsList, 'ts');
    for (const [from, to, path] of [
      ['Output raw JSON', 'Output raw JSON.', 'option --json'],
      ['[default: event]', '[default: prop]', 'option --type'],
      ['-q, --query', '    --query', 'option --query'],
      ['--type prop -q email --json', '--type prop -q email', 'examples'],
      ['List catalog definitions (events by default)\n', 'List definitions\n', 'description'],
    ]) {
      const diffs = diffHelp(ts, parseHelp(rustList.replace(from, to), 'rust'));
      assert.deepEqual(diffs.map((d) => d.path), [path], from);
    }
  });

  it('compares arguments from the usage line, skipping clap\'s required options', () => {
    const ts = parseHelp('Usage: vendo jobs tail [options] [jobId]\n\nTail a job\n', 'ts');
    const rust = parseHelp('Tail a job\n\nUsage: vendo jobs tail [OPTIONS] [jobId]\n', 'rust');
    assert.deepEqual(ts.arguments, ['[jobId]']);
    assert.deepEqual(diffHelp(ts, rust), []);
    const create = parseHelp('Create\n\nUsage: vendo apps create [OPTIONS] --type <appType> --name <displayName>\n', 'rust');
    assert.deepEqual(create.arguments, []);
    const shell = parseHelp('Generate\n\nUsage: vendo completions [OPTIONS] <SHELL>\n', 'rust');
    assert.deepEqual(diffHelp(parseHelp('Usage: vendo completions [options] <shell>\n\nGenerate\n', 'ts'), shell).map((d) => d.path), [
      'arguments',
    ]);
  });

  it('reads subcommands with aliases, and the root flags only on the root', () => {
    const ts = `Usage: vendo [options] [command]

Vendo CLI

Options:
  -V, --version          output the version number
  --profile <name>       Use a specific account profile
  -h, --help             display help for command

Commands:
  integrations|int       Manage data export integrations
  measurement            Inspect Marketing Measurement methodologies, LTV, and
                         signals
  help [command]         display help for command
`;
    const rust = `Vendo CLI

Usage: vendo [OPTIONS] <COMMAND>

Commands:
  integrations  Manage data export integrations [alias: int]
  measurement   Inspect Marketing Measurement methodologies, LTV, and signals
  help          Print this message or the help of the given subcommand(s)

Options:
      --profile <name>  Use a specific account profile
  -V, --version         Print version
  -h, --help            Print help
`;
    const parsed = parseHelp(ts, 'ts', { root: true });
    assert.deepEqual(parsed.commands, [
      { name: 'integrations', aliases: ['int'], description: 'Manage data export integrations' },
      { name: 'measurement', aliases: [], description: 'Inspect Marketing Measurement methodologies, LTV, and signals' },
    ]);
    assert.deepEqual(parsed.options.map((o) => o.long), ['--profile']);
    assert.deepEqual(diffHelp(parsed, parseHelp(rust, 'rust', { root: true })), []);
    assert.deepEqual(parseHelp(rust, 'rust').options, [], 'clap lists --profile on every command; only the root compares it');
    const noAlias = rust.replace(' [alias: int]', '');
    assert.deepEqual(diffHelp(parsed, parseHelp(noAlias, 'rust', { root: true })).map((d) => d.path), ['subcommands']);
  });
});

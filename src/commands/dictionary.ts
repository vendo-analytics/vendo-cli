import { Command } from 'commander';

import {
  DICTIONARY_SUBJECT_TYPES,
  dictionaryApi,
  type DictionaryItem,
} from '../api/dictionary.js';
import {
  addExamples,
  c,
  colorStatus,
  createTable,
  printCount,
  printField,
  printJson,
  resolveOutputMode,
  runAction,
  timeAgo,
} from '../output.js';

const TYPE_HELP = DICTIONARY_SUBJECT_TYPES.join(', ');

function dash(value: string | null | undefined): string {
  return value && value.length > 0 ? value : c.dim('—');
}

function formatTags(tags: string[] | null | undefined): string {
  if (!tags || tags.length === 0) return c.dim('—');
  return tags.join(', ');
}

function printDefinition(item: DictionaryItem): void {
  const title = item.displayName || item.subjectId;
  console.log();
  console.log(c.bold(title), c.dim(`(${item.subjectType})`));
  console.log();
  console.log(`  Subject:      ${item.subjectId}`);
  console.log(`  Type:         ${item.subjectType}`);
  console.log(`  Display:      ${dash(item.displayName)}`);
  console.log(`  Data type:    ${dash(item.dataType)}`);
  console.log(`  Semantic:     ${dash(item.semanticType)}`);
  console.log(`  Origin:       ${dash(item.origin)}`);
  console.log(
    `  Status:       ${item.status ? colorStatus(item.status) : c.dim('—')}`,
  );
  console.log(`  Last seen:    ${timeAgo(item.lastSeenAt)}`);
  console.log(`  Tags:         ${formatTags(item.tags)}`);
  if (item.description) {
    console.log();
    console.log(`  ${item.description}`);
  }
}

export function registerDictionaryCommand(program: Command): void {
  const cmd = program
    .command('dictionary')
    .description('Browse the data dictionary catalog');

  const listCmd = cmd
    .command('list')
    .description('List catalog definitions (events by default)')
    .option('--type <type>', `Filter by subject type (${TYPE_HELP})`, 'event')
    .option('-q, --query <text>', 'Text search across name and description')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option(
      '--output <field>',
      'Print a single field per row (e.g. subjectId, displayName)',
    )
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const res = await runAction('Fetching dictionary...', () =>
        dictionaryApi.list({
          type: opts.type,
          q: opts.query,
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

      const table = createTable(['Name', 'Display', 'Description']);

      for (const item of res.data) {
        table.push([
          c.cyan(item.subjectId),
          dash(item.displayName),
          dash(item.description),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, opts.type);
    });

  addExamples(listCmd, [
    'vendo dictionary list',
    'vendo dictionary list --type event --query checkout',
    'vendo dictionary list --type prop -q email --json',
    'vendo dictionary list --output subjectId',
  ]);

  const searchCmd = cmd
    .command('search <query>')
    .description('Search catalog names and descriptions')
    .option('--type <type>', `Filter by subject type (${TYPE_HELP})`, 'event')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option(
      '--output <field>',
      'Print a single field per row (e.g. subjectId, displayName)',
    )
    .action(async (query: string, opts) => {
      const outputMode = resolveOutputMode(opts);

      const res = await runAction('Searching dictionary...', () =>
        dictionaryApi.list({
          type: opts.type,
          q: query,
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

      const table = createTable(['Name', 'Display', 'Description']);

      for (const item of res.data) {
        table.push([
          c.cyan(item.subjectId),
          dash(item.displayName),
          dash(item.description),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, opts.type);
    });

  addExamples(searchCmd, [
    'vendo dictionary search checkout',
    'vendo dictionary search email --type prop --json',
  ]);

  const getCmd = cmd
    .command('get <subjectId>')
    .description('Look up one catalog definition by subject id')
    .option('--json', 'Output raw JSON')
    .action(async (subjectId: string, opts: { json?: boolean }) => {
      const res = await runAction('Fetching dictionary entry...', () =>
        dictionaryApi.get(subjectId),
      );

      if (opts.json) {
        printJson(res);
        return;
      }

      const item = res.data;
      if (!item.found || !item.definition) {
        console.log();
        console.log(c.dim(`No dictionary entry for ${subjectId}`));
        return;
      }

      printDefinition(item.definition);
    });

  addExamples(getCmd, [
    'vendo dictionary get event:checkout_completed',
    'vendo dictionary get event:checkout_completed --json',
  ]);
}

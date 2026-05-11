import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  addExamples,
  c,
  createTable,
  printCount,
  printField,
  printJson,
  resolveOutputMode,
  runAction,
  shortId,
  timeAgo,
} from '../output.js';

interface ModelItem {
  id: string;
  name: string;
  description?: string | null;
  dataType: string;
  isValid: boolean;
  validationError?: string | null;
  lastValidatedAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

interface ModelDetail extends ModelItem {
  sqlQuery?: string | null;
  columns?: unknown;
  primaryKeyColumns?: string[] | null;
  incrementalColumn?: string | null;
}

export function registerModelsCommand(program: Command): void {
  const cmd = program.command('models').description('Manage data models');

  // models list
  const listCmd = cmd
    .command('list')
    .description('List all data models')
    .option('--type <type>', 'Filter by data type')
    .option('--valid', 'Only show valid models')
    .option('--invalid', 'Only show invalid models')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id, name)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);
      const params: Record<string, string | number | boolean | undefined> = {
        data_type: opts.type,
        limit: opts.limit,
        offset: opts.offset,
      };
      if (opts.valid) params.is_valid = true;
      if (opts.invalid) params.is_valid = false;

      const res = await runAction('Fetching models...', () =>
        getClient().get<ModelItem[]>('/models', params),
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

      const table = createTable([
        'ID',
        'Name',
        'Type',
        'Valid',
        'Last Validated',
      ]);

      for (const model of res.data) {
        table.push([
          c.dim(shortId(model.id)),
          model.name,
          model.dataType,
          model.isValid ? c.green('yes') : c.red('no'),
          timeAgo(model.lastValidatedAt),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, 'model');
    });

  addExamples(listCmd, [
    'vendo models list',
    'vendo models list --valid',
    'vendo models list --output id',
  ]);

  // models get
  const getCmd = cmd
    .command('get <modelId>')
    .description('Get model details')
    .option('--json', 'Output raw JSON')
    .action(async (modelId: string, opts: { json?: boolean }) => {
      const res = await runAction('Fetching model...', () =>
        getClient().get<ModelDetail>(`/models/${modelId}`),
      );

      if (opts.json) {
        printJson(res);
        return;
      }

      const model = res.data;
      console.log();
      console.log(c.bold(model.name), c.dim(`(${model.dataType})`));
      console.log();
      console.log(`  ID:            ${model.id}`);
      console.log(`  Data Type:     ${model.dataType}`);
      console.log(
        `  Valid:         ${model.isValid ? c.green('yes') : c.red('no')}`,
      );
      console.log(`  Validated:     ${timeAgo(model.lastValidatedAt)}`);
      console.log(`  Created:       ${timeAgo(model.createdAt)}`);

      if (model.description) {
        console.log(`  Description:   ${model.description}`);
      }
      if (model.primaryKeyColumns && model.primaryKeyColumns.length > 0) {
        console.log(`  Primary Keys:  ${model.primaryKeyColumns.join(', ')}`);
      }
      if (model.incrementalColumn) {
        console.log(`  Incremental:   ${model.incrementalColumn}`);
      }
      if (model.validationError) {
        console.log();
        console.log(`  ${c.red('Validation Error:')} ${model.validationError}`);
      }
      if (model.sqlQuery) {
        console.log();
        console.log(c.bold('  SQL Query:'));
        console.log(
          model.sqlQuery
            .split('\n')
            .map((line) => `    ${c.dim(line)}`)
            .join('\n'),
        );
      }
    });

  addExamples(getCmd, [
    'vendo models get <modelId>',
    'vendo models get <modelId> --json',
  ]);
}

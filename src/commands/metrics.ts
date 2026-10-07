import { Command } from 'commander';
import { readFileSync } from 'node:fs';

import { webApp } from '../api/web-app.js';
import {
  addExamples,
  c,
  confirm,
  createTable,
  printCount,
  printField,
  printJson,
  resolveOutputMode,
  runAction,
  shortId,
  timeAgo,
} from '../output.js';

export function readMetricDefinition(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch (error) {
    const detail = error instanceof Error ? error.message : 'read error';
    throw new Error(`Failed to read Metric definition ${path}: ${detail}`);
  }
}

function metricReportType(definition: unknown): string {
  if (!definition || typeof definition !== 'object') return 'unknown';
  const reportType = (definition as Record<string, unknown>).reportType;
  return typeof reportType === 'string' ? reportType : 'unknown';
}

export function registerMetricsCommand(program: Command): void {
  const cmd = program
    .command('metrics')
    .description('Manage custom metrics in the Metrics Library');

  // metrics list
  const listCmd = cmd
    .command('list')
    .description('List all custom metrics')
    .option('--status <status>', 'Filter by status (draft, active, archived)')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id, name)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const { data: res } = await runAction('Fetching metrics...', () =>
        webApp.metrics.list({
          status: opts.status,
          limit: opts.limit,
          offset: opts.offset,
        }),
      );

      if (outputMode === 'json') {
        printJson({
          data: res.metrics,
          meta: { pagination: { total: res.total } },
        });
        return;
      }

      if (outputMode === 'field') {
        printField(
          res.metrics as unknown as Record<string, unknown>[],
          opts.output,
        );
        return;
      }

      const table = createTable([
        'ID',
        'Name',
        'Type',
        'Format',
        'Status',
        'Updated',
      ]);

      for (const metric of res.metrics) {
        table.push([
          c.dim(shortId(metric.id)),
          metric.name,
          metric.metric_type,
          metric.format,
          metric.status === 'active'
            ? c.green(metric.status)
            : metric.status === 'draft'
              ? c.yellow(metric.status)
              : c.dim(metric.status),
          timeAgo(metric.updated_at),
        ]);
      }

      console.log(table.toString());
      printCount(res.total, 'metric');
    });

  addExamples(listCmd, [
    'vendo metrics list',
    'vendo metrics list --status active',
    'vendo metrics list --output id',
  ]);

  // metrics get
  const getCmd = cmd
    .command('get <metricId>')
    .description('Get metric details')
    .option('--json', 'Output raw JSON')
    .action(async (metricId: string, opts: { json?: boolean }) => {
      const { data: res } = await runAction('Fetching metric...', () =>
        webApp.metrics.get(metricId),
      );

      if (opts.json) {
        printJson({ data: res.metric });
        return;
      }

      const metric = res.metric;
      console.log();
      console.log(c.bold(metric.name), c.dim(`(${metric.metric_type})`));
      console.log();
      console.log(`  ID:           ${metric.id}`);
      console.log(`  Type:         ${metric.metric_type}`);
      console.log(`  Format:       ${metric.format}`);
      console.log(
        `  Status:       ${metric.status === 'active' ? c.green(metric.status) : metric.status}`,
      );
      console.log(`  Updated:      ${timeAgo(metric.updated_at)}`);

      if (metric.description) {
        console.log(`  Description:  ${metric.description}`);
      }
      if (metric.unit) {
        console.log(`  Unit:         ${metric.unit}`);
      }
      console.log(
        `  Higher=Better: ${metric.higher_is_better ? c.green('yes') : c.red('no')}`,
      );

      console.log(
        `  Calculation:  ${c.cyan(metricReportType(metric.definition))}`,
      );
    });

  addExamples(getCmd, [
    'vendo metrics get <metricId>',
    'vendo metrics get <metricId> --json',
  ]);

  // metrics create
  const createCmd = cmd
    .command('create')
    .description('Create a new metric')
    .requiredOption('--name <name>', 'Metric name')
    .requiredOption('--definition <file>', 'QuerySpec v2 definition JSON file')
    .option('--description <desc>', 'Description')
    .option(
      '--format <format>',
      'Display format: number, currency, percentage, multiplier',
      'number',
    )
    .option('--unit <unit>', 'Unit suffix (e.g., "$", "%")')
    .option('--json', 'Output raw JSON')
    .action(async (opts) => {
      const body: Record<string, unknown> = {
        name: opts.name,
        definition: readMetricDefinition(opts.definition),
        format: opts.format,
      };

      if (opts.description) body.description = opts.description;
      if (opts.unit) body.unit = opts.unit;

      const { data: res } = await runAction('Creating metric...', () =>
        webApp.metrics.create(body),
      );

      if (opts.json) {
        printJson({ data: res.metric });
        return;
      }

      console.log();
      console.log(c.green('✓'), `Metric "${res.metric.name}" created`);
      console.log(`  ID:     ${res.metric.id}`);
      console.log(`  Status: ${res.metric.status}`);
      if (res.metric.status === 'draft') {
        console.log();
        console.log(
          c.dim(
            '  The calculation could not compile. Update its definition before activating it.',
          ),
        );
      }
    });

  addExamples(createCmd, [
    'vendo metrics create --name "ROAS" --definition roas.query.json',
    'vendo metrics create --name "Total Revenue" --definition revenue.query.json --format currency',
    'vendo metrics create --name "CTR" --definition ctr.query.json --format percentage',
  ]);

  // metrics update
  const updateCmd = cmd
    .command('update <metricId>')
    .description('Update a metric')
    .option('--name <name>', 'New name')
    .option('--description <desc>', 'New description')
    .option('--definition <file>', 'New QuerySpec v2 definition JSON file')
    .option('--format <format>', 'New format')
    .option('--unit <unit>', 'New unit')
    .option('--status <status>', 'New status')
    .option('--json', 'Output raw JSON')
    .action(async (metricId: string, opts) => {
      const body: Record<string, unknown> = {};

      if (opts.name) body.name = opts.name;
      if (opts.description) body.description = opts.description;
      if (opts.definition)
        body.definition = readMetricDefinition(opts.definition);
      if (opts.format) body.format = opts.format;
      if (opts.unit) body.unit = opts.unit;
      if (opts.status) body.status = opts.status;

      if (Object.keys(body).length === 0) {
        console.error(c.red('Error:'), 'No updates provided');
        process.exit(1);
      }

      const { data: res } = await runAction('Updating metric...', () =>
        webApp.metrics.update(metricId, body),
      );

      if (opts.json) {
        printJson({ data: res.metric });
        return;
      }

      console.log();
      console.log(c.green('✓'), `Metric "${res.metric.name}" updated`);
    });

  addExamples(updateCmd, [
    'vendo metrics update <metricId> --name "New Name"',
    'vendo metrics update <metricId> --status active',
    'vendo metrics update <metricId> --definition revised.query.json',
  ]);

  // metrics activate
  const activateCmd = cmd
    .command('activate <metricId>')
    .description('Activate a draft metric')
    .option('--json', 'Output raw JSON')
    .action(async (metricId: string, opts: { json?: boolean }) => {
      const { data: res } = await runAction('Activating metric...', () =>
        webApp.metrics.update(metricId, { status: 'active' }),
      );

      if (opts.json) {
        printJson({ data: res.metric });
        return;
      }

      console.log();
      console.log(c.green('✓'), `Metric "${res.metric.name}" is now active`);
    });

  addExamples(activateCmd, ['vendo metrics activate <metricId>']);

  // metrics delete
  const deleteCmd = cmd
    .command('delete <metricId>')
    .description('Delete a metric')
    .option('-y, --yes', 'Skip confirmation')
    .option('--json', 'Output raw JSON (implies --yes)')
    .action(
      async (metricId: string, opts: { yes?: boolean; json?: boolean }) => {
        if (!opts.yes && !opts.json) {
          const confirmed = await confirm(
            `Delete metric ${shortId(metricId)}? This cannot be undone.`,
          );
          if (!confirmed) {
            console.log('Cancelled');
            return;
          }
        }

        const { data: res } = await runAction('Deleting metric...', () =>
          webApp.metrics.remove(metricId),
        );

        if (opts.json) {
          printJson({ data: res });
          return;
        }

        console.log();
        console.log(c.green('✓'), 'Metric deleted');
      },
    );

  addExamples(deleteCmd, [
    'vendo metrics delete <metricId>',
    'vendo metrics delete <metricId> -y',
  ]);
}

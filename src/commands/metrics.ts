import { Command } from 'commander';

import { getClient } from '../client.js';
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

interface MetricRow {
  id: string;
  account_id: string;
  name: string;
  description: string | null;
  category: string | null;
  metric_type: string;
  formula: string | null;
  format: string;
  higher_is_better: boolean;
  unit: string | null;
  status: string;
  created_at: string;
  updated_at: string;
}

interface MetricDetail extends MetricRow {
  building_blocks: unknown[];
}

interface MetricsListResponse {
  metrics: MetricRow[];
  total: number;
  limit: number;
  offset: number;
}

interface MetricResponse {
  metric: MetricDetail;
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
    .option('--category <category>', 'Filter by category')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id, name)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const { data: res } = await runAction('Fetching metrics...', () =>
        getClient().getRaw<MetricsListResponse>('/api/metrics', {
          status: opts.status,
          category: opts.category,
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
    'vendo metrics list --category Revenue',
    'vendo metrics list --output id',
  ]);

  // metrics get
  const getCmd = cmd
    .command('get <metricId>')
    .description('Get metric details')
    .option('--json', 'Output raw JSON')
    .action(async (metricId: string, opts: { json?: boolean }) => {
      const { data: res } = await runAction('Fetching metric...', () =>
        getClient().getRaw<MetricResponse>(`/api/metrics/${metricId}`),
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
      if (metric.category) {
        console.log(`  Category:     ${metric.category}`);
      }
      if (metric.unit) {
        console.log(`  Unit:         ${metric.unit}`);
      }
      console.log(
        `  Higher=Better: ${metric.higher_is_better ? c.green('yes') : c.red('no')}`,
      );

      if (metric.formula) {
        console.log();
        console.log(c.bold('  Formula:'));
        console.log(`    ${c.cyan(metric.formula)}`);
      }

      if (metric.building_blocks && metric.building_blocks.length > 0) {
        console.log();
        console.log(c.bold('  Building Blocks:'));
        for (const block of metric.building_blocks as Array<{
          label: string;
          sourceType: string;
          table: string;
          field: string;
          measure: string;
        }>) {
          console.log(
            `    ${c.cyan(block.label)}: ${block.measure}(${block.table}.${block.field})`,
          );
        }
      }
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
    .requiredOption(
      '--type <type>',
      'Metric type: derived (single source) or composed (formula)',
    )
    .option('--description <desc>', 'Description')
    .option('--category <category>', 'Category for grouping')
    .option(
      '--formula <formula>',
      'Formula for composed metrics (e.g., "A / B")',
    )
    .option(
      '--format <format>',
      'Display format: number, currency, percentage, multiplier',
      'number',
    )
    .option('--unit <unit>', 'Unit suffix (e.g., "$", "%")')
    .option('--status <status>', 'Status: draft or active', 'draft')
    .option('--json', 'Output raw JSON')
    .action(async (opts) => {
      const body: Record<string, unknown> = {
        name: opts.name,
        metric_type: opts.type,
        format: opts.format,
        status: opts.status,
      };

      if (opts.description) body.description = opts.description;
      if (opts.category) body.category = opts.category;
      if (opts.formula) body.formula = opts.formula;
      if (opts.unit) body.unit = opts.unit;

      const { data: res } = await runAction('Creating metric...', () =>
        getClient().postRaw<MetricResponse>('/api/metrics', body),
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
            '  Run `vendo metrics activate <id>` to make it available for use.',
          ),
        );
      }
    });

  addExamples(createCmd, [
    'vendo metrics create --name "ROAS" --type composed --formula "A / B"',
    'vendo metrics create --name "Total Revenue" --type derived --format currency',
    'vendo metrics create --name "CTR" --type composed --formula "A / B * 100" --format percentage',
  ]);

  // metrics update
  const updateCmd = cmd
    .command('update <metricId>')
    .description('Update a metric')
    .option('--name <name>', 'New name')
    .option('--description <desc>', 'New description')
    .option('--category <category>', 'New category')
    .option('--formula <formula>', 'New formula')
    .option('--format <format>', 'New format')
    .option('--unit <unit>', 'New unit')
    .option('--status <status>', 'New status')
    .option('--json', 'Output raw JSON')
    .action(async (metricId: string, opts) => {
      const body: Record<string, unknown> = {};

      if (opts.name) body.name = opts.name;
      if (opts.description) body.description = opts.description;
      if (opts.category) body.category = opts.category;
      if (opts.formula) body.formula = opts.formula;
      if (opts.format) body.format = opts.format;
      if (opts.unit) body.unit = opts.unit;
      if (opts.status) body.status = opts.status;

      if (Object.keys(body).length === 0) {
        console.error(c.red('Error:'), 'No updates provided');
        process.exit(1);
      }

      const { data: res } = await runAction('Updating metric...', () =>
        getClient().patchRaw<MetricResponse>(`/api/metrics/${metricId}`, body),
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
    'vendo metrics update <metricId> --formula "A / B * 100"',
  ]);

  // metrics activate
  const activateCmd = cmd
    .command('activate <metricId>')
    .description('Activate a draft metric')
    .option('--json', 'Output raw JSON')
    .action(async (metricId: string, opts: { json?: boolean }) => {
      const { data: res } = await runAction('Activating metric...', () =>
        getClient().patchRaw<MetricResponse>(`/api/metrics/${metricId}`, {
          status: 'active',
        }),
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
          getClient().deleteRaw<{ deleted: boolean; id: string }>(
            `/api/metrics/${metricId}`,
          ),
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

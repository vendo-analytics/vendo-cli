import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  formatJobProgress,
  getActiveJobForResource,
  getActiveJobs,
} from '../job-progress.js';
import {
  addExamples,
  c,
  colorStatus,
  createTable,
  printCount,
  printField,
  printJson,
  printLabel,
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
  timeAgo,
} from '../output.js';
import {
  type PipelineResourceConfig,
  readJsonFile,
  registerDeleteCommand,
  registerStateActionCommand,
  registerSyncCommand,
} from './pipeline-resource.js';

interface SourceItem {
  id: string;
  appId: string;
  appName?: string | null;
  syncType: string;
  state: string;
  integrationStatus: string;
  lastSyncAt?: string | null;
  lastError?: string | null;
  consecutiveFailures?: number;
  createdAt: string;
}

interface SourceDetail extends SourceItem {
  appType?: string | null;
  importTasks?: string[];
  syncFrequency?: string | null;
  syncAnchorTime?: string | null;
  syncAnchorTimezone?: string | null;
  rollingPeriodDays?: number | null;
  isOnboarding?: boolean;
  config?: unknown;
  latestJobId?: string | null;
  earliestDataAt?: string | null;
  latestDataAt?: string | null;
  datasetId?: string | null;
  updatedAt: string;
}

const RESOURCE: PipelineResourceConfig = {
  singular: 'source',
  idParam: 'sourceId',
  apiPath: '/sources',
};

export function registerSourcesCommand(program: Command): void {
  const cmd = program.command('sources').description('Manage data sources');

  // sources list
  const listCmd = cmd
    .command('list')
    .description('List all data sources')
    .option('--state <state>', 'Filter by state (active, inactive)')
    .option('--type <type>', 'Filter by sync type')
    .option('--app <appId>', 'Filter by app ID')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const sourcesRequest = () =>
        getClient().get<SourceItem[]>('/sources', {
          state: opts.state,
          sync_type: opts.type,
          app_id: opts.app,
          limit: opts.limit,
          offset: opts.offset,
        });

      if (outputMode === 'json') {
        const res = await runAction('Fetching sources...', sourcesRequest);
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        const res = await runAction('Fetching sources...', sourcesRequest);
        printField(
          res.data as unknown as Record<string, unknown>[],
          opts.output,
        );
        return;
      }

      const [res, activeJobs] = await runAction(
        'Fetching sources...',
        async () => Promise.all([sourcesRequest(), getActiveJobs()]),
      );

      const activeJobsBySourceId = new Map(
        activeJobs
          .filter((job) => job.sourceId)
          .map((job) => [job.sourceId!, job]),
      );

      const table = createTable([
        'ID',
        'Name',
        'Type',
        'Status',
        'Progress',
        'Failures',
        'Last Sync',
      ]);

      for (const source of res.data) {
        table.push([
          c.dim(shortId(source.id)),
          source.appName ?? c.dim('—'),
          source.syncType,
          colorStatus(source.integrationStatus),
          formatJobProgress(activeJobsBySourceId.get(source.id)),
          source.consecutiveFailures && source.consecutiveFailures > 0
            ? c.red(String(source.consecutiveFailures))
            : c.dim('0'),
          timeAgo(source.lastSyncAt),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, 'source');
    });

  addExamples(listCmd, [
    'vendo sources list',
    'vendo sources list --state active',
    'vendo sources list --output id',
  ]);

  // sources get
  const getCmd = cmd
    .command('get <sourceId>')
    .description('Get source details')
    .option('--json', 'Output raw JSON')
    .action(async (sourceId: string, opts: { json?: boolean }) => {
      const sourceRequest = () =>
        getClient().get<SourceDetail>(`/sources/${sourceId}`);

      if (opts.json) {
        const res = await runAction('Fetching source...', sourceRequest);
        printJson(res);
        return;
      }

      const [res, activeJob] = await runAction('Fetching source...', async () =>
        Promise.all([
          sourceRequest(),
          getActiveJobForResource('source', sourceId),
        ]),
      );

      const src = res.data;
      console.log();
      console.log(
        c.bold(src.appName ?? src.syncType),
        c.dim(`(${src.syncType})`),
      );
      console.log();
      console.log(`  ID:          ${src.id}`);
      console.log(
        `  App:         ${src.appName ?? c.dim('—')} ${c.dim(src.appId)}`,
      );
      console.log(`  Type:        ${src.syncType}`);
      console.log(`  State:       ${colorStatus(src.state)}`);
      console.log(`  Status:      ${colorStatus(src.integrationStatus)}`);
      console.log(`  Progress:    ${formatJobProgress(activeJob)}`);
      console.log(`  Frequency:   ${src.syncFrequency ?? c.dim('—')}`);
      console.log(
        `  Anchor:      ${src.syncAnchorTime ?? c.dim('—')} ${src.syncAnchorTimezone ?? ''}`,
      );
      console.log(`  Last Sync:   ${timeAgo(src.lastSyncAt)}`);
      console.log(
        `  Data Range:  ${src.earliestDataAt ? new Date(src.earliestDataAt).toLocaleDateString() : '—'} → ${src.latestDataAt ? new Date(src.latestDataAt).toLocaleDateString() : '—'}`,
      );
      console.log(`  Dataset:     ${src.datasetId ?? c.dim('—')}`);
      console.log(`  Created:     ${timeAgo(src.createdAt)}`);

      if (src.lastError) {
        console.log(`  Error:       ${c.red(src.lastError)}`);
      }
      if (src.consecutiveFailures && src.consecutiveFailures > 0) {
        console.log(
          `  Failures:    ${c.red(String(src.consecutiveFailures))} consecutive`,
        );
      }
      if (src.latestJobId) {
        console.log(`  Latest Job:  ${c.dim(src.latestJobId)}`);
      }
      if (src.importTasks && src.importTasks.length > 0) {
        console.log(`  Tasks:       ${src.importTasks.join(', ')}`);
      }
    });

  addExamples(getCmd, [
    'vendo sources get <sourceId>',
    'vendo sources get <sourceId> --json',
  ]);

  // sources sync (with idempotent check)
  registerSyncCommand<SourceDetail>(
    cmd,
    RESOURCE,
    (src) => ({
      Name: src.appName ?? '—',
      State: colorStatus(src.state),
    }),
    [
      'vendo sources sync <sourceId>',
      'vendo sources sync <sourceId> --watch',
      'vendo sources sync <sourceId> --dry-run',
    ],
  );

  // sources pause
  registerStateActionCommand(cmd, RESOURCE, {
    name: 'pause',
    gerund: 'Pausing',
    pastTense: 'paused',
    description: 'Pause a data source',
    examples: [
      'vendo sources pause <sourceId>',
      'vendo sources pause <sourceId> --dry-run',
    ],
  });

  // sources resume
  registerStateActionCommand(cmd, RESOURCE, {
    name: 'resume',
    gerund: 'Resuming',
    pastTense: 'resumed',
    description: 'Resume a paused data source',
    examples: [
      'vendo sources resume <sourceId>',
      'vendo sources resume <sourceId> --dry-run',
    ],
  });

  // sources delete
  registerDeleteCommand(cmd, RESOURCE, 'Delete a data source (soft delete)', [
    'vendo sources delete <sourceId>',
    'vendo sources delete <sourceId> --yes',
    'vendo sources delete <sourceId> --dry-run',
  ]);

  // sources create
  const createCmd = cmd
    .command('create')
    .description('Create a new data source')
    .requiredOption(
      '--app <appId>',
      'App connection ID (must have source role)',
    )
    .requiredOption(
      '--sync-type <syncType>',
      'Connector type (e.g. google_ads)',
    )
    .option(
      '--import-tasks <tasks>',
      'Comma-separated import task IDs (streams to pull)',
    )
    .option('--frequency <value>', 'Sync frequency value', '24')
    .option('--unit <unit>', 'Sync frequency unit (hours, days)', 'hours')
    .option(
      '--config-file <path>',
      'Path to a JSON file with source-specific config',
    )
    .option('--run-now', 'Trigger an initial sync immediately after creation')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (opts) => {
      const body: Record<string, unknown> = {
        appId: opts.app,
        syncType: opts.syncType,
        syncFrequencyValue: Number(opts.frequency),
        syncFrequencyUnit: opts.unit,
        runNow: Boolean(opts.runNow),
      };

      if (opts.importTasks) {
        body.importTasks = String(opts.importTasks)
          .split(',')
          .map((t: string) => t.trim())
          .filter(Boolean);
      }
      if (opts.configFile) body.config = readJsonFile(opts.configFile);

      const res = await runAction('Creating source...', () =>
        getClient().post<SourceDetail>('/sources', body),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      const src = res.data;
      if (!src) {
        printSuccess('Source created.');
        return;
      }

      if (outputMode === 'field') {
        console.log(src.id);
        return;
      }

      printSuccess(
        `Source ${c.bold(src.syncType)} (${shortId(src.id)}) created.`,
      );
      printLabel('App', src.appId);
      printLabel('State', colorStatus(src.state));
    });

  addExamples(createCmd, [
    'vendo sources create --app <appId> --sync-type google_ads',
    'vendo sources create --app <appId> --sync-type shopify --import-tasks orders,customers --run-now',
  ]);

  // sources update
  const updateCmd = cmd
    .command('update <sourceId>')
    .description('Update a data source')
    .option('--import-tasks <tasks>', 'Comma-separated import task IDs')
    .option('--frequency <value>', 'Sync frequency value')
    .option('--unit <unit>', 'Sync frequency unit (hours, days)')
    .option('--config-file <path>', 'Replace config from a JSON file')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (sourceId: string, opts) => {
      const body: Record<string, unknown> = {};
      if (opts.importTasks !== undefined) {
        body.importTasks = String(opts.importTasks)
          .split(',')
          .map((t: string) => t.trim())
          .filter(Boolean);
      }
      if (opts.frequency !== undefined) {
        body.syncFrequencyValue = Number(opts.frequency);
      }
      if (opts.unit !== undefined) body.syncFrequencyUnit = opts.unit;
      if (opts.configFile) body.config = readJsonFile(opts.configFile);

      if (Object.keys(body).length === 0) {
        throw new Error('Nothing to update — pass at least one flag.');
      }

      const res = await runAction('Updating source...', () =>
        getClient().patch<SourceDetail>(`/sources/${sourceId}`, body),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        console.log(sourceId);
        return;
      }

      printSuccess(`Source ${shortId(sourceId)} updated.`);
    });

  addExamples(updateCmd, [
    'vendo sources update <sourceId> --frequency 6 --unit hours',
    'vendo sources update <sourceId> --import-tasks orders,customers',
  ]);
}

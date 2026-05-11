import { Command } from 'commander';
import { readFileSync } from 'node:fs';

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
  confirm,
  createTable,
  printCount,
  printDryRun,
  printField,
  printJson,
  printLabel,
  printSingleField,
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
  timeAgo,
} from '../output.js';
import { watchTriggeredResourceJob } from '../watch-job.js';

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

interface SyncTriggerResponse {
  jobId?: string;
  status?: string;
  message?: string;
}

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
  const syncCmd = cmd
    .command('sync <sourceId>')
    .description('Trigger a manual sync')
    .option('--json', 'Output raw JSON')
    .option('--watch', 'Watch the job until it completes')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id for job ID)')
    .action(
      async (
        sourceId: string,
        opts: {
          json?: boolean;
          watch?: boolean;
          dryRun?: boolean;
          output?: string;
        },
      ) => {
        // Dry-run: show what would happen
        if (opts.dryRun) {
          const [sourceRes, activeJob] = await runAction(
            'Checking source...',
            async () =>
              Promise.all([
                getClient().get<SourceDetail>(`/sources/${sourceId}`),
                getActiveJobForResource('source', sourceId),
              ]),
          );
          printDryRun('trigger sync for', 'source', sourceId, {
            Name: sourceRes.data.appName ?? '—',
            State: colorStatus(sourceRes.data.state),
            'Active Job': activeJob
              ? `${shortId(activeJob.id)} (${activeJob.status})`
              : 'none',
          });
          return;
        }

        // Idempotent: check for existing active job
        const existingJob = await runAction('Checking for active jobs...', () =>
          getActiveJobForResource('source', sourceId),
        );

        if (existingJob) {
          const outputMode = resolveOutputMode(opts);

          if (outputMode === 'json') {
            printJson({
              data: {
                jobId: existingJob.id,
                status: existingJob.status,
                message: 'Sync already in progress',
              },
            });
            return;
          }

          if (outputMode === 'field') {
            printSingleField(
              existingJob as unknown as Record<string, unknown>,
              opts.output ?? 'id',
            );
            return;
          }

          console.log(
            c.yellow('Sync already in progress'),
            `for source ${shortId(sourceId)}.`,
          );
          console.log(
            `  Job: ${existingJob.id} (${colorStatus(existingJob.status)})`,
          );

          if (opts.watch) {
            await watchTriggeredResourceJob(
              sourceId,
              'source',
              existingJob.id,
              new Date().toISOString(),
            );
          }
          return;
        }

        // No active job — trigger a new sync
        const syncRequestedAt = new Date().toISOString();
        const res = await runAction('Triggering sync...', () =>
          getClient().post<SyncTriggerResponse>(`/sources/${sourceId}/sync`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          printSingleField(
            { id: res.data.jobId ?? sourceId, ...res.data } as Record<
              string,
              unknown
            >,
            opts.output ?? 'id',
          );
          return;
        }

        printSuccess(`Sync triggered for source ${shortId(sourceId)}.`);

        if (opts.watch) {
          await watchTriggeredResourceJob(
            sourceId,
            'source',
            res.data.jobId,
            syncRequestedAt,
          );
        } else {
          console.log(
            c.dim(`Use 'vendo jobs list --source ${sourceId}' to monitor.`),
          );
        }
      },
    );

  addExamples(syncCmd, [
    'vendo sources sync <sourceId>',
    'vendo sources sync <sourceId> --watch',
    'vendo sources sync <sourceId> --dry-run',
  ]);

  // sources pause
  const pauseCmd = cmd
    .command('pause <sourceId>')
    .description('Pause a data source')
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        sourceId: string,
        opts: { json?: boolean; dryRun?: boolean; output?: string },
      ) => {
        if (opts.dryRun) {
          printDryRun('pause', 'source', sourceId);
          return;
        }

        const res = await runAction('Pausing source...', () =>
          getClient().post(`/sources/${sourceId}/pause`),
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

        printSuccess(`Source ${shortId(sourceId)} paused.`);
      },
    );

  addExamples(pauseCmd, [
    'vendo sources pause <sourceId>',
    'vendo sources pause <sourceId> --dry-run',
  ]);

  // sources resume
  const resumeCmd = cmd
    .command('resume <sourceId>')
    .description('Resume a paused data source')
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        sourceId: string,
        opts: { json?: boolean; dryRun?: boolean; output?: string },
      ) => {
        if (opts.dryRun) {
          printDryRun('resume', 'source', sourceId);
          return;
        }

        const res = await runAction('Resuming source...', () =>
          getClient().post(`/sources/${sourceId}/resume`),
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

        printSuccess(`Source ${shortId(sourceId)} resumed.`);
      },
    );

  addExamples(resumeCmd, [
    'vendo sources resume <sourceId>',
    'vendo sources resume <sourceId> --dry-run',
  ]);

  // sources delete
  const deleteCmd = cmd
    .command('delete <sourceId>')
    .description('Delete a data source (soft delete)')
    .option('--json', 'Output raw JSON')
    .option('-y, --yes', 'Skip confirmation prompt')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        sourceId: string,
        opts: {
          json?: boolean;
          yes?: boolean;
          dryRun?: boolean;
          output?: string;
        },
      ) => {
        if (opts.dryRun) {
          printDryRun('delete', 'source', sourceId);
          return;
        }

        if (!opts.yes && !opts.json) {
          const ok = await confirm(`Delete source ${shortId(sourceId)}?`);
          if (!ok) return;
        }

        const res = await runAction('Deleting source...', () =>
          getClient().delete(`/sources/${sourceId}`),
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

        printSuccess(`Source ${shortId(sourceId)} deleted.`);
      },
    );

  addExamples(deleteCmd, [
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

function readJsonFile(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch (err) {
    const msg = err instanceof Error ? err.message : 'read error';
    throw new Error(`Failed to read ${path}: ${msg}`);
  }
}

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

interface IntegrationItem {
  id: string;
  sourceAppId?: string | null;
  sourceAppName?: string | null;
  sourceAppType?: string | null;
  destinationAppId: string;
  destinationAppName?: string | null;
  destinationAppType?: string | null;
  dataType: string;
  state: string;
  status: string;
  lastSyncAt?: string | null;
  lastError?: string | null;
  consecutiveFailures?: number;
  latestJobId?: string | null;
  createdAt: string;
}

interface IntegrationDetail extends IntegrationItem {
  config?: unknown;
  schedule?: unknown;
  isActive?: boolean;
  importDependencyStatus?: string | null;
  latestJobId?: string | null;
  historicalSyncStartDate?: string | null;
  updatedAt: string;
}

interface SyncTriggerResponse {
  jobId?: string;
  status?: string;
  message?: string;
}

export function registerIntegrationsCommand(program: Command): void {
  const cmd = program
    .command('integrations')
    .alias('int')
    .description('Manage data export integrations');

  // integrations list
  const listCmd = cmd
    .command('list')
    .description('List all integrations')
    .option('--state <state>', 'Filter by state (active, inactive)')
    .option('--status <status>', 'Filter by status')
    .option('--type <type>', 'Filter by data type')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const integrationsRequest = () =>
        getClient().get<IntegrationItem[]>('/integrations', {
          state: opts.state,
          status: opts.status,
          data_type: opts.type,
          limit: opts.limit,
          offset: opts.offset,
        });

      if (outputMode === 'json') {
        const res = await runAction(
          'Fetching integrations...',
          integrationsRequest,
        );
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        const res = await runAction(
          'Fetching integrations...',
          integrationsRequest,
        );
        printField(
          res.data as unknown as Record<string, unknown>[],
          opts.output,
        );
        return;
      }

      const [res, activeJobs] = await runAction(
        'Fetching integrations...',
        async () => Promise.all([integrationsRequest(), getActiveJobs()]),
      );

      const activeJobsByIntegrationId = new Map(
        activeJobs
          .filter((job) => job.integrationId)
          .map((job) => [job.integrationId!, job]),
      );

      const table = createTable([
        'ID',
        'Source',
        'Destination',
        'Data Type',
        'Status',
        'Progress',
        'Last Sync',
      ]);

      for (const int of res.data) {
        table.push([
          c.dim(shortId(int.id)),
          int.sourceAppName ?? c.dim('—'),
          int.destinationAppName ?? c.dim('—'),
          int.dataType,
          colorStatus(int.status),
          formatJobProgress(activeJobsByIntegrationId.get(int.id)),
          timeAgo(int.lastSyncAt),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, 'integration');
    });

  addExamples(listCmd, [
    'vendo integrations list',
    'vendo int list --state active',
    'vendo integrations list --output id',
  ]);

  // integrations get
  const getCmd = cmd
    .command('get <integrationId>')
    .description('Get integration details')
    .option('--json', 'Output raw JSON')
    .action(async (integrationId: string, opts: { json?: boolean }) => {
      const integrationRequest = () =>
        getClient().get<IntegrationDetail>(`/integrations/${integrationId}`);

      if (opts.json) {
        const res = await runAction(
          'Fetching integration...',
          integrationRequest,
        );
        printJson(res);
        return;
      }

      const [res, activeJob] = await runAction(
        'Fetching integration...',
        async () =>
          Promise.all([
            integrationRequest(),
            getActiveJobForResource('integration', integrationId),
          ]),
      );

      const int = res.data;
      console.log();
      console.log(
        c.bold(
          `${int.sourceAppName ?? '—'} → ${int.destinationAppName ?? '—'}`,
        ),
        c.dim(`(${int.dataType})`),
      );
      console.log();
      console.log(`  ID:            ${int.id}`);
      console.log(
        `  Source App:    ${int.sourceAppName ?? c.dim('—')} ${c.dim(int.sourceAppId ?? '')}`,
      );
      console.log(
        `  Dest App:     ${int.destinationAppName ?? c.dim('—')} ${c.dim(int.destinationAppId)}`,
      );
      console.log(`  Data Type:    ${int.dataType}`);
      console.log(`  State:        ${colorStatus(int.state)}`);
      console.log(`  Status:       ${colorStatus(int.status)}`);
      console.log(`  Progress:     ${formatJobProgress(activeJob)}`);
      console.log(`  Last Sync:    ${timeAgo(int.lastSyncAt)}`);
      console.log(`  Created:      ${timeAgo(int.createdAt)}`);

      if (int.schedule) {
        console.log(`  Schedule:     ${JSON.stringify(int.schedule)}`);
      }
      if (int.lastError) {
        console.log(`  Error:        ${c.red(int.lastError)}`);
      }
      if (int.consecutiveFailures && int.consecutiveFailures > 0) {
        console.log(
          `  Failures:     ${c.red(String(int.consecutiveFailures))} consecutive`,
        );
      }
      if (int.latestJobId) {
        console.log(`  Latest Job:   ${c.dim(int.latestJobId)}`);
      }
    });

  addExamples(getCmd, [
    'vendo integrations get <integrationId>',
    'vendo int get <integrationId> --json',
  ]);

  // integrations sync (with idempotent check)
  const syncCmd = cmd
    .command('sync <integrationId>')
    .description('Trigger a manual sync')
    .option('--json', 'Output raw JSON')
    .option('--watch', 'Watch the job until it completes')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id for job ID)')
    .action(
      async (
        integrationId: string,
        opts: {
          json?: boolean;
          watch?: boolean;
          dryRun?: boolean;
          output?: string;
        },
      ) => {
        // Dry-run: show what would happen
        if (opts.dryRun) {
          const [intRes, activeJob] = await runAction(
            'Checking integration...',
            async () =>
              Promise.all([
                getClient().get<IntegrationDetail>(
                  `/integrations/${integrationId}`,
                ),
                getActiveJobForResource('integration', integrationId),
              ]),
          );
          printDryRun('trigger sync for', 'integration', integrationId, {
            Source: intRes.data.sourceAppName ?? '—',
            Destination: intRes.data.destinationAppName ?? '—',
            'Data Type': intRes.data.dataType,
            'Active Job': activeJob
              ? `${shortId(activeJob.id)} (${activeJob.status})`
              : 'none',
          });
          return;
        }

        // Idempotent: check for existing active job
        const existingJob = await runAction('Checking for active jobs...', () =>
          getActiveJobForResource('integration', integrationId),
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
            `for integration ${shortId(integrationId)}.`,
          );
          console.log(
            `  Job: ${existingJob.id} (${colorStatus(existingJob.status)})`,
          );

          if (opts.watch) {
            await watchTriggeredResourceJob(
              integrationId,
              'integration',
              existingJob.id,
              new Date().toISOString(),
            );
          }
          return;
        }

        // No active job — trigger a new sync
        const syncRequestedAt = new Date().toISOString();
        const res = await runAction('Triggering sync...', () =>
          getClient().post<SyncTriggerResponse>(
            `/integrations/${integrationId}/sync`,
          ),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          printSingleField(
            { id: res.data.jobId ?? integrationId, ...res.data } as Record<
              string,
              unknown
            >,
            opts.output ?? 'id',
          );
          return;
        }

        printSuccess(
          `Sync triggered for integration ${shortId(integrationId)}.`,
        );

        if (opts.watch) {
          await watchTriggeredResourceJob(
            integrationId,
            'integration',
            res.data.jobId,
            syncRequestedAt,
          );
        } else {
          console.log(
            c.dim(
              `Use 'vendo jobs list --integration ${integrationId}' to monitor.`,
            ),
          );
        }
      },
    );

  addExamples(syncCmd, [
    'vendo integrations sync <integrationId>',
    'vendo int sync <integrationId> --watch',
    'vendo integrations sync <integrationId> --dry-run',
  ]);

  // integrations pause
  const pauseCmd = cmd
    .command('pause <integrationId>')
    .description('Pause an integration')
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        integrationId: string,
        opts: { json?: boolean; dryRun?: boolean; output?: string },
      ) => {
        if (opts.dryRun) {
          printDryRun('pause', 'integration', integrationId);
          return;
        }

        const res = await runAction('Pausing integration...', () =>
          getClient().post(`/integrations/${integrationId}/pause`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(integrationId);
          return;
        }

        printSuccess(`Integration ${shortId(integrationId)} paused.`);
      },
    );

  addExamples(pauseCmd, [
    'vendo integrations pause <integrationId>',
    'vendo int pause <integrationId> --dry-run',
  ]);

  // integrations resume
  const resumeCmd = cmd
    .command('resume <integrationId>')
    .description('Resume a paused integration')
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        integrationId: string,
        opts: { json?: boolean; dryRun?: boolean; output?: string },
      ) => {
        if (opts.dryRun) {
          printDryRun('resume', 'integration', integrationId);
          return;
        }

        const res = await runAction('Resuming integration...', () =>
          getClient().post(`/integrations/${integrationId}/resume`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(integrationId);
          return;
        }

        printSuccess(`Integration ${shortId(integrationId)} resumed.`);
      },
    );

  addExamples(resumeCmd, [
    'vendo integrations resume <integrationId>',
    'vendo integrations resume <integrationId> --dry-run',
  ]);

  // integrations delete
  const deleteCmd = cmd
    .command('delete <integrationId>')
    .description('Delete an integration (soft delete)')
    .option('--json', 'Output raw JSON')
    .option('-y, --yes', 'Skip confirmation prompt')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        integrationId: string,
        opts: {
          json?: boolean;
          yes?: boolean;
          dryRun?: boolean;
          output?: string;
        },
      ) => {
        if (opts.dryRun) {
          printDryRun('delete', 'integration', integrationId);
          return;
        }

        if (!opts.yes && !opts.json) {
          const ok = await confirm(
            `Delete integration ${shortId(integrationId)}?`,
          );
          if (!ok) return;
        }

        const res = await runAction('Deleting integration...', () =>
          getClient().delete(`/integrations/${integrationId}`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(integrationId);
          return;
        }

        printSuccess(`Integration ${shortId(integrationId)} deleted.`);
      },
    );

  addExamples(deleteCmd, [
    'vendo integrations delete <integrationId>',
    'vendo int delete <integrationId> --yes',
    'vendo integrations delete <integrationId> --dry-run',
  ]);

  // integrations create
  const createCmd = cmd
    .command('create')
    .description('Create a new integration (source → destination pipeline)')
    .requiredOption(
      '--dest-app <appId>',
      'Destination app ID (must have destination role)',
    )
    .option(
      '--source-app <appId>',
      'Source app ID (optional for some data types)',
    )
    .requiredOption(
      '--data-type <type>',
      'Data type (e.g. events, user_properties, conversions)',
    )
    .requiredOption(
      '--config-file <path>',
      'Path to JSON with { global?, tasks: [...] }',
    )
    .option('--schedule-file <path>', 'Path to JSON with schedule overrides')
    .option('--frequency <value>', 'Sync frequency value', '1')
    .option(
      '--unit <unit>',
      'Sync frequency unit (hours, days, weeks, months)',
      'days',
    )
    .option('--run-now', 'Trigger first sync immediately')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (opts) => {
      const config = readJsonFile(opts.configFile);
      const schedule = opts.scheduleFile
        ? (readJsonFile(opts.scheduleFile) as Record<string, unknown>)
        : {
            frequencyValue: Number(opts.frequency),
            frequencyUnit: opts.unit,
            runNow: Boolean(opts.runNow),
          };

      const body = {
        destinationAppId: opts.destApp,
        sourceAppId: opts.sourceApp,
        dataType: opts.dataType,
        config,
        schedule,
      };

      const res = await runAction('Creating integration...', () =>
        getClient().post<IntegrationDetail>('/integrations', body),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      const int = res.data;
      if (!int) {
        printSuccess('Integration created.');
        return;
      }

      if (outputMode === 'field') {
        console.log(int.id);
        return;
      }

      printSuccess(`Integration ${shortId(int.id)} created.`);
      printLabel('Data type', int.dataType);
      printLabel('State', colorStatus(int.state));
    });

  addExamples(createCmd, [
    'vendo int create --dest-app <onesignal-app-id> --data-type events --config-file tasks.json',
    'vendo int create --source-app <bq-id> --dest-app <os-id> --data-type user_properties --config-file tasks.json --run-now',
  ]);

  // integrations update
  const updateCmd = cmd
    .command('update <integrationId>')
    .description('Update an integration')
    .option('--config-file <path>', 'Replace config from a JSON file')
    .option('--schedule-file <path>', 'Replace schedule from a JSON file')
    .option('--frequency <value>', 'Sync frequency value')
    .option('--unit <unit>', 'Sync frequency unit')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (integrationId: string, opts) => {
      const body: Record<string, unknown> = {};
      if (opts.configFile) body.config = readJsonFile(opts.configFile);

      if (opts.scheduleFile) {
        body.schedule = readJsonFile(opts.scheduleFile);
      } else if (opts.frequency !== undefined || opts.unit !== undefined) {
        const schedule: Record<string, unknown> = {};
        if (opts.frequency !== undefined) {
          schedule.frequencyValue = Number(opts.frequency);
        }
        if (opts.unit !== undefined) schedule.frequencyUnit = opts.unit;
        body.schedule = schedule;
      }

      if (Object.keys(body).length === 0) {
        throw new Error('Nothing to update — pass at least one flag.');
      }

      const res = await runAction('Updating integration...', () =>
        getClient().patch<IntegrationDetail>(
          `/integrations/${integrationId}`,
          body,
        ),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        console.log(integrationId);
        return;
      }

      printSuccess(`Integration ${shortId(integrationId)} updated.`);
    });

  addExamples(updateCmd, [
    'vendo int update <integrationId> --frequency 6 --unit hours',
    'vendo int update <integrationId> --config-file new-tasks.json',
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

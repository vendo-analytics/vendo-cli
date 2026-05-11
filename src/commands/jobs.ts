import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  createJobDetailLines,
  createJobErrorLines,
  formatJobDuration,
} from '../job-output.js';
import { formatJobProgress } from '../job-progress.js';
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
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
  showArgError,
  timeAgo,
} from '../output.js';
import {
  getLatestJobForResource,
  tailJob,
  watchActiveJobs,
  watchJob,
} from '../watch-job.js';

interface JobItem {
  id: string;
  sourceId?: string | null;
  integrationId?: string | null;
  jobType: string;
  connectorType?: string | null;
  status: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  errorMessage?: string | null;
  errorCode?: string | null;
  errorCategory?: string | null;
  progressPct?: number | null;
  rowsProcessed?: number | null;
  rowsWritten?: number | null;
  chunkIndex?: number | null;
  totalChunks?: number | null;
  createdAt: string;
}

interface JobDetail extends JobItem {
  executionType?: string | null;
  trigger?: string | null;
  tasks?: unknown;
  parentJobId?: string | null;
  chunkIndex?: number | null;
  totalChunks?: number | null;
  isOnboarding?: boolean;
}

export function registerJobsCommand(program: Command): void {
  const cmd = program.command('jobs').description('Monitor sync jobs');

  // jobs list
  const listCmd = cmd
    .command('list')
    .description('List sync jobs')
    .option(
      '--status <status>',
      'Filter by status (pending, running, completed, failed, cancelled)',
    )
    .option('--type <type>', 'Filter by job type (import, export)')
    .option('--source <sourceId>', 'Filter by source ID')
    .option('--integration <integrationId>', 'Filter by integration ID')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const res = await runAction('Fetching jobs...', () =>
        getClient().get<JobItem[]>('/jobs', {
          status: opts.status,
          job_type: opts.type,
          source_id: opts.source,
          integration_id: opts.integration,
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

      const table = createTable([
        'ID',
        'Type',
        'Connector',
        'Status',
        'Progress',
        'Started',
        'Duration',
      ]);

      for (const job of res.data) {
        table.push([
          c.dim(shortId(job.id)),
          job.jobType,
          job.connectorType ?? c.dim('—'),
          colorStatus(job.status),
          formatJobProgress(job),
          timeAgo(job.startedAt ?? job.createdAt),
          formatJobDuration(job.startedAt, job.finishedAt),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, 'job');
    });

  addExamples(listCmd, [
    'vendo jobs list',
    'vendo jobs list --status running',
    'vendo jobs list --source <sourceId>',
    'vendo jobs list --output id',
  ]);

  // jobs get
  const getCmd = cmd
    .command('get <jobId>')
    .description('Get job details')
    .option('--json', 'Output raw JSON')
    .action(async (jobId: string, opts: { json?: boolean }) => {
      const res = await runAction('Fetching job...', () =>
        getClient().get<JobDetail>(`/jobs/${jobId}`),
      );

      if (opts.json) {
        printJson(res);
        return;
      }

      const job = res.data;
      console.log();
      console.log(
        c.bold(`${job.jobType} job`),
        c.dim(`(${job.connectorType ?? 'unknown'})`),
      );
      console.log();
      for (const line of createJobDetailLines(job)) {
        console.log(line);
      }
      console.log(`  Finished:      ${timeAgo(job.finishedAt)}`);

      const errorLines = createJobErrorLines(job);
      if (errorLines.length > 0) {
        console.log();
        for (const line of errorLines) {
          console.log(line);
        }
      }
    });

  addExamples(getCmd, [
    'vendo jobs get <jobId>',
    'vendo jobs get <jobId> --json',
  ]);

  // jobs cancel
  const cancelCmd = cmd
    .command('cancel <jobId>')
    .description('Cancel a pending or running job')
    .option('--json', 'Output raw JSON')
    .option('-y, --yes', 'Skip confirmation prompt')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(
      async (
        jobId: string,
        opts: {
          json?: boolean;
          yes?: boolean;
          dryRun?: boolean;
          output?: string;
        },
      ) => {
        if (opts.dryRun) {
          printDryRun('cancel', 'job', jobId);
          return;
        }

        if (!opts.yes && !opts.json) {
          const ok = await confirm(`Cancel job ${shortId(jobId)}?`);
          if (!ok) return;
        }

        const res = await runAction('Cancelling job...', () =>
          getClient().post(`/jobs/${jobId}/cancel`),
        );

        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson(res);
          return;
        }

        if (outputMode === 'field') {
          console.log(jobId);
          return;
        }

        printSuccess(`Job ${shortId(jobId)} cancelled.`);
      },
    );

  addExamples(cancelCmd, [
    'vendo jobs cancel <jobId>',
    'vendo jobs cancel <jobId> --yes',
    'vendo jobs cancel <jobId> --dry-run',
  ]);

  // jobs watch
  const watchCmd = cmd
    .command('watch')
    .description('Watch running and pending jobs (live polling)')
    .option('--interval <seconds>', 'Polling interval in seconds', '5')
    .option('--source <sourceId>', 'Filter by source ID')
    .option('--integration <integrationId>', 'Filter by integration ID')
    .action(async (opts) => {
      const intervalSeconds = Number(opts.interval ?? '5');
      if (!Number.isFinite(intervalSeconds) || intervalSeconds <= 0) {
        showArgError('Polling interval must be a positive number of seconds.', [
          'vendo jobs watch',
          'vendo jobs watch --interval 10',
        ]);
      }

      await watchActiveJobs({
        intervalMs: Math.floor(intervalSeconds * 1000),
        sourceId: opts.source,
        integrationId: opts.integration,
      });
    });

  addExamples(watchCmd, [
    'vendo jobs watch',
    'vendo jobs watch --source <sourceId>',
    'vendo jobs watch --interval 10',
  ]);

  // jobs tail
  const tailCmd = cmd
    .command('tail [jobId]')
    .description('Tail a single job or the latest job for a source/integration')
    .option('--source <sourceId>', 'Tail the latest job for a source')
    .option(
      '--integration <integrationId>',
      'Tail the latest job for an integration',
    )
    .option(
      '--next',
      'Wait for the next new job when tailing a source or integration',
    )
    .option('--interval <seconds>', 'Polling interval in seconds', '3')
    .action(
      async (
        jobId: string | undefined,
        opts: {
          source?: string;
          integration?: string;
          next?: boolean;
          interval?: string;
        },
      ) => {
        const targetCount = [jobId, opts.source, opts.integration].filter(
          Boolean,
        ).length;

        if (targetCount !== 1) {
          showArgError(
            'Specify exactly one target: <jobId>, --source <sourceId>, or --integration <integrationId>.',
            [
              'vendo jobs tail <jobId>',
              'vendo jobs tail --source <sourceId>',
              'vendo jobs tail --integration <integrationId>',
              'vendo jobs tail --source <sourceId> --next',
            ],
          );
        }

        const intervalSeconds = Number(opts.interval ?? '3');
        if (!Number.isFinite(intervalSeconds) || intervalSeconds <= 0) {
          showArgError(
            'Polling interval must be a positive number of seconds.',
            [
              'vendo jobs tail <jobId>',
              'vendo jobs tail --source <sourceId> --interval 5',
            ],
          );
        }

        const intervalMs = Math.floor(intervalSeconds * 1000);

        if (jobId) {
          if (opts.next) {
            showArgError(
              '`--next` can only be used with `--source` or `--integration`.',
              [
                'vendo jobs tail --source <sourceId> --next',
                'vendo jobs tail --integration <integrationId> --next',
              ],
            );
          }

          await tailJob(jobId, { intervalMs });
          return;
        }

        if (opts.source) {
          if (opts.next) {
            const baseline = await getLatestJobForResource(
              opts.source,
              'source',
            );
            await watchJob(opts.source, 'source', {
              intervalMs,
              afterCreatedAt: baseline?.createdAt,
              skipJobId: baseline?.id,
            });
            return;
          }

          await watchJob(opts.source, 'source', { intervalMs });
          return;
        }

        if (opts.next) {
          const baseline = await getLatestJobForResource(
            opts.integration!,
            'integration',
          );
          await watchJob(opts.integration!, 'integration', {
            intervalMs,
            afterCreatedAt: baseline?.createdAt,
            skipJobId: baseline?.id,
          });
          return;
        }

        await watchJob(opts.integration!, 'integration', { intervalMs });
      },
    );

  addExamples(tailCmd, [
    'vendo jobs tail <jobId>',
    'vendo jobs tail --source <sourceId>',
    'vendo jobs tail --source <sourceId> --next',
    'vendo jobs tail --integration <integrationId>',
  ]);
}

import { ClientError, getClient } from './client.js';
import {
  type JobOutputDetail,
  createJobDetailLines,
  createJobErrorLines,
  formatCompletedJobSummary,
  formatJobDuration,
} from './job-output.js';
import { formatJobProgress, getActiveJobs } from './job-progress.js';
import {
  c,
  colorStatus,
  createTable,
  exitWithError,
  printError,
  printSuccess,
  shortId,
  spinner,
  timeAgo,
} from './output.js';

interface JobListItem {
  id: string;
  status: string;
  createdAt: string;
}

type JobDetail = JobOutputDetail;

interface WatchJobOptions {
  intervalMs?: number;
  afterCreatedAt?: string;
  skipJobId?: string;
}

interface WatchActiveJobsOptions {
  intervalMs?: number;
  sourceId?: string;
  integrationId?: string;
}

const TERMINAL_STATUSES = new Set([
  'completed',
  'failed',
  'cancelled',
  'errored',
]);
const DEFAULT_POLL_INTERVAL_MS = 3000;
const MAX_WAIT_MS = 30 * 60 * 1000;

export async function watchActiveJobs(
  options: WatchActiveJobsOptions = {},
): Promise<void> {
  const intervalMs = options.intervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const intervalSeconds = Math.max(1, Math.floor(intervalMs / 1000));
  let stopped = false;
  let isPolling = false;
  let lastRendered = '';

  const stopWatching = () => {
    if (stopped) {
      return;
    }

    stopped = true;
    process.off('SIGINT', handleSigInt);
    console.log();
    console.log(c.dim('Stopped watching.'));
  };

  const handleSigInt = () => {
    stopWatching();
  };

  const poll = async () => {
    if (isPolling || stopped) {
      return;
    }

    isPolling = true;
    try {
      const jobs = await getActiveJobs({
        limit: 20,
        sourceId: options.sourceId,
        integrationId: options.integrationId,
      });
      const snapshot = renderActiveJobsSnapshot(jobs, intervalSeconds, options);
      renderSnapshot(snapshot, lastRendered);
      lastRendered = snapshot;
    } catch (err) {
      if (isFatalPollingError(err)) {
        process.off('SIGINT', handleSigInt);
        exitWithError(err);
      }

      const message = err instanceof Error ? err.message : String(err);
      const snapshot = renderActiveJobsSnapshot(
        undefined,
        intervalSeconds,
        options,
        message,
      );
      renderSnapshot(snapshot, lastRendered);
      lastRendered = snapshot;
    } finally {
      isPolling = false;
    }
  };

  process.on('SIGINT', handleSigInt);

  await poll();

  while (!stopped) {
    await sleep(intervalMs);
    await poll();
  }
}

/**
 * Poll jobs for a given source or integration, resolve the latest job, then tail it.
 */
export async function watchJob(
  resourceId: string,
  resourceType: 'source' | 'integration',
  options: WatchJobOptions = {},
): Promise<void> {
  const intervalMs = options.intervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const label = resourceType === 'source' ? 'source' : 'integration';
  const waitingForNextJob =
    options.skipJobId !== undefined || options.afterCreatedAt !== undefined;
  const s = spinner(
    waitingForNextJob
      ? `Waiting for next ${label} job...`
      : `Waiting for latest ${label} job...`,
  );
  s.start();

  const startedAt = Date.now();

  while (Date.now() - startedAt < MAX_WAIT_MS) {
    try {
      const latestJob = await getLatestJobForResource(resourceId, resourceType);

      if (shouldTailJob(latestJob, options)) {
        s.stop();
        await tailJob(latestJob.id, { intervalMs });
        return;
      }
    } catch (err) {
      if (isFatalPollingError(err)) {
        s.stop();
        exitWithError(err);
      }
    }

    await sleep(intervalMs);
  }

  s.stop();
  console.log(
    c.dim(
      `Timed out waiting for ${waitingForNextJob ? 'the next' : 'a'} ${label} job. Use \`vendo jobs list --${label} ${resourceId}\` to check status.`,
    ),
  );
}

export async function getLatestJobForResource(
  resourceId: string,
  resourceType: 'source' | 'integration',
): Promise<JobListItem | undefined> {
  const paramKey = resourceType === 'source' ? 'source_id' : 'integration_id';
  const res = await getClient().get<JobListItem[]>('/jobs', {
    [paramKey]: resourceId,
    limit: 1,
    sort: 'created_at:desc',
  });

  return res.data[0];
}

export async function watchTriggeredResourceJob(
  resourceId: string,
  resourceType: 'source' | 'integration',
  jobId: string | undefined,
  syncRequestedAt: string,
  intervalMs?: number,
): Promise<void> {
  if (jobId) {
    await tailJob(jobId, { intervalMs });
    return;
  }

  await watchJob(resourceId, resourceType, {
    intervalMs,
    afterCreatedAt: syncRequestedAt,
  });
}

export async function tailJob(
  jobId: string,
  options: WatchJobOptions = {},
): Promise<void> {
  const intervalMs = options.intervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const client = getClient();
  const startedAt = Date.now();
  let lastRendered = '';
  let lastKnownJob: JobDetail | undefined;

  while (Date.now() - startedAt < MAX_WAIT_MS) {
    try {
      const res = await client.get<JobDetail>(`/jobs/${jobId}`);
      const job = res.data;
      lastKnownJob = job;

      const snapshot = renderTailSnapshot(job, intervalMs);
      renderSnapshot(snapshot, lastRendered);
      lastRendered = snapshot;

      if (TERMINAL_STATUSES.has(job.status)) {
        printTailResult(job);
        return;
      }
    } catch (err) {
      if (isFatalPollingError(err)) {
        exitWithError(err);
      }

      const message = err instanceof Error ? err.message : String(err);
      const snapshot = renderTailSnapshot(lastKnownJob, intervalMs, message);
      renderSnapshot(snapshot, lastRendered);
      lastRendered = snapshot;
    }

    await sleep(intervalMs);
  }

  printError(
    `Timed out waiting for job ${shortId(jobId)}. Use \`vendo jobs get ${jobId}\` to check status.`,
  );
}

function renderTailSnapshot(
  job: JobDetail | undefined,
  intervalMs: number,
  pollError?: string,
): string {
  const lines = [
    c.dim(
      `Tailing job ${job ? shortId(job.id) : '...'} (refreshing every ${Math.max(1, Math.floor(intervalMs / 1000))}s)`,
    ),
    '',
  ];

  if (!job) {
    lines.push(c.dim('Waiting for job details...'));
    if (pollError) {
      lines.push('');
      lines.push(`${c.yellow('Last poll failed:')} ${pollError}`);
    }
    return lines.join('\n');
  }

  lines.push(...createJobDetailLines(job));

  const errorLines = createJobErrorLines(job);
  if (errorLines.length > 0) {
    lines.push('');
    lines.push(...errorLines);
  }
  if (pollError) {
    lines.push('');
    lines.push(`${c.yellow('Last poll failed:')} ${pollError}`);
  }

  return lines.join('\n');
}

function renderActiveJobsSnapshot(
  jobs: JobOutputDetail[] | undefined,
  intervalSeconds: number,
  options: WatchActiveJobsOptions,
  pollError?: string,
): string {
  const scopeLabel = getWatchScopeLabel(options);
  const lines = [
    c.dim(
      `Watching ${scopeLabel}jobs... (Ctrl+C to stop, refreshing every ${intervalSeconds}s)`,
    ),
    '',
  ];

  if (!jobs) {
    lines.push(c.dim('Waiting for jobs...'));
  } else if (jobs.length === 0) {
    lines.push(c.dim('No running or pending jobs.'));
  } else {
    const table = createTable([
      'ID',
      'Type',
      'Connector',
      'Status',
      'Progress',
      'Started',
      'Duration',
    ]);

    for (const job of jobs) {
      table.push([
        c.dim(shortId(job.id)),
        job.jobType ?? c.dim('—'),
        job.connectorType ?? c.dim('—'),
        colorStatus(job.status),
        formatJobProgress(job),
        timeAgo(job.startedAt ?? job.createdAt),
        formatJobDuration(job.startedAt, job.finishedAt),
      ]);
    }

    lines.push(table.toString());

    const running = jobs.filter((job) => job.status === 'running').length;
    const pending = jobs.filter((job) => job.status === 'pending').length;
    lines.push(c.dim(`${running} running, ${pending} pending`));
  }

  if (pollError) {
    lines.push('');
    lines.push(`${c.yellow('Last poll failed:')} ${pollError}`);
  }

  return lines.join('\n');
}

function getWatchScopeLabel(options: WatchActiveJobsOptions): string {
  if (options.sourceId) {
    return `source ${options.sourceId} `;
  }

  if (options.integrationId) {
    return `integration ${options.integrationId} `;
  }

  return '';
}

function shouldTailJob(
  latestJob: JobListItem | undefined,
  options: WatchJobOptions,
): latestJob is JobListItem {
  if (!latestJob) {
    return false;
  }

  if (options.skipJobId && latestJob.id === options.skipJobId) {
    return false;
  }

  if (!options.afterCreatedAt) {
    return true;
  }

  const thresholdMs = Date.parse(options.afterCreatedAt);
  const createdAtMs = Date.parse(latestJob.createdAt);

  if (!Number.isFinite(thresholdMs) || !Number.isFinite(createdAtMs)) {
    return true;
  }

  return createdAtMs > thresholdMs;
}

function renderSnapshot(snapshot: string, lastRendered: string): void {
  if (process.stdout.isTTY) {
    process.stdout.write('\x1B[2J\x1B[0f');
    console.log(snapshot);
    return;
  }

  if (snapshot !== lastRendered) {
    console.log(snapshot);
    console.log();
  }
}

function printTailResult(job: JobDetail): void {
  console.log();

  if (job.status === 'completed') {
    printSuccess(formatCompletedJobSummary(job));
    return;
  }

  if (job.status === 'failed' || job.status === 'errored') {
    printError(
      `Job ${shortId(job.id)} ${job.status}: ${job.errorMessage ?? 'Unknown error'}`,
    );
    return;
  }

  console.log(`Job ${shortId(job.id)} ${colorStatus(job.status)}.`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isFatalPollingError(err: unknown): boolean {
  if (!(err instanceof Error)) {
    return true;
  }

  if (!(err instanceof ClientError)) {
    return true;
  }

  return err.statusCode === 401 || err.statusCode === 403;
}

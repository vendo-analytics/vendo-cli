import { formatJobProgress } from './job-progress.js';
import { c, colorStatus, formatNumber, shortId, timeAgo } from './output.js';

export interface JobOutputDetail {
  id: string;
  status: string;
  jobType?: string | null;
  connectorType?: string | null;
  sourceId?: string | null;
  integrationId?: string | null;
  progressPct?: number | null;
  rowsProcessed?: number | null;
  rowsWritten?: number | null;
  errorMessage?: string | null;
  errorCode?: string | null;
  errorCategory?: string | null;
  startedAt?: string | null;
  finishedAt?: string | null;
  createdAt?: string | null;
  executionType?: string | null;
  trigger?: string | null;
  parentJobId?: string | null;
  chunkIndex?: number | null;
  totalChunks?: number | null;
}

export function formatJobDuration(
  startedAt: string | null | undefined,
  finishedAt: string | null | undefined,
): string {
  if (!startedAt) return c.dim('—');

  const start = new Date(startedAt).getTime();
  const end = finishedAt ? new Date(finishedAt).getTime() : Date.now();
  const diffMs = end - start;

  if (diffMs < 1000) return '0s';

  const seconds = Math.floor(diffMs / 1000);
  if (seconds < 60) return `${seconds}s`;

  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  if (minutes < 60)
    return remainingSeconds > 0
      ? `${minutes}m ${remainingSeconds}s`
      : `${minutes}m`;

  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

export function createJobDetailLines(job: JobOutputDetail): string[] {
  const lines = [
    `  ID:            ${job.id}`,
    `  Status:        ${colorStatus(job.status)}`,
    `  Progress:      ${formatJobProgress(job)}`,
    `  Type:          ${job.jobType ?? c.dim('—')}`,
    `  Connector:     ${job.connectorType ?? c.dim('—')}`,
    `  Started:       ${timeAgo(job.startedAt ?? job.createdAt)}`,
    `  Duration:      ${formatJobDuration(job.startedAt, job.finishedAt)}`,
    `  Rows Read:     ${formatNumber(job.rowsProcessed)}`,
    `  Rows Written:  ${formatNumber(job.rowsWritten)}`,
  ];

  if (job.sourceId) {
    lines.push(`  Source:        ${c.dim(job.sourceId)}`);
  }
  if (job.integrationId) {
    lines.push(`  Integration:   ${c.dim(job.integrationId)}`);
  }
  if (job.executionType) {
    lines.push(`  Execution:     ${job.executionType}`);
  }
  if (job.trigger) {
    lines.push(`  Trigger:       ${job.trigger}`);
  }
  if (job.parentJobId) {
    lines.push(`  Parent Job:    ${c.dim(job.parentJobId)}`);
  }
  if (
    job.chunkIndex != null &&
    job.totalChunks != null &&
    job.totalChunks > 1
  ) {
    lines.push(`  Chunk:         ${job.chunkIndex + 1} of ${job.totalChunks}`);
  }

  return lines;
}

export function createJobErrorLines(
  job: Pick<JobOutputDetail, 'errorMessage' | 'errorCode' | 'errorCategory'>,
): string[] {
  if (!job.errorMessage) {
    return [];
  }

  const lines = [`  ${c.red('Error:')}  ${job.errorMessage}`];

  if (job.errorCode) {
    lines.push(`  ${c.red('Code:')}   ${job.errorCode}`);
  }
  if (job.errorCategory) {
    lines.push(`  ${c.red('Category:')} ${job.errorCategory}`);
  }

  return lines;
}

export function formatCompletedJobSummary(
  job: Pick<JobOutputDetail, 'id' | 'rowsProcessed' | 'rowsWritten'>,
): string {
  return `Job ${shortId(job.id)} completed. ${formatNumber(job.rowsProcessed)} rows processed, ${formatNumber(job.rowsWritten)} written.`;
}

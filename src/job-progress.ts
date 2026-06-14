import { getClient } from './client.js';
import { type JobOutputDetail } from './job-output.js';
import { c, formatNumber } from './output.js';

export interface ProgressJob {
  id: string;
  sourceId?: string | null;
  integrationId?: string | null;
  status: string;
  progressPct?: number | null;
  rowsProcessed?: number | null;
  rowsWritten?: number | null;
  chunkIndex?: number | null;
  totalChunks?: number | null;
}

export interface ActiveJobsQuery {
  limit?: number;
  sourceId?: string;
  integrationId?: string;
}

/**
 * Fetch running/pending jobs, optionally scoped to a single source or
 * integration. The full {@link JobOutputDetail} shape is returned so callers
 * that render rich tables (e.g. `watchActiveJobs`) have the fields they need;
 * lighter callers simply read the subset they care about.
 */
export async function getActiveJobs(
  query: ActiveJobsQuery = {},
): Promise<JobOutputDetail[]> {
  const { limit = 100, sourceId, integrationId } = query;
  const res = await getClient().get<JobOutputDetail[]>('/jobs', {
    status: 'running,pending',
    limit,
    sort: 'created_at:desc',
    source_id: sourceId,
    integration_id: integrationId,
  });

  return res.data;
}

export async function getActiveJobForResource(
  resourceType: 'source' | 'integration',
  resourceId: string,
): Promise<ProgressJob | undefined> {
  const paramKey = resourceType === 'source' ? 'source_id' : 'integration_id';
  const res = await getClient().get<ProgressJob[]>('/jobs', {
    status: 'running,pending',
    limit: 1,
    sort: 'created_at:desc',
    [paramKey]: resourceId,
  });

  return res.data[0];
}

export function formatJobProgress(job?: ProgressJob | null): string {
  if (!job) return c.dim('—');

  const parts: string[] = [];

  if (job.progressPct != null) {
    parts.push(`${job.progressPct}%`);
  }

  if (
    job.chunkIndex != null &&
    job.totalChunks != null &&
    job.totalChunks > 1
  ) {
    parts.push(`chunk ${job.chunkIndex + 1}/${job.totalChunks}`);
  }

  if (job.rowsProcessed != null && job.rowsWritten != null) {
    if (job.rowsProcessed === job.rowsWritten) {
      parts.push(`${formatNumber(job.rowsProcessed)} rows`);
    } else {
      parts.push(`${formatNumber(job.rowsProcessed)} read`);
      parts.push(`${formatNumber(job.rowsWritten)} written`);
    }
  } else if (job.rowsProcessed != null) {
    parts.push(`${formatNumber(job.rowsProcessed)} rows`);
  } else if (job.rowsWritten != null) {
    parts.push(`${formatNumber(job.rowsWritten)} written`);
  }

  if (parts.length > 0) {
    return parts.join(' · ');
  }

  if (job.status === 'pending') return c.dim('queued');
  if (job.status === 'running') return c.dim('starting');
  return c.dim('—');
}

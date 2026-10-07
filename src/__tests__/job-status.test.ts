import { describe, expect, it, vi } from 'vitest';

// Job status vocabulary the API actually uses (staging jobs table,
// 2026-10-05): completed, warning, failed, canceled, queued, running, and
// the legacy cancelled (VE-3695).
const get = vi.fn(async () => ({ data: [] }));
vi.mock('../client.js', () => ({ getClient: () => ({ get }) }));

const { formatJobProgress, getActiveJobForResource, getActiveJobs } =
  await import('../job-progress.js');
const { isTerminalJobStatus } = await import('../watch-job.js');

describe('job statuses (VE-3695)', () => {
  it('treats every finished status as terminal, including canceled and warning', () => {
    for (const status of [
      'completed',
      'warning',
      'failed',
      'canceled',
      'cancelled',
      'errored',
    ]) {
      expect(isTerminalJobStatus(status), status).toBe(true);
    }
    for (const status of ['queued', 'pending', 'running']) {
      expect(isTerminalJobStatus(status), status).toBe(false);
    }
  });

  it('shows queued jobs as queued', () => {
    expect(formatJobProgress({ id: 'j', status: 'queued' })).toContain(
      'queued',
    );
  });

  it('asks for queued jobs when listing active ones', async () => {
    await getActiveJobs({ sourceId: 'src-1' });
    await getActiveJobForResource('integration', 'int-1');
    expect(get).toHaveBeenNthCalledWith(
      1,
      '/jobs',
      expect.objectContaining({ status: 'running,pending,queued' }),
    );
    expect(get).toHaveBeenNthCalledWith(
      2,
      '/jobs',
      expect.objectContaining({ status: 'running,pending,queued' }),
    );
  });
});

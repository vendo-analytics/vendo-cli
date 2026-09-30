import { Command } from 'commander';
import { afterEach, describe, expect, it, vi } from 'vitest';

const { get } = vi.hoisted(() => ({ get: vi.fn() }));
vi.mock('../client.js', () => ({ getClient: () => ({ get }) }));
vi.mock('../job-progress.js', () => ({
  getActiveJobForResource: async () => null,
  getActiveJobs: async () => [],
  formatJobProgress: () => 'No active job',
}));
vi.mock('../output.js', async (original) => ({
  ...(await original<typeof import('../output.js')>()),
  runAction: async (_message: string, action: () => Promise<unknown>) =>
    action(),
}));

import { registerSourcesCommand } from '../commands/sources.js';

afterEach(() => vi.restoreAllMocks());

async function showSource(progress?: object | null) {
  get.mockResolvedValueOnce({
    data: {
      id: 'source-1',
      appId: 'app-1',
      syncType: 'mongodb',
      state: 'active',
      integrationStatus: 'warning',
      createdAt: '2026-01-01T00:00:00Z',
      lastSyncAt: '2026-09-30T01:00:00Z',
      earliestDataAt: '2020-01-01T00:00:00Z',
      latestDataAt: '2025-01-01T00:00:00Z',
      importProgress: progress,
    },
  });
  const log = vi.spyOn(console, 'log').mockImplementation(() => {});
  const program = new Command();
  registerSourcesCommand(program);
  await program.parseAsync(['sources', 'get', 'source-1'], { from: 'user' });
  expect(get).toHaveBeenCalledWith('/sources/source-1');
  return log.mock.calls.map((args) => args.join(' ')).join('\n');
}

describe('sources get progress', () => {
  it('renders the server checkpoint and partial-stream evidence, not legacy dates', async () => {
    const output = await showSource({
      latestCheckpointAt: '2026-09-30T00:00:00Z',
      enabledStreamCount: 3,
      checkpointStreamCount: 2,
      attentionStreamCount: 1,
    });
    expect(output).toContain(
      'Latest import checkpoint: 2026-09-30T00:00:00.000Z',
    );
    expect(output).toContain('2/3 with checkpoints; 1 need attention');
    expect(output).toContain('Last successful sync:');
    expect(output).toContain('Record date range: Not measured');
    expect(output).not.toContain('2020');
    expect(output).not.toContain('2025');
  });

  it('handles an older server without substituting its legacy range or sync time', async () => {
    const output = await showSource();
    expect(output).toContain('Latest import checkpoint: Not available');
    expect(output).not.toContain('Data Range:');
    expect(output).not.toContain('undefined/undefined');
  });

  it('does not label read-in-place sources as imports', async () => {
    const output = await showSource(null);
    expect(output).toContain('Data access: Read in place');
    expect(output).not.toContain('Latest import checkpoint:');
  });

  it('does not print an invalid server date as a checkpoint', async () => {
    const output = await showSource({
      latestCheckpointAt: 'invalid',
      enabledStreamCount: 0,
      checkpointStreamCount: 0,
      attentionStreamCount: 0,
    });
    expect(output).toContain('Latest import checkpoint: Not available');
    expect(output).not.toContain('Invalid Date');
  });
});

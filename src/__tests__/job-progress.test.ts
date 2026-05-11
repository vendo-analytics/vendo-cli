import { describe, expect, it } from 'vitest';

import { formatJobProgress } from '../job-progress.js';

describe('formatJobProgress', () => {
  it('returns em dash when no job is present', () => {
    expect(formatJobProgress()).toContain('—');
  });

  it('formats matching read/write counts as rows', () => {
    expect(
      formatJobProgress({
        id: 'job-1',
        status: 'running',
        progressPct: 50,
        rowsProcessed: 1200,
        rowsWritten: 1200,
      }),
    ).toBe('50% · 1,200 rows');
  });

  it('formats separate read and written counts', () => {
    expect(
      formatJobProgress({
        id: 'job-2',
        status: 'running',
        rowsProcessed: 1200,
        rowsWritten: 900,
      }),
    ).toBe('1,200 read · 900 written');
  });

  it('shows percentage-only progress when that is all the backend can estimate', () => {
    expect(
      formatJobProgress({
        id: 'job-2b',
        status: 'running',
        progressPct: 38,
      }),
    ).toBe('38%');
  });

  it('includes chunk progress when available', () => {
    expect(
      formatJobProgress({
        id: 'job-3',
        status: 'running',
        chunkIndex: 1,
        totalChunks: 4,
        rowsProcessed: 500,
      }),
    ).toBe('chunk 2/4 · 500 rows');
  });

  it('shows queued state for pending jobs with no row counts', () => {
    expect(
      formatJobProgress({
        id: 'job-4',
        status: 'pending',
      }),
    ).toContain('queued');
  });
});

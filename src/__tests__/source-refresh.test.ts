import { describe, expect, it } from 'vitest';

import {
  resolveRefreshWindow,
  summarizeEnsureSourceData,
} from '../source-refresh.js';

const NOW = new Date('2026-07-02T12:00:00.000Z');

describe('resolveRefreshWindow', () => {
  it('defaults to a 7-day window ending now', () => {
    const window = resolveRefreshWindow(undefined, undefined, NOW);
    expect(window.requestedEnd).toBe('2026-07-02T12:00:00.000Z');
    expect(window.requestedStart).toBe('2026-06-25T12:00:00.000Z');
  });

  it('accepts explicit ISO datetimes and bare dates', () => {
    const window = resolveRefreshWindow(
      '2026-06-29',
      '2026-07-02T23:59:59Z',
      NOW,
    );
    expect(window.requestedStart).toBe('2026-06-29T00:00:00.000Z');
    expect(window.requestedEnd).toBe('2026-07-02T23:59:59.000Z');
  });

  it('defaults --from relative to an explicit --to, not to now', () => {
    const window = resolveRefreshWindow(undefined, '2026-06-10', NOW);
    expect(window.requestedStart).toBe('2026-06-03T00:00:00.000Z');
    expect(window.requestedEnd).toBe('2026-06-10T00:00:00.000Z');
  });

  it('rejects invalid dates, naming the flag', () => {
    expect(() => resolveRefreshWindow('not-a-date', undefined, NOW)).toThrow(
      /--from/,
    );
    expect(() => resolveRefreshWindow(undefined, 'nope', NOW)).toThrow(/--to/);
  });

  it('rejects an empty or reversed window', () => {
    expect(() =>
      resolveRefreshWindow('2026-07-03', '2026-07-01', NOW),
    ).toThrow(/earlier than/);
    expect(() =>
      resolveRefreshWindow('2026-07-01', '2026-07-01', NOW),
    ).toThrow(/earlier than/);
  });
});

describe('summarizeEnsureSourceData', () => {
  it('treats ready as success with no jobs', () => {
    const summary = summarizeEnsureSourceData({
      status: 'ready',
      importJobIds: [],
      message: 'Source data is already available',
    });
    expect(summary.tone).toBe('success');
    expect(summary.jobIds).toEqual([]);
  });

  it('treats importing as success and carries job ids', () => {
    const summary = summarizeEnsureSourceData({
      status: 'importing',
      importJobIds: ['job_1', 'job_2'],
    });
    expect(summary.tone).toBe('success');
    expect(summary.jobIds).toEqual(['job_1', 'job_2']);
    expect(summary.headline).toMatch(/2 import job/);
  });

  it('treats unavailable as an error', () => {
    const summary = summarizeEnsureSourceData({ status: 'unavailable' });
    expect(summary.tone).toBe('error');
  });

  it('falls back to info for unknown statuses', () => {
    const summary = summarizeEnsureSourceData({ status: 'partial' });
    expect(summary.tone).toBe('info');
    expect(summary.headline).toMatch(/partial/);
  });
});

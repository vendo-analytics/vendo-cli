import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { colorStatus, formatNumber, shortId, timeAgo } from '../output.js';

// Mock ora before importing output
vi.mock('ora', () => {
  const mockSpinner = {
    start: vi.fn().mockReturnThis(),
    stop: vi.fn().mockReturnThis(),
    succeed: vi.fn().mockReturnThis(),
    fail: vi.fn().mockReturnThis(),
  };
  return {
    default: vi.fn(() => mockSpinner),
  };
});

describe('output', () => {
  describe('colorStatus', () => {
    it('returns the status string for all known statuses', () => {
      // In non-TTY environments (test runner), the color wrappers are no-ops,
      // so the returned string should contain the original status text.
      const statuses = [
        'active',
        'completed',
        'running',
        'pending',
        'paused',
        'inactive',
        'cancelled',
        'errored',
        'failed',
        'warning',
      ];
      for (const status of statuses) {
        const result = colorStatus(status);
        expect(result).toContain(status);
      }
    });

    it('returns unknown statuses unchanged', () => {
      expect(colorStatus('some_new_status')).toBe('some_new_status');
    });
  });

  describe('timeAgo', () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it('returns dim dash for null/undefined', () => {
      // In non-TTY, c.dim is identity, so just check we get the dash character
      const result = timeAgo(null);
      expect(result).toContain('\u2014'); // em dash
    });

    it('returns "just now" for future dates', () => {
      vi.setSystemTime(new Date('2026-01-15T12:00:00Z'));
      expect(timeAgo('2026-01-15T13:00:00Z')).toBe('just now');
    });

    it('returns "just now" for less than 60 seconds ago', () => {
      vi.setSystemTime(new Date('2026-01-15T12:00:30Z'));
      expect(timeAgo('2026-01-15T12:00:00Z')).toBe('just now');
    });

    it('returns minutes ago', () => {
      vi.setSystemTime(new Date('2026-01-15T12:05:00Z'));
      expect(timeAgo('2026-01-15T12:00:00Z')).toBe('5m ago');
    });

    it('returns hours ago', () => {
      vi.setSystemTime(new Date('2026-01-15T15:00:00Z'));
      expect(timeAgo('2026-01-15T12:00:00Z')).toBe('3h ago');
    });

    it('returns days ago', () => {
      vi.setSystemTime(new Date('2026-01-20T12:00:00Z'));
      expect(timeAgo('2026-01-15T12:00:00Z')).toBe('5d ago');
    });

    it('returns formatted date for 30+ days', () => {
      vi.setSystemTime(new Date('2026-03-15T12:00:00Z'));
      const result = timeAgo('2026-01-01T12:00:00Z');
      // Should be a locale date string, not "Xd ago"
      expect(result).not.toContain('d ago');
      expect(result).not.toBe('just now');
    });
  });

  describe('shortId', () => {
    it('truncates long IDs with ellipsis', () => {
      expect(shortId('550e8400-e29b-41d4-a716-446655440000')).toBe(
        '550e8400...',
      );
    });

    it('returns short strings unchanged', () => {
      expect(shortId('abc123')).toBe('abc123');
    });

    it('returns exactly 12-char strings unchanged', () => {
      expect(shortId('123456789012')).toBe('123456789012');
    });

    it('truncates 13+ char strings', () => {
      expect(shortId('1234567890123')).toBe('12345678...');
    });
  });

  describe('formatNumber', () => {
    it('formats numbers with locale separators', () => {
      const result = formatNumber(1234567);
      // The exact format depends on locale, but should contain digits
      expect(result).toContain('1');
      expect(result).toContain('234');
    });

    it('returns dim dash for null/undefined', () => {
      const resultNull = formatNumber(null);
      const resultUndefined = formatNumber(undefined);
      expect(resultNull).toContain('\u2014');
      expect(resultUndefined).toContain('\u2014');
    });

    it('formats zero', () => {
      expect(formatNumber(0)).toBe('0');
    });
  });
});

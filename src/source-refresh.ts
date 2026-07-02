/**
 * Helpers for `vendo integrations refresh-source` (VE-1565) — the CLI surface
 * for the web API's `refresh-source` route (which forwards to the pipelines
 * `ensure-source-data` endpoint): check whether the source has data for a
 * requested window and trigger top-up import(s) for whatever is missing.
 * Pure functions here so the commander action stays thin and the
 * window/response logic is unit-testable.
 */

/** Millis in one day, for the default 7-day lookback window. */
const DAY_MS = 24 * 60 * 60 * 1000;
const DEFAULT_LOOKBACK_DAYS = 7;

export interface RefreshWindow {
  /** ISO datetime (UTC) the availability check starts from. */
  requestedStart: string;
  /** ISO datetime (UTC) the availability check runs to. */
  requestedEnd: string;
}

/** Datetime string already carrying a timezone: trailing Z or ±hh[:]mm. */
const HAS_TIMEZONE_RE = /(?:[zZ]|[+-]\d{2}:?\d{2})$/;

/**
 * Parse a `--from`/`--to` value as UTC. Accepts a full ISO datetime or a bare
 * `YYYY-MM-DD` (UTC midnight). Offset-less datetimes (`2026-07-01T10:00:00`)
 * are ALSO treated as UTC — ECMA-262 would parse them as local time, which
 * would make the same command request different windows on a laptop vs a UTC
 * CI runner (VE-1603). Throws on anything unparseable, naming the flag so the
 * error reads well.
 */
function parseWindowBound(flag: string, value: string): Date {
  const normalized =
    value.includes('T') && !HAS_TIMEZONE_RE.test(value) ? `${value}Z` : value;
  const parsed = new Date(normalized);
  if (Number.isNaN(parsed.getTime())) {
    throw new Error(
      `Invalid ${flag} value: "${value}". Use an ISO datetime (2026-07-01T00:00:00Z) or date (2026-07-01).`,
    );
  }
  return parsed;
}

/**
 * Resolve the availability window from optional flags: `to` defaults to now,
 * `from` defaults to 7 days before `to`. Throws when the window is empty or
 * reversed. `now` is a parameter so tests are deterministic.
 */
export function resolveRefreshWindow(
  from: string | undefined,
  to: string | undefined,
  now: Date,
): RefreshWindow {
  const end = to ? parseWindowBound('--to', to) : now;
  const start = from
    ? parseWindowBound('--from', from)
    : new Date(end.getTime() - DEFAULT_LOOKBACK_DAYS * DAY_MS);

  if (start.getTime() >= end.getTime()) {
    throw new Error(
      `--from (${start.toISOString()}) must be earlier than --to (${end.toISOString()}).`,
    );
  }

  return {
    requestedStart: start.toISOString(),
    requestedEnd: end.toISOString(),
  };
}

/** refresh-source response, camelCased by the client envelope. */
export interface EnsureSourceDataResult {
  status?: string;
  importJobIds?: string[];
  message?: string;
}

export interface EnsureSourceDataSummary {
  tone: 'success' | 'error' | 'info';
  headline: string;
  jobIds: string[];
}

/**
 * Turn the API result into a printable summary: `ready` and `importing` are
 * both successes (nothing missing vs. top-ups dispatched); `unavailable`
 * means the check couldn't trigger imports and should read as an error.
 */
export function summarizeEnsureSourceData(
  result: EnsureSourceDataResult,
): EnsureSourceDataSummary {
  const jobIds = result.importJobIds ?? [];
  switch (result.status) {
    case 'ready':
      return {
        tone: 'success',
        headline:
          result.message ?? 'Source data already available — nothing to import.',
        jobIds,
      };
    case 'importing':
      return {
        tone: 'success',
        headline:
          result.message ??
          `Triggered ${jobIds.length} import job(s) for missing data.`,
        jobIds,
      };
    case 'unavailable':
      return {
        tone: 'error',
        headline:
          result.message ??
          'Could not trigger imports — check the source configuration.',
        jobIds,
      };
    default:
      return {
        tone: 'info',
        headline: result.message ?? `Availability status: ${result.status ?? 'unknown'}`,
        jobIds,
      };
  }
}

const TRUTHY_DEBUG_VALUES = new Set(['1', 'true', 'yes', 'on']);

let debugEnabled = false;

export function setDebugEnabled(enabled: boolean): void {
  debugEnabled = enabled;
}

export function isDebugEnabled(): boolean {
  if (debugEnabled) {
    return true;
  }

  const value = process.env.VENDO_DEBUG?.trim().toLowerCase();
  return value ? TRUTHY_DEBUG_VALUES.has(value) : false;
}

export function printDebug(
  message: string,
  fields?: Record<string, unknown>,
): void {
  if (!isDebugEnabled()) {
    return;
  }

  if (fields && Object.keys(fields).length > 0) {
    console.error(`[debug] ${message} ${formatDebugFields(fields)}`);
    return;
  }

  console.error(`[debug] ${message}`);
}

function formatDebugFields(fields: Record<string, unknown>): string {
  return Object.entries(fields)
    .filter(([, value]) => value !== undefined)
    .map(([key, value]) => `${key}=${formatDebugValue(value)}`)
    .join(' ');
}

function formatDebugValue(value: unknown): string {
  if (typeof value === 'string') {
    return /\s/.test(value) ? JSON.stringify(value) : value;
  }

  if (
    typeof value === 'number' ||
    typeof value === 'boolean' ||
    value === null
  ) {
    return String(value);
  }

  return JSON.stringify(value);
}

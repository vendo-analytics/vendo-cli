import chalk from 'chalk';
import Table from 'cli-table3';
import { createInterface } from 'node:readline';
import ora, { type Ora } from 'ora';

const isTTY = process.stdout.isTTY ?? false;

// ─── Colors (no-op when piped) ────────────────────────

export const c = {
  bold: (s: string) => (isTTY ? chalk.bold(s) : s),
  dim: (s: string) => (isTTY ? chalk.dim(s) : s),
  green: (s: string) => (isTTY ? chalk.green(s) : s),
  red: (s: string) => (isTTY ? chalk.red(s) : s),
  yellow: (s: string) => (isTTY ? chalk.yellow(s) : s),
  blue: (s: string) => (isTTY ? chalk.blue(s) : s),
  cyan: (s: string) => (isTTY ? chalk.cyan(s) : s),
  gray: (s: string) => (isTTY ? chalk.gray(s) : s),
};

// ─── Status colors ────────────────────────────────────

export function colorStatus(status: string): string {
  switch (status) {
    case 'active':
    case 'completed':
      return c.green(status);
    case 'running':
    case 'pending':
      return c.blue(status);
    case 'paused':
    case 'inactive':
    case 'cancelled':
      return c.gray(status);
    case 'errored':
    case 'failed':
      return c.red(status);
    case 'warning':
      return c.yellow(status);
    default:
      return status;
  }
}

// ─── Table rendering ──────────────────────────────────

export function createTable(headers: string[]): Table.Table {
  return new Table({
    head: headers.map((h) => (isTTY ? chalk.bold.cyan(h) : h)),
    style: {
      head: [],
      border: isTTY ? ['gray'] : [],
    },
    chars: isTTY
      ? undefined
      : {
          top: '',
          'top-mid': '',
          'top-left': '',
          'top-right': '',
          bottom: '',
          'bottom-mid': '',
          'bottom-left': '',
          'bottom-right': '',
          left: '',
          'left-mid': '',
          mid: '',
          'mid-mid': '',
          right: '',
          'right-mid': '',
          middle: '  ',
        },
  });
}

// ─── Spinners ─────────────────────────────────────────
// Frames sourced from https://github.com/vyfor/rattles (MIT)

export const SPINNERS = {
  columns: {
    interval: 60,
    frames: [
      '⡀⠀⠀',
      '⡄⠀⠀',
      '⡆⠀⠀',
      '⡇⠀⠀',
      '⣇⠀⠀',
      '⣧⠀⠀',
      '⣷⠀⠀',
      '⣿⠀⠀',
      '⣿⡀⠀',
      '⣿⡄⠀',
      '⣿⡆⠀',
      '⣿⡇⠀',
      '⣿⣇⠀',
      '⣿⣧⠀',
      '⣿⣷⠀',
      '⣿⣿⠀',
      '⣿⣿⡀',
      '⣿⣿⡄',
      '⣿⣿⡆',
      '⣿⣿⡇',
      '⣿⣿⣇',
      '⣿⣿⣧',
      '⣿⣿⣷',
      '⣿⣿⣿',
      '⣿⣿⣿',
      '⠀⠀⠀',
    ],
  },
  waverows: {
    interval: 90,
    frames: [
      '⠖⠉⠉⠑',
      '⡠⠖⠉⠉',
      '⣠⡠⠖⠉',
      '⣄⣠⡠⠖',
      '⠢⣄⣠⡠',
      '⠙⠢⣄⣠',
      '⠉⠙⠢⣄',
      '⠊⠉⠙⠢',
      '⠜⠊⠉⠙',
      '⡤⠜⠊⠉',
      '⣀⡤⠜⠊',
      '⢤⣀⡤⠜',
      '⠣⢤⣀⡤',
      '⠑⠣⢤⣀',
      '⠉⠑⠣⢤',
      '⠋⠉⠑⠣',
    ],
  },
  breathe: {
    interval: 100,
    frames: [
      '⠀',
      '⠂',
      '⠌',
      '⡑',
      '⢕',
      '⢝',
      '⣫',
      '⣟',
      '⣿',
      '⣟',
      '⣫',
      '⢝',
      '⢕',
      '⡑',
      '⠌',
      '⠂',
      '⠀',
    ],
  },
  cascade: {
    interval: 60,
    frames: [
      '⠀⠀⠀⠀',
      '⠀⠀⠀⠀',
      '⠁⠀⠀⠀',
      '⠋⠀⠀⠀',
      '⠞⠁⠀⠀',
      '⡴⠋⠀⠀',
      '⣠⠞⠁⠀',
      '⢀⡴⠋⠀',
      '⠀⣠⠞⠁',
      '⠀⢀⡴⠋',
      '⠀⠀⣠⠞',
      '⠀⠀⢀⡴',
      '⠀⠀⠀⣠',
      '⠀⠀⠀⢀',
    ],
  },
  diagswipe: {
    interval: 60,
    frames: [
      '⠁⠀',
      '⠋⠀',
      '⠟⠁',
      '⡿⠋',
      '⣿⠟',
      '⣿⡿',
      '⣿⣿',
      '⣿⣿',
      '⣾⣿',
      '⣴⣿',
      '⣠⣾',
      '⢀⣴',
      '⠀⣠',
      '⠀⢀',
      '⠀⠀',
      '⠀⠀',
    ],
  },
} as const;

// ora wants mutable string[]; SPINNERS is readonly via `as const`.
const DEFAULT_SPINNER = {
  ...SPINNERS.columns,
  frames: [...SPINNERS.columns.frames],
};

export function spinner(text: string): Ora {
  return ora({ text, spinner: DEFAULT_SPINNER, isSilent: !isTTY });
}

// ─── Time formatting ──────────────────────────────────

export function timeAgo(dateStr: string | null | undefined): string {
  if (!dateStr) return c.dim('—');

  const now = Date.now();
  const then = new Date(dateStr).getTime();
  const diffMs = now - then;

  if (diffMs < 0) return 'just now';

  const seconds = Math.floor(diffMs / 1000);
  if (seconds < 60) return 'just now';

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;

  return new Date(dateStr).toLocaleDateString();
}

// ─── UUID shortening ──────────────────────────────────

export function shortId(id: string): string {
  return id.length > 12 ? id.slice(0, 8) + '...' : id;
}

// ─── Number formatting ───────────────────────────────

export function formatNumber(n: number | null | undefined): string {
  if (n == null) return c.dim('—');
  return n.toLocaleString();
}

// ─── JSON output ──────────────────────────────────────

export function printJson(data: unknown): void {
  console.log(JSON.stringify(data, null, 2));
}

// ─── Error output ─────────────────────────────────────

export function printError(message: string): void {
  console.error(c.red('Error:'), message);
}

export function formatError(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err);

  if (!err || typeof err !== 'object') {
    return message;
  }

  const requestId =
    getStringField(err, 'serverRequestId') ?? getStringField(err, 'requestId');

  if (!requestId) {
    return message;
  }

  return `${message}\n${c.dim(`Request ID: ${requestId}`)}`;
}

// ─── Success output ───────────────────────────────────

export function printSuccess(message: string): void {
  console.log(c.green('Done:'), message);
}

// ─── Count summary ────────────────────────────────────

export function printCount(count: number, label: string): void {
  console.log(c.dim(`${count} ${label}${count === 1 ? '' : 's'}`));
}

// ─── Confirmation prompt ─────────────────────────────

export async function confirm(message: string): Promise<boolean> {
  if (!isTTY) return true;

  const rl = createInterface({ input: process.stdin, output: process.stdout });

  return new Promise((resolve) => {
    rl.question(`${message} ${c.dim('(y/N)')} `, (answer) => {
      rl.close();
      resolve(answer.trim().toLowerCase() === 'y');
    });
  });
}

export async function searchSelectOption(
  message: string,
  options: Array<{
    value: string;
    label: string;
    searchText?: string;
  }>,
): Promise<string | undefined> {
  if (!isTTY || options.length === 0) return undefined;

  console.log(message);
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const ask = (prompt: string) =>
    new Promise<string>((resolve) => rl.question(prompt, resolve));

  while (true) {
    const query = (
      await ask(`Search ${c.dim('(ENTER for all, q to cancel)')} `)
    )
      .trim()
      .toLowerCase();

    if (query === 'q') {
      rl.close();
      return undefined;
    }

    const filtered = options.filter((option) =>
      query
        ? `${option.label} ${option.searchText ?? ''}`
            .toLowerCase()
            .includes(query)
        : true,
    );

    if (filtered.length === 0) {
      console.log(c.dim('No matching profiles. Try a different search.'));
      continue;
    }

    const displayed = filtered.slice(0, 10);
    for (const [index, option] of displayed.entries()) {
      console.log(`  ${index + 1}. ${option.label}`);
    }
    if (filtered.length > displayed.length) {
      console.log(
        c.dim(`  … ${filtered.length - displayed.length} more matches`),
      );
    }

    const selection = (
      await ask(`Choose an option ${c.dim('(ENTER to search again)')} `)
    ).trim();

    if (!selection) {
      continue;
    }

    const selectedIndex = Number(selection);
    if (
      Number.isInteger(selectedIndex) &&
      selectedIndex >= 1 &&
      selectedIndex <= displayed.length
    ) {
      rl.close();
      return displayed[selectedIndex - 1]?.value;
    }

    const matchedOption = filtered.find(
      (option) => option.value === selection || option.label === selection,
    );

    if (matchedOption) {
      rl.close();
      return matchedOption.value;
    }

    console.log(
      c.dim('Invalid selection. Search again or choose a listed number.'),
    );
  }
}

// ─── Action runner ───────────────────────────────────

export async function runAction<T>(
  label: string,
  fn: () => Promise<T>,
): Promise<T> {
  const s = spinner(label);
  s.start();

  try {
    const result = await fn();
    s.stop();
    return result;
  } catch (err) {
    s.stop();
    exitWithError(err);
  }
}

export function exitWithError(err: unknown): never {
  printError(formatError(err));
  process.exit(1);
}

// ─── Agent/automation helpers ────────────────────────

export function addExamples(
  cmd: { addHelpText: (position: 'after', text: string) => void },
  examples: string[],
): void {
  const text = '\nExamples:\n' + examples.map((e) => `  $ ${e}`).join('\n');
  cmd.addHelpText('after', text);
}

/**
 * `--output field` mode: print one column for each row in `items`.
 */
export function printField(
  items: Array<Record<string, unknown>>,
  field: string,
): void {
  for (const item of items) {
    const value = item[field];
    if (value != null) {
      console.log(String(value));
    }
  }
}

/**
 * Labeled key/value, used after success messages
 * (e.g. after `vendo apps create` to echo the new resource's properties).
 */
export function printLabel(label: string, value: unknown): void {
  if (value == null) return;
  console.log(`  ${label}: ${String(value)}`);
}

export function printSingleField(
  item: Record<string, unknown>,
  field: string,
): void {
  const value = item[field];
  if (value != null) {
    console.log(String(value));
  }
}

export function resolveOutputMode(opts: {
  json?: boolean;
  output?: string;
}): 'json' | 'field' | 'table' {
  if (opts.json) return 'json';
  if (opts.output) return 'field';
  return 'table';
}

export function printDryRun(
  action: string,
  resourceType: string,
  resourceId: string,
  details?: Record<string, string>,
): void {
  console.log(
    c.yellow('[dry-run]'),
    `Would ${action} ${resourceType} ${shortId(resourceId)}`,
  );
  if (details) {
    for (const [key, value] of Object.entries(details)) {
      console.log(`  ${key}: ${value}`);
    }
  }
}

export function showArgError(message: string, examples: string[]): never {
  const text = [
    c.red('Error:') + ' ' + message,
    '',
    'Usage:',
    ...examples.map((e) => `  $ ${e}`),
  ].join('\n');
  console.error(text);
  process.exit(1);
}

function getStringField(
  value: object,
  key: 'requestId' | 'serverRequestId',
): string | undefined {
  const field = (value as Record<string, unknown>)[key];
  return typeof field === 'string' && field.length > 0 ? field : undefined;
}

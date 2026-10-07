import { Command } from 'commander';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  CohortLtvResponse,
  MethodologyListResponse,
  SignalListResponse,
} from '../api/web-app.js';

// The measurement list routes answer through the web app's `apiResponse.success`,
// so their body is `{ data: … }` and the raw client wraps that once more (VE-3673).
const methodologiesBody: { data: MethodologyListResponse } = {
  data: {
    methodologies: [
      {
        id: 'm-system-1',
        account_id: null,
        name: 'Last click',
        description: null,
        click_path_model: 'last_click',
        ensemble_weights: { click_path: 1 },
        signal_params: null,
        is_system: true,
        version: 1,
        created_at: null,
        updated_at: null,
      },
      {
        id: 'm-account-2',
        account_id: 'acct-1',
        name: 'Blended',
        description: 'Click path and survey',
        click_path_model: 'linear',
        ensemble_weights: { click_path: 0.7, survey: 0.3 },
        signal_params: null,
        is_system: false,
        version: 3,
        created_at: null,
        updated_at: null,
      },
    ],
  },
};

const ltvBody: { data: CohortLtvResponse } = {
  data: {
    granularity: 'monthly',
    segment_key: 'all',
    cohorts: [
      {
        cohort_period: '2026-08-01',
        cohort_granularity: 'monthly',
        segment_key: 'all',
        cohort_size: 120,
        realised: {
          ltv_30d: 10,
          ltv_90d: 20,
          ltv_12m: null,
          ltv_full: null,
          cac: 5,
          cac_ltv_ratio: 0.5,
          payback_period_days: null,
          computed_at: null,
        },
        predicted: null,
      },
    ],
    total_returned: 1,
  },
};

const signalsBody: { data: SignalListResponse } = {
  data: {
    signals: [
      { id: 'click_path', state: 'live', availability: { available: true } },
      {
        id: 'mmm',
        state: 'stub',
        availability: { available: false, reason: 'Not enough spend history' },
      },
    ],
  },
};

vi.mock('../api/web-app.js', () => ({
  webApp: {
    measurement: {
      methodologies: vi.fn(async () => ({ data: methodologiesBody })),
      ltv: vi.fn(async () => ({ data: ltvBody })),
      signals: vi.fn(async () => ({ data: signalsBody })),
    },
  },
}));

const { registerMeasurementCommand } =
  await import('../commands/measurement.js');

let stdout: string[];
let stderr: string[];

beforeEach(() => {
  stdout = [];
  stderr = [];
  vi.spyOn(console, 'log').mockImplementation((...args: unknown[]) => {
    stdout.push(args.join(' '));
  });
  vi.spyOn(console, 'error').mockImplementation((...args: unknown[]) => {
    stderr.push(args.join(' '));
  });
  vi.spyOn(process, 'exit').mockImplementation(
    (code?: string | number | null) => {
      throw new Error(`process.exit(${code})`);
    },
  );
});

afterEach(() => {
  vi.restoreAllMocks();
});

async function run(...args: string[]): Promise<string> {
  const program = new Command().exitOverride();
  registerMeasurementCommand(program);
  await program.parseAsync(['node', 'vendo', 'measurement', ...args]);
  return stdout.join('\n');
}

describe('measurement commands read the enveloped list responses', () => {
  it('methodologies list prints one table row per methodology', async () => {
    const out = await run('methodologies', 'list');
    expect(out).toContain('Last click');
    expect(out).toContain('Blended');
    expect(out).toContain('2 methodologys');
  });

  it('methodologies list --output id prints the ids', async () => {
    const out = await run('methodologies', 'list', '--output', 'id');
    expect(out.split('\n')).toEqual(['m-system-1', 'm-account-2']);
  });

  it('methodologies get finds the row by id, in text and --json', async () => {
    expect(await run('methodologies', 'get', 'm-account-2')).toContain(
      'Blended',
    );
    stdout = [];
    const json = JSON.parse(
      await run('methodologies', 'get', 'm-account-2', '--json'),
    );
    expect(json).toEqual(methodologiesBody.data.methodologies[1]);
  });

  it('methodologies get reports a missing id instead of crashing', async () => {
    await expect(run('methodologies', 'get', 'nope')).rejects.toThrow(
      'process.exit(1)',
    );
    expect(stderr.join('\n')).toContain('Methodology nope not found');
  });

  it('ltv list prints cohorts and the --output field', async () => {
    expect(await run('ltv', 'list')).toContain('2026-08-01');
    stdout = [];
    expect(await run('ltv', 'list', '--output', 'cohort_period')).toBe(
      '2026-08-01',
    );
  });

  it('signals list prints one row per signal', async () => {
    const out = await run('signals', 'list');
    expect(out).toContain('click_path');
    expect(out).toContain('Not enough spend history');
  });

  it('--json still prints the server body unchanged', async () => {
    expect(await run('methodologies', 'list', '--json')).toBe(
      JSON.stringify(methodologiesBody, null, 2),
    );
    stdout = [];
    expect(await run('ltv', 'list', '--json')).toBe(
      JSON.stringify(ltvBody, null, 2),
    );
    stdout = [];
    expect(await run('signals', 'list', '--json')).toBe(
      JSON.stringify(signalsBody, null, 2),
    );
  });
});

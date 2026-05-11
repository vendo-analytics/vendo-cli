import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  addExamples,
  c,
  createTable,
  formatNumber,
  printCount,
  printField,
  printJson,
  resolveOutputMode,
  runAction,
  shortId,
  showArgError,
  timeAgo,
} from '../output.js';

// ── Types mirroring `apps/web/lib/vendo/measurement/queries.ts` ──────────────
// The web API returns snake_case verbatim through `getRaw`/`postRaw`, so the
// types here use snake_case to match the wire format.

interface MethodologyRow {
  id: string;
  account_id: string | null;
  name: string;
  description: string | null;
  click_path_model: string;
  ensemble_weights: Record<string, number>;
  signal_params: Record<string, unknown> | null;
  is_system: boolean;
  version: number;
  created_at: string | null;
  updated_at: string | null;
}

interface MethodologyListResponse {
  methodologies: MethodologyRow[];
}

interface RulePreviewRow {
  context: {
    campaign_objective: string | null;
    channel_grouping: string | null;
    custom_label: string | null;
  };
  sample_count: number;
  resolved_methodology: {
    id: string;
    name: string;
    click_path_model: string;
  };
  matched_rule_id: string | null;
  via: 'rule' | 'default_fallback';
}

interface RulePreviewResponse {
  previews: RulePreviewRow[];
  total_distinct_contexts: number;
}

interface CohortLtvRow {
  cohort_period: string;
  cohort_granularity: string;
  segment_key: string;
  cohort_size: number;
  realised: {
    ltv_30d: number | null;
    ltv_90d: number | null;
    ltv_12m: number | null;
    ltv_full: number | null;
    cac: number | null;
    cac_ltv_ratio: number | null;
    payback_period_days: number | null;
    computed_at: string | null;
  };
  predicted: unknown | null;
}

interface CohortLtvResponse {
  granularity: 'daily' | 'weekly' | 'monthly';
  segment_key: string;
  cohorts: CohortLtvRow[];
  total_returned: number;
}

interface SignalRow {
  id: 'click_path' | 'mmm' | 'geo_lift' | 'survey';
  state: 'live' | 'stub';
  availability: {
    available: boolean;
    reason?: string | null;
    [key: string]: unknown;
  } | null;
}

interface SignalListResponse {
  signals: SignalRow[];
}

// /api/measurement/ltv/cohort/[period] — getCohortDetail() shape.
interface CohortDetailResponse {
  cohort_period: string;
  cohort_granularity: 'daily' | 'weekly' | 'monthly';
  segment_key: string;
  cohort_size: number;
  retention_matrix: Array<{
    period_offset_days: number;
    retained_customers: number;
    retained_revenue: number;
    retention_rate: number | null;
  }>;
  cumulative_curve: Array<{
    period_offset_days: number;
    cumulative_gross_revenue: number;
    cumulative_revenue_after_cogs: number;
  }>;
  prediction: {
    method: string;
    ltv_30d_predicted: number | null;
    ltv_90d_predicted: number | null;
    ltv_12m_predicted: number | null;
    metadata: Record<string, unknown> | null;
    computed_at: string | null;
  } | null;
}

// /api/measurement/ltv/customer/[customer_id] — getCustomerLtv() shape.
interface CustomerLtvResponse {
  cohort: {
    customer_id: string;
    acquisition_date: string;
    cohort_period_daily: string;
    cohort_period_weekly: string;
    cohort_period_monthly: string;
    acquisition_channel: string | null;
    acquisition_campaign: string | null;
    country: string | null;
    is_reactivated: boolean;
    [key: string]: unknown;
  } | null;
  revenue: Array<{
    revenue_period_daily: string;
    period_offset_days: number;
    gross_revenue: number;
    refund_amount: number;
    cogs_amount: number;
    revenue_after_cogs: number;
    order_count: number;
    is_subscription: boolean;
  }>;
  realised: {
    ltv_30d: number | null;
    ltv_90d: number | null;
    ltv_12m: number | null;
    ltv_full: number | null;
    ltv_30d_after_cogs: number | null;
    ltv_90d_after_cogs: number | null;
    ltv_12m_after_cogs: number | null;
    ltv_full_after_cogs: number | null;
  };
}

// /api/measurement/signals/click-path — getClickPathStatus() shape.
interface ClickPathStatusResponse {
  status: {
    enabled: boolean;
    lastComputedAt: string | null;
    sampleEstimates: Array<{
      tier_label?: string | null;
      attribution_decision?: string | null;
      [key: string]: unknown;
    }>;
    readiness: {
      available: boolean;
      reason?: string | null;
      readiness?: Array<{
        key: string;
        label: string;
        ok: boolean;
        detail?: string | null;
      }>;
      [key: string]: unknown;
    };
  };
}

// ── Helpers ───────────────────────────────────────────────────────────────

function methodologyAsField(row: MethodologyRow): Record<string, unknown> {
  // `printField` reads camelCase keys from `--output`. Provide both casings so the user can
  // pick either form (e.g. `--output id` or `--output click_path_model`).
  return {
    ...row,
    id: row.id,
    name: row.name,
    description: row.description,
    isSystem: row.is_system,
    is_system: row.is_system,
    clickPathModel: row.click_path_model,
    click_path_model: row.click_path_model,
  };
}

function ltvCohortAsField(row: CohortLtvRow): Record<string, unknown> {
  return {
    ...row,
    cohortPeriod: row.cohort_period,
    cohort_period: row.cohort_period,
    segmentKey: row.segment_key,
    segment_key: row.segment_key,
    cohortSize: row.cohort_size,
    cohort_size: row.cohort_size,
  };
}

function fmtMoney(n: number | null | undefined): string {
  if (n == null) return c.dim('—');
  return n.toLocaleString(undefined, {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: 2,
  });
}

function fmtRatio(n: number | null | undefined): string {
  if (n == null) return c.dim('—');
  return n.toFixed(2);
}

// ── Command registration ──────────────────────────────────────────────────

export function registerMeasurementCommand(program: Command): void {
  const cmd = program
    .command('measurement')
    .description(
      'Inspect Marketing Measurement methodologies, LTV, and signals',
    );

  registerMethodologiesGroup(cmd);
  registerRulesGroup(cmd);
  registerLtvGroup(cmd);
  registerSignalsGroup(cmd);
}

// ── methodologies ─────────────────────────────────────────────────────────

function registerMethodologiesGroup(parent: Command): void {
  const group = parent
    .command('methodologies')
    .description('List measurement methodologies');

  const listCmd = group
    .command('list')
    .description('List methodologies (system + account)')
    .option('--no-system', 'Exclude system-seeded methodologies')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print one field per row (e.g. id, name)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);
      const params: Record<string, string | number | boolean | undefined> = {};
      // commander's --no-system flips opts.system to false. Translate to the
      // include_system query param the API expects.
      if (opts.system === false) params.include_system = 'false';

      const res = await runAction('Fetching methodologies...', () =>
        getClient().getRaw<MethodologyListResponse>(
          '/api/measurement/methodologies',
          params,
        ),
      );

      if (outputMode === 'json') {
        printJson(res.data);
        return;
      }

      const rows = res.data.methodologies;

      if (outputMode === 'field') {
        printField(rows.map(methodologyAsField), opts.output);
        return;
      }

      const table = createTable([
        'ID',
        'Name',
        'Click Path',
        'Scope',
        'Version',
        'Updated',
      ]);
      for (const row of rows) {
        table.push([
          c.dim(shortId(row.id)),
          row.name,
          row.click_path_model,
          row.is_system ? c.cyan('system') : 'account',
          String(row.version),
          timeAgo(row.updated_at),
        ]);
      }
      console.log(table.toString());
      printCount(rows.length, 'methodology');
    });

  addExamples(listCmd, [
    'vendo measurement methodologies list',
    'vendo measurement methodologies list --no-system',
    'vendo measurement methodologies list --json',
    'vendo measurement methodologies list --output id',
  ]);

  const getCmd = group
    .command('get <methodologyId>')
    .description('Show one methodology by ID')
    .option('--json', 'Output raw JSON')
    .action(async (methodologyId: string, opts: { json?: boolean }) => {
      // No dedicated GET-by-id endpoint in Phase 1c; filter the list response.
      const res = await runAction('Fetching methodology...', () =>
        getClient().getRaw<MethodologyListResponse>(
          '/api/measurement/methodologies',
          {},
        ),
      );

      const row = res.data.methodologies.find((m) => m.id === methodologyId);
      if (!row) {
        console.error(
          c.red('Error:'),
          `Methodology ${methodologyId} not found`,
        );
        process.exit(1);
      }

      if (opts.json) {
        printJson(row);
        return;
      }

      console.log();
      console.log(c.bold(row.name), c.dim(`(${row.click_path_model})`));
      console.log();
      console.log(`  ID:             ${row.id}`);
      console.log(
        `  Scope:          ${row.is_system ? c.cyan('system') : 'account'}`,
      );
      console.log(`  Version:        ${row.version}`);
      console.log(`  Updated:        ${timeAgo(row.updated_at)}`);
      if (row.description) {
        console.log(`  Description:   ${row.description}`);
      }
      const weights = row.ensemble_weights ?? {};
      const weightKeys = Object.keys(weights);
      if (weightKeys.length > 0) {
        console.log();
        console.log(c.bold('  Ensemble weights:'));
        for (const key of weightKeys) {
          console.log(
            `    ${key.padEnd(12)}  ${formatNumber(weights[key] ?? 0)}`,
          );
        }
      }
    });

  addExamples(getCmd, [
    'vendo measurement methodologies get <methodologyId>',
    'vendo measurement methodologies get <methodologyId> --json',
  ]);
}

// ── rules ────────────────────────────────────────────────────────────────

function registerRulesGroup(parent: Command): void {
  const group = parent
    .command('rules')
    .description('Methodology segmentation rules');

  const previewCmd = group
    .command('preview')
    .description('Preview which segmentation rule fires for recent rows')
    .requiredOption('--from <date>', 'Inclusive ISO date (YYYY-MM-DD)')
    .requiredOption('--to <date>', 'Inclusive ISO date (YYYY-MM-DD)')
    .option('--limit <n>', 'Max distinct contexts to return', '50')
    .option('--json', 'Output raw JSON')
    .action(
      async (opts: {
        from: string;
        to: string;
        limit: string;
        json?: boolean;
      }) => {
        const limit = Number(opts.limit);
        if (!Number.isFinite(limit) || limit <= 0 || limit > 200) {
          showArgError('--limit must be 1..200', [
            'vendo measurement rules preview --from 2025-01-01 --to 2025-01-31 --limit 50',
          ]);
        }

        const res = await runAction('Previewing segmentation rules...', () =>
          getClient().postRaw<RulePreviewResponse>(
            '/api/measurement/methodologies/rules/preview',
            { from_date: opts.from, to_date: opts.to, limit },
          ),
        );

        if (opts.json) {
          printJson(res.data);
          return;
        }

        const rows = res.data.previews;
        const table = createTable([
          'Objective',
          'Channel',
          'Custom Label',
          'Methodology',
          'Via',
          'Sample',
        ]);
        for (const row of rows) {
          table.push([
            row.context.campaign_objective ?? c.dim('—'),
            row.context.channel_grouping ?? c.dim('—'),
            row.context.custom_label ?? c.dim('—'),
            row.resolved_methodology.name,
            row.via === 'rule' ? 'rule' : c.dim('default'),
            String(row.sample_count),
          ]);
        }
        console.log(table.toString());
        printCount(res.data.total_distinct_contexts, 'distinct context');
      },
    );

  addExamples(previewCmd, [
    'vendo measurement rules preview --from 2025-01-01 --to 2025-01-31',
    'vendo measurement rules preview --from 2025-01-01 --to 2025-01-31 --limit 100 --json',
  ]);
}

// ── ltv ──────────────────────────────────────────────────────────────────

function registerLtvGroup(parent: Command): void {
  const group = parent
    .command('ltv')
    .description('Cohort lifetime-value views');

  const listCmd = group
    .command('list')
    .description('List cohort LTV rows')
    .option('--granularity <value>', 'daily | weekly | monthly', 'monthly')
    .option(
      '--segment <key>',
      'Segment key (e.g. "all", "channel:meta")',
      'all',
    )
    .option('--from <period>', 'Inclusive ISO cohort period (YYYY-MM-DD)')
    .option('--to <period>', 'Inclusive ISO cohort period (YYYY-MM-DD)')
    .option('--limit <n>', 'Max rows', '50')
    .option('--no-predicted', 'Skip the naive-decay prediction join')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print one field per row')
    .action(async (opts) => {
      const params = ltvParams(opts);
      const outputMode = resolveOutputMode(opts);

      const res = await runAction('Fetching LTV cohorts...', () =>
        getClient().getRaw<CohortLtvResponse>('/api/measurement/ltv', params),
      );

      if (outputMode === 'json') {
        printJson(res.data);
        return;
      }

      const rows = res.data.cohorts;

      if (outputMode === 'field') {
        printField(rows.map(ltvCohortAsField), opts.output);
        return;
      }

      const table = createTable([
        'Cohort',
        'Segment',
        'Size',
        'LTV 30d',
        'LTV 90d',
        'LTV 12m',
        'CAC',
        'CAC:LTV',
      ]);
      for (const row of rows) {
        table.push([
          row.cohort_period,
          row.segment_key,
          formatNumber(row.cohort_size),
          fmtMoney(row.realised.ltv_30d),
          fmtMoney(row.realised.ltv_90d),
          fmtMoney(row.realised.ltv_12m),
          fmtMoney(row.realised.cac),
          fmtRatio(row.realised.cac_ltv_ratio),
        ]);
      }
      console.log(table.toString());
      printCount(res.data.total_returned, 'cohort');
    });

  addExamples(listCmd, [
    'vendo measurement ltv list',
    'vendo measurement ltv list --granularity weekly --segment channel:meta',
    'vendo measurement ltv list --from 2025-01-01 --to 2025-06-30 --no-predicted',
  ]);

  const cohortCmd = group
    .command('cohort <period>')
    .description(
      'Show one cohort by period — retention matrix + cumulative LTV curve + prediction',
    )
    .option('--granularity <value>', 'daily | weekly | monthly', 'monthly')
    .option('--segment <key>', 'Segment key', 'all')
    .option('--json', 'Output raw JSON')
    .action(
      async (
        period: string,
        opts: {
          granularity: 'daily' | 'weekly' | 'monthly';
          segment: string;
          json?: boolean;
        },
      ) => {
        if (!/^\d{4}-\d{2}-\d{2}$/.test(period)) {
          showArgError('cohort period must be YYYY-MM-DD', [
            'vendo measurement ltv cohort 2025-01-01',
          ]);
        }

        const res = await runAction('Fetching cohort...', () =>
          getClient().getRaw<CohortDetailResponse>(
            `/api/measurement/ltv/cohort/${encodeURIComponent(period)}`,
            {
              granularity: opts.granularity,
              segment_key: opts.segment,
            },
          ),
        );

        if (opts.json) {
          printJson(res.data);
          return;
        }

        const row = res.data;
        console.log();
        console.log(
          c.bold(`Cohort ${row.cohort_period}`),
          c.dim(`(${row.cohort_granularity}, segment=${row.segment_key})`),
        );
        console.log();
        console.log(`  Size:           ${formatNumber(row.cohort_size)}`);
        console.log(
          `  Retention pts:  ${formatNumber(row.retention_matrix.length)}`,
        );
        console.log(
          `  Curve points:   ${formatNumber(row.cumulative_curve.length)}`,
        );

        if (row.cumulative_curve.length > 0) {
          const last = row.cumulative_curve[row.cumulative_curve.length - 1];
          console.log(
            `  Cum revenue:    ${fmtMoney(last?.cumulative_gross_revenue ?? 0)} ` +
              c.dim(`(t+${last?.period_offset_days ?? 0}d)`),
          );
          console.log(
            `  After COGS:     ${fmtMoney(last?.cumulative_revenue_after_cogs ?? 0)}`,
          );
        }

        if (row.prediction) {
          console.log();
          console.log(c.bold('  Prediction:'));
          console.log(`    Method:       ${row.prediction.method}`);
          console.log(
            `    LTV 30d:      ${fmtMoney(row.prediction.ltv_30d_predicted)}`,
          );
          console.log(
            `    LTV 90d:      ${fmtMoney(row.prediction.ltv_90d_predicted)}`,
          );
          console.log(
            `    LTV 12m:      ${fmtMoney(row.prediction.ltv_12m_predicted)}`,
          );
          console.log(
            `    Computed:     ${timeAgo(row.prediction.computed_at)}`,
          );
        } else {
          console.log(c.dim('  Prediction:     (none)'));
        }
      },
    );

  addExamples(cohortCmd, [
    'vendo measurement ltv cohort 2025-01-01',
    'vendo measurement ltv cohort 2025-01-01 --granularity weekly --segment channel:meta',
    'vendo measurement ltv cohort 2025-01-01 --json',
  ]);

  const customerCmd = group
    .command('customer <customerId>')
    .description("Show one customer's cohort + realised LTV + revenue timeline")
    .option('--json', 'Output raw JSON')
    .action(async (customerId: string, opts: { json?: boolean }) => {
      if (!customerId) {
        showArgError('customerId is required', [
          'vendo measurement ltv customer cust_abc123',
        ]);
      }

      const res = await runAction('Fetching customer LTV...', () =>
        getClient().getRaw<CustomerLtvResponse>(
          `/api/measurement/ltv/customer/${encodeURIComponent(customerId)}`,
        ),
      );

      if (opts.json) {
        printJson(res.data);
        return;
      }

      const { cohort, revenue, realised } = res.data;
      console.log();
      console.log(c.bold(`Customer ${customerId}`));

      if (cohort) {
        console.log();
        console.log(`  Acquired:        ${cohort.acquisition_date}`);
        console.log(
          `  Channel:         ${cohort.acquisition_channel ?? c.dim('—')}`,
        );
        console.log(
          `  Campaign:        ${cohort.acquisition_campaign ?? c.dim('—')}`,
        );
        console.log(`  Country:         ${cohort.country ?? c.dim('—')}`);
        console.log(
          `  Reactivated:     ${cohort.is_reactivated ? c.yellow('yes') : 'no'}`,
        );
        console.log(`  Monthly cohort:  ${cohort.cohort_period_monthly}`);
      } else {
        console.log(
          c.dim('  (no cohort row — customer not in customer_cohorts)'),
        );
      }

      console.log();
      console.log(c.bold('  Realised LTV:'));
      console.log(`    30d:           ${fmtMoney(realised.ltv_30d)}`);
      console.log(`    90d:           ${fmtMoney(realised.ltv_90d)}`);
      console.log(`    12m:           ${fmtMoney(realised.ltv_12m)}`);
      console.log(`    full:          ${fmtMoney(realised.ltv_full)}`);
      console.log(c.dim(`    after-COGS variants in --json`));
      console.log();
      console.log(`  Revenue points:  ${formatNumber(revenue.length)}`);
    });

  addExamples(customerCmd, [
    'vendo measurement ltv customer cust_abc123',
    'vendo measurement ltv customer cust_abc123 --json',
  ]);
}

function ltvParams(opts: {
  granularity?: string;
  segment?: string;
  from?: string;
  to?: string;
  limit?: string;
  predicted?: boolean;
}): Record<string, string | number | boolean | undefined> {
  return {
    granularity: opts.granularity,
    segment_key: opts.segment,
    from_period: opts.from,
    to_period: opts.to,
    limit: opts.limit,
    // commander's --no-predicted flips opts.predicted to false.
    include_predicted: opts.predicted === false ? 'false' : undefined,
  };
}

// ── signals ──────────────────────────────────────────────────────────────

function registerSignalsGroup(parent: Command): void {
  const group = parent
    .command('signals')
    .description('Measurement signal availability');

  const listCmd = group
    .command('list')
    .description('Per-signal availability summary')
    .option('--json', 'Output raw JSON')
    .action(async (opts: { json?: boolean }) => {
      const res = await runAction('Fetching signal availability...', () =>
        getClient().getRaw<SignalListResponse>('/api/measurement/signals'),
      );

      if (opts.json) {
        printJson(res.data);
        return;
      }

      const table = createTable([
        'Signal',
        'State',
        'Available',
        'Reason / Notes',
      ]);
      for (const row of res.data.signals) {
        const available = row.availability?.available;
        table.push([
          row.id,
          row.state === 'live' ? c.green('live') : c.dim('stub'),
          available == null
            ? c.dim('—')
            : available
              ? c.green('yes')
              : c.red('no'),
          (row.availability?.reason as string | undefined) ?? c.dim('—'),
        ]);
      }
      console.log(table.toString());
      printCount(res.data.signals.length, 'signal');
    });

  addExamples(listCmd, [
    'vendo measurement signals list',
    'vendo measurement signals list --json',
  ]);

  const clickPathCmd = group
    .command('click-path')
    .description(
      'Click-path signal status — readiness, last-computed, recent SignalEstimates',
    )
    .option(
      '--sample-limit <n>',
      'Number of recent SignalEstimates to fetch (1..500)',
    )
    .option('--json', 'Output raw JSON')
    .action(async (opts: { sampleLimit?: string; json?: boolean }) => {
      const params: Record<string, string | number | boolean | undefined> = {};
      if (opts.sampleLimit !== undefined) {
        const n = Number(opts.sampleLimit);
        if (!Number.isFinite(n) || n < 1 || n > 500) {
          showArgError('--sample-limit must be 1..500', [
            'vendo measurement signals click-path --sample-limit 100',
          ]);
        }
        params.sampleLimit = n;
      }

      const res = await runAction('Fetching click-path status...', () =>
        getClient().getRaw<ClickPathStatusResponse>(
          '/api/measurement/signals/click-path',
          params,
        ),
      );

      if (opts.json) {
        printJson(res.data);
        return;
      }

      const { status } = res.data;
      console.log();
      console.log(c.bold('Click-path signal'));
      console.log();
      console.log(
        `  Enabled:        ${status.enabled ? c.green('yes') : c.red('no')}`,
      );
      console.log(`  Last computed:  ${timeAgo(status.lastComputedAt)}`);
      console.log(
        `  Sample rows:    ${formatNumber(status.sampleEstimates.length)}`,
      );

      const readiness = status.readiness?.readiness;
      if (Array.isArray(readiness) && readiness.length > 0) {
        console.log();
        console.log(c.bold('  Readiness:'));
        for (const item of readiness) {
          const dot = item.ok ? c.green('✓') : c.red('✗');
          console.log(`    ${dot} ${item.label}`);
          if (!item.ok && item.detail) {
            console.log(`      ${c.dim(item.detail)}`);
          }
        }
      } else if (status.readiness?.reason) {
        console.log(
          `  Note:           ${c.dim(String(status.readiness.reason))}`,
        );
      }
    });

  addExamples(clickPathCmd, [
    'vendo measurement signals click-path',
    'vendo measurement signals click-path --sample-limit 50 --json',
  ]);
}

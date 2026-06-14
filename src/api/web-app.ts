/**
 * Typed facade over the Vendo web-app HTTP routes (`/api/metrics`,
 * `/api/measurement/*`).
 *
 * These run on the same host as the pipelines API but use a different
 * convention: they are NOT under `/api/v1`, are not account-scoped by the
 * client, and return their canonical snake_case JSON verbatim (no envelope or
 * camelCase normalization). The client reaches them via its `*Raw` methods;
 * this module is the single place that knows their paths and response shapes,
 * so command files no longer hard-code `/api/...` strings or hand-mirror the
 * web contract types inline.
 *
 * The pipelines (`/api/v1`) surface is the normalized `VendoClient` itself
 * (`getClient().get/post/...`), which already returns typed, camelCased
 * envelopes — no separate wrapper needed.
 *
 * The interfaces below mirror `apps/web/lib/vendo/measurement/queries.ts` and
 * the metrics API; they use snake_case to match the wire format.
 */
import { type ApiResponse, getClient } from '../client.js';

type RawParams = Record<string, string | number | boolean | undefined>;

// ── Metrics (`/api/metrics`) ────────────────────────────────────────────────

export interface MetricRow {
  id: string;
  account_id: string;
  name: string;
  description: string | null;
  category: string | null;
  metric_type: string;
  formula: string | null;
  format: string;
  higher_is_better: boolean;
  unit: string | null;
  status: string;
  created_at: string;
  updated_at: string;
}

export interface MetricDetail extends MetricRow {
  building_blocks: unknown[];
}

export interface MetricsListResponse {
  metrics: MetricRow[];
  total: number;
  limit: number;
  offset: number;
}

export interface MetricResponse {
  metric: MetricDetail;
}

// ── Measurement (`/api/measurement/*`) ──────────────────────────────────────

export interface MethodologyRow {
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

export interface MethodologyListResponse {
  methodologies: MethodologyRow[];
}

export interface RulePreviewRow {
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

export interface RulePreviewResponse {
  previews: RulePreviewRow[];
  total_distinct_contexts: number;
}

export interface CohortLtvRow {
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

export interface CohortLtvResponse {
  granularity: 'daily' | 'weekly' | 'monthly';
  segment_key: string;
  cohorts: CohortLtvRow[];
  total_returned: number;
}

export interface SignalRow {
  id: 'click_path' | 'mmm' | 'geo_lift' | 'survey';
  state: 'live' | 'stub';
  availability: {
    available: boolean;
    reason?: string | null;
    [key: string]: unknown;
  } | null;
}

export interface SignalListResponse {
  signals: SignalRow[];
}

// /api/measurement/ltv/cohort/[period] — getCohortDetail() shape.
export interface CohortDetailResponse {
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
export interface CustomerLtvResponse {
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
export interface ClickPathStatusResponse {
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

// ── Facade ──────────────────────────────────────────────────────────────────

export const webApp = {
  metrics: {
    list: (params: RawParams): Promise<ApiResponse<MetricsListResponse>> =>
      getClient().getRaw<MetricsListResponse>('/api/metrics', params),
    get: (id: string): Promise<ApiResponse<MetricResponse>> =>
      getClient().getRaw<MetricResponse>(`/api/metrics/${id}`),
    create: (body: unknown): Promise<ApiResponse<MetricResponse>> =>
      getClient().postRaw<MetricResponse>('/api/metrics', body),
    update: (
      id: string,
      body: unknown,
    ): Promise<ApiResponse<MetricResponse>> =>
      getClient().patchRaw<MetricResponse>(`/api/metrics/${id}`, body),
    remove: (
      id: string,
    ): Promise<ApiResponse<{ deleted: boolean; id: string }>> =>
      getClient().deleteRaw<{ deleted: boolean; id: string }>(
        `/api/metrics/${id}`,
      ),
  },

  measurement: {
    methodologies: (
      params: RawParams = {},
    ): Promise<ApiResponse<MethodologyListResponse>> =>
      getClient().getRaw<MethodologyListResponse>(
        '/api/measurement/methodologies',
        params,
      ),
    previewRules: (body: unknown): Promise<ApiResponse<RulePreviewResponse>> =>
      getClient().postRaw<RulePreviewResponse>(
        '/api/measurement/methodologies/rules/preview',
        body,
      ),
    ltv: (params: RawParams): Promise<ApiResponse<CohortLtvResponse>> =>
      getClient().getRaw<CohortLtvResponse>('/api/measurement/ltv', params),
    cohort: (
      period: string,
      params: RawParams = {},
    ): Promise<ApiResponse<CohortDetailResponse>> =>
      getClient().getRaw<CohortDetailResponse>(
        `/api/measurement/ltv/cohort/${encodeURIComponent(period)}`,
        params,
      ),
    customer: (
      customerId: string,
    ): Promise<ApiResponse<CustomerLtvResponse>> =>
      getClient().getRaw<CustomerLtvResponse>(
        `/api/measurement/ltv/customer/${encodeURIComponent(customerId)}`,
      ),
    signals: (): Promise<ApiResponse<SignalListResponse>> =>
      getClient().getRaw<SignalListResponse>('/api/measurement/signals'),
    clickPath: (
      params: RawParams = {},
    ): Promise<ApiResponse<ClickPathStatusResponse>> =>
      getClient().getRaw<ClickPathStatusResponse>(
        '/api/measurement/signals/click-path',
        params,
      ),
  },
};

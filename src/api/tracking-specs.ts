import { type ApiResponse, getClient } from '../client.js';

export const TRACKING_SPEC_VALUE_TYPES = [
  'string',
  'number',
  'boolean',
  'datetime',
  'array',
  'object',
] as const;

export type TrackingSpecValueType = (typeof TRACKING_SPEC_VALUE_TYPES)[number];

export type TrackingSpecAllowedValue = string | number | boolean;

export interface TrackingSpecEventPropertyInput {
  stableKey?: string;
  name: string;
  type: TrackingSpecValueType;
  description?: string;
  required?: boolean;
  allowedValues?: TrackingSpecAllowedValue[];
}

export interface TrackingSpecEventInput {
  stableKey?: string;
  name: string;
  description?: string;
  category?: 'acquisition' | 'engagement' | 'conversion' | 'retention';
  triggerDescription?: string | null;
  source?: 'web' | 'ios' | 'android' | 'server' | 'third_party' | null;
  destinations?: string[];
  specStatus?: 'planned' | 'implemented' | 'validated' | 'deprecated';
  analyticsRequirement?: string | null;
  properties?: TrackingSpecEventPropertyInput[];
}

export interface TrackingSpecUserPropertyInput {
  stableKey?: string;
  name: string;
  definition?: string;
  dataType?: TrackingSpecValueType;
  required?: boolean;
  allowedValues?: TrackingSpecAllowedValue[];
  identify?: boolean;
  setOnce?: boolean;
  bqLocation?: string | null;
  specStatus?: 'planned' | 'implemented' | 'validated' | 'deprecated';
}

export interface TrackingSpecGroupPropertyInput {
  stableKey?: string;
  groupType: string;
  name: string;
  definition?: string;
  dataType?: TrackingSpecValueType;
  required?: boolean;
  allowedValues?: TrackingSpecAllowedValue[];
  sources?: string[];
  isGroupKey?: boolean;
  specStatus?: 'planned' | 'implemented' | 'validated' | 'deprecated';
}

export interface TrackingSpecPayloadInput {
  events?: TrackingSpecEventInput[];
  userProperties?: TrackingSpecUserPropertyInput[];
  groupProperties?: TrackingSpecGroupPropertyInput[];
}

export interface TrackingSpecEventProperty extends TrackingSpecEventPropertyInput {
  stableKey: string;
  description: string;
  required: boolean;
  allowedValues: TrackingSpecAllowedValue[];
}

export interface TrackingSpecEvent extends TrackingSpecEventInput {
  stableKey: string;
  description: string;
  category: 'acquisition' | 'engagement' | 'conversion' | 'retention';
  triggerDescription: string | null;
  source: 'web' | 'ios' | 'android' | 'server' | 'third_party' | null;
  destinations: string[];
  specStatus: 'planned' | 'implemented' | 'validated' | 'deprecated';
  analyticsRequirement: string | null;
  properties: TrackingSpecEventProperty[];
}

export interface TrackingSpecUserProperty extends TrackingSpecUserPropertyInput {
  stableKey: string;
  definition: string;
  dataType: TrackingSpecValueType;
  required: boolean;
  allowedValues: TrackingSpecAllowedValue[];
  identify: boolean;
  setOnce: boolean;
  bqLocation: string | null;
  specStatus: 'planned' | 'implemented' | 'validated' | 'deprecated';
}

export interface TrackingSpecGroupProperty extends TrackingSpecGroupPropertyInput {
  stableKey: string;
  definition: string;
  dataType: TrackingSpecValueType;
  required: boolean;
  allowedValues: TrackingSpecAllowedValue[];
  sources: string[];
  isGroupKey: boolean;
  specStatus: 'planned' | 'implemented' | 'validated' | 'deprecated';
}

export interface TrackingSpecPayload {
  events: TrackingSpecEvent[];
  userProperties: TrackingSpecUserProperty[];
  groupProperties: TrackingSpecGroupProperty[];
}

interface TrackingSpecBindingBase {
  displayName?: string;
  validationConfig?: Record<string, unknown>;
}

/**
 * A spec is source-independent, while a binding chooses where it is
 * validated. The discriminated union prevents an ID from crossing the
 * tracking-write-key / source-semantic-mapping domain boundary.
 */
export interface TrackingWriteKeySpecBindingInput extends TrackingSpecBindingBase {
  kind: 'tracking_write_key';
  trackingWriteKeyId: string;
}

export interface SemanticMappingSpecBindingInput extends TrackingSpecBindingBase {
  kind: 'semantic_mapping';
  sourceSemanticMappingId: string;
  environment: 'production' | 'staging' | 'development';
}

export type TrackingSpecBindingInput =
  | TrackingWriteKeySpecBindingInput
  | SemanticMappingSpecBindingInput;

interface TrackingSpecBindingResponseBase {
  id: string;
  environment: 'production' | 'staging' | 'development';
  displayName: string | null;
  validationConfig: Record<string, unknown>;
  isActive: boolean;
}

export interface TrackingWriteKeySpecBinding extends TrackingSpecBindingResponseBase {
  kind: 'tracking_write_key';
  trackingWriteKeyId: string;
}

export interface SemanticMappingSpecBinding extends TrackingSpecBindingResponseBase {
  kind: 'semantic_mapping';
  sourceSemanticMappingId: string;
}

export type TrackingSpecBinding =
  | TrackingWriteKeySpecBinding
  | SemanticMappingSpecBinding;

export interface CreateTrackingSpecInput {
  name: string;
  description?: string;
  strategyId?: string | null;
  payload?: TrackingSpecPayloadInput;
  bindings?: TrackingSpecBindingInput[];
}

export interface UpdateTrackingSpecInput {
  name?: string;
  description?: string;
  payload?: TrackingSpecPayloadInput;
  bindings?: TrackingSpecBindingInput[];
  expectedRevision: number;
}

export interface PublishTrackingSpecInput {
  expectedRevision: number;
  changelog?: string;
}

export type TrackingSpecStatus = 'draft' | 'active' | 'archived';

export interface TrackingSpecVersionSummary {
  id: string;
  versionNumber: number;
  checksum: string;
  changelog: string | null;
  publishedAt: string;
  publishedBy: string | null;
}

export interface TrackingSpecVersion extends TrackingSpecVersionSummary {
  trackingSpecId: string;
  bindingsSnapshot: TrackingSpecBinding[];
  createdAt: string;
  payload?: TrackingSpecPayload;
}

export interface TrackingSpecCounts {
  events: number;
  eventProperties: number;
  userProperties: number;
  groupProperties: number;
  bindings: number;
}

export interface TrackingSpecReconciliationCounts {
  specOnly: number;
  specAndLive: number;
  liveOnly: number;
  nonconforming: number;
}

export interface TrackingSpecReconciliationItem {
  subjectId: string;
  subjectType: 'event' | 'event_property' | 'user_property' | 'group_property';
  displayName: string;
  state: 'spec_only' | 'spec_and_live' | 'live_only' | 'nonconforming';
  required: boolean;
  lastSeenAt: string | null;
  conformance: {
    status: 'unknown' | 'passing' | 'warning' | 'failing';
    issues: string[];
  };
}

export interface TrackingSpecReconciliation {
  summary: TrackingSpecReconciliationCounts;
  items: TrackingSpecReconciliationItem[];
}

export interface TrackingSpecSummary {
  id: string;
  accountId: string;
  name: string;
  description: string;
  status: TrackingSpecStatus;
  strategyId: string | null;
  revision: number;
  createdAt: string;
  updatedAt: string;
  currentVersion: TrackingSpecVersionSummary | null;
  counts: TrackingSpecCounts;
}

export interface TrackingSpec extends TrackingSpecSummary {
  draftPayload: TrackingSpecPayload;
  bindings: TrackingSpecBinding[];
  reconciliation: TrackingSpecReconciliation;
}

export interface TrackingSpecDiffChange {
  change: 'added' | 'removed' | 'changed';
  kind: 'event' | 'event_property' | 'user_property' | 'group_property';
  stableKey: string;
  before?: unknown;
  after?: unknown;
}

export interface TrackingSpecDiff {
  fromVersion: TrackingSpecVersionSummary;
  toVersion: TrackingSpecVersionSummary;
  changes: TrackingSpecDiffChange[];
}

export interface TrackingSpecValidationResult {
  id: string;
  subjectId: string;
  status: 'passed' | 'warning' | 'failed' | 'error';
  result: unknown;
  startedAt: string | null;
  finishedAt: string | null;
  trackingSpecVersionId: string;
  trackingSpecBindingId: string | null;
  checkId: string | null;
}

type QueryParams = Record<string, string | number | boolean | undefined>;

const basePath = '/specs';

function itemPath(specId: string): string {
  return `${basePath}/${encodeURIComponent(specId)}`;
}

/**
 * Typed facade for the canonical account-scoped tracking-spec REST surface.
 * Account selection remains centralized in VendoClient, so every operation
 * uses the active profile's account and sends the same X-Account-Id context.
 */
export const trackingSpecsApi = {
  list: (
    params: QueryParams = {},
  ): Promise<ApiResponse<TrackingSpecSummary[]>> =>
    getClient().getCanonical<TrackingSpecSummary[]>(basePath, params),

  get: (specId: string): Promise<ApiResponse<TrackingSpec>> =>
    getClient().getCanonical<TrackingSpec>(itemPath(specId)),

  create: (body: CreateTrackingSpecInput): Promise<ApiResponse<TrackingSpec>> =>
    getClient().postCanonical<TrackingSpec>(basePath, body),

  update: (
    specId: string,
    body: UpdateTrackingSpecInput,
  ): Promise<ApiResponse<TrackingSpec>> =>
    getClient().patchCanonical<TrackingSpec>(itemPath(specId), body),

  archive: (
    specId: string,
    expectedRevision: number,
  ): Promise<ApiResponse<TrackingSpec>> =>
    getClient().deleteCanonical<TrackingSpec>(itemPath(specId), {
      expected_revision: expectedRevision,
    }),

  publish: (
    specId: string,
    body: PublishTrackingSpecInput,
  ): Promise<ApiResponse<TrackingSpec>> =>
    getClient().postCanonical<TrackingSpec>(
      `${itemPath(specId)}/publish`,
      body,
    ),

  versions: (
    specId: string,
    includePayload = false,
  ): Promise<ApiResponse<TrackingSpecVersion[]>> => {
    const path = `${itemPath(specId)}/versions`;
    return includePayload
      ? getClient().getCanonical<TrackingSpecVersion[]>(path, {
          include_payload: true,
        })
      : getClient().getCanonical<TrackingSpecVersion[]>(path);
  },

  diff: (
    specId: string,
    fromVersionId: string,
    toVersionId: string,
  ): Promise<ApiResponse<TrackingSpecDiff>> =>
    getClient().getCanonical<TrackingSpecDiff>(`${itemPath(specId)}/diff`, {
      from_version_id: fromVersionId,
      to_version_id: toVersionId,
    }),

  validationResults: (
    specId: string,
    params: QueryParams = {},
  ): Promise<ApiResponse<TrackingSpecValidationResult[]>> =>
    getClient().getCanonical<TrackingSpecValidationResult[]>(
      `${itemPath(specId)}/validation-results`,
      params,
    ),
};

import { type ApiResponse, getClient } from '../client.js';

/** The `type` values the server accepts (`isDictionarySubjectType`). */
export const DICTIONARY_SUBJECT_TYPES = [
  'event',
  'prop',
  'group',
  'column',
  'metric',
  'model',
  'audience',
] as const;

export type DictionarySubjectType = (typeof DICTIONARY_SUBJECT_TYPES)[number];

/**
 * One catalog definition, exactly as vendo-web-v2 serves it
 * (`toDictionaryItem` in apps/web/app/api/v1/_lib/route-handlers/dictionary/
 * serialize.ts). `getCanonical` keeps the server's keys verbatim, so these
 * names are the wire contract; `dictionary-contract.test.ts` pins them.
 *
 * For event, prop, group, metric and audience, `subjectId` is the published
 * semantic registry ID (32 hex characters). Columns and models keep their
 * `source:…/table:…/col:…` style paths.
 */
export interface DictionaryItem {
  subjectId: string;
  subjectType: string;
  displayName: string | null;
  description: string | null;
  dataType: string | null;
  semanticType: string | null;
  tags: string[] | null;
  origin: string;
  lastSeenAt: string | null;
  status: string;
}

/** `GET /dictionary/lookup`: `definition` is present only when `found`. */
export interface DictionaryLookup {
  /** The `subject_id` that was asked for, echoed back. */
  subjectId: string;
  found: boolean;
  definition?: DictionaryItem;
}

/** Query params of `GET /dictionary` (collection.ts). */
export type DictionaryListParams = {
  /** Server default: `event`. */
  type?: string;
  /** Case-insensitive text match. */
  q?: string;
  /** Server default 20, capped at 100. */
  limit?: string | number;
  offset?: string | number;
};

const basePath = '/dictionary';

/**
 * Typed facade for the canonical account-scoped dictionary REST surface.
 * Account selection remains centralized in VendoClient.
 */
export const dictionaryApi = {
  list: (
    params: DictionaryListParams = {},
  ): Promise<ApiResponse<DictionaryItem[]>> =>
    getClient().getCanonical<DictionaryItem[]>(basePath, params),

  /** `subjectId` is a subject ID from `list`, or an alias such as `event:<name>`. */
  get: (subjectId: string): Promise<ApiResponse<DictionaryLookup>> =>
    getClient().getCanonical<DictionaryLookup>(`${basePath}/lookup`, {
      subject_id: subjectId,
    }),
};

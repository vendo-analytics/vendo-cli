import { type ApiResponse, getClient } from '../client.js';

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

export interface DictionaryItem {
  subjectId: string;
  subjectType: string;
  displayName: string | null;
  description: string | null;
  dataType: string | null;
  semanticType: string | null;
  tags: string[] | null;
  origin: string | null;
  lastSeenAt: string | null;
  status: string | null;
}

export interface DictionaryLookup {
  subjectId: string;
  found: boolean;
  definition?: DictionaryItem;
}

type QueryParams = Record<string, string | number | boolean | undefined>;

const basePath = '/dictionary';

/**
 * Typed facade for the canonical account-scoped dictionary REST surface.
 * Account selection remains centralized in VendoClient.
 */
export const dictionaryApi = {
  list: (params: QueryParams = {}): Promise<ApiResponse<DictionaryItem[]>> =>
    getClient().getCanonical<DictionaryItem[]>(basePath, params),

  get: (subjectId: string): Promise<ApiResponse<DictionaryLookup>> =>
    getClient().getCanonical<DictionaryLookup>(`${basePath}/lookup`, {
      subject_id: subjectId,
    }),
};

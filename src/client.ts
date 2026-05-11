import { randomUUID } from 'node:crypto';

import { getBaseUrl, requireAccountId, requireApiKey } from './config.js';
import { printDebug } from './debug.js';

export interface ApiResponse<T = unknown> {
  data: T;
  meta?: {
    pagination?: {
      total: number;
      limit: number;
      offset: number;
      hasMore: boolean;
    };
  };
}

export interface ApiError {
  error: {
    code: string;
    message: string;
    details?: unknown;
  };
}

export interface RequestOptions {
  method?: string;
  body?: unknown;
  params?: Record<string, string | number | boolean | undefined>;
  timeout?: number;
  /**
   * Skip the `/api/v1` prefix, the path-mapping/account-injection adapter,
   * and the snake_case → camelCase normalization. Use for endpoints that
   * already serve a stable JSON shape (e.g. `/api/measurement/*`).
   */
  rawPath?: boolean;
}

// Paths that require /accounts/{accountId} prefix
const ACCOUNT_SCOPED_PREFIXES = [
  '/apps',
  '/connections',
  '/sources',
  '/jobs',
  '/models',
  '/pipeline',
  '/triggers',
  '/costs',
  '/events',
  '/pulse',
  '/bigquery',
];

const ACCOUNT_HEADER_PATHS = ['/me'];

const COLLECTION_RESPONSE_KEYS = new Set([
  'apps',
  'connections',
  'sources',
  'jobs',
  'connectors',
  'models',
  'streams',
  'catalog',
  'destinations',
  'integrations',
  'triggers',
  'events',
  'costs',
  'usage',
]);

// Action sub-paths where CLI sends POST but API expects PATCH
const PATCH_ACTION_PATTERNS = [/\/(pause|resume|activate)$/];

class VendoClient {
  private apiKey: string;
  private baseUrl: string;

  constructor() {
    this.apiKey = requireApiKey();
    this.baseUrl = getBaseUrl();
  }

  async request<T = unknown>(
    path: string,
    options: RequestOptions = {},
  ): Promise<ApiResponse<T>> {
    // `method` is reassigned below for action-style POST → PATCH mapping;
    // body/params/timeout never are, so split into let + const.
    let { method = 'GET' } = options;
    const { body, params, timeout = 30_000, rawPath = false } = options;

    let accountId: string | undefined;
    if (!rawPath) {
      // Map CLI paths to API paths
      path = this.mapPath(path);
      accountId = this.requiresAccountContext(path)
        ? requireAccountId()
        : undefined;
      path = this.injectAccountPrefix(path, accountId);

      // Fix HTTP method: CLI sends POST for actions, API expects PATCH
      if (
        method === 'POST' &&
        PATCH_ACTION_PATTERNS.some((p) => p.test(path))
      ) {
        method = 'PATCH';
      }
    }

    // Build URL with query params. rawPath calls pass an absolute API path
    // (e.g. `/api/measurement/...`); pipelines calls get the `/api/v1` prefix.
    const url = new URL(rawPath ? path : `/api/v1${path}`, this.baseUrl);
    if (params) {
      for (const [key, value] of Object.entries(params)) {
        if (value !== undefined) {
          url.searchParams.set(key, String(value));
        }
      }
    }

    const requestId = `cli-${randomUUID()}`;
    const headers: Record<string, string> = {
      Authorization: `Bearer ${this.apiKey}`,
      'X-API-Secret': this.apiKey,
      'Content-Type': 'application/json',
      'X-Request-Id': requestId,
      'X-Actor': 'vendo-cli',
    };
    if (accountId) {
      headers['X-Account-Id'] = accountId;
    }

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeout);
    const startedAt = Date.now();

    printDebug('request', {
      method,
      url: url.toString(),
      requestId,
      accountId,
    });

    try {
      const res = await fetch(url.toString(), {
        method,
        headers,
        body: body ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });

      clearTimeout(timer);
      const durationMs = Date.now() - startedAt;
      const serverRequestId = getResponseRequestId(res.headers);

      printDebug('response', {
        method,
        url: url.toString(),
        requestId,
        serverRequestId,
        status: res.status,
        durationMs,
      });

      // Check rate limit headers
      const remaining = res.headers.get('X-RateLimit-Remaining');
      if (remaining && parseInt(remaining, 10) < 5) {
        const resetAt = res.headers.get('X-RateLimit-Reset');
        const resetDate = resetAt
          ? new Date(parseInt(resetAt, 10) * 1000).toLocaleTimeString()
          : 'soon';
        console.warn(
          `Warning: Rate limit low (${remaining} remaining, resets ${resetDate})`,
        );
      }

      if (!res.ok) {
        // 204 No Content (e.g. DELETE) — return empty
        if (res.status === 204) {
          return { data: {} as T };
        }

        let errorBody: ApiError;
        try {
          errorBody = (await res.json()) as ApiError;
        } catch {
          printDebug('response_error', {
            method,
            url: url.toString(),
            requestId,
            serverRequestId,
            status: res.status,
            statusText: res.statusText,
            durationMs,
          });
          throw new ClientError(
            friendlyHttpError(res.status, res.statusText),
            res.status,
            undefined,
            {
              requestId,
              serverRequestId,
            },
          );
        }

        printDebug('response_error', {
          method,
          url: url.toString(),
          requestId,
          serverRequestId,
          status: res.status,
          statusText: res.statusText,
          errorMessage: errorBody.error?.message,
          durationMs,
          code: errorBody.error?.code,
          details: errorBody.error?.details,
        });

        throw new ClientError(
          errorBody.error?.message || friendlyHttpError(res.status),
          res.status,
          errorBody.error?.code,
          {
            requestId,
            serverRequestId,
            details: errorBody.error?.details,
          },
        );
      }

      // 204 No Content
      if (res.status === 204) {
        return { data: {} as T };
      }

      const rawBody = await res.json();
      if (rawPath) {
        // Endpoint already returns its canonical JSON shape — no envelope or key conversion.
        return { data: rawBody as T };
      }
      return this.normalizeResponse<T>(rawBody);
    } catch (err) {
      clearTimeout(timer);
      if (err instanceof ClientError) throw err;
      if (err instanceof Error && err.name === 'AbortError') {
        printDebug('request_failed', {
          method,
          url: url.toString(),
          requestId,
          durationMs: Date.now() - startedAt,
          error: 'Request timed out',
        });
        throw new ClientError('Request timed out', 408);
      }

      printDebug('request_failed', {
        method,
        url: url.toString(),
        requestId,
        durationMs: Date.now() - startedAt,
        error: err instanceof Error ? err.message : 'Unknown error',
      });

      throw new ClientError(
        err instanceof Error ? err.message : 'Unknown error',
        0,
      );
    }
  }

  /**
   * Map CLI paths to the actual API paths.
   *
   * - /integrations → /connections (with account prefix)
   * - /catalog/{type} → /connectors/{type}/catalog (global)
   * - /catalog → /connectors (global)
   * - Account-scoped paths get /accounts/{id} prefix
   */
  private mapPath(path: string): string {
    // /catalog/{type} → /connectors/{type}/catalog
    const catalogMatch = path.match(/^\/catalog\/(.+)$/);
    if (catalogMatch) {
      return `/connectors/${catalogMatch[1]}/catalog`;
    }

    // /catalog → /connectors
    if (path === '/catalog') {
      return '/connectors';
    }

    // /integrations → /connections
    if (path.startsWith('/integrations')) {
      path = path.replace('/integrations', '/connections');
    }

    return path;
  }

  private requiresAccountContext(path: string): boolean {
    return (
      ACCOUNT_SCOPED_PREFIXES.some((prefix) => path.startsWith(prefix)) ||
      ACCOUNT_HEADER_PATHS.some(
        (prefix) => path === prefix || path.startsWith(`${prefix}/`),
      )
    );
  }

  private injectAccountPrefix(path: string, accountId?: string): string {
    if (
      accountId &&
      ACCOUNT_SCOPED_PREFIXES.some((prefix) => path.startsWith(prefix))
    ) {
      return `/accounts/${accountId}${path}`;
    }

    return path;
  }

  /**
   * Normalize API responses to the CLI's expected envelope format.
   *
   * API returns: { apps: [...], total: 42 } or { id: "...", name: "..." }
   * CLI expects: { data: [...], meta: { pagination: { total: 42 } } }
   */
  private normalizeResponse<T>(body: unknown): ApiResponse<T> {
    if (body == null || typeof body !== 'object') {
      return { data: body as T };
    }

    const obj = body as Record<string, unknown>;

    // If already in expected format
    if ('data' in obj) {
      return toCamelCaseDeep(obj) as ApiResponse<T>;
    }

    // Treat only known collection keys as list responses.
    const arrayKey = Object.keys(obj).find(
      (key) => COLLECTION_RESPONSE_KEYS.has(key) && Array.isArray(obj[key]),
    );
    if (arrayKey) {
      const items = fixFieldTypes(toCamelCaseDeep(obj[arrayKey])) as T;
      const total =
        typeof obj.total === 'number' ? obj.total : (items as unknown[]).length;
      return {
        data: items,
        meta: {
          pagination: {
            total,
            limit: 0,
            offset: 0,
            hasMore: false,
          },
        },
      };
    }

    // Single item response — convert keys and wrap
    return { data: fixFieldTypes(toCamelCaseDeep(obj)) as T };
  }

  // Convenience methods
  async get<T = unknown>(
    path: string,
    params?: Record<string, string | number | boolean | undefined>,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, { params });
  }

  async post<T = unknown>(
    path: string,
    body?: unknown,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, { method: 'POST', body });
  }

  async patch<T = unknown>(
    path: string,
    body: unknown,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, { method: 'PATCH', body });
  }

  async delete<T = unknown>(path: string): Promise<ApiResponse<T>> {
    return this.request<T>(path, { method: 'DELETE' });
  }

  /**
   * GET a non-`/api/v1` endpoint by absolute API path (e.g. `/api/measurement/...`).
   * Bypasses path mapping, account injection, and key normalization. The response is
   * returned verbatim under `data` so callers see the endpoint's canonical shape.
   */
  async getRaw<T = unknown>(
    path: string,
    params?: Record<string, string | number | boolean | undefined>,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, { params, rawPath: true });
  }

  /** POST counterpart to {@link getRaw}. */
  async postRaw<T = unknown>(
    path: string,
    body?: unknown,
    params?: Record<string, string | number | boolean | undefined>,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, {
      method: 'POST',
      body,
      params,
      rawPath: true,
    });
  }

  /** PATCH counterpart to {@link getRaw}. */
  async patchRaw<T = unknown>(
    path: string,
    body?: unknown,
    params?: Record<string, string | number | boolean | undefined>,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, {
      method: 'PATCH',
      body,
      params,
      rawPath: true,
    });
  }

  /** DELETE counterpart to {@link getRaw}. */
  async deleteRaw<T = unknown>(
    path: string,
    params?: Record<string, string | number | boolean | undefined>,
  ): Promise<ApiResponse<T>> {
    return this.request<T>(path, {
      method: 'DELETE',
      params,
      rawPath: true,
    });
  }
}

/**
 * Post-process camelCased objects to fix field type mismatches
 * between the API response and what CLI commands expect.
 */
function fixFieldTypes(obj: unknown): unknown {
  if (Array.isArray(obj)) {
    return obj.map(fixFieldTypes);
  }
  if (obj !== null && typeof obj === 'object') {
    const record = obj as Record<string, unknown>;
    // role: API returns string, CLI expects string[]
    if (typeof record.role === 'string') {
      record.role = [record.role];
    }
    // appTypeId → appType alias (CLI uses appType for display)
    if (record.appTypeId && !record.appType) {
      record.appType = record.appTypeId;
    }
    // connectionType → appType alias (catalog/connectors endpoint)
    if (record.connectionType && !record.appType) {
      record.appType = record.connectionType;
    }
    // platform → category alias (catalog display)
    if (record.platform && !record.category) {
      record.category = record.platform;
    }
    // connectorType alias for jobs
    if (record.connectionType && !record.connectorType) {
      record.connectorType = record.connectionType;
    }
    // Ensure supportedRoles exists for catalog items
    if (record.appType && !record.supportedRoles) {
      record.supportedRoles = ['source'];
    }
    return record;
  }
  return obj;
}

// ── Key conversion helpers ─────────────────────────────────────────

function snakeToCamel(s: string): string {
  return s.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
}

export function toCamelCaseDeep(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(toCamelCaseDeep);
  }
  if (value !== null && typeof value === 'object' && !(value instanceof Date)) {
    const obj = value as Record<string, unknown>;
    const result: Record<string, unknown> = {};
    for (const [key, val] of Object.entries(obj)) {
      result[snakeToCamel(key)] = toCamelCaseDeep(val);
    }
    return result;
  }
  return value;
}

// ── Error handling ─────────────────────────────────────────────────

function friendlyHttpError(status: number, fallback?: string): string {
  switch (status) {
    case 401:
      return 'Authentication failed. Run `vendo login` to re-authenticate or check your API key.';
    case 403:
      return 'Permission denied. Your API key may not have access to this resource.';
    case 404:
      return 'Resource not found. Check the ID and try again.';
    case 429:
      return 'Rate limit exceeded. Wait a moment and try again.';
    default:
      return fallback ? `HTTP ${status}: ${fallback}` : `HTTP ${status}`;
  }
}

function getResponseRequestId(headers: Headers): string | undefined {
  return (
    headers.get('X-Request-Id') ?? headers.get('x-request-id') ?? undefined
  );
}

export class ClientError extends Error {
  public requestId?: string;
  public serverRequestId?: string;
  public details?: unknown;

  constructor(
    message: string,
    public statusCode: number,
    public code?: string,
    metadata?: {
      requestId?: string;
      serverRequestId?: string;
      details?: unknown;
    },
  ) {
    super(message);
    this.name = 'ClientError';
    this.requestId = metadata?.requestId;
    this.serverRequestId = metadata?.serverRequestId;
    this.details = metadata?.details;
  }
}

let _client: VendoClient | null = null;

export function getClient(): VendoClient {
  if (!_client) {
    _client = new VendoClient();
  }
  return _client;
}

import debug from 'debug';

import { callCoreRpc } from '../coreRpcClient';

const log = debug('skillRegistryApi');

/**
 * Catalog reads (`browse`/`search`/`sources`/`categories`) all funnel through
 * the backend's single-flight `browse_catalog`, whose COLD fetch downloads the
 * full ~90k-entry registry and can take ~80s. That comfortably exceeds the
 * default 30s `CORE_RPC_TIMEOUT_MS`, so a first load (or post-TTL revalidate)
 * would spuriously time out. Give these the longer per-call timeout the RPC
 * client supports for exactly such "slow-but-alive" calls; warm-cache reads
 * still return in milliseconds, so this only raises the ceiling for the rare
 * cold path. 120s = ~80s cold download + margin (well under the 10min clamp).
 */
const CATALOG_RPC_TIMEOUT_MS = 120_000;

/**
 * `browse()` is PAGED and filtered server-side.
 *
 * It used to take only `force_refresh` and return the entire catalog — measured
 * at 90,696 entries / ~39 MB per call — which the UI then searched and paged
 * client-side, so every cache miss re-pulled and re-parsed the whole payload
 * over RPC (renderer heap ~188 MB after one load). A module-level copy of that
 * array was kept here to soften repeat visits, which traded the re-pull for
 * holding tens of MB resident for the session.
 *
 * Now `query`, `sources`, `offset` and `limit` go to the core, which does the
 * filtering and returns one window plus `total`. A page of 60 rows is small
 * enough that no frontend cache is worth its memory, so there is none: each
 * call is one RPC for one page. `limit` is clamped core-side (200 max);
 * omitting it still returns everything, which is what keeps non-UI callers of
 * `skill_registry_browse` working unchanged.
 */
export interface BrowseCatalogOptions {
  /** Case-insensitive substring over name/description/tags/category/author. */
  query?: string;
  /** Restrict to these upstream sources. Omit or pass [] for no filter. */
  sources?: string[];
  offset?: number;
  limit?: number;
  /** Re-fetch the catalog from upstream before answering (server-side refresh). */
  forceRefresh?: boolean;
}

export interface CatalogPage {
  entries: CatalogEntry[];
  /** Matches before paging. Falls back to the page length on a legacy reply. */
  total: number;
}

export interface CatalogEntry {
  id: string;
  name: string;
  description: string;
  source: string;
  category: string;
  author: string | null;
  version: string | null;
  tags: string[];
  platforms: string[];
  download_url: string;
  docs_path: string | null;
  commands: string[];
  env_vars: string[];
  license: string | null;
}

interface RegistryInstallResult {
  url: string;
  stdout: string;
  stderr: string;
  newSkills: string[];
}

interface RawRegistryInstallResult {
  url: string;
  stdout: string;
  stderr: string;
  new_skills: string[];
}

interface RegistryUninstallResult {
  name: string;
  removedPath: string;
  scope: string;
}

interface RawRegistryUninstallResult {
  name: string;
  removed_path: string;
  scope: string;
}

interface ControllerSchemaSummary {
  namespace: string;
  function: string;
  description: string;
  inputs: Array<Record<string, unknown>>;
  outputs: Array<Record<string, unknown>>;
}

interface Envelope<T> {
  data?: T;
}

function unwrap<T>(response: Envelope<T> | T): T {
  if (response && typeof response === 'object' && 'data' in response) {
    const env = response as Envelope<T>;
    if (env.data !== undefined) return env.data as T;
  }
  return response as T;
}

export const skillRegistryApi = {
  browse: async (options: BrowseCatalogOptions = {}): Promise<CatalogPage> => {
    const { query, sources, offset, limit, forceRefresh = false } = options;
    log(
      'browse: query=%s sources=%o offset=%s limit=%s forceRefresh=%s',
      query,
      sources,
      offset,
      limit,
      forceRefresh
    );
    type RawPage = { entries: CatalogEntry[]; total?: number };
    const response = await callCoreRpc<Envelope<RawPage> | RawPage>({
      method: 'openhuman.skill_registry_browse',
      params: {
        force_refresh: forceRefresh,
        ...(query ? { query } : {}),
        ...(sources && sources.length > 0 ? { sources } : {}),
        ...(offset !== undefined ? { offset } : {}),
        ...(limit !== undefined ? { limit } : {}),
      },
      timeoutMs: CATALOG_RPC_TIMEOUT_MS,
    });
    const result = unwrap(response);
    const entries = result.entries ?? [];
    const total = result.total ?? entries.length;
    log('browse: count=%d total=%d', entries.length, total);
    return { entries, total };
  },

  search: async (query: string, source?: string, category?: string): Promise<CatalogEntry[]> => {
    log('search: query=%s source=%s category=%s', query, source, category);
    const response = await callCoreRpc<
      Envelope<{ entries: CatalogEntry[] }> | { entries: CatalogEntry[] }
    >({
      method: 'openhuman.skill_registry_search',
      params: { query, ...(source ? { source } : {}), ...(category ? { category } : {}) },
      timeoutMs: CATALOG_RPC_TIMEOUT_MS,
    });
    const result = unwrap(response);
    log('search: count=%d', result.entries.length);
    return result.entries;
  },

  sources: async (): Promise<string[]> => {
    log('sources: request');
    const response = await callCoreRpc<Envelope<{ sources: string[] }> | { sources: string[] }>({
      method: 'openhuman.skill_registry_sources',
      timeoutMs: CATALOG_RPC_TIMEOUT_MS,
    });
    const result = unwrap(response);
    log('sources: count=%d', result.sources.length);
    return result.sources;
  },

  categories: async (): Promise<string[]> => {
    log('categories: request');
    const response = await callCoreRpc<
      Envelope<{ categories: string[] }> | { categories: string[] }
    >({ method: 'openhuman.skill_registry_categories', timeoutMs: CATALOG_RPC_TIMEOUT_MS });
    const result = unwrap(response);
    log('categories: count=%d', result.categories.length);
    return result.categories;
  },

  install: async (entryId: string): Promise<RegistryInstallResult> => {
    log('install: entryId=%s', entryId);
    const response = await callCoreRpc<
      Envelope<RawRegistryInstallResult> | RawRegistryInstallResult
    >({ method: 'openhuman.skill_registry_install', params: { entry_id: entryId } });
    const raw = unwrap(response);
    const result: RegistryInstallResult = {
      url: raw.url,
      stdout: raw.stdout,
      stderr: raw.stderr,
      newSkills: raw.new_skills ?? [],
    };
    log('install: newSkills=%d', result.newSkills.length);
    return result;
  },

  uninstall: async (name: string): Promise<RegistryUninstallResult> => {
    log('uninstall: name=%s', name);
    const response = await callCoreRpc<
      Envelope<RawRegistryUninstallResult> | RawRegistryUninstallResult
    >({ method: 'openhuman.skill_registry_uninstall', params: { name } });
    const raw = unwrap(response);
    const result: RegistryUninstallResult = {
      name: raw.name,
      removedPath: raw.removed_path,
      scope: raw.scope,
    };
    log('uninstall: removedPath=%s', result.removedPath);
    return result;
  },

  schemas: async (): Promise<ControllerSchemaSummary[]> => {
    log('schemas: request');
    const response = await callCoreRpc<
      Envelope<{ schemas: ControllerSchemaSummary[] }> | { schemas: ControllerSchemaSummary[] }
    >({ method: 'openhuman.skill_registry_schemas' });
    const result = unwrap(response);
    log('schemas: count=%d', result.schemas.length);
    return result.schemas;
  },
};

import debug from 'debug';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { LuLibrary, LuRefreshCw, LuSparkles } from 'react-icons/lu';

import { useT } from '../../lib/i18n/I18nContext';
import { type CatalogEntry, skillRegistryApi } from '../../services/api/skillRegistryApi';
import {
  type InstallWorkflowFromUrlResult,
  skillsApi,
  type WorkflowSummary,
} from '../../services/api/skillsApi';
import EmptyStateCard from '../EmptyStateCard';
import ChipTabs from '../layout/ChipTabs';
import { Badge, DataTable, type DataTableColumn, ModalShell } from '../ui';
import Button from '../ui/Button';
import { TableCell, TableRow } from '../ui/Table';
import InstallSkillDialog from './InstallSkillDialog';
import UninstallSkillConfirmDialog from './UninstallSkillConfirmDialog';

const log = debug('skills:explorer-tab');
const CATALOG_PAGE_SIZE = 60;
const SEARCH_DEBOUNCE_MS = 300;

function slugifyInstallKey(value: string | null | undefined): string | null {
  const raw = value?.trim();
  if (!raw) return null;

  let out = '';
  let lastDash = false;
  for (const ch of raw) {
    if (/[a-z0-9]/i.test(ch)) {
      out += ch.toLowerCase();
      lastDash = false;
    } else if (!lastDash && out.length > 0) {
      out += '-';
      lastDash = true;
    }
  }
  return out.replace(/-+$/, '') || null;
}

function lastPathSegment(value: string | null | undefined): string | null {
  const raw = value?.trim();
  if (!raw) return null;
  const parts = raw.split(/[/:#?]+/).filter(Boolean);
  return parts.at(-1) ?? null;
}

function parentPathSegment(value: string | null | undefined): string | null {
  const raw = value?.trim();
  if (!raw) return null;
  const parts = raw.split(/[\\/]+/).filter(Boolean);
  return parts.length >= 2 ? (parts.at(-2) ?? null) : null;
}

function catalogInstallKeys(entry: CatalogEntry): string[] {
  return [
    slugifyInstallKey(entry.id),
    slugifyInstallKey(lastPathSegment(entry.id)),
    slugifyInstallKey(parentPathSegment(entry.docs_path)),
    slugifyInstallKey(parentPathSegment(entry.download_url)),
  ].filter((key): key is string => Boolean(key));
}

function workflowInstallKeys(skill: WorkflowSummary): string[] {
  return [slugifyInstallKey(skill.id), slugifyInstallKey(parentPathSegment(skill.location))].filter(
    (key): key is string => Boolean(key)
  );
}

function isCatalogEntryInstalled(entry: CatalogEntry, installedKeys: Set<string>): boolean {
  return catalogInstallKeys(entry).some(key => installedKeys.has(key));
}

/**
 * Source tone table. Six sources, four themeable ramps — so the hue encodes the
 * distinction a reader acts on (where does this skill come from: shipped with
 * the app, or fetched from a remote catalogue) rather than naming each
 * catalogue twice. The badge already prints the catalogue's name, so the four
 * remote rows fall through to the shared neutral tone instead of reaching for
 * unthemeable ramps. See `gitbooks/developing/theming.md`.
 */
const BADGE_NEUTRAL_TONE = 'bg-surface-muted text-content-secondary border-line';

function SourceBadge({ source }: { source: string }) {
  const SOURCE_COLORS: Record<string, string> = {
    'built-in':
      'bg-sage-50 text-sage-700 border-sage-200 dark:bg-sage-500/10 dark:text-sage-300 dark:border-sage-500/30',
    optional:
      'bg-primary-50 text-primary-700 border-primary-200 dark:bg-primary-500/10 dark:text-primary-300 dark:border-primary-500/30',
  };
  const colors = SOURCE_COLORS[source] ?? BADGE_NEUTRAL_TONE;
  return (
    <span
      className={`inline-flex items-center rounded-full border px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider ${colors}`}>
      {source}
    </span>
  );
}

/**
 * Format tone table. Three distinct tones for five formats, so it fits inside
 * the four themeable ramps with no collision and every distinction survives:
 * the Hermes family on `primary`, the ClawHub family on `sage`, and `legacy`
 * on `amber` because it is the one row that means "deprecated".
 */
const FORMAT_TONE = {
  hermes:
    'bg-primary-50 text-primary-700 border-primary-200 dark:bg-primary-500/10 dark:text-primary-300 dark:border-primary-500/30',
  clawhub:
    'bg-sage-50 text-sage-700 border-sage-200 dark:bg-sage-500/10 dark:text-sage-300 dark:border-sage-500/30',
  legacy:
    'bg-amber-50 text-amber-700 border-amber-200 dark:bg-amber-500/10 dark:text-amber-300 dark:border-amber-500/30',
} as const;

function SkillFormatBadge({ format }: { format: string }) {
  const lower = format.toLowerCase();
  const FORMAT_MAP: Record<string, { label: string; colors: string }> = {
    hermes: { label: 'Hermes', colors: FORMAT_TONE.hermes },
    agentskills: { label: 'AgentSkills', colors: FORMAT_TONE.hermes },
    openclaw: { label: 'OpenClaw', colors: FORMAT_TONE.clawhub },
    clawhub: { label: 'ClawHub', colors: FORMAT_TONE.clawhub },
    legacy: { label: 'Legacy', colors: FORMAT_TONE.legacy },
  };
  const entry = FORMAT_MAP[lower] ?? {
    label: format || 'Skill',
    colors: BADGE_NEUTRAL_TONE,
  };
  return (
    <span
      className={`inline-flex items-center rounded-full border px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider ${entry.colors}`}>
      {entry.label}
    </span>
  );
}

function SkillScopeBadge({ scope }: { scope: string }) {
  const { t } = useT();
  const label =
    scope === 'user'
      ? t('skills.explorer.scopeUser')
      : scope === 'project'
        ? t('skills.explorer.scopeProject')
        : t('skills.explorer.scopeLegacy');
  return (
    <span className="inline-flex items-center rounded-full border border-line bg-surface-muted px-1.5 py-0.5 text-[9px] font-medium text-content-muted">
      {label}
    </span>
  );
}

interface SkillTileProps {
  skill: WorkflowSummary;
  onUninstall: () => void;
  onClick: () => void;
}

function InstalledSkillRow({ skill, onUninstall, onClick }: SkillTileProps) {
  const { t } = useT();
  return (
    <TableRow
      data-testid={`skill-explorer-tile-${skill.id}`}
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={event => {
        if (event.key === 'Enter') onClick();
        if (event.key === ' ' || event.key === 'Space') {
          event.preventDefault();
          onClick();
        }
      }}
      className="cursor-pointer">
      <TableCell className="min-w-48">
        <Button
          type="button"
          variant="tertiary"
          size="xs"
          onClick={onClick}
          className="h-auto max-w-full p-0 text-left font-medium hover:bg-transparent">
          <span className="truncate">{skill.name}</span>
        </Button>
      </TableCell>
      <TableCell className="min-w-[18rem] max-w-xl text-xs text-content-muted">
        <span className="line-clamp-1">
          {skill.description || t('skills.explorer.noDescription')}
        </span>
        {(skill.tags.length > 0 || skill.warnings.length > 0) && (
          <div className="mt-1 flex flex-wrap items-center gap-1">
            {skill.tags.map(tag => (
              <Badge key={tag} variant="neutral">
                {tag}
              </Badge>
            ))}
            {skill.warnings.map(warning => (
              <span key={warning} className="text-[10px] text-amber-700 dark:text-amber-300">
                {warning}
              </span>
            ))}
          </div>
        )}
      </TableCell>
      <TableCell className="whitespace-nowrap">
        <div className="flex flex-wrap items-center gap-1">
          <SkillFormatBadge format={skill.sourceFormat} />
          <SkillScopeBadge scope={skill.scope} />
          {skill.version && (
            <span className="text-[10px] font-mono text-content-faint">v{skill.version}</span>
          )}
        </div>
      </TableCell>
      <TableCell className="w-px whitespace-nowrap text-right">
        {skill.scope === 'user' ? (
          <Button
            variant="secondary"
            tone="danger"
            size="xs"
            data-testid={`skill-uninstall-${skill.id}`}
            onClick={event => {
              event.stopPropagation();
              onUninstall();
            }}>
            {t('skills.disconnect')}
          </Button>
        ) : (
          <Badge variant="neutral">{t('skills.explorer.installed')}</Badge>
        )}
      </TableCell>
    </TableRow>
  );
}

interface CatalogTileProps {
  entry: CatalogEntry;
  installed: boolean;
  installing: boolean;
  onInstall: () => void;
  onClick: () => void;
}

interface SkillDetailDialogProps {
  entry: CatalogEntry | null;
  skill: WorkflowSummary | null;
  installed: boolean;
  onClose: () => void;
  onInstall?: () => void;
  installing?: boolean;
}

function CatalogRow({ entry, installed, installing, onInstall, onClick }: CatalogTileProps) {
  const { t } = useT();
  return (
    <TableRow
      className="group cursor-pointer"
      data-testid={`registry-tile-${entry.id}`}
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={event => {
        if (event.key === 'Enter') onClick();
        if (event.key === ' ' || event.key === 'Space') {
          event.preventDefault();
          onClick();
        }
      }}>
      <TableCell className="min-w-48">
        <Button
          type="button"
          variant="tertiary"
          size="xs"
          onClick={onClick}
          className="h-auto max-w-full p-0 text-left font-medium hover:bg-transparent">
          <span className="truncate">{entry.name}</span>
        </Button>
      </TableCell>
      <TableCell className="min-w-[18rem] max-w-xl text-xs text-content-muted">
        <span className="line-clamp-1">{entry.description}</span>
      </TableCell>
      <TableCell className="whitespace-nowrap">
        <SourceBadge source={entry.source} />
      </TableCell>
      <TableCell className="w-px whitespace-nowrap text-right">
        {installed ? (
          <Badge variant="success">{t('skills.explorer.installed')}</Badge>
        ) : (
          <Button
            variant="secondary"
            size="xs"
            data-testid={`registry-install-${entry.id}`}
            disabled={installing}
            onClick={event => {
              event.stopPropagation();
              onInstall();
            }}>
            {installing ? t('skills.explorer.installing') : t('skills.explorer.install')}
          </Button>
        )}
      </TableCell>
    </TableRow>
  );
}

function SkillDetailDialog({
  entry,
  skill,
  installed,
  onClose,
  onInstall,
  installing,
}: SkillDetailDialogProps) {
  const { t } = useT();
  const name = entry?.name ?? skill?.name ?? '';
  const description = entry?.description ?? skill?.description ?? '';
  const tags = entry?.tags ?? skill?.tags ?? [];
  const version = entry?.version ?? skill?.version ?? '';
  const author = entry?.author ?? '';
  const source = entry?.source ?? '';
  const category = entry?.category ?? '';
  const downloadUrl = entry?.download_url ?? '';
  const license = entry?.license ?? '';

  return (
    <ModalShell
      onClose={onClose}
      titleId="skill-detail-title"
      maxWidthClassName="max-w-lg"
      contentClassName="p-5 space-y-4"
      title={
        <span className="flex items-center gap-2">
          <span className="truncate">{name}</span>
          {installed && (
            <span className="shrink-0 rounded-full border border-sage-200 dark:border-sage-500/30 bg-sage-50 dark:bg-sage-500/10 px-2 py-0.5 text-[10px] font-medium text-sage-700 dark:text-sage-300">
              {t('skills.explorer.installed')}
            </span>
          )}
        </span>
      }
      subtitle={
        <span className="mt-1.5 flex items-center gap-1.5">
          {source && <SourceBadge source={source} />}
          {category && (
            <span className="inline-flex items-center rounded-full border border-line bg-surface-muted px-1.5 py-0.5 text-[9px] font-medium text-content-muted">
              {category}
            </span>
          )}
        </span>
      }
      footer={
        !installed && onInstall ? (
          <div className="flex justify-end">
            <Button variant="secondary" size="sm" disabled={installing} onClick={onInstall}>
              {installing ? t('skills.explorer.installing') : t('skills.explorer.install')}
            </Button>
          </div>
        ) : undefined
      }>
      <>
        {description && (
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-wider text-content-faint mb-1">
              {t('skills.detail.description')}
            </h3>
            <p className="text-sm text-content-secondary leading-relaxed whitespace-pre-wrap">
              {description}
            </p>
          </div>
        )}

        <div className="flex flex-wrap gap-x-6 gap-y-2">
          {version && (
            <div>
              <span className="text-[10px] font-semibold uppercase tracking-wider text-content-faint">
                {t('skills.detail.version')}
              </span>
              <p className="text-xs font-mono text-content-secondary">v{version}</p>
            </div>
          )}
          {author && (
            <div>
              <span className="text-[10px] font-semibold uppercase tracking-wider text-content-faint">
                {t('skills.detail.author')}
              </span>
              <p className="text-xs text-content-secondary">{author}</p>
            </div>
          )}
          {license && (
            <div>
              <span className="text-[10px] font-semibold uppercase tracking-wider text-content-faint">
                {t('skills.detail.license')}
              </span>
              <p className="text-xs text-content-secondary">{license}</p>
            </div>
          )}
        </div>

        {tags.length > 0 && (
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-wider text-content-faint mb-1.5">
              {t('skills.detail.tags')}
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {tags.map(tag => (
                <span
                  key={tag}
                  className="rounded-full bg-surface-subtle px-2 py-0.5 text-[10px] font-medium text-content-secondary">
                  {tag}
                </span>
              ))}
            </div>
          </div>
        )}

        {downloadUrl && (
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-wider text-content-faint mb-1">
              {t('skills.detail.source')}
            </h3>
            <p className="text-[11px] font-mono text-content-faint break-all">{downloadUrl}</p>
          </div>
        )}
      </>
    </ModalShell>
  );
}

type ExplorerView = 'installed' | 'registry';

interface SkillsExplorerTabProps {
  onToast?: (toast: { type: 'success' | 'error'; title: string; message?: string }) => void;
}

export default function SkillsExplorerTab({ onToast }: SkillsExplorerTabProps) {
  const { t } = useT();
  const [view, setView] = useState<ExplorerView>('registry');

  const [skills, setSkills] = useState<WorkflowSummary[]>([]);
  const [skillsLoading, setSkillsLoading] = useState(true);
  const [skillsError, setSkillsError] = useState<string | null>(null);

  // Catalog rows loaded so far — the first page, plus whatever "Show more" has
  // appended. The registry holds ~90k entries (~39 MB), so the search box, the
  // source filter and the paging all run server-side and we only ever hold the
  // pages the user actually revealed.
  const [catalogEntries, setCatalogEntries] = useState<CatalogEntry[]>([]);
  const [catalogTotal, setCatalogTotal] = useState(0);
  const [catalogLoading, setCatalogLoading] = useState(false);
  // Separate from `catalogLoading` so appending a page keeps the rows on screen
  // instead of swapping the table for the loading state.
  const [catalogLoadingMore, setCatalogLoadingMore] = useState(false);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [catalogInitialized, setCatalogInitialized] = useState(false);
  const [installingId, setInstallingId] = useState<string | null>(null);
  // Catalog entry ids we just installed this session. The "installed" badge is
  // otherwise derived purely from `isCatalogEntryInstalled`, a heuristic that
  // maps a refetched installed skill (whose post-install id/location can differ
  // from the catalog entry) back to the catalog card. When that mapping misses,
  // a successful install fell back to "Install" — the only signal was a fleeting
  // toast, so the card looked unchanged (#4150). Recording the installed entry
  // id here makes the card flip to "Installed" deterministically on success.
  const [installedEntryIds, setInstalledEntryIds] = useState<Set<string>>(new Set());

  const [sources, setSources] = useState<string[]>([]);
  const [activeSources, setActiveSources] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState('');
  const [debouncedQuery, setDebouncedQuery] = useState('');
  const [installDialogOpen, setInstallDialogOpen] = useState(false);
  const [uninstallTarget, setUninstallTarget] = useState<WorkflowSummary | null>(null);
  const [detailEntry, setDetailEntry] = useState<CatalogEntry | null>(null);
  const [detailSkill, setDetailSkill] = useState<WorkflowSummary | null>(null);

  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Monotonic id of the newest catalog request; older replies are discarded. */
  const catalogRequestRef = useRef(0);

  // Debounce search input
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      setDebouncedQuery(searchQuery);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [searchQuery]);

  const fetchSkills = useCallback(async () => {
    log('fetchSkills: start');
    setSkillsLoading(true);
    setSkillsError(null);
    try {
      // Include `skills/`-root installs (registry installs land there) so they
      // appear in the Installed tab and flip the catalog Install button.
      const result = await skillsApi.listWorkflows({ includeSkills: true });
      log('fetchSkills: count=%d', result.length);
      setSkills(result);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      log('fetchSkills: error=%s', msg);
      setSkillsError(msg);
    } finally {
      setSkillsLoading(false);
    }
  }, []);

  // Active source filter as a stable string key, so it can be an effect dep
  // without a fresh array identity re-firing the fetch on every render. Empty
  // string = no filter (all or none of the chips selected).
  const activeSourceKey = useMemo(() => {
    if (activeSources.size === 0 || activeSources.size >= sources.length) return '';
    return [...activeSources].sort().join('\n');
  }, [activeSources, sources.length]);

  // Fetch one page of the catalog. The query and the source filter are sent to
  // the core, which filters ~90k entries there and returns `limit` rows plus the
  // pre-paging `total` — the list is never pulled into the renderer whole.
  const fetchCatalogPage = useCallback(
    async (query: string, sourceKey: string, forceRefresh: boolean, offset: number) => {
      const append = offset > 0;
      log(
        'fetchCatalogPage: query=%s sources=%s forceRefresh=%s offset=%d append=%s',
        query,
        sourceKey,
        forceRefresh,
        offset,
        append
      );
      // Pages append now, so a response that lost the race must be dropped:
      // concatenating it would splice rows from an abandoned query into the
      // current one. (The pre-paging code replaced the list, so a late reply
      // was merely stale, never mixed.)
      const requestId = ++catalogRequestRef.current;
      if (append) {
        setCatalogLoadingMore(true);
      } else {
        // This request supersedes anything in flight, so a pending append's
        // `finally` will bail on the request-id check and never clear its own
        // flag — leaving "Show more" disabled after a refresh. Clear it here,
        // where the supersession actually happens.
        setCatalogLoadingMore(false);
        setCatalogLoading(true);
      }
      setCatalogError(null);
      try {
        const page = await skillRegistryApi.browse({
          query: query || undefined,
          sources: sourceKey ? sourceKey.split('\n') : undefined,
          offset,
          limit: CATALOG_PAGE_SIZE,
          forceRefresh,
        });
        if (catalogRequestRef.current !== requestId) {
          log('fetchCatalogPage: superseded, discarding %d entries', page.entries.length);
          return;
        }
        log('fetchCatalogPage: got=%d total=%d', page.entries.length, page.total);
        setCatalogTotal(page.total);
        setCatalogEntries(prev => (append ? [...prev, ...page.entries] : page.entries));
        setCatalogInitialized(true);
      } catch (err) {
        if (catalogRequestRef.current !== requestId) return;
        const msg = err instanceof Error ? err.message : String(err);
        log('fetchCatalogPage: error=%s', msg);
        setCatalogError(msg);
      } finally {
        // Only the newest request owns the loading flag. A superseded request
        // still reaches this block, and if it settles second it would clear the
        // spinner while the request whose results will actually be rendered is
        // still in flight — stale rows, no loading state, for the active query.
        if (catalogRequestRef.current === requestId) {
          if (append) setCatalogLoadingMore(false);
          else setCatalogLoading(false);
        }
      }
    },
    []
  );

  useEffect(() => {
    void fetchSkills();
    skillRegistryApi
      .sources()
      .then(s => {
        setSources(s);
        setActiveSources(new Set(s));
      })
      .catch(() => {});
  }, [fetchSkills]);

  // Trigger a fresh first page when the debounced query or source filter changes
  useEffect(() => {
    if (view === 'registry') {
      void fetchCatalogPage(debouncedQuery, activeSourceKey, false, 0);
    }
  }, [view, debouncedQuery, activeSourceKey, fetchCatalogPage]);

  const installedKeys = useMemo(
    () => new Set(skills.flatMap(skill => workflowInstallKeys(skill))),
    [skills]
  );

  // A catalog entry counts as installed if the refetched installed list maps
  // back to it (`isCatalogEntryInstalled`) OR we installed it this session. The
  // latter guarantees the card reflects a successful install even when the
  // heuristic key-match misses (#4150).
  const entryInstalled = useCallback(
    (entry: CatalogEntry): boolean =>
      installedEntryIds.has(entry.id) || isCatalogEntryInstalled(entry, installedKeys),
    [installedEntryIds, installedKeys]
  );

  const filteredSkills = useMemo(() => {
    const q = searchQuery.toLowerCase().trim();
    if (!q) return skills;
    return skills.filter(
      s =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q) ||
        s.tags.some(tag => tag.toLowerCase().includes(q)) ||
        s.sourceFormat.toLowerCase().includes(q)
    );
  }, [skills, searchQuery]);

  const sortedSkills = useMemo(() => {
    return [...filteredSkills].sort((a, b) => {
      if (a.sourceFormat === 'hermes' && b.sourceFormat !== 'hermes') return -1;
      if (a.sourceFormat !== 'hermes' && b.sourceFormat === 'hermes') return 1;
      return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' });
    });
  }, [filteredSkills]);

  const handleInstalled = useCallback(
    (result: InstallWorkflowFromUrlResult) => {
      log('handleInstalled: newSkills=%d', result.newWorkflows.length);
      void fetchSkills();
      if (result.newWorkflows.length > 0) {
        onToast?.({
          type: 'success',
          title: t('skills.install.installComplete'),
          message: t('skills.install.successDiscovered').replace(
            '{count}',
            String(result.newWorkflows.length)
          ),
        });
      }
    },
    [fetchSkills, onToast, t]
  );

  const handleUninstalled = useCallback(() => {
    log('handleUninstalled');
    void fetchSkills();
    onToast?.({ type: 'success', title: t('skills.explorer.uninstallSuccess') });
  }, [fetchSkills, onToast, t]);

  const handleRegistryInstall = useCallback(
    async (entry: CatalogEntry) => {
      log('handleRegistryInstall: id=%s source=%s', entry.id, entry.source);
      setInstallingId(entry.id);
      try {
        const result = await skillRegistryApi.install(entry.id);
        // Authoritatively mark this entry installed so the card flips to
        // "Installed" on success regardless of whether the refetched list maps
        // back to it via the install-key heuristic (#4150).
        setInstalledEntryIds(prev => {
          const next = new Set(prev);
          next.add(entry.id);
          return next;
        });
        // Await the refetch so `installedKeys` is fresh before the button
        // re-renders — otherwise it briefly flips back to "Install" between
        // clearing the installing state and the list updating. `fetchSkills`
        // swallows its own errors, so this never throws into the catch below.
        await fetchSkills();
        onToast?.({
          type: 'success',
          title: t('skills.install.installComplete'),
          message: `Installed ${entry.name}${result.newSkills.length > 0 ? ` (${result.newSkills.join(', ')})` : ''}`,
        });
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        log('handleRegistryInstall: error=%s', msg);
        onToast?.({ type: 'error', title: t('skills.install.errors.genericTitle'), message: msg });
      } finally {
        setInstallingId(null);
      }
    },
    [fetchSkills, onToast, t]
  );

  const loading = view === 'installed' ? skillsLoading : catalogLoading;
  const error = view === 'installed' ? skillsError : catalogError;

  const columns: DataTableColumn<never>[] = [
    { id: 'name', header: t('skills.explorer.colSkill'), className: 'min-w-48' },
    { id: 'description', header: t('skills.explorer.colDescription'), className: 'min-w-[18rem]' },
    { id: 'provider', header: t('skills.explorer.colProvider') },
    { id: 'action', header: t('skills.explorer.colAction'), align: 'right' },
  ];

  // The two views share one toolbar so the tab chips, the search box and the
  // row of actions do not jump between them. Registry-only affordances (source
  // filter, catalog refresh) render conditionally rather than in a second bar.
  const toolbarStart = (
    <>
      <ChipTabs<ExplorerView>
        ariaLabel={t('skills.explorer.title')}
        className="flex flex-wrap gap-1.5"
        testIdPrefix="skill-explorer-tab"
        value={view}
        onChange={setView}
        items={[
          {
            id: 'registry',
            label: (
              <>
                {t('skills.explorer.registryTab')}
                {catalogTotal > 0 && (
                  <span className="tabular-nums opacity-70">{catalogTotal.toLocaleString()}</span>
                )}
              </>
            ),
          },
          {
            id: 'installed',
            label: (
              <>
                {t('skills.explorer.installedTab')}
                {skills.length > 0 && (
                  <span className="tabular-nums opacity-70">{skills.length}</span>
                )}
              </>
            ),
          },
        ]}
      />
      <Button
        variant="secondary"
        size="sm"
        data-testid="skill-install-from-url-btn"
        onClick={() => setInstallDialogOpen(true)}
        className="shrink-0">
        {t('skills.explorer.installFromUrl')}
      </Button>
    </>
  );

  const toolbarEnd =
    view === 'registry' ? (
      <Button
        iconOnly
        variant="secondary"
        size="md"
        onClick={() => void fetchCatalogPage(debouncedQuery, activeSourceKey, true, 0)}
        disabled={catalogLoading}
        title={t('skills.explorer.refreshRegistry')}
        aria-label={t('skills.explorer.refreshRegistry')}
        className="shrink-0 text-content-muted shadow-xs">
        <LuRefreshCw className={`h-4 w-4 ${catalogLoading ? 'animate-spin' : ''}`} />
      </Button>
    ) : null;

  const filters =
    view === 'registry' && sources.length > 0
      ? [
          {
            id: 'source',
            label: t('common.filter'),
            ariaLabel: t('skills.explorer.sourceFilterAria'),
            testId: 'skill-source-filter',
            options: sources.map(source => ({ value: source })),
            selected: activeSources,
            onChange: setActiveSources,
          },
        ]
      : undefined;

  const errorNode =
    !loading && error ? (
      <div className="rounded-xl border border-coral-200 bg-coral-50 p-3 dark:border-coral-500/30 dark:bg-coral-500/10">
        <p className="text-xs font-medium text-coral-700 dark:text-coral-300">{error}</p>
        <Button
          variant="secondary"
          tone="danger"
          size="xs"
          onClick={() =>
            void (view === 'installed'
              ? fetchSkills()
              : fetchCatalogPage(debouncedQuery, activeSourceKey, true, 0))
          }
          className="mt-2">
          {t('common.retry')}
        </Button>
      </div>
    ) : null;

  const installedEmpty =
    // A search that matched nothing is not the same as having no skills — the
    // second offers an install CTA, the first would be nonsense.
    skills.length > 0 ? (
      <p className="px-1 py-8 text-center text-xs text-content-faint">{t('skills.noResults')}</p>
    ) : (
      <EmptyStateCard
        className="mx-1 mb-3 py-10"
        icon={<LuSparkles className="h-7 w-7 text-primary-500" strokeWidth={1.5} />}
        title={t('skills.explorer.emptyTitle')}
        description={t('skills.explorer.emptyDescription')}
        actionLabel={t('skills.explorer.emptyCta')}
        onAction={() => setInstallDialogOpen(true)}
      />
    );

  const registryEmpty = catalogInitialized ? (
    <EmptyStateCard
      className="mx-1 mb-3 py-10"
      icon={<LuLibrary className="h-7 w-7 text-primary-500" strokeWidth={1.5} />}
      title={debouncedQuery ? t('skills.noResults') : t('skills.explorer.registryEmptyTitle')}
      description={debouncedQuery ? '' : t('skills.explorer.registryEmptyDescription')}
      actionLabel={debouncedQuery ? undefined : t('skills.explorer.refreshRegistry')}
      onAction={debouncedQuery ? undefined : () => void fetchCatalogPage('', '', true, 0)}
    />
  ) : null;

  // "Show more" now fetches the next server page instead of revealing more of a
  // fully-downloaded list, so the control is driven by the server-reported total.
  const registryFooter =
    catalogEntries.length < catalogTotal ? (
      <div className="mt-3 flex flex-col items-center gap-1">
        <Button
          variant="secondary"
          size="sm"
          data-testid="registry-show-more"
          disabled={catalogLoadingMore}
          onClick={() =>
            void fetchCatalogPage(
              debouncedQuery,
              activeSourceKey,
              false,
              catalogEntries.length
            )
          }
          className="h-auto border-line px-4 py-2 text-xs font-medium text-content-secondary shadow-soft">
          {t('common.showMore')}
        </Button>
        <p className="text-[11px] text-content-faint">
          {catalogEntries.length.toLocaleString()} / {catalogTotal.toLocaleString()}
        </p>
      </div>
    ) : null;

  const shared = {
    columns,
    search: {
      value: searchQuery,
      onChange: setSearchQuery,
      placeholder: t('skills.explorer.searchPlaceholder'),
      testId: 'skill-search-input',
    },
    toolbarStart,
    toolbarEnd,
    filters,
    loading,
    error: errorNode,
    ariaLabel: t('skills.explorer.title'),
  };

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden animate-fade-up">
      {view === 'installed' ? (
        <DataTable<WorkflowSummary>
          {...shared}
          columns={columns as DataTableColumn<WorkflowSummary>[]}
          rows={sortedSkills}
          rowKey={skill => skill.id}
          empty={installedEmpty}
          renderRow={skill => (
            <InstalledSkillRow
              key={skill.id}
              skill={skill}
              onClick={() => setDetailSkill(skill)}
              onUninstall={() => setUninstallTarget(skill)}
            />
          )}
        />
      ) : (
        <DataTable<CatalogEntry>
          {...shared}
          columns={columns as DataTableColumn<CatalogEntry>[]}
          rows={catalogEntries}
          rowKey={entry => `${entry.source}-${entry.id}`}
          empty={registryEmpty}
          footer={registryFooter}
          renderRow={entry => (
            <CatalogRow
              key={`${entry.source}-${entry.id}`}
              entry={entry}
              installed={entryInstalled(entry)}
              installing={installingId === entry.id}
              onClick={() => setDetailEntry(entry)}
              onInstall={() => void handleRegistryInstall(entry)}
            />
          )}
        />
      )}

      {installDialogOpen && (
        <InstallSkillDialog
          onClose={() => setInstallDialogOpen(false)}
          onInstalled={handleInstalled}
        />
      )}

      {uninstallTarget && (
        <UninstallSkillConfirmDialog
          skill={uninstallTarget}
          onClose={() => setUninstallTarget(null)}
          onUninstalled={handleUninstalled}
        />
      )}

      {(detailEntry || detailSkill) && (
        <SkillDetailDialog
          entry={detailEntry}
          skill={detailSkill}
          installed={detailEntry ? entryInstalled(detailEntry) : true}
          onClose={() => {
            setDetailEntry(null);
            setDetailSkill(null);
          }}
          onInstall={
            detailEntry && !entryInstalled(detailEntry)
              ? () => {
                  void handleRegistryInstall(detailEntry);
                  setDetailEntry(null);
                }
              : undefined
          }
          installing={detailEntry ? installingId === detailEntry.id : false}
        />
      )}
    </div>
  );
}

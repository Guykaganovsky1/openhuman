/**
 * Tests for MemorySourcesRegistry (issue #3295):
 *
 * RC#1 — concurrent syncs: multiple sources can show "syncing" simultaneously.
 * RC#2 — source_id matching: events matched by source_id (preferred) or
 *         connection_id (fallback) for backward compat.
 * RC#4 — tolerant parseSyncProgress: numeric ratio, stage fallback, and
 *         indeterminate (null) cases.
 */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { renderWithProviders } from '../../../test/test-utils';
import {
  MemorySourcesRegistry,
  parseIngestedCount,
  parseSyncNote,
  parseSyncProgress,
  resolveSyncRowId,
  stripSourceScopePrefix,
} from '../MemorySourcesRegistry';
import { resetMemorySyncActivityForTests } from '../memorySyncActivityStore';

// ── i18n mock (returns key as the translation) ────────────────────────────────
vi.mock('../../../lib/i18n/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));

// ── memorySourcesService mock ─────────────────────────────────────────────────
vi.mock('../../../services/memorySourcesService', () => ({
  listMemorySources: vi.fn().mockResolvedValue([]),
  memorySourcesStatusList: vi.fn().mockResolvedValue([]),
  syncMemorySource: vi.fn().mockResolvedValue(undefined),
  removeMemorySource: vi.fn().mockResolvedValue(undefined),
  updateMemorySource: vi
    .fn()
    .mockImplementation((id: string, patch: Record<string, unknown>) =>
      Promise.resolve({ id, kind: 'folder', label: 'Test', enabled: true, ...patch })
    ),
  SOURCE_KIND_ICONS: {
    folder: 'F',
    composio: 'C',
    github_repo: 'G',
    rss_feed: 'R',
    web_page: 'W',
    twitter_query: 'T',
  },
  SOURCE_KIND_LABEL_KEYS: {
    folder: 'memorySources.kind.folder',
    composio: 'memorySources.kind.composio',
    github_repo: 'memorySources.kind.github_repo',
    rss_feed: 'memorySources.kind.rss_feed',
    web_page: 'memorySources.kind.web_page',
    twitter_query: 'memorySources.kind.twitter_query',
  },
}));

// ── tauriCommands mock ────────────────────────────────────────────────────────
// memoryTreePipelineStatus is polled for downstream pipeline health (GH-4690);
// default to a healthy running snapshot so rows keep their clean synced state.
const mockTrackAnalyticsEvent = vi.fn();
vi.mock('../../analytics', () => ({
  trackAnalyticsEvent: (...args: unknown[]) => mockTrackAnalyticsEvent(...args),
}));

vi.mock('../../../utils/tauriCommands/memoryTree', () => ({
  memoryTreeBackfillConnectorTrees: vi.fn(),
  memoryTreeFlushSource: vi.fn().mockResolvedValue({ seals_fired: 0 }),
  memoryTreePipelineStatus: vi
    .fn()
    .mockResolvedValue({
      status: 'running',
      reason: null,
      last_sync_ms: 0,
      total_chunks: 0,
      wiki_size_bytes: 0,
      pipeline_jobs: { ready: 0, running: 0, failed: 0 },
      is_syncing: false,
      is_paused: false,
    }),
}));

// ── helpers ───────────────────────────────────────────────────────────────────

function makeSyncStageEvent(detail: {
  stage: string;
  source_id?: string | null;
  connection_id?: string | null;
  detail?: string;
}): CustomEvent {
  return new CustomEvent('openhuman:memory-sync-stage', { detail });
}

function makeSource(id: string) {
  return { id, kind: 'folder' as const, label: `Source ${id}`, enabled: true };
}

// ── parseSyncProgress unit tests ──────────────────────────────────────────────

describe('parseSyncProgress', () => {
  it('returns ratio for "N/M ..." numeric pattern', () => {
    expect(parseSyncProgress('5/10 processed', 'ingesting')).toBe(50);
    expect(parseSyncProgress('3/12 docs', 'fetching')).toBe(25);
    expect(parseSyncProgress('1/1 done', 'ingesting')).toBe(100);
  });

  it('returns stage fallback when no numeric ratio is present', () => {
    expect(parseSyncProgress('queue_depth=3', 'ingesting')).toBe(40);
    expect(parseSyncProgress('listing items', 'fetching')).toBe(5);
    expect(parseSyncProgress(null, 'requested')).toBe(2);
    expect(parseSyncProgress('canonicalized 3 chunks', 'stored')).toBe(15);
    expect(parseSyncProgress('queued chunk extraction', 'queued')).toBe(25);
  });

  it('returns 100 for completed stage', () => {
    expect(parseSyncProgress(null, 'completed')).toBe(100);
    expect(parseSyncProgress('ingested 5 item(s)', 'completed')).toBe(100);
  });

  it('returns null when no ratio and no recognized stage', () => {
    expect(parseSyncProgress('some detail', 'unknown_stage')).toBeNull();
    expect(parseSyncProgress(null, undefined)).toBeNull();
    expect(parseSyncProgress(null)).toBeNull();
  });

  it('handles non-ratio numeric strings gracefully', () => {
    // "N discovered" — no slash — should use stage fallback, not parse as ratio
    expect(parseSyncProgress('3 discovered', 'stored')).toBe(15);
    // "N/0 ..." — divide by zero guard
    expect(parseSyncProgress('5/0 items', 'fetching')).toBe(5); // falls through to stage fallback
  });
});

// ── parseIngestedCount unit tests ─────────────────────────────────────────────

describe('parseIngestedCount', () => {
  it('parses the count from "ingested N item(s)"', () => {
    expect(parseIngestedCount('ingested 5 item(s)')).toBe(5);
    expect(parseIngestedCount('ingested 0 item(s)')).toBe(0);
    expect(parseIngestedCount('ingested 123 items')).toBe(123);
  });

  it('returns null when no count is present', () => {
    expect(parseIngestedCount(null)).toBeNull();
    expect(parseIngestedCount('done')).toBeNull();
    expect(parseIngestedCount('delegating to composio sync')).toBeNull();
  });
});

// ── parseSyncNote unit tests ──────────────────────────────────────────────────

describe('parseSyncNote', () => {
  it('reads "more pending" from the remainder after the count', () => {
    expect(parseSyncNote('ingested 100 item(s), more pending — Sync again to continue')).toBe(
      'more_pending'
    );
  });

  it('reads a spent budget, and prefers it when both appear', () => {
    expect(parseSyncNote("ingested 0 item(s); today's provider request budget is spent")).toBe(
      'budget_spent'
    );
    expect(
      parseSyncNote(
        "ingested 0 item(s), more pending — Sync again to continue; today's provider request budget is spent"
      )
    ).toBe('budget_spent');
  });

  it('returns null for a plain count or no detail', () => {
    expect(parseSyncNote('ingested 5 item(s)')).toBeNull();
    expect(parseSyncNote(null)).toBeNull();
  });
});

// ── MemorySourcesRegistry integration tests ───────────────────────────────────

describe('MemorySourcesRegistry', () => {
  // The live sync state is a page-wide store now (openhuman#6019); each test
  // starts from nothing in flight.
  beforeEach(() => {
    resetMemorySyncActivityForTests();
  });

  // Expose mock so tests can control what listMemorySources returns.
  let listMemorySources: ReturnType<typeof vi.fn>;
  let memorySourcesStatusList: ReturnType<typeof vi.fn>;
  let syncMemorySource: ReturnType<typeof vi.fn>;
  let removeMemorySource: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    const svc = await import('../../../services/memorySourcesService');
    listMemorySources = svc.listMemorySources as ReturnType<typeof vi.fn>;
    memorySourcesStatusList = svc.memorySourcesStatusList as ReturnType<typeof vi.fn>;
    syncMemorySource = svc.syncMemorySource as ReturnType<typeof vi.fn>;
    removeMemorySource = svc.removeMemorySource as ReturnType<typeof vi.fn>;
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('shows two sources as syncing when two concurrent sync-stage events arrive (RC#1)', async () => {
    const sources = [makeSource('src-alpha'), makeSource('src-beta')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);

    // Wait for sources to render.
    await waitFor(() => {
      expect(screen.getByText('Source src-alpha')).toBeInTheDocument();
      expect(screen.getByText('Source src-beta')).toBeInTheDocument();
    });

    // Dispatch two concurrent "requested" events with different source_ids.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'requested',
          source_id: 'src-alpha',
          connection_id: 'src-alpha',
        })
      );
      window.dispatchEvent(
        makeSyncStageEvent({ stage: 'requested', source_id: 'src-beta', connection_id: 'src-beta' })
      );
    });

    // Both rows should show the syncing spinner/text (sync.syncing key → "sync.syncing").
    await waitFor(() => {
      const syncingButtons = screen.getAllByText('sync.syncing');
      expect(syncingButtons).toHaveLength(2);
    });
  });

  it('clears only one source when completed, leaving the other syncing (RC#1)', async () => {
    const sources = [makeSource('src-alpha'), makeSource('src-beta')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);

    await waitFor(() => {
      expect(screen.getByText('Source src-alpha')).toBeInTheDocument();
    });

    // Both start syncing.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'requested',
          source_id: 'src-alpha',
          connection_id: 'src-alpha',
        })
      );
      window.dispatchEvent(
        makeSyncStageEvent({ stage: 'requested', source_id: 'src-beta', connection_id: 'src-beta' })
      );
    });

    await waitFor(() => {
      expect(screen.getAllByText('sync.syncing')).toHaveLength(2);
    });

    // Complete src-alpha — only src-beta should remain syncing.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-alpha',
          connection_id: 'src-alpha',
        })
      );
    });

    await waitFor(() => {
      const syncingButtons = screen.getAllByText('sync.syncing');
      expect(syncingButtons).toHaveLength(1);
    });

    // The one remaining syncing button should be for src-beta.
    // The sync button for src-alpha should now show "sync.sync".
    const syncButtons = screen.getAllByText('sync.sync');
    expect(syncButtons).toHaveLength(1); // src-alpha back to idle
  });

  it('failed event also clears the syncing source (RC#1)', async () => {
    const sources = [makeSource('src-gamma')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);

    await waitFor(() => {
      expect(screen.getByText('Source src-gamma')).toBeInTheDocument();
    });

    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'fetching', source_id: 'src-gamma' }));
    });

    await waitFor(() => {
      expect(screen.getByText('sync.syncing')).toBeInTheDocument();
    });

    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'failed', source_id: 'src-gamma' }));
    });

    await waitFor(() => {
      expect(screen.queryByText('sync.syncing')).not.toBeInTheDocument();
      expect(screen.getByText('sync.sync')).toBeInTheDocument();
    });
  });

  it('matches events by source_id when present, ignoring connection_id (RC#2)', async () => {
    const sources = [makeSource('src-new')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);

    await waitFor(() => {
      expect(screen.getByText('Source src-new')).toBeInTheDocument();
    });

    // Event with source_id matching the row but a different connection_id
    // (e.g. an intermediate stage from the bridge where connection_id = document_id).
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'ingesting',
          source_id: 'src-new', // matches the row
          connection_id: 'mem_src:src-new:doc-123', // ingest-pipeline id, NOT the row id
        })
      );
    });

    // Row should light up because source_id matches.
    await waitFor(() => {
      expect(screen.getByText('sync.syncing')).toBeInTheDocument();
    });
  });

  it('falls back to connection_id when source_id is absent (RC#2 backward compat)', async () => {
    const sources = [makeSource('src-legacy')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);

    await waitFor(() => {
      expect(screen.getByText('Source src-legacy')).toBeInTheDocument();
    });

    // Old-style event: no source_id, connection_id is the row id.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'fetching',
          // source_id absent
          connection_id: 'src-legacy',
        })
      );
    });

    await waitFor(() => {
      expect(screen.getByText('sync.syncing')).toBeInTheDocument();
    });
  });

  it('ignores events with neither source_id nor connection_id', async () => {
    const sources = [makeSource('src-quiet')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);

    await waitFor(() => {
      expect(screen.getByText('Source src-quiet')).toBeInTheDocument();
    });

    act(() => {
      window.dispatchEvent(
        new CustomEvent('openhuman:memory-sync-stage', {
          detail: { stage: 'fetching' }, // no source_id, no connection_id
        })
      );
    });

    // Row should remain idle.
    await waitFor(() => {
      expect(screen.queryByText('sync.syncing')).not.toBeInTheDocument();
    });
  });

  // ── Terminal result chips (#3295) ──────────────────────────────────────────

  it('shows "N items synced" after a completed event with a non-zero count', async () => {
    const sources = [makeSource('src-done')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-done')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'fetching', source_id: 'src-done' }));
    });
    await waitFor(() => expect(screen.getByText('sync.syncing')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-done',
          detail: 'ingested 5 item(s)',
        })
      );
    });

    // The result chip persists and shows the parsed count + the (mocked) i18n key.
    await waitFor(() => {
      const chip = screen.getByTestId('memory-source-result-src-done');
      expect(chip).toHaveTextContent('5');
      expect(chip).toHaveTextContent('memorySources.sync.itemsSynced');
    });
    // The progress bar / syncing state is gone.
    expect(screen.queryByText('sync.syncing')).not.toBeInTheDocument();
  });

  it('shows "Up to date" after a completed event with zero new items', async () => {
    const sources = [makeSource('src-noop')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-noop')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-noop',
          detail: 'ingested 0 item(s)',
        })
      );
    });

    await waitFor(() => {
      const chip = screen.getByTestId('memory-source-result-src-noop');
      expect(chip).toHaveTextContent('memorySources.sync.upToDate');
    });
  });

  it('shows the failure reason after a failed event', async () => {
    const sources = [makeSource('src-bad')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-bad')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'failed',
          source_id: 'src-bad',
          detail: 'composio sync failed: rate limit exceeded',
        })
      );
    });

    await waitFor(() => {
      const chip = screen.getByTestId('memory-source-result-src-bad');
      expect(chip).toHaveTextContent('memorySources.sync.failedLabel');
      expect(chip).toHaveTextContent('rate limit exceeded');
    });
  });

  it('clears a prior result chip when a new sync starts for that row', async () => {
    const sources = [makeSource('src-redo')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-redo')).toBeInTheDocument());

    // First sync completes → chip shown.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-redo',
          detail: 'ingested 2 item(s)',
        })
      );
    });
    await waitFor(() =>
      expect(screen.getByTestId('memory-source-result-src-redo')).toBeInTheDocument()
    );

    // A new sync starts (non-terminal event) → chip cleared, progress shown.
    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'fetching', source_id: 'src-redo' }));
    });
    await waitFor(() => {
      expect(screen.queryByTestId('memory-source-result-src-redo')).not.toBeInTheDocument();
      expect(screen.getByText('sync.syncing')).toBeInTheDocument();
    });
  });

  it('shows a failed chip when the sync RPC itself rejects (no stage event arrives)', async () => {
    const sources = [makeSource('src-rpcfail')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);
    syncMemorySource.mockRejectedValueOnce(new Error('core unreachable'));

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-rpcfail')).toBeInTheDocument());

    // Click the row's sync button (folder kind → testid suffix "folder").
    fireEvent.click(screen.getByTestId('memory-source-sync-folder'));

    await waitFor(() => {
      const chip = screen.getByTestId('memory-source-result-src-rpcfail');
      expect(chip).toHaveTextContent('memorySources.sync.failedLabel');
      expect(chip).toHaveTextContent('core unreachable');
    });
    // The optimistic syncing state is cleared after the RPC rejection.
    expect(screen.queryByText('sync.syncing')).not.toBeInTheDocument();
  });
  it('says the budget is spent instead of "Up to date" when zero items arrive for that reason', async () => {
    // The core writes why a run stopped after the count; a spent budget with
    // zero new items used to read as "Up to date" — the opposite of what
    // happened (openhuman#6012 follow-up).
    const sources = [makeSource('src-budget')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);
    const onToast = vi.fn();

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} onToast={onToast} />);
    await waitFor(() => expect(screen.getByText('Source src-budget')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-budget',
          detail:
            "ingested 0 item(s), more pending — Sync again to continue; today's provider request budget is spent",
        })
      );
    });

    await waitFor(() => {
      const chip = screen.getByTestId('memory-source-result-src-budget');
      expect(chip).toHaveTextContent('memorySources.sync.budgetSpent');
      expect(chip).not.toHaveTextContent('memorySources.sync.upToDate');
    });
    expect(onToast).toHaveBeenCalledWith(
      expect.objectContaining({ type: 'warning', message: 'memorySources.sync.budgetSpent' })
    );
  });

  it('keeps the budget note beside a nonzero count in the toast', async () => {
    // A pass that filed some mail and then ran out for the day is the common
    // partial case; "N items synced" alone read as a finished sync.
    const sources = [makeSource('src-partial')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);
    const onToast = vi.fn();

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} onToast={onToast} />);
    await waitFor(() => expect(screen.getByText('Source src-partial')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-partial',
          detail: "ingested 3 item(s); today's provider request budget is spent",
        })
      );
    });

    await waitFor(() =>
      expect(onToast).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'warning',
          message: '3 memorySources.sync.itemsSynced — memorySources.sync.budgetSpent',
        })
      )
    );
  });

  it('shows the "more to sync" note beside a count when the run stopped at its cap', async () => {
    const sources = [makeSource('src-more')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-more')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-more',
          detail: 'ingested 100 item(s), more pending — Sync again to continue',
        })
      );
    });

    await waitFor(() => {
      expect(screen.getByTestId('memory-source-result-src-more')).toHaveTextContent(
        '100 memorySources.sync.itemsSynced'
      );
      expect(screen.getByTestId('memory-source-note-src-more')).toHaveTextContent(
        'memorySources.sync.morePending'
      );
    });
  });

  it('repairs older memories only after a preview and a confirmation', async () => {
    // Backfill has no other entry point in the app (openhuman#6012). The real
    // pass embeds every document it files, so the button previews (dry run)
    // and asks before it writes anything.
    const memoryTree = await import('../../../utils/tauriCommands/memoryTree');
    const backfill = memoryTree.memoryTreeBackfillConnectorTrees as ReturnType<typeof vi.fn>;
    backfill
      .mockResolvedValueOnce({
        executed: false,
        scanned: 42,
        ingested: 0,
        already_present: 0,
        skipped: 0,
        more_pending: false,
        notes: [],
      })
      .mockResolvedValueOnce({
        executed: true,
        scanned: 42,
        ingested: 40,
        already_present: 1,
        skipped: 1,
        more_pending: false,
        notes: [],
      });
    listMemorySources.mockResolvedValue([makeSource('src-a')]);
    memorySourcesStatusList.mockResolvedValue([]);
    const onToast = vi.fn();

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} onToast={onToast} />);
    await waitFor(() => expect(screen.getByText('Source src-a')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('repair-memories-button'));
    await waitFor(() => expect(backfill).toHaveBeenCalledWith({ dryRun: true }));
    // The confirmation names the preview count and nothing has been written.
    await waitFor(() =>
      expect(screen.getByText('memorySources.repair.confirm')).toBeInTheDocument()
    );
    expect(backfill).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByText('memorySources.repair.confirm'));
    await waitFor(() => expect(backfill).toHaveBeenCalledWith({ dryRun: false }));
    // The successful domain outcome is tracked with the privacy-safe count only.
    await waitFor(() =>
      expect(mockTrackAnalyticsEvent).toHaveBeenCalledWith('memory_repair_succeeded', { count: 40 })
    );
    // The i18n mock returns keys, so the placeholders in the summary are
    // not substituted here; the counts are covered by the wrapper's tests.
    await waitFor(() =>
      expect(onToast).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'success', title: 'memorySources.repair.success' })
      )
    );
  });

  it('does not ask when the preview finds nothing to repair', async () => {
    const memoryTree = await import('../../../utils/tauriCommands/memoryTree');
    const backfill = memoryTree.memoryTreeBackfillConnectorTrees as ReturnType<typeof vi.fn>;
    backfill.mockReset();
    backfill.mockResolvedValueOnce({
      executed: false,
      scanned: 0,
      ingested: 0,
      already_present: 0,
      skipped: 0,
      more_pending: false,
      notes: [],
    });
    listMemorySources.mockResolvedValue([makeSource('src-b')]);
    memorySourcesStatusList.mockResolvedValue([]);
    const onToast = vi.fn();

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} onToast={onToast} />);
    await waitFor(() => expect(screen.getByText('Source src-b')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('repair-memories-button'));
    await waitFor(() =>
      expect(onToast).toHaveBeenCalledWith(
        expect.objectContaining({ title: 'memorySources.repair.nothing' })
      )
    );
    expect(screen.queryByText('memorySources.repair.confirm')).not.toBeInTheDocument();
    expect(backfill).toHaveBeenCalledTimes(1);
  });
  it('says "more to sync" — not "Up to date" — when zero items arrived because the run stopped at its cap', async () => {
    // A capped run can write nothing (everything on the page was already
    // ingested) and still have more to read. Zero + more-pending used to
    // render as "Up to date" beside a "more to sync" note — contradictory.
    const sources = [makeSource('src-zero-more')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-zero-more')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-zero-more',
          detail: 'ingested 0 item(s), more pending — Sync again to continue',
        })
      );
    });

    await waitFor(() => {
      const chip = screen.getByTestId('memory-source-result-src-zero-more');
      expect(chip).toHaveTextContent('memorySources.sync.morePending');
      expect(chip).not.toHaveTextContent('memorySources.sync.upToDate');
    });
    expect(screen.queryByTestId('memory-source-note-src-zero-more')).not.toBeInTheDocument();
  });

  it('keeps the live bar and then the result across an unmount and remount (openhuman#6019)', async () => {
    // Leaving Brain › Sources mid-sync used to drop the bar and miss the
    // terminal event: the sync kept running in the core, the screen said idle.
    listMemorySources.mockResolvedValue([makeSource('src-persist')]);
    memorySourcesStatusList.mockResolvedValue([]);

    const first = renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-persist')).toBeInTheDocument());
    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'running', source_id: 'src-persist' }));
    });
    await waitFor(() => expect(screen.getByText('running')).toBeInTheDocument());

    first.unmount();
    // The run goes on with no screen listening.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({ stage: 'fetching', source_id: 'src-persist', detail: '1/3 pages' })
      );
    });

    const second = renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-persist')).toBeInTheDocument());
    expect(screen.getByText('fetching')).toBeInTheDocument();

    second.unmount();
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'src-persist',
          detail: 'ingested 12 item(s)',
        })
      );
    });
    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() =>
      expect(screen.getByTestId('memory-source-result-src-persist')).toHaveTextContent(
        '12 memorySources.sync.itemsSynced'
      )
    );
  });

  it('shows a run the core reports in flight on a cold mount (openhuman#6019)', async () => {
    // An app reload mid-sync: no stage event ever reached this page, but the
    // status list says the core is still running the source.
    listMemorySources.mockResolvedValue([makeSource('src-cold')]);
    memorySourcesStatusList.mockResolvedValue([
      {
        source_id: 'src-cold',
        chunks_synced: 0,
        chunks_pending: 0,
        last_chunk_at_ms: null,
        freshness: 'idle',
        sync_stage: 'running',
        sync_detail: null,
      },
    ]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('running')).toBeInTheDocument());
    expect(screen.getByText('sync.syncing')).toBeInTheDocument();
  });

  // ── U3: scoped source_id resolution + poll reconciliation ─────────────────

  it('matches a row when the event carries a scoped source_id (U3)', async () => {
    const sources = [makeSource('src_scoped')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src_scoped')).toBeInTheDocument());

    // The core attributes folder syncs with a scoped id; the row is keyed by the
    // bare id. Before the fix this keyed the progress map by the scoped string
    // and the row never lit up.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'fetching',
          source_id: 'workspace:folder:src_scoped',
          connection_id: 'src_scoped',
        })
      );
    });

    await waitFor(() => expect(screen.getByText('sync.syncing')).toBeInTheDocument());
  });

  it('leaves the syncing state when the terminal event carries a scoped source_id (U3)', async () => {
    const sources = [makeSource('src_scoped')];
    listMemorySources.mockResolvedValue(sources);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src_scoped')).toBeInTheDocument());

    // Manual sync adds the *bare* id optimistically.
    fireEvent.click(screen.getByTestId('memory-source-sync-folder'));
    await waitFor(() => expect(screen.getByText('sync.syncing')).toBeInTheDocument());

    // The terminal event names the row with the scoped id. Before the fix the
    // Set delete missed and the row stayed "Syncing…" until remount.
    act(() => {
      window.dispatchEvent(
        makeSyncStageEvent({
          stage: 'completed',
          source_id: 'workspace:folder:src_scoped',
          connection_id: 'src_scoped',
          detail: 'ingested 3 item(s)',
        })
      );
    });

    await waitFor(() => {
      expect(screen.queryByText('sync.syncing')).not.toBeInTheDocument();
      expect(screen.getByText('sync.sync')).toBeInTheDocument();
    });
  });

  it('drops a syncing id the refreshed source list no longer names (U3 safety net)', async () => {
    listMemorySources.mockResolvedValue([makeSource('src-ghost')]);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={10} />);
    await waitFor(() => expect(screen.getByText('Source src-ghost')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'fetching', source_id: 'src-ghost' }));
    });
    await waitFor(() => expect(screen.getByText('sync.syncing')).toBeInTheDocument());

    // The source leaves the list (removed elsewhere) — the terminal event for it
    // will never arrive, so the poll has to reconcile.
    listMemorySources.mockResolvedValue([]);
    await waitFor(() => expect(screen.queryByText('Source src-ghost')).not.toBeInTheDocument());

    // It comes back: with the reconciliation the row is idle, without it the
    // stale id is still in the set and the row renders "Syncing…" forever.
    listMemorySources.mockResolvedValue([makeSource('src-ghost')]);
    await waitFor(() => expect(screen.getByText('Source src-ghost')).toBeInTheDocument());
    expect(screen.queryByText('sync.syncing')).not.toBeInTheDocument();
    expect(screen.getByText('sync.sync')).toBeInTheDocument();
  });

  // The safety net's own failure mode: `listMemorySources` answers `[]` when
  // the RPC fails, and reconciling against that reads as "no source exists" —
  // one dropped call would take down every running sync.
  it('keeps a running sync when the source list request fails (U3 safety net)', async () => {
    listMemorySources.mockResolvedValue([makeSource('src-live')]);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={10} />);
    await waitFor(() => expect(screen.getByText('Source src-live')).toBeInTheDocument());

    act(() => {
      window.dispatchEvent(makeSyncStageEvent({ stage: 'fetching', source_id: 'src-live' }));
    });
    await waitFor(() => expect(screen.getByText('sync.syncing')).toBeInTheDocument());

    // The poll's list call fails. The row goes away with the list, but the run
    // is still in flight — so when the next poll succeeds it must come back
    // syncing, not idle.
    listMemorySources.mockRejectedValue(new Error('rpc down'));
    await waitFor(() => expect(screen.queryByText('Source src-live')).not.toBeInTheDocument());

    listMemorySources.mockResolvedValue([makeSource('src-live')]);
    await waitFor(() => expect(screen.getByText('Source src-live')).toBeInTheDocument());
    expect(screen.getByText('sync.syncing')).toBeInTheDocument();
  });

  // ── U18: removing a source is confirmed ───────────────────────────────────

  it('asks for confirmation before removing a source, and removes on confirm (U18)', async () => {
    listMemorySources.mockResolvedValue([makeSource('src-del')]);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-del')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'memorySources.remove' }));

    // The RPC must not fire on the click alone.
    expect(removeMemorySource).not.toHaveBeenCalled();

    const confirm = await screen.findByRole('button', { name: 'common.remove' });
    fireEvent.click(confirm);

    await waitFor(() => expect(removeMemorySource).toHaveBeenCalledWith('src-del'));
  });

  it('does not remove a source when the confirmation is cancelled (U18)', async () => {
    listMemorySources.mockResolvedValue([makeSource('src-keep')]);
    memorySourcesStatusList.mockResolvedValue([]);

    renderWithProviders(<MemorySourcesRegistry pollIntervalMs={0} />);
    await waitFor(() => expect(screen.getByText('Source src-keep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'memorySources.remove' }));

    const cancel = await screen.findByRole('button', { name: 'common.cancel' });
    fireEvent.click(cancel);

    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'common.remove' })).not.toBeInTheDocument()
    );
    expect(removeMemorySource).not.toHaveBeenCalled();
  });
});

// ── U3: row-id resolution unit tests ────────────────────────────────────────

describe('resolveSyncRowId', () => {
  const known = new Set(['src_abc', 'src-legacy']);

  it('strips a <kind>:<subkind>: scope prefix down to the listed row id', () => {
    expect(resolveSyncRowId({ source_id: 'workspace:folder:src_abc' }, known)).toBe('src_abc');
  });

  it('prefers a bare source_id that already names a row', () => {
    expect(
      resolveSyncRowId({ source_id: 'src_abc', connection_id: 'mem_src:src_abc:doc-1' }, known)
    ).toBe('src_abc');
  });

  it('falls back to connection_id when source_id names no row', () => {
    expect(resolveSyncRowId({ source_id: null, connection_id: 'src-legacy' }, known)).toBe(
      'src-legacy'
    );
  });

  it('returns the raw source_id when nothing matches (list not loaded yet)', () => {
    expect(resolveSyncRowId({ source_id: 'src-unknown' }, new Set())).toBe('src-unknown');
  });

  it('returns null when the event names no row at all', () => {
    expect(resolveSyncRowId({}, known)).toBeNull();
    expect(resolveSyncRowId(null, known)).toBeNull();
  });
});

describe('stripSourceScopePrefix', () => {
  it('drops the first two colon-separated segments', () => {
    expect(stripSourceScopePrefix('workspace:folder:src_abc')).toBe('src_abc');
    expect(stripSourceScopePrefix('mem_src:src_abc:https://x/y')).toBe('https://x/y');
  });

  it('returns null when there is no two-segment prefix', () => {
    expect(stripSourceScopePrefix('src_abc')).toBeNull();
    expect(stripSourceScopePrefix('github:owner/repo')).toBeNull();
    expect(stripSourceScopePrefix('workspace:folder:')).toBeNull();
  });
});

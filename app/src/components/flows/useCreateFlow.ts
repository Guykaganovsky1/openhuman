/**
 * `useCreateFlow` (Phase 4a/4c) — shared create-and-open logic for the
 * new-workflow chooser, the template gallery, and the Workflows empty state.
 * Persists a candidate `WorkflowGraph` via `flows_create`, forces it disabled
 * (see below) and, on success, navigates into the editable canvas at
 * `/flows/:id`. Single-flight: a second call while one is in flight is
 * ignored, so a double-click can't create two flows.
 *
 * `busyKey` identifies which affordance is mid-create (a template id, or
 * `'blank'` for start-from-scratch) so a caller can show the spinner on just
 * that card/button. On failure the key clears and `error` is set to the
 * localized `flows.chooser.createError` message, leaving the surface open to
 * retry.
 */
import createDebug from 'debug';
import { useCallback, useState } from 'react';
import { useNavigate } from 'react-router-dom';

import type { WorkflowGraph } from '../../lib/flows/types';
import { useT } from '../../lib/i18n/I18nContext';
import { createFlow, setFlowEnabled } from '../../services/api/flowsApi';

const log = createDebug('app:flows:create');

/** Sentinel `busyKey` for the "start from scratch" path (not a template id). */
export const BLANK_FLOW_KEY = 'blank';

interface UseCreateFlow {
  /** Persist `graph` under `name`, then navigate into its canvas. `key` tags the busy affordance. */
  create: (key: string, name: string, graph: WorkflowGraph) => Promise<void>;
  /** The `key` of the create currently in flight, or `null`. */
  busyKey: string | null;
  /** Localized create-failure message, or `null`. */
  error: string | null;
  /** Clear the error banner (e.g. when the user switches views). */
  clearError: () => void;
}

export function useCreateFlow(): UseCreateFlow {
  const navigate = useNavigate();
  const { t } = useT();
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const create = useCallback(
    async (key: string, name: string, graph: WorkflowGraph) => {
      if (busyKey) {
        log('create: ignored — already creating key=%s', busyKey);
        return;
      }
      log('create: key=%s name=%s nodes=%d', key, name, graph.nodes.length);
      setBusyKey(key);
      setError(null);
      try {
        const flow = await createFlow(name, graph);
        // Born disabled. `flows_create` persists a manual-trigger graph
        // `enabled: true` (B29 Rule 1 only force-disables automatic triggers),
        // so "Start from scratch" and every template card armed a workflow the
        // user had not written a single node into yet, before the canvas even
        // opened. Arming is the user's call, made from the canvas after a save.
        //
        // Two writes, not one transaction — the same shape the agent-side
        // `create_workflow` tool uses (`flows/builder_tools_part_02.rs`), and
        // with the same brief window where the row is enabled in between.
        // `flows_create` takes no `enabled` parameter, so this is the only way
        // to express it from a client.
        //
        // A failed disable is NOT a failed create: the flow exists, and
        // reporting `createError` here would leave an armed orphan behind a
        // message saying nothing was created. Log it and still open the canvas,
        // where the enable toggle shows the flow's real state.
        if (flow.enabled) {
          log('create: force-disabling the new flow id=%s (born enabled)', flow.id);
          try {
            await setFlowEnabled(flow.id, false);
          } catch (disableErr) {
            log(
              'create: could not disable the new flow id=%s — it remains ENABLED err=%o',
              flow.id,
              disableErr
            );
          }
        }
        log('create: created id=%s — navigating to canvas', flow.id);
        navigate(`/flows/${flow.id}`);
      } catch (err) {
        log('create: failed key=%s err=%o', key, err);
        setError(t('flows.chooser.createError'));
        setBusyKey(null);
      }
    },
    [busyKey, navigate, t]
  );

  const clearError = useCallback(() => setError(null), []);

  return { create, busyKey, error, clearError };
}

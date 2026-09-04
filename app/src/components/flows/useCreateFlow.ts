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
  /**
   * Localized failure message, or `null`. Covers both a failed create and a
   * create whose "born disabled" repair did not stick — the second one names
   * the armed workflow explicitly, because that is the state the user would
   * otherwise not know to look for.
   */
  error: string | null;
  /** Clear the error banner (e.g. when the user switches views). */
  clearError: () => void;
}

/**
 * Turn a freshly created flow off, retrying once.
 *
 * One retry, not a loop: the failure this is written for is a transient RPC
 * against a core that is up, and a core that is genuinely refusing the write
 * will refuse the third attempt too — at which point the honest move is to
 * tell the user, not to keep the button spinning.
 */
async function disableWithOneRetry(id: string): Promise<boolean> {
  for (const attempt of [1, 2]) {
    try {
      await setFlowEnabled(id, false);
      return true;
    } catch (err) {
      log('create: disable attempt %d failed id=%s err=%o', attempt, id, err);
    }
  }
  log('create: could not disable the new flow id=%s — it remains ENABLED', id);
  return false;
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
        // A failed disable is NOT a failed create — the flow exists, so
        // reporting `createError` would leave an armed orphan behind a message
        // saying nothing was created. It is not nothing either: an armed
        // workflow with no nodes in it can fire before the user has looked at
        // it, and navigating to the canvas does not stop that. So it gets one
        // retry (the common failure is a transient RPC), and a persistent
        // failure stops the flow of the UI and says plainly that the workflow
        // is running — the one state the user cannot discover on their own.
        if (flow.enabled) {
          log('create: force-disabling the new flow id=%s (born enabled)', flow.id);
          if (!(await disableWithOneRetry(flow.id))) {
            setError(t('flows.chooser.createdButArmed'));
            setBusyKey(null);
            return;
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

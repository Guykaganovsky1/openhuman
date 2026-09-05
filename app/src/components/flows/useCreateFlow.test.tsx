import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { WorkflowGraph } from '../../lib/flows/types';
import { createFlow, setFlowEnabled } from '../../services/api/flowsApi';
import { useCreateFlow } from './useCreateFlow';

const navigate = vi.fn();

vi.mock('react-router-dom', () => ({ useNavigate: () => navigate }));
vi.mock('../../lib/i18n/I18nContext', () => ({ useT: () => ({ t: (k: string) => k }) }));
vi.mock('../../services/api/flowsApi', () => ({ createFlow: vi.fn(), setFlowEnabled: vi.fn() }));

const GRAPH = { nodes: [], edges: [] } as unknown as WorkflowGraph;

/** What the RPC answers with: the persisted flow, whose `enabled` is the truth. */
const disabled = { id: 'f1', enabled: false } as never;
const stillArmed = { id: 'f1', enabled: true } as never;

describe('useCreateFlow', () => {
  beforeEach(() => {
    navigate.mockReset();
    vi.mocked(createFlow).mockReset();
    vi.mocked(setFlowEnabled).mockReset();
    vi.mocked(createFlow).mockResolvedValue({ id: 'f1', enabled: true } as never);
  });

  it('opens the canvas once the new flow has been turned off', async () => {
    vi.mocked(setFlowEnabled).mockResolvedValue(disabled);
    const { result } = renderHook(() => useCreateFlow());

    await act(async () => {
      await result.current.create('blank', 'New', GRAPH);
    });

    expect(setFlowEnabled).toHaveBeenCalledWith('f1', false);
    expect(navigate).toHaveBeenCalledWith('/flows/f1');
    expect(result.current.error).toBeNull();
  });

  // The transient case: a single failed RPC must not cost the user a warning
  // about an armed workflow that is, after the retry, not armed at all.
  it('retries a failed disable once before giving up', async () => {
    vi.mocked(setFlowEnabled)
      .mockRejectedValueOnce(new Error('rpc blip'))
      .mockResolvedValueOnce(disabled);
    const { result } = renderHook(() => useCreateFlow());

    await act(async () => {
      await result.current.create('blank', 'New', GRAPH);
    });

    expect(setFlowEnabled).toHaveBeenCalledTimes(2);
    expect(navigate).toHaveBeenCalledWith('/flows/f1');
    expect(result.current.error).toBeNull();
  });

  // The state the user cannot discover on their own: the flow exists, it is
  // running, and nothing said so. Navigating to the canvas does not stop it.
  it('says the workflow is armed when the disable will not stick', async () => {
    vi.mocked(setFlowEnabled).mockRejectedValue(new Error('core down'));
    const { result } = renderHook(() => useCreateFlow());

    await act(async () => {
      await result.current.create('blank', 'New', GRAPH);
    });

    expect(setFlowEnabled).toHaveBeenCalledTimes(2);
    expect(navigate).not.toHaveBeenCalled();
    await waitFor(() => expect(result.current.error).toBe('flows.chooser.createdButArmed'));
    // The affordance is released, so the user can retry rather than sit on a
    // spinner next to a workflow that is running.
    expect(result.current.busyKey).toBeNull();
  });

  // The failure a resolved promise hides: the call succeeded, the write did
  // not. Only the returned flow's `enabled` says which happened.
  it('treats a response that is still enabled as a failed disable', async () => {
    vi.mocked(setFlowEnabled).mockResolvedValue(stillArmed);
    const { result } = renderHook(() => useCreateFlow());

    await act(async () => {
      await result.current.create('blank', 'New', GRAPH);
    });

    expect(setFlowEnabled).toHaveBeenCalledTimes(2);
    expect(navigate).not.toHaveBeenCalled();
    await waitFor(() => expect(result.current.error).toBe('flows.chooser.createdButArmed'));
  });

  it('reports a failed create as a failed create', async () => {
    vi.mocked(createFlow).mockRejectedValue(new Error('nope'));
    const { result } = renderHook(() => useCreateFlow());

    await act(async () => {
      await result.current.create('blank', 'New', GRAPH);
    });

    expect(navigate).not.toHaveBeenCalled();
    await waitFor(() => expect(result.current.error).toBe('flows.chooser.createError'));
  });
});

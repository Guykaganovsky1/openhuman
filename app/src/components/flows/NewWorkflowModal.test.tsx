/**
 * NewWorkflowModal (Phase 4a) behavior tests.
 *
 * Covers the three chooser paths:
 *  - "Start from scratch" creates a flow whose graph has a single `manual`
 *    trigger node and no edges, then navigates into the new flow's canvas.
 *  - "From a template" reveals the gallery; picking a card calls `flows_create`
 *    with that template's exact graph and navigates into the canvas.
 *  - A `flows_create` rejection surfaces the localized error banner.
 *
 * `react-router-dom`'s `useNavigate` and `flowsApi.createFlow` are mocked so the
 * suite asserts only this component's orchestration.
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { FLOW_TEMPLATES } from '../../lib/flows/templates';
import NewWorkflowModal from './NewWorkflowModal';

const navigate = vi.hoisted(() => vi.fn());
vi.mock('react-router-dom', async orig => ({
  ...(await orig<typeof import('react-router-dom')>()),
  useNavigate: () => navigate,
}));

const createFlow = vi.hoisted(() => vi.fn());
const setFlowEnabled = vi.hoisted(() => vi.fn());
vi.mock('../../services/api/flowsApi', () => ({ createFlow, setFlowEnabled }));

function renderModal() {
  const onClose = vi.fn();
  render(<NewWorkflowModal onClose={onClose} />);
  return { onClose };
}

describe('NewWorkflowModal', () => {
  beforeEach(() => {
    navigate.mockReset();
    createFlow.mockReset();
    setFlowEnabled.mockReset();
    setFlowEnabled.mockResolvedValue({ id: 'flow-new', enabled: false });
  });

  it('start from scratch creates a manual-trigger flow and opens its canvas', async () => {
    createFlow.mockResolvedValue({ id: 'flow-new' });
    renderModal();

    fireEvent.click(screen.getByTestId('new-workflow-scratch'));

    await waitFor(() => expect(createFlow).toHaveBeenCalledTimes(1));
    const [, graph] = createFlow.mock.calls[0];
    expect(graph.nodes).toHaveLength(1);
    expect(graph.nodes[0].kind).toBe('trigger');
    expect(graph.nodes[0].config.trigger_kind).toBe('manual');
    expect(graph.edges).toEqual([]);
    await waitFor(() => expect(navigate).toHaveBeenCalledWith('/flows/flow-new'));
  });

  it('start from scratch leaves the new flow DISABLED, not armed before its first save', async () => {
    // `flows_create` persists a manual-trigger graph `enabled: true` (B29
    // Rule 1 only force-disables automatic triggers), so the chooser used to
    // arm a workflow the user had not put a single node into yet. `flows_create`
    // takes no `enabled` parameter, so the client force-disables the way the
    // agent-side `create_workflow` tool already does.
    createFlow.mockResolvedValue({ id: 'flow-new', enabled: true });
    renderModal();

    fireEvent.click(screen.getByTestId('new-workflow-scratch'));

    await waitFor(() => expect(setFlowEnabled).toHaveBeenCalledWith('flow-new', false));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith('/flows/flow-new'));
  });

  it('does not re-disable a flow the core already persisted disabled', async () => {
    createFlow.mockResolvedValue({ id: 'flow-auto', enabled: false });
    renderModal();

    fireEvent.click(screen.getByTestId('new-workflow-scratch'));

    await waitFor(() => expect(navigate).toHaveBeenCalledWith('/flows/flow-auto'));
    expect(setFlowEnabled).not.toHaveBeenCalled();
  });

  it('warns instead of opening the canvas when the flow could not be turned off', async () => {
    // The flow exists at that point, so this is not a failed create — but it is
    // also not nothing: the workflow is armed and will run. Opening the canvas
    // does not stop that, and nothing else on screen would say it happened.
    createFlow.mockResolvedValue({ id: 'flow-new', enabled: true });
    setFlowEnabled.mockRejectedValue(new Error('store unavailable'));
    renderModal();

    fireEvent.click(screen.getByTestId('new-workflow-scratch'));

    await waitFor(() => expect(screen.getByTestId('new-workflow-error')).toBeInTheDocument());
    // One retry, then the warning — a core that refuses twice will refuse again.
    expect(setFlowEnabled).toHaveBeenCalledTimes(2);
    expect(navigate).not.toHaveBeenCalled();
  });

  it('creating from a template calls flows_create with that template graph', async () => {
    createFlow.mockResolvedValue({ id: 'flow-tpl' });
    renderModal();

    // Open the gallery.
    fireEvent.click(screen.getByTestId('new-workflow-template'));
    expect(screen.getByTestId('flow-template-gallery')).toBeTruthy();

    const template = FLOW_TEMPLATES[0];
    fireEvent.click(screen.getByTestId(`flow-template-${template.id}`));

    await waitFor(() => expect(createFlow).toHaveBeenCalledTimes(1));
    const [, graph] = createFlow.mock.calls[0];
    expect(graph).toBe(template.graph);
    await waitFor(() => expect(navigate).toHaveBeenCalledWith('/flows/flow-tpl'));
  });

  it('does not offer a redundant "Describe it" option (the prompt bar covers it)', () => {
    renderModal();
    expect(screen.queryByTestId('new-workflow-describe')).not.toBeInTheDocument();
  });

  it('can navigate from the gallery back to the chooser', () => {
    renderModal();
    fireEvent.click(screen.getByTestId('new-workflow-template'));
    expect(screen.getByTestId('flow-template-gallery')).toBeTruthy();

    fireEvent.click(screen.getByTestId('new-workflow-gallery-back'));
    expect(screen.getByTestId('new-workflow-scratch')).toBeTruthy();
  });

  it('surfaces an error banner when flows_create rejects', async () => {
    createFlow.mockRejectedValue(new Error('boom'));
    renderModal();

    fireEvent.click(screen.getByTestId('new-workflow-scratch'));

    await waitFor(() => expect(screen.getByTestId('new-workflow-error')).toBeTruthy());
    expect(navigate).not.toHaveBeenCalled();
  });
});

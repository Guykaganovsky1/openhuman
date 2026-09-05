import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import AgentActivityPanel from './AgentActivityPanel';

const navigateBack = vi.fn();

vi.mock('../hooks/useSettingsNavigation', () => ({
  useSettingsNavigation: () => ({
    navigateBack,
    breadcrumbs: [{ label: 'Settings' }, { label: 'Agents' }],
  }),
}));

// Mock SettingsBackButton so the test does not depend on the route-aware
// visibility rules of the shared back button. We assert the panel wiring
// instead: that the back button renders and drives `navigateBack`, and that the
// level options drive the RPC.
vi.mock('../components/SettingsBackButton', () => ({
  default: ({ onBack }: { onBack?: () => void }) => (
    <button type="button" data-testid="settings-header-back" onClick={onBack}>
      back
    </button>
  ),
}));

const callCoreRpc = vi.fn();
vi.mock('../../../services/coreRpcClient', () => ({
  callCoreRpc: (arg: { method: string; params: unknown }) => callCoreRpc(arg),
}));

function settingsResult(level = 2) {
  return {
    result: {
      level,
      level_label: 'moderate',
      sync_interval_secs: 3600,
      heartbeat_enabled: true,
      subconscious_enabled: true,
      token_budget_per_cycle: null,
      estimated_monthly_cost_min_usd: 1,
      estimated_monthly_cost_max_usd: 5,
    },
  };
}

const costResult = { result: { month: '2026-06', total_cost_usd: 0, total_syncs: 0 } };

/**
 * The level options. They are `role="radio"` inside a `role="radiogroup"`
 * (they are a single-choice group), which also separates them from the mocked
 * SettingsHeader back button without filtering by test id.
 */
function levelButtons() {
  return screen.getAllByRole('radio');
}

beforeEach(() => {
  vi.clearAllMocks();
  callCoreRpc.mockImplementation((arg: { method: string }) => {
    switch (arg.method) {
      case 'openhuman.config_get_activity_level_settings':
        return Promise.resolve(settingsResult());
      case 'openhuman.memory_sources_monthly_cost_summary':
        return Promise.resolve(costResult);
      case 'openhuman.config_update_activity_level_settings':
        return Promise.resolve(settingsResult(4));
      default:
        return Promise.reject(new Error(`unexpected method ${arg.method}`));
    }
  });
});

describe('<AgentActivityPanel />', () => {
  it('renders the back button and the five level options once loaded', async () => {
    render(<AgentActivityPanel />);

    // The level options only render after the initial load resolves (the loading
    // state has none), so this also asserts the panel left the loading state.
    await waitFor(() => expect(levelButtons()).toHaveLength(5));
  });

  it('invokes the back handler from the back button', async () => {
    render(<AgentActivityPanel />);
    await screen.findByTestId('settings-header-back');

    fireEvent.click(screen.getByTestId('settings-header-back'));
    expect(navigateBack).toHaveBeenCalledTimes(1);
  });

  it('exposes the level options as a radiogroup with the current level checked', async () => {
    render(<AgentActivityPanel />);
    await waitFor(() => expect(levelButtons()).toHaveLength(5));

    // One group, labelled, wrapping exactly the five options.
    const group = screen.getByRole('radiogroup');
    expect(group).toHaveAttribute('aria-label');
    expect(within(group).getAllByRole('radio')).toHaveLength(5);

    // The loaded level is 2 ("moderate"), the third option, and it is the only
    // one reported as checked.
    const checked = levelButtons().filter(b => b.getAttribute('aria-checked') === 'true');
    expect(checked).toHaveLength(1);
    expect(checked[0]).toHaveAttribute('data-testid', 'activity-level-moderate');
  });

  it('activates a level option from the keyboard', async () => {
    render(<AgentActivityPanel />);
    await waitFor(() => expect(levelButtons()).toHaveLength(5));

    const options = levelButtons();
    options[options.length - 1].focus();
    fireEvent.keyDown(document.activeElement!, { key: 'Enter', code: 'Enter' });
    // jsdom does not synthesise the click a real <button> fires on Enter, so
    // assert the element is a focusable native button (which gets Enter/Space
    // activation from the platform) rather than a div with a hand-rolled role.
    expect(document.activeElement!.tagName).toBe('BUTTON');
    expect(document.activeElement).toHaveAttribute('role', 'radio');
  });

  // A radiogroup is one tab stop, and the arrows move within it. Without this
  // the five cards were five tab stops and the arrow keys did nothing — the
  // role announced a widget the keyboard did not implement.
  it('is a single tab stop whose arrows move and select', async () => {
    render(<AgentActivityPanel />);
    await waitFor(() => expect(levelButtons()).toHaveLength(5));

    // Loaded level is 2 ("moderate", index 2): it is the only tabbable option.
    const tabbable = levelButtons().filter(b => b.getAttribute('tabindex') === '0');
    expect(tabbable).toHaveLength(1);
    expect(tabbable[0]).toHaveAttribute('data-testid', 'activity-level-moderate');

    // ArrowDown selects the next option and takes focus with it.
    fireEvent.keyDown(tabbable[0], { key: 'ArrowDown' });
    await waitFor(() => {
      expect(callCoreRpc).toHaveBeenCalledWith(
        expect.objectContaining({ params: { level: 'active' } })
      );
    });
    expect(document.activeElement).toHaveAttribute('data-testid', 'activity-level-active');

    // Home goes to the first, End to the last — both wrap the whole group.
    callCoreRpc.mockClear();
    fireEvent.keyDown(document.activeElement!, { key: 'Home' });
    await waitFor(() => {
      expect(callCoreRpc).toHaveBeenCalledWith(
        expect.objectContaining({ params: { level: 'off' } })
      );
    });
    expect(document.activeElement).toHaveAttribute('data-testid', 'activity-level-off');

    // End goes to the last option.
    callCoreRpc.mockClear();
    fireEvent.keyDown(document.activeElement!, { key: 'End' });
    await waitFor(() => {
      expect(callCoreRpc).toHaveBeenCalledWith(
        expect.objectContaining({ params: { level: 'always_on' } })
      );
    });
    expect(document.activeElement).toHaveAttribute('data-testid', 'activity-level-alwaysOn');

    // ArrowDown from the last wraps to the first, as a radio group does.
    callCoreRpc.mockClear();
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    await waitFor(() => {
      expect(callCoreRpc).toHaveBeenCalledWith(
        expect.objectContaining({ params: { level: 'off' } })
      );
    });
  });

  it('persists a new level selection via the update RPC', async () => {
    render(<AgentActivityPanel />);
    await waitFor(() => expect(levelButtons()).toHaveLength(5));

    // The last option is "Always-on" (level 4 -> api key "always_on").
    const options = levelButtons();
    fireEvent.click(options[options.length - 1]);

    await waitFor(() => {
      expect(callCoreRpc).toHaveBeenCalledWith(
        expect.objectContaining({
          method: 'openhuman.config_update_activity_level_settings',
          params: { level: 'always_on' },
        })
      );
    });
  });
});

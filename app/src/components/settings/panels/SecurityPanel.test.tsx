/**
 * SecurityPanel — the secret-storage mode badge.
 *
 * The core reports `StorageMode` in snake_case; the i18n table keys the labels
 * in camelCase. Interpolating the raw value rendered the key itself for three
 * of the four modes, so each one is pinned here.
 */
import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { renderWithProviders } from '../../../test/test-utils';
import SecurityPanel from './SecurityPanel';

const useCoreStateMock = vi.fn();
vi.mock('../../../providers/CoreStateProvider', () => ({ useCoreState: () => useCoreStateMock() }));

vi.mock('../../../services/keyringApi', () => ({
  decideKeyringConsent: vi.fn(),
  retryKeyringProbe: vi.fn(),
}));

vi.mock('../hooks/useSettingsNavigation', () => ({
  useSettingsNavigation: () => ({
    navigateBack: vi.fn(),
    navigateToSettings: vi.fn(),
    breadcrumbs: [],
    currentRoute: 'security',
  }),
}));

function renderPanel(activeMode: string) {
  useCoreStateMock.mockReturnValue({
    snapshot: {
      keyringStatus: {
        activeMode,
        backendName: 'test-backend',
        available: true,
        failureReason: null,
      },
    },
  });
  return renderWithProviders(<SecurityPanel />, { preloadedState: { locale: { current: 'en' } } });
}

describe('SecurityPanel storage-mode label', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it.each([
    ['os_keyring', 'OS Keychain'],
    ['local_encrypted', 'Local Encrypted'],
    ['consent_pending', 'Not configured'],
    ['declined', 'Declined'],
  ])('renders a translated label for %s', (mode, label) => {
    renderPanel(mode);

    expect(screen.getByText(label)).toBeInTheDocument();
    // The raw key must never leak into the UI.
    expect(screen.queryByText(`keyring.settings.mode.${mode}`)).not.toBeInTheDocument();
  });

  it('falls back to the raw value for a mode the map does not know', () => {
    renderPanel('some_future_mode');

    expect(screen.getByText('keyring.settings.mode.some_future_mode')).toBeInTheDocument();
  });
});

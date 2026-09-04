/**
 * App — top-level wiring.
 *
 * The external-link guard is installed above the router so it is live for the
 * whole session; without it a link that navigates the single desktop webview
 * strands the user on that page with no route back. Nothing else asserted the
 * wiring existed, so a dropped `useEffect` would have been silent.
 */
import { render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import App from './App';

const hoisted = vi.hoisted(() => ({ installExternalLinkGuard: vi.fn(() => vi.fn()) }));

vi.mock('./utils/externalLinkGuard', () => ({
  installExternalLinkGuard: hoisted.installExternalLinkGuard,
  isExternalNavigation: vi.fn(() => false),
}));

// Cut the tree off immediately below the providers: the guard is wired in
// App's own body, so nothing under the boot gate needs to render.
vi.mock('./components/BootCheckGate/BootCheckGate', () => ({ default: () => null }));

// redux-persist's gate would block on a real persistor rehydrating.
vi.mock('redux-persist/integration/react', () => ({
  PersistGate: ({ children }: { children: React.ReactNode }) => children,
}));

// Module-scope boot services — started on import, not under test here.
vi.mock('./lib/nativeNotifications', () => ({
  startNativeNotificationsService: vi.fn(),
  stopNativeNotificationsService: vi.fn(),
}));
vi.mock('./services/coreHealthMonitor', () => ({
  startCoreHealthMonitor: vi.fn(),
  stopCoreHealthMonitor: vi.fn(),
}));
vi.mock('./services/internetStatusListener', () => ({
  startInternetStatusListener: vi.fn(),
  stopInternetStatusListener: vi.fn(),
}));

describe('App', () => {
  beforeEach(() => {
    hoisted.installExternalLinkGuard.mockClear();
  });

  it('installs the external-link guard once on mount', () => {
    render(<App />);

    expect(hoisted.installExternalLinkGuard).toHaveBeenCalledTimes(1);
  });
});

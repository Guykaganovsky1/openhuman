/**
 * `MockRuntimeProvider` is the runtime under `#/dev/assistant-ui`, the vendored
 * upstream `base` demo. It had no test, and the page being unlinked from any
 * nav is exactly why nothing noticed that it threw on mount.
 *
 * Every route renders under the app-wide `ChatRuntimeProvider`, which installs
 * an assistant runtime of its own. `useRemoteThreadListRuntime` refuses to run
 * inside one unless it is told to:
 *
 *   "useRemoteThreadListRuntime cannot be nested inside another
 *    RemoteThreadListRuntime. Set allowNesting: true to allow nesting (the
 *    inner runtime will become a no-op)."
 *
 * so the page dropped straight into the global error boundary.
 *
 * `@assistant-ui/react` is mocked here with that same guard — the mock throws
 * unless `allowNesting` is set, exactly as the real hook does when nested — so
 * the assertions below are about this component's option, not about the
 * library's internals. The seeded transcript and the canned chat model are
 * mocked away for the same reason.
 */
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

const remoteThreadListCalls: { allowNesting?: boolean }[] = [];

vi.mock('@assistant-ui/react', () => ({
  AssistantRuntimeProvider: ({
    children,
    runtime,
  }: {
    children: React.ReactNode;
    runtime: unknown;
  }) => (
    <div data-testid="assistant-runtime-provider" data-runtime={String(runtime)}>
      {children}
    </div>
  ),
  CompositeAttachmentAdapter: class {},
  InMemoryThreadListAdapter: class {},
  SimpleImageAttachmentAdapter: class {},
  SimpleTextAttachmentAdapter: class {},
  useLocalRuntime: () => 'local-mock-runtime',
  useRemoteThreadListRuntime: (options: { runtimeHook: () => unknown; allowNesting?: boolean }) => {
    remoteThreadListCalls.push({ allowNesting: options.allowNesting });
    // The real hook's nesting guard. `/dev/assistant-ui` always renders inside
    // the app's own runtime, so `isNested` is always true for this page.
    if (!options.allowNesting) {
      throw new Error(
        'useRemoteThreadListRuntime cannot be nested inside another RemoteThreadListRuntime. ' +
          'Set allowNesting: true to allow nesting (the inner runtime will become a no-op).'
      );
    }
    return options.runtimeHook();
  },
}));

vi.mock('../assistant-ui-demo/assistantUiMock', () => ({
  buildSeedMessages: () => [],
  mockChatModelAdapter: {},
}));

const { MockRuntimeProvider } = await import('../assistant-ui-demo/MockRuntimeProvider');

describe('MockRuntimeProvider (#/dev/assistant-ui)', () => {
  it('mounts under the app-wide assistant runtime instead of throwing the nesting error', () => {
    render(
      <MockRuntimeProvider>
        <span data-testid="demo-child">base demo</span>
      </MockRuntimeProvider>
    );

    expect(screen.getByTestId('demo-child')).toBeInTheDocument();
    expect(remoteThreadListCalls.at(-1)?.allowNesting).toBe(true);
  });

  it('still hands the children the mocked local runtime, not the app runtime', () => {
    // The isolation guarantee in `MockRuntimeProvider`'s own docs: mounting the
    // demo "cannot touch a thread, a message, or the core". Allowing nesting
    // must not quietly swap in the ambient runtime.
    render(
      <MockRuntimeProvider>
        <span>base demo</span>
      </MockRuntimeProvider>
    );

    expect(screen.getByTestId('assistant-runtime-provider')).toHaveAttribute(
      'data-runtime',
      'local-mock-runtime'
    );
  });
});

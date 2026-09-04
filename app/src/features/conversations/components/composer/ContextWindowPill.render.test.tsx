/**
 * Rendering tests for the composer's context-window chip.
 *
 * The mapper is covered in `ContextWindowPill.test.ts`; this file covers what
 * the chip actually shows, because the defect it guards is purely visual: with
 * no model context window known (`limit === 0`) the chip used to print `0/—`,
 * a fraction with a placeholder denominator that reads as a broken number
 * rather than as "we don't know the limit yet".
 */
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { type ContextUsage, ContextWindowPill } from './ContextWindowPill';

function usage(overrides: Partial<ContextUsage> = {}): ContextUsage {
  return { used: 0, limit: 0, input: 0, cachedInput: 0, output: 0, costUsd: 0, ...overrides };
}

describe('ContextWindowPill', () => {
  it('renders used/limit when the model context window is known', () => {
    render(<ContextWindowPill usage={usage({ used: 12_000, limit: 128_000 })} />);
    expect(screen.getByRole('button')).toHaveTextContent('12k/128k');
  });

  it('renders only the used count when the limit is unknown', () => {
    render(<ContextWindowPill usage={usage({ used: 0, limit: 0 })} />);
    const pill = screen.getByRole('button');
    expect(pill).toHaveTextContent(/^0$/);
    expect(pill.textContent).not.toContain('/');
    expect(pill.textContent).not.toContain('—');
  });

  it('still shows a non-zero used count with no limit, without a fraction', () => {
    render(<ContextWindowPill usage={usage({ used: 3_400, limit: 0 })} />);
    const pill = screen.getByRole('button');
    expect(pill).toHaveTextContent('3.4k');
    expect(pill.textContent).not.toContain('/');
  });
});

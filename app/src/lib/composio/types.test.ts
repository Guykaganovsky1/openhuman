import { describe, expect, it } from 'vitest';

import { type ComposioConnection, deriveComposioState } from './types';

function connection(status: string): ComposioConnection {
  return { id: `ca_${status.toLowerCase()}`, toolkit: 'gmail', status };
}

describe('deriveComposioState', () => {
  it('treats expired Composio auth as a first-class expired state', () => {
    expect(deriveComposioState(connection('EXPIRED'))).toBe('expired');
  });

  it('treats a revoked grant as its own state, not as never-connected', () => {
    // It used to fall through to `disconnected`, which is what made a revoked
    // account render identically to a toolkit the user never connected.
    expect(deriveComposioState(connection('REVOKED'))).toBe('revoked');
    expect(deriveComposioState(connection('revoked'))).toBe('revoked');
  });

  it('still reports a genuinely absent connection as disconnected', () => {
    expect(deriveComposioState(undefined)).toBe('disconnected');
    expect(deriveComposioState(connection('SOMETHING_ELSE'))).toBe('disconnected');
  });

  it('keeps failed and generic error statuses as error', () => {
    expect(deriveComposioState(connection('FAILED'))).toBe('error');
    expect(deriveComposioState(connection('ERROR'))).toBe('error');
  });

  it('keeps active and pending statuses unchanged', () => {
    expect(deriveComposioState(connection('ACTIVE'))).toBe('connected');
    expect(deriveComposioState(connection('CONNECTED'))).toBe('connected');
    expect(deriveComposioState(connection('PENDING'))).toBe('pending');
  });
});

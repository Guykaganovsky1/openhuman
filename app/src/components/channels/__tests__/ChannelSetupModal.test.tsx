import { screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { FALLBACK_DEFINITIONS } from '../../../lib/channels/definitions';
import { renderWithProviders } from '../../../test/test-utils';
import type { ChannelDefinition } from '../../../types/channels';
import ChannelSetupModal from '../ChannelSetupModal';

// YuanbaoConfig pulls in API + Tauri helpers we don't need for the routing
// branches under test — stub it so we only assert ChannelSetupModal's own
// behavior (icon branch + yuanbao switch case).
vi.mock('../YuanbaoConfig', () => ({
  default: () => <div data-testid="yuanbao-config">Yuanbao Config</div>,
}));

vi.mock('../TelegramConfig', () => ({
  default: () => <div data-testid="telegram-config">Telegram Config</div>,
}));

vi.mock('../DiscordConfig', () => ({
  default: () => <div data-testid="discord-config">Discord Config</div>,
}));

const yuanbaoDef: ChannelDefinition = {
  id: 'yuanbao',
  display_name: '元宝',
  description: '通过元宝（Yuanbao）机器人收发消息。',
  icon: 'yuanbao',
  auth_modes: [
    {
      mode: 'api_key',
      description: '提供元宝开放平台的 AppID 和 AppSecret。',
      fields: [],
      auth_action: undefined,
    },
  ],
  capabilities: ['send_text', 'receive_text'],
};

describe('ChannelSetupModal', () => {
  it('renders the YuanbaoConfig body and brand SVG icon for the yuanbao channel', () => {
    renderWithProviders(<ChannelSetupModal definition={yuanbaoDef} onClose={() => {}} />);
    // Header title + body routing both exercised.
    expect(screen.getByText('元宝')).toBeInTheDocument();
    expect(screen.getByTestId('yuanbao-config')).toBeInTheDocument();
    // YuanbaoIcon emits an aria-hidden SVG in the header; the emoji-based
    // fallback should NOT also render for yuanbao.
    const dialog = screen.getByRole('dialog');
    expect(dialog.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
  });

  it('renders the emoji icon and TelegramConfig body for the telegram channel', () => {
    const telegramDef = FALLBACK_DEFINITIONS.find(d => d.id === 'telegram')!;
    renderWithProviders(<ChannelSetupModal definition={telegramDef} onClose={() => {}} />);
    expect(screen.getByTestId('telegram-config')).toBeInTheDocument();
    // Emoji branch produces a span sibling to the title.
    expect(screen.getByText('\u2708\uFE0F')).toBeInTheDocument();
  });

  it('falls back to the unavailable-channel message for an unknown channel id', () => {
    const unknown: ChannelDefinition = { ...yuanbaoDef, id: 'unknown', display_name: 'Unknown' };
    renderWithProviders(<ChannelSetupModal definition={unknown} onClose={() => {}} />);
    expect(screen.getByText(/Configuration for/i)).toBeInTheDocument();
  });

  // Both channels used to fall through to that same `default:` arm, so opening
  // "Web / Manage" or "iMessage / Setup" from the Connections grid produced an
  // empty sheet reading "Configuration for Web" with no control in it.
  it('tells the truth about Web instead of rendering an empty configuration shell', () => {
    const webDef = FALLBACK_DEFINITIONS.find(d => d.id === 'web')!;
    renderWithProviders(<ChannelSetupModal definition={webDef} onClose={() => {}} />);

    expect(screen.queryByText(/Configuration for/i)).not.toBeInTheDocument();
    // Web is the built-in chat surface: nothing to configure, always on.
    expect(screen.getByText('Always available')).toBeInTheDocument();
  });

  it('renders the real iMessage requirements: the Full Disk Access grant and allowed contacts', () => {
    const imessageDef: ChannelDefinition = {
      id: 'imessage',
      display_name: 'iMessage',
      description: 'Send and receive via macOS Messages (local, AppleScript bridge).',
      icon: 'imessage',
      auth_modes: [
        {
          mode: 'managed_dm',
          description: 'Local-only — no credentials. Grant Full Disk Access to OpenHuman.',
          fields: [
            {
              key: 'allowed_contacts',
              label: 'Allowed Contacts',
              field_type: 'string',
              required: false,
              placeholder: 'Comma-separated phone numbers or emails; * to allow any',
            },
          ],
          auth_action: undefined,
        },
      ],
      capabilities: ['send_text', 'receive_text'],
    };

    renderWithProviders(<ChannelSetupModal definition={imessageDef} onClose={() => {}} />);

    expect(screen.queryByText(/Configuration for/i)).not.toBeInTheDocument();
    // The macOS permission the channel actually depends on — without it the
    // channel connects and then silently receives nothing. It reaches the sheet
    // twice: as the how-to-connect callout and as the auth mode's own
    // description, which is why this counts rather than picking one.
    expect(screen.getAllByText(/Full Disk Access/i).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/System Settings/i)).toBeInTheDocument();
    // The one field the core declares, plus a control that does something.
    expect(screen.getByLabelText(/Allowed Contacts/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect' })).toBeInTheDocument();
  });

  it('invokes onClose when the Escape key is pressed', () => {
    const onClose = vi.fn();
    renderWithProviders(<ChannelSetupModal definition={yuanbaoDef} onClose={onClose} />);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

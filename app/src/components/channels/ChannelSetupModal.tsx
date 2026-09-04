/**
 * Reusable modal for configuring a channel integration (Telegram, Discord, etc.).
 * Built on the shared `ModalShell` primitive (Radix `Dialog` underneath). Can be
 * opened from the Skills page or Settings.
 */
import { useId } from 'react';

import { useT } from '../../lib/i18n/I18nContext';
import type { ChannelDefinition, ChannelType } from '../../types/channels';
import Badge from '../ui/Badge';
import { ModalShell } from '../ui/ModalShell';
import ChannelConnectHelp from './ChannelConnectHelp';
import { renderChannelIcon } from './channelIcon';
import CredentialChannelConfig from './CredentialChannelConfig';
import DiscordConfig from './DiscordConfig';
import TelegramConfig from './TelegramConfig';
import WebChannelConfig from './WebChannelConfig';
import YuanbaoConfig from './YuanbaoConfig';

interface ChannelSetupModalProps {
  definition: ChannelDefinition;
  onClose: () => void;
}

function renderChannelConfig(
  definition: ChannelDefinition,
  channelId: ChannelType,
  t: (key: string, fallback?: string) => string
) {
  // iMessage is a real channel the core ships, but the frontend `ChannelType`
  // union predates it and does not list it, so it cannot be a `case` arm here.
  // It is matched on the raw definition id instead: widening the union would
  // ripple through every `Record<ChannelType, …>` consumer for no behavioural
  // gain. Its definition declares one optional `allowed_contacts` field plus
  // the "grant Full Disk Access" instruction, and `CredentialChannelConfig`
  // renders whatever the core declares; it fell through to the empty
  // "Configuration for iMessage" shell before.
  if (definition.id === 'imessage') {
    return <CredentialChannelConfig definition={definition} />;
  }
  switch (channelId) {
    case 'telegram':
      return <TelegramConfig definition={definition} />;
    case 'discord':
      return <DiscordConfig definition={definition} />;
    case 'yuanbao':
      return <YuanbaoConfig definition={definition} />;
    // Credential-form channels (Lark/DingTalk/Email) render the same generic
    // form here as on the Channels page — otherwise clicking their Skills-grid
    // tile fell through to "config not available" (#4280 review).
    case 'lark':
    case 'dingtalk':
    case 'email':
      return <CredentialChannelConfig definition={definition} />;
    // Web has genuinely nothing to configure — it is the built-in chat surface
    // and is always available. Say that instead of rendering an empty sheet.
    case 'web':
      return <WebChannelConfig definition={definition} />;
    default:
      return (
        <p className="py-4 text-sm text-content-faint">
          {t('channels.configNotAvailable')} {definition.display_name}
        </p>
      );
  }
}

function ChannelConfigContent({ definition }: { definition: ChannelDefinition }) {
  const { t } = useT();
  const channelId = definition.id as ChannelType;
  return (
    <div className="space-y-3">
      <ChannelConnectHelp channelId={channelId} />
      {renderChannelConfig(definition, channelId, t)}
    </div>
  );
}

export default function ChannelSetupModal({ definition, onClose }: ChannelSetupModalProps) {
  const { t } = useT();
  const titleId = useId();

  return (
    <ModalShell
      onClose={onClose}
      titleId={titleId}
      icon={renderChannelIcon(definition.icon)}
      maxWidthClassName="max-w-[500px]"
      contentClassName="max-h-[70vh] overflow-y-auto px-5 py-4"
      title={
        <span className="flex items-center gap-2">
          {definition.display_name}
          <Badge variant="primary">{t('channels.channel')}</Badge>
        </span>
      }
      subtitle={definition.description}>
      <ChannelConfigContent definition={definition} />
    </ModalShell>
  );
}

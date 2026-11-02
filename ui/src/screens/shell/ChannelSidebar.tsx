import { useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import type { Channel, Id, ServerSummary } from '../../core';
import { Avatar, Badge, Code, Name } from '../../components/common';
import { CheckIcon, GearIcon, HashIcon, LinkIcon } from '../../components/icons';

const NO_CHANNELS: Channel[] = [];

export function ChannelSidebar({
  server,
  channelId,
  onSelectChannel,
  onOpenSettings,
}: {
  server: ServerSummary;
  channelId: Id;
  onSelectChannel: (id: Id) => void;
  onOpenSettings: () => void;
}) {
  const store = useAppStore();
  const channels = useStore(store, (s) => s.channels[server.id] ?? NO_CHANNELS);
  const identity = useStore(store, (s) => s.identity)!;
  const intl = useIntl();
  const [copied, setCopied] = useState(false);

  const copyInvite = async () => {
    try {
      await navigator.clipboard.writeText(server.inviteLink);
    } catch {
      // clipboard may be unavailable (e.g. insecure context); the link is still shown in the title
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <aside className="sidebar" aria-label={intl.formatMessage({ id: 'channels.label' })}>
      <header className="sidebar-header">
        <h1 className="server-name">
          <Name>{server.name}</Name>
        </h1>
        <span className="age-tag">
          <FormattedMessage id={`age.${server.ageRestriction === 'none' ? 'none' : server.ageRestriction.slice(0, 2)}`} />
        </span>
        <button
          type="button"
          className="icon-button"
          onClick={copyInvite}
          aria-label={intl.formatMessage({ id: copied ? 'common.copied' : 'server.invite' })}
          title={server.inviteLink}
        >
          {copied ? <CheckIcon /> : <LinkIcon />}
        </button>
      </header>
      <nav className="channel-list" aria-label={intl.formatMessage({ id: 'channels.label' })}>
        <ul>
          {channels.map((c) => {
            const active = c.id === channelId;
            const parts = [
              c.name,
              c.adult ? intl.formatMessage({ id: 'channel.adult' }) : null,
              c.mentions ? intl.formatMessage({ id: 'mentions.label' }, { count: c.mentions }) : null,
              c.unread && !active ? intl.formatMessage({ id: 'unread.label' }) : null,
            ];
            return (
              <li key={c.id}>
                <button
                  type="button"
                  className={`channel ${active ? 'active' : ''} ${c.unread && !active ? 'unread' : ''}`}
                  aria-current={active ? 'page' : undefined}
                  aria-label={parts.filter(Boolean).join(', ')}
                  onClick={() => onSelectChannel(c.id)}
                >
                  <HashIcon />
                  <span className="channel-name">
                    <bdi>{c.name}</bdi>
                  </span>
                  {c.adult && (
                    <span className="adult-tag" aria-hidden="true">
                      18+
                    </span>
                  )}
                  {c.unread && !active && !c.mentions && <span className="unread-dot" aria-hidden="true" />}
                  <Badge count={c.mentions} label={intl.formatMessage({ id: 'mentions.label' }, { count: c.mentions })} />
                </button>
              </li>
            );
          })}
        </ul>
      </nav>
      <footer className="me-panel">
        <Avatar name={identity.displayName || identity.shortCode} color={identity.avatarColor} src={identity.avatarDataUrl} size={32} presence="online" />
        <div className="me-text">
          <span className="me-name">
            <Name>{identity.displayName || identity.shortCode}</Name>
          </span>
          <Code value={identity.shortCode} label={intl.formatMessage({ id: 'chat.code' }, { code: identity.shortCode })} />
        </div>
        <button
          type="button"
          className="icon-button"
          onClick={onOpenSettings}
          aria-label={intl.formatMessage({ id: 'me.settings' })}
          title={intl.formatMessage({ id: 'me.settings' })}
        >
          <GearIcon />
        </button>
      </footer>
    </aside>
  );
}

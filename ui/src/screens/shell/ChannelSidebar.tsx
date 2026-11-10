import { useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import type { Channel, Id, ServerSummary } from '../../core';
import { Avatar, Badge, Code, InviteBox, Name } from '../../components/common';
import { GearIcon, HashIcon, LinkIcon, PlusIcon } from '../../components/icons';
import { CallBar, VoiceRoomEntry, useCallEngine } from './CallBar';
import { useContextMenu, type MenuItem } from '../../components/ContextMenu';
import { copyText } from '../../app/clipboard';
import { useCore } from '../../core';

const NO_CHANNELS: Channel[] = [];

export function ChannelSidebar({
  server,
  channelId,
  onSelectChannel,
  onOpenSettings,
  onCreateChannel,
  onEditChannel,
  onDeleteChannel,
  onOpenServerSettings,
}: {
  server: ServerSummary;
  channelId: Id;
  onSelectChannel: (id: Id) => void;
  onOpenSettings: () => void;
  /** Owner only (B11c). */
  onCreateChannel: () => void;
  onEditChannel: (channel: Channel) => void;
  /** Owner only: the delete confirmation, from the channel menu. */
  onDeleteChannel: (channel: Channel) => void;
  /** Owner only (B11): the server settings screen. */
  onOpenServerSettings: () => void;
}) {
  const store = useAppStore();
  const channels = useStore(store, (s) => s.channels[server.id] ?? NO_CHANNELS);
  const identity = useStore(store, (s) => s.identity)!;
  const intl = useIntl();
  // The invite link, shown selectable with a Copy button (P0 from v0.2.0: the
  // desktop page can't write the clipboard by itself, see app/clipboard.ts).
  const [showInvite, setShowInvite] = useState(false);
  const owner = server.ownerId === identity.id;
  const core = useCore();
  // The one voice engine for my call, shared by the room entry and the call bar.
  const engine = useCallEngine(identity.id);
  const { bind, menu } = useContextMenu();
  // B25 §3: the channel menu. Rename and delete are also behind the channel's gear.
  const channelItems = (c: Channel, active: boolean): MenuItem[] => [
    ...(c.unread && !active ? [{ key: 'read', label: intl.formatMessage({ id: 'menu.markRead' }), onSelect: () => void core.markRead(c.id) }] : []),
    ...(owner ? [{ key: 'rename', label: intl.formatMessage({ id: 'menu.renameChannel' }), onSelect: () => onEditChannel(c) }] : []),
    { kind: 'separator' },
    {
      key: 'copy-id',
      label: intl.formatMessage({ id: 'menu.copyChannelId' }),
      onSelect: async () => store.showFlash((await copyText(c.id)) ? 'common.copied' : 'menu.copyFailed'),
    },
    { kind: 'separator' },
    ...(owner && !c.isDefault
      ? [{ key: 'delete', label: intl.formatMessage({ id: 'channel.delete' }), danger: true, onSelect: () => onDeleteChannel(c) }]
      : []),
  ];

  return (
    <aside className="sidebar" aria-label={intl.formatMessage({ id: 'channels.label' })}>
      <header className="sidebar-header">
        <h1 className="server-name">
          <Name>{server.name}</Name>
        </h1>
        <span className="age-tag">
          <FormattedMessage id={`age.${server.ageRestriction === 'none' ? 'none' : server.ageRestriction.slice(0, 2)}`} />
        </span>
        {owner && (
          <button
            type="button"
            className="icon-button server-settings-button"
            onClick={onOpenServerSettings}
            aria-label={intl.formatMessage({ id: 'server.settings.open' })}
            title={intl.formatMessage({ id: 'server.settings.open' })}
          >
            <GearIcon />
          </button>
        )}
        <button
          type="button"
          className="icon-button"
          onClick={() => setShowInvite((v) => !v)}
          aria-label={intl.formatMessage({ id: 'server.invite' })}
          aria-expanded={showInvite}
          aria-controls="sidebar-invite"
        >
          <LinkIcon />
        </button>
      </header>
      {showInvite && (
        <div id="sidebar-invite" className="sidebar-invite">
          <InviteBox link={server.inviteLink} />
        </div>
      )}
      <nav className="channel-list" aria-label={intl.formatMessage({ id: 'channels.label' })}>
        {owner && (
          <div className="channel-list-header">
            <span>
              <FormattedMessage id="channels.label" />
            </span>
            <button
              type="button"
              className="icon-button small"
              onClick={onCreateChannel}
              aria-label={intl.formatMessage({ id: 'channel.create' })}
              title={intl.formatMessage({ id: 'channel.create' })}
            >
              <PlusIcon />
            </button>
          </div>
        )}
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
              <li key={c.id} className={owner ? 'channel-row has-edit' : 'channel-row'}>
                <button
                  type="button"
                  className={`channel ${active ? 'active' : ''} ${c.unread && !active ? 'unread' : ''}`}
                  aria-current={active ? 'page' : undefined}
                  aria-label={parts.filter(Boolean).join(', ')}
                  onClick={() => onSelectChannel(c.id)}
                  {...bind(intl.formatMessage({ id: 'menu.forChannel' }, { channel: c.name }), () => channelItems(c, active))}
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
                {owner && (
                  <button
                    type="button"
                    className="icon-button small channel-edit"
                    onClick={() => onEditChannel(c)}
                    aria-label={`${intl.formatMessage({ id: 'channel.edit' })}: ${c.name}`}
                  >
                    <GearIcon />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
        <VoiceRoomEntry serverId={server.id} myId={identity.id} engine={engine} />
      </nav>
      <CallBar myId={identity.id} engine={engine} />
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
      {menu}
    </aside>
  );
}

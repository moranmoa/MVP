import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useSettings } from '../../app/settings';
import { phoneQuery, useMediaQuery } from '../../app/media';
import { isCoreError } from '../../core';
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
  // A closed server is read-only (#261): no owner controls there.
  const owner = server.ownerId === identity.id && !server.closed;
  const core = useCore();
  // The one voice engine for my call, shared by the room entry and the call bar.
  const engine = useCallEngine(identity.id);
  const { bind, menu } = useContextMenu();
  const phone = useMediaQuery(phoneQuery(useSettings().settings.uiScale));

  // G-11 (#323): the owner reorders the channels; everyone sees the core's order.
  const canMove = owner && channels.length > 1;
  const [arranging, setArranging] = useState(false);
  const [announce, setAnnounce] = useState('');
  const [drag, setDrag] = useState<{ id: Id; startY: number; active: boolean; target: number } | null>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const focusAfterMove = useRef<Id | null>(null);
  const moveChannel = (c: Channel, to: number, { focus = false } = {}) => {
    const rest = channels.filter((x) => x.id !== c.id);
    const at = Math.max(0, Math.min(rest.length, to));
    if (channels[at]?.id === c.id && channels.indexOf(c) === at) return;
    const before = rest[at]?.id;
    const after = before ? undefined : rest[rest.length - 1]?.id;
    // Right away here; the core's answer (or a re-read on refusal) settles it.
    store.setChannels(server.id, [...rest.slice(0, at), c, ...rest.slice(at)]);
    setAnnounce(intl.formatMessage({ id: 'channelOrder.moved' }, { name: c.name, pos: at + 1, total: channels.length }));
    if (focus) focusAfterMove.current = c.id;
    core
      .moveChannel(server.id, c.id, before, after)
      .then((list) => store.setChannels(server.id, list))
      .catch((e) => {
        store.reloadChannels(server.id);
        store.showFlash(isCoreError(e) && e.code === 'not-allowed' ? 'channelOrder.notAllowed' : 'error.generic');
      });
  };
  useEffect(() => {
    const id = focusAfterMove.current;
    if (!id) return;
    focusAfterMove.current = null;
    listRef.current?.querySelector<HTMLButtonElement>(`button.channel[data-channel-id="${CSS.escape(id)}"]`)?.focus();
  });
  const onChannelKey = (c: Channel, i: number) => (e: KeyboardEvent) => {
    if (!canMove || !e.altKey || !e.shiftKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
    e.preventDefault();
    e.stopPropagation();
    moveChannel(c, i + (e.key === 'ArrowDown' ? 1 : -1), { focus: true });
  };
  // Mouse drag from the handle: 4px, the row dims, a line shows where it lands, Esc cancels.
  const targetAt = (y: number): number => {
    const rows = [...(listRef.current?.querySelectorAll<HTMLElement>('li.channel-row') ?? [])];
    const i = rows.findIndex((li) => {
      const r = li.getBoundingClientRect();
      return y < r.top + r.height / 2;
    });
    return i < 0 ? rows.length : i;
  };
  const onHandleDown = (id: Id) => (e: ReactPointerEvent) => {
    if (e.pointerType !== 'mouse' || e.button !== 0) return;
    e.preventDefault();
    setDrag({ id, startY: e.clientY, active: false, target: -1 });
  };
  useEffect(() => {
    if (!drag) return;
    const onMove = (e: PointerEvent) =>
      setDrag((d) => d && { ...d, active: d.active || Math.abs(e.clientY - d.startY) >= 4, target: targetAt(e.clientY) });
    const onUp = () =>
      setDrag((d) => {
        const c = d?.active ? channels.find((x) => x.id === d.id) : undefined;
        if (c && d) {
          const from = channels.indexOf(c);
          moveChannel(c, d.target > from ? d.target - 1 : d.target);
        }
        return null;
      });
    const onKey = (e: globalThis.KeyboardEvent) => e.key === 'Escape' && setDrag(null);
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('keydown', onKey, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drag?.id]);
  const dragging = drag?.active ? drag : null;

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
    ...(canMove && channels.indexOf(c) > 0
      ? [{ key: 'up', label: intl.formatMessage({ id: 'channelOrder.moveUp' }), onSelect: () => moveChannel(c, channels.indexOf(c) - 1) }]
      : []),
    ...(canMove && channels.indexOf(c) < channels.length - 1
      ? [{ key: 'down', label: intl.formatMessage({ id: 'channelOrder.moveDown' }), onSelect: () => moveChannel(c, channels.indexOf(c) + 1) }]
      : []),
    ...(canMove && phone ? [{ key: 'arrange', label: intl.formatMessage({ id: 'channelOrder.arrange' }), onSelect: () => setArranging(true) }] : []),
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
        <span className="visually-hidden" aria-live="polite">
          {announce}
        </span>
        {arranging && (
          <button type="button" className="primary small channel-arrange-done" autoFocus onClick={() => setArranging(false)}>
            <FormattedMessage id="channelOrder.done" />
          </button>
        )}
        <ul ref={listRef} className={dragging ? 'dragging' : undefined}>
          {channels.map((c, i) => {
            const active = c.id === channelId;
            const parts = [
              c.name,
              c.adult ? intl.formatMessage({ id: 'channel.adult' }) : null,
              c.mentions ? intl.formatMessage({ id: 'mentions.label' }, { count: c.mentions }) : null,
              c.unread && !active ? intl.formatMessage({ id: 'unread.label' }) : null,
            ];
            return (
              <li
                key={c.id}
                className={[
                  'channel-row',
                  owner ? 'has-edit' : '',
                  dragging?.id === c.id ? 'drag-source' : '',
                  dragging && dragging.target === i ? 'drop-before' : '',
                  dragging && i === channels.length - 1 && dragging.target === channels.length ? 'drop-after' : '',
                ]
                  .filter(Boolean)
                  .join(' ')}
              >
                {canMove && !arranging && (
                  <span
                    className="channel-handle"
                    role="img"
                    aria-label={intl.formatMessage({ id: 'channelOrder.handle' })}
                    title={intl.formatMessage({ id: 'channelOrder.handle' })}
                    onPointerDown={onHandleDown(c.id)}
                  >
                    ⠿
                  </span>
                )}
                <button
                  type="button"
                  className={`channel ${active ? 'active' : ''} ${c.unread && !active ? 'unread' : ''}`}
                  aria-current={active ? 'page' : undefined}
                  aria-label={parts.filter(Boolean).join(', ')}
                  data-channel-id={c.id}
                  onClick={() => onSelectChannel(c.id)}
                  {...bind(intl.formatMessage({ id: 'menu.forChannel' }, { channel: c.name }), () => channelItems(c, active))}
                  onKeyDown={(e) => {
                    onChannelKey(c, i)(e);
                    if (!e.defaultPrevented) bind(intl.formatMessage({ id: 'menu.forChannel' }, { channel: c.name }), () => channelItems(c, active)).onKeyDown(e);
                  }}
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
                {arranging && (
                  <span className="channel-arrange">
                    <button
                      type="button"
                      className="icon-button"
                      disabled={i === 0}
                      aria-label={`${intl.formatMessage({ id: 'channelOrder.moveUp' })}: ${c.name}`}
                      onClick={() => moveChannel(c, i - 1)}
                    >
                      ▲
                    </button>
                    <button
                      type="button"
                      className="icon-button"
                      disabled={i === channels.length - 1}
                      aria-label={`${intl.formatMessage({ id: 'channelOrder.moveDown' })}: ${c.name}`}
                      onClick={() => moveChannel(c, i + 1)}
                    >
                      ▼
                    </button>
                  </span>
                )}
                {owner && !arranging && (
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
        {/* A closed server is read-only (#261): no calls; the core ends any call there. */}
        {!server.closed && <VoiceRoomEntry serverId={server.id} myId={identity.id} engine={engine} />}
      </nav>
      <CallBar myId={identity.id} engine={engine} />
      <footer className="me-panel">
        <button
          type="button"
          className="me-profile"
          onClick={() => store.openSettings('account')}
          aria-label={intl.formatMessage({ id: 'profile.edit.open' })}
          title={intl.formatMessage({ id: 'profile.edit.open' })}
        >
          <Avatar name={identity.displayName || identity.shortCode} color={identity.avatarColor} src={identity.avatarDataUrl} size={32} presence="online" />
          <span className="me-text">
            <span className="me-name">
              <Name>{identity.displayName || identity.shortCode}</Name>
            </span>
            <Code value={identity.shortCode} />
          </span>
        </button>
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

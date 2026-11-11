import { useEffect, useRef, useState, type CSSProperties, type HTMLAttributes, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useSettings } from '../../app/settings';
import { phoneQuery, useMediaQuery } from '../../app/media';
import { moveTo, sortByOrder } from '../../app/serverOrder';
import { isCoreError, useCore, type Id, type ServerSummary } from '../../core';
import { GearIcon, HomeIcon, PlusIcon, RefreshIcon } from '../../components/icons';
import { useContextMenu, type MenuItem } from '../../components/ContextMenu';
import { copyText } from '../../app/clipboard';
import { isMutedKey, serverMuteKey } from '../../app/mute';
import { MuteDialog, type MuteTarget } from './MuteDialog';

/** How far the mouse moves before a press becomes a drag (G-01). */
const DRAG_START_PX = 4;

type Drag = { id: Id; startY: number; y: number; x: number; active: boolean; target: number };

export function ServerRail({
  selected,
  onSelect,
  onAdd,
  onOpenSettings,
  onOpenServerSettings,
  onLeaveServer,
}: {
  selected: Id | 'home';
  onSelect: (id: Id | 'home') => void;
  onAdd: () => void;
  onOpenSettings: () => void;
  /** Owner only (B11): opens that server's settings. */
  onOpenServerSettings: (id: Id) => void;
  /** Any member but the owner (#261): opens the leave confirmation. */
  onLeaveServer: (id: Id) => void;
}) {
  const store = useAppStore();
  const joined = useStore(store, (s) => s.servers);
  const update = useStore(store, (s) => s.update);
  const core = useCore();
  const intl = useIntl();
  const { settings, update: updateSettings } = useSettings();
  const phone = useMediaQuery(phoneQuery(settings.uiScale));
  const listRef = useRef<HTMLUListElement>(null);
  const myId = useStore(store, (s) => s.identity?.id);
  const { bind, menu } = useContextMenu();
  const mutes = useStore(store, (s) => s.mutes);
  const [muteTarget, setMuteTarget] = useState<MuteTarget | null>(null);
  const isServerMuted = (s: ServerSummary) => isMutedKey(mutes, serverMuteKey(s.id));
  // G-04: every unread channel of the server, through the core's markRead.
  const markAllRead = (s: ServerSummary) => {
    for (const c of store.getState().channels[s.id] ?? []) if (c.unread || c.mentions) void core.markRead(c.id);
  };

  // G-01: my order, local only. New servers come at the end.
  const servers = sortByOrder(joined, settings.serverOrder ?? []);
  const [announce, setAnnounce] = useState('');
  const [arranging, setArranging] = useState(false);
  const [drag, setDrag] = useState<Drag | null>(null);
  const focusAfterMove = useRef<Id | null>(null);
  const suppressClick = useRef(false);

  const move = (id: Id, to: number, { focus = false } = {}) => {
    const ids = moveTo(
      servers.map((s) => s.id),
      id,
      to,
    );
    if (ids.join() === servers.map((s) => s.id).join()) return;
    updateSettings({ serverOrder: ids });
    const s = servers.find((x) => x.id === id);
    setAnnounce(intl.formatMessage({ id: 'serverOrder.moved' }, { name: s?.name ?? '', pos: ids.indexOf(id) + 1, total: ids.length }));
    if (focus) focusAfterMove.current = id;
  };
  useEffect(() => {
    const id = focusAfterMove.current;
    if (!id) return;
    focusAfterMove.current = null;
    listRef.current?.querySelector<HTMLButtonElement>(`[data-server-id="${CSS.escape(id)}"]`)?.focus();
  });

  // B25 §4: the server menu in the rail. A closed server is read-only: no items.
  const serverItems = (s: ServerSummary): MenuItem[] => {
    if (s.closed) return []; // still movable by keyboard and drag
    const i = servers.findIndex((x) => x.id === s.id);
    const order: MenuItem[] = [
      ...(i > 0 ? [{ key: 'up', label: intl.formatMessage({ id: 'serverOrder.moveUp' }), onSelect: () => move(s.id, i - 1) }] : []),
      ...(i < servers.length - 1 ? [{ key: 'down', label: intl.formatMessage({ id: 'serverOrder.moveDown' }), onSelect: () => move(s.id, i + 1) }] : []),
      ...(phone && servers.length > 1 ? [{ key: 'arrange', label: intl.formatMessage({ id: 'serverOrder.arrange' }), onSelect: () => setArranging(true) }] : []),
    ];
    const rest: MenuItem[] = s.closed
      ? []
      : [
          ...(s.unread || s.mentions ? [{ key: 'read', label: intl.formatMessage({ id: 'unread.markAllRead' }), onSelect: () => markAllRead(s) }] : []),
          isServerMuted(s)
            ? { key: 'unmute', label: intl.formatMessage({ id: 'notify.unmute' }), onSelect: () => store.unmute(serverMuteKey(s.id)) }
            : { key: 'mute', label: intl.formatMessage({ id: 'notify.muteMenu' }), onSelect: () => setMuteTarget({ key: serverMuteKey(s.id), name: s.name }) },
          { kind: 'separator' },
          {
            key: 'invite',
            label: intl.formatMessage({ id: 'menu.copyInvite' }),
            onSelect: async () => store.showFlash((await copyText(s.inviteLink)) ? 'common.copied' : 'menu.copyFailed'),
          },
          ...(s.ownerId === myId ? [{ key: 'settings', label: intl.formatMessage({ id: 'server.settings.open' }), onSelect: () => onOpenServerSettings(s.id) }] : []),
          { kind: 'separator' },
          ...(s.ownerId !== myId ? [{ key: 'leave', label: intl.formatMessage({ id: 'server.leave' }), danger: true, onSelect: () => onLeaveServer(s.id) }] : []),
        ];
    // The order items last, under their own separator.
    return rest.length && order.length ? [...rest, { kind: 'separator' }, ...order] : [...rest, ...order];
  };

  // Arrow keys move between rail buttons (one Tab stop for the whole list);
  // Alt+Shift+arrows move the focused server itself (G-01).
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const dir = e.key === 'ArrowDown' ? 1 : -1;
    if (e.altKey && e.shiftKey) {
      const id = (document.activeElement as HTMLElement | null)?.dataset.serverId;
      const i = servers.findIndex((s) => s.id === id);
      if (id && i >= 0) {
        e.preventDefault();
        e.stopPropagation();
        move(id, i + dir, { focus: true });
      }
      return;
    }
    if (e.altKey) return; // reserved for channel switching
    const buttons = [...(listRef.current?.querySelectorAll<HTMLButtonElement>('button.rail-button') ?? [])];
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = buttons[(i + dir + buttons.length) % buttons.length];
    next?.focus();
    e.preventDefault();
  };

  // Mouse drag: after 4px the icon dims in place, a copy follows the pointer,
  // and a line shows where it lands. Esc cancels. The rail is vertical, so RTL
  // changes nothing here.
  const targetAt = (y: number): number => {
    const items = [...(listRef.current?.querySelectorAll<HTMLElement>('li[data-server-li]') ?? [])];
    const i = items.findIndex((li) => {
      const r = li.getBoundingClientRect();
      return y < r.top + r.height / 2;
    });
    return i < 0 ? items.length : i;
  };
  const onPointerDown = (id: Id) => (e: ReactPointerEvent) => {
    if (e.pointerType !== 'mouse' || e.button !== 0 || arranging) return;
    setDrag({ id, startY: e.clientY, y: e.clientY, x: e.clientX, active: false, target: -1 });
  };
  useEffect(() => {
    if (!drag) return;
    const onMove = (e: PointerEvent) =>
      setDrag((d) => {
        if (!d) return d;
        const active = d.active || Math.abs(e.clientY - d.startY) >= DRAG_START_PX;
        return { ...d, y: e.clientY, x: e.clientX, active, target: active ? targetAt(e.clientY) : -1 };
      });
    const onUp = () =>
      setDrag((d) => {
        if (d?.active && d.target >= 0) {
          const from = servers.findIndex((s) => s.id === d.id);
          // Dropping below itself: the gap it leaves shifts the rest up one.
          move(d.id, d.target > from ? d.target - 1 : d.target);
          suppressClick.current = true;
          setTimeout(() => (suppressClick.current = false), 0);
        }
        return null;
      });
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') setDrag(null);
    };
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

  const label = (s: ServerSummary) =>
    [
      s.name,
      s.closed ? intl.formatMessage({ id: 'server.closedTag' }) : null,
      isServerMuted(s) ? intl.formatMessage({ id: 'notify.muted' }) : null,
      s.mentions > 0 ? intl.formatMessage({ id: 'mentions.label' }, { count: s.mentions }) : null,
      s.unread ? intl.formatMessage({ id: 'unread.label' }) : null,
    ]
      .filter(Boolean)
      .join(', ');

  const dragging = drag?.active ? drag : null;
  const dragged = dragging ? servers.find((s) => s.id === dragging.id) : undefined;

  return (
    <>
    {/* Phone: arrange mode widens the rail over the channel list; dim that (UX on #322). */}
    {arranging && <div className="rail-arrange-backdrop" aria-hidden="true" onClick={() => setArranging(false)} />}
    <nav className={`rail ${arranging ? 'arranging' : ''} ${dragging ? 'dragging' : ''}`} aria-label={intl.formatMessage({ id: 'rail.label' })}>
      <span className="visually-hidden" aria-live="polite">
        {announce}
      </span>
      {arranging && (
        <button type="button" className="primary small rail-arrange-done" onClick={() => setArranging(false)} autoFocus>
          <FormattedMessage id="serverOrder.done" />
        </button>
      )}
      <ul ref={listRef} onKeyDown={onKeyDown}>
        <li>
          <RailButton
            active={selected === 'home'}
            label={intl.formatMessage({ id: 'rail.home' })}
            onClick={() => onSelect('home')}
            className="rail-home"
            tabIndex={selected === 'home' ? 0 : -1}
          >
            <HomeIcon />
          </RailButton>
        </li>
        <li className="rail-sep" aria-hidden="true" />
        {servers.map((s, i) => (
          <li
            key={s.id}
            data-server-li=""
            className={[
              dragging?.id === s.id ? 'drag-source' : '',
              dragging && dragging.target === i ? 'drop-before' : '',
              dragging && i === servers.length - 1 && dragging.target === servers.length ? 'drop-after' : '',
            ]
              .filter(Boolean)
              .join(' ')}
            onPointerDown={onPointerDown(s.id)}
          >
            <RailButton
              active={selected === s.id}
              label={label(s)}
              title={s.name}
              onClick={() => !suppressClick.current && !arranging && onSelect(s.id)}
              style={{ ['--server-color' as string]: s.color }}
              tabIndex={selected === s.id ? 0 : -1}
              unread={s.unread}
              muted={isServerMuted(s)}
              mentions={s.mentions}
              serverId={s.id}
              menuProps={arranging ? undefined : bind(intl.formatMessage({ id: 'menu.forServer' }, { name: s.name }), () => serverItems(s))}
            >
              <bdi className="initials">{s.initials}</bdi>
            </RailButton>
            {arranging && (
              <span className="rail-arrange">
                <button
                  type="button"
                  className="icon-button"
                  aria-disabled={i === 0 || undefined}
                  aria-label={`${intl.formatMessage({ id: 'serverOrder.moveUp' })}: ${s.name}`}
                  onClick={() => i > 0 && move(s.id, i - 1)}
                >
                  ▲
                </button>
                <button
                  type="button"
                  className="icon-button"
                  aria-disabled={i === servers.length - 1 || undefined}
                  aria-label={`${intl.formatMessage({ id: 'serverOrder.moveDown' })}: ${s.name}`}
                  onClick={() => i < servers.length - 1 && move(s.id, i + 1)}
                >
                  ▼
                </button>
              </span>
            )}
          </li>
        ))}
        {!arranging && (
          <li>
            <RailButton label={intl.formatMessage({ id: 'rail.add' })} onClick={onAdd} className="rail-add" tabIndex={-1}>
              <PlusIcon />
            </RailButton>
          </li>
        )}
      </ul>
      {dragged && dragging && (
        <div className="rail-drag-ghost" aria-hidden="true" style={{ top: dragging.y, left: dragging.x, ['--server-color' as string]: dragged.color }}>
          <bdi className="initials">{dragged.initials}</bdi>
        </div>
      )}
      <div className="rail-bottom">
        {update && (
          <button
            type="button"
            className="update-pill"
            // Refused in a call (wait for it to end) or with nothing staged (hide).
            onClick={() => core.restartToUpdate().catch((e) => isCoreError(e) && store.updateRefused(e.code))}
            disabled={update.waitingForCall}
          >
            <RefreshIcon />
            <span>
              <FormattedMessage id={update.waitingForCall ? 'update.waitingForCall' : 'update.ready'} />
            </span>
          </button>
        )}
        <button
          type="button"
          className="rail-button rail-settings"
          onClick={onOpenSettings}
          aria-label={intl.formatMessage({ id: 'me.settings' })}
          title={intl.formatMessage({ id: 'me.settings' })}
        >
          <GearIcon />
        </button>
      </div>
      {menu}
      <MuteDialog target={muteTarget} onClose={() => setMuteTarget(null)} />
    </nav>
    </>
  );
}

function RailButton({
  active,
  label,
  title,
  onClick,
  children,
  className,
  style,
  tabIndex,
  unread,
  muted,
  mentions = 0,
  menuProps,
  serverId,
}: {
  active?: boolean;
  label: string;
  title?: string;
  onClick: () => void;
  children: ReactNode;
  className?: string;
  style?: CSSProperties;
  tabIndex?: number;
  unread?: boolean;
  /** Muted on this device (G-03): shown quieter. */
  muted?: boolean;
  mentions?: number;
  menuProps?: HTMLAttributes<HTMLButtonElement>;
  /** Servers only: lets the keyboard reorder find the focused one. */
  serverId?: Id;
}) {
  return (
    <div className={`rail-item ${active ? 'active' : ''} ${unread ? 'unread' : ''} ${muted ? 'muted' : ''}`}>
      <span className="rail-pip" aria-hidden="true" />
      <button
        type="button"
        className={['rail-button', className].filter(Boolean).join(' ')}
        aria-label={label}
        aria-current={active ? 'page' : undefined}
        title={title ?? label}
        onClick={onClick}
        style={style}
        tabIndex={tabIndex}
        data-server-id={serverId}
        {...menuProps}
      >
        {children}
      </button>
      {mentions > 0 && (
        <span className="badge rail-badge" aria-hidden="true">
          {mentions > 99 ? '99+' : mentions}
        </span>
      )}
    </div>
  );
}

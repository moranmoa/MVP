import { useRef, type CSSProperties, type HTMLAttributes, type KeyboardEvent, type ReactNode } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { isCoreError, useCore, type Id, type ServerSummary } from '../../core';
import { GearIcon, HomeIcon, PlusIcon, RefreshIcon } from '../../components/icons';
import { useContextMenu, type MenuItem } from '../../components/ContextMenu';
import { copyText } from '../../app/clipboard';

export function ServerRail({
  selected,
  onSelect,
  onAdd,
  onOpenSettings,
  onOpenServerSettings,
}: {
  selected: Id | 'home';
  onSelect: (id: Id | 'home') => void;
  onAdd: () => void;
  onOpenSettings: () => void;
  /** Owner only (B11): opens that server's settings. */
  onOpenServerSettings: (id: Id) => void;
}) {
  const store = useAppStore();
  const servers = useStore(store, (s) => s.servers);
  const update = useStore(store, (s) => s.update);
  const core = useCore();
  const intl = useIntl();
  const listRef = useRef<HTMLUListElement>(null);
  const myId = useStore(store, (s) => s.identity?.id);
  const { bind, menu } = useContextMenu();
  // B25 §4: the server menu in the rail. Leaving a server waits for a core command.
  const serverItems = (s: ServerSummary): MenuItem[] => [
    {
      key: 'invite',
      label: intl.formatMessage({ id: 'menu.copyInvite' }),
      onSelect: async () => store.showFlash((await copyText(s.inviteLink)) ? 'common.copied' : 'menu.copyFailed'),
    },
    ...(s.ownerId === myId ? [{ key: 'settings', label: intl.formatMessage({ id: 'server.settings.open' }), onSelect: () => onOpenServerSettings(s.id) }] : []),
  ];

  // Arrow keys move between rail buttons (one Tab stop for the whole list).
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    if (e.altKey) return; // reserved for channel switching
    const buttons = [...(listRef.current?.querySelectorAll<HTMLButtonElement>('button') ?? [])];
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = buttons[(i + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length];
    next?.focus();
    e.preventDefault();
  };

  const label = (s: ServerSummary) =>
    [
      s.name,
      s.mentions > 0 ? intl.formatMessage({ id: 'mentions.label' }, { count: s.mentions }) : null,
      s.unread ? intl.formatMessage({ id: 'unread.label' }) : null,
    ]
      .filter(Boolean)
      .join(', ');

  return (
    <nav className="rail" aria-label={intl.formatMessage({ id: 'rail.label' })}>
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
        {servers.map((s) => (
          <li key={s.id}>
            <RailButton
              active={selected === s.id}
              label={label(s)}
              title={s.name}
              onClick={() => onSelect(s.id)}
              style={{ ['--server-color' as string]: s.color }}
              tabIndex={selected === s.id ? 0 : -1}
              unread={s.unread}
              mentions={s.mentions}
              menuProps={bind(intl.formatMessage({ id: 'menu.forServer' }, { name: s.name }), () => serverItems(s))}
            >
              <bdi className="initials">{s.initials}</bdi>
            </RailButton>
          </li>
        ))}
        <li>
          <RailButton label={intl.formatMessage({ id: 'rail.add' })} onClick={onAdd} className="rail-add" tabIndex={-1}>
            <PlusIcon />
          </RailButton>
        </li>
      </ul>
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
    </nav>
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
  mentions = 0,
  menuProps,
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
  mentions?: number;
  menuProps?: HTMLAttributes<HTMLButtonElement>;
}) {
  return (
    <div className={`rail-item ${active ? 'active' : ''} ${unread ? 'unread' : ''}`}>
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

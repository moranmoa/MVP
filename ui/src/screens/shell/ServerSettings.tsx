// B11 server settings (spec docs/ux/specs/B11-server-settings.md): one screen
// for the owner, opened from the gear by the server name, the server's menu in
// the rail, or the channel list. Only what the core API supports today is here:
// the server's details (read only: there is no updateServer yet), channels,
// the invite link and members. Age, removed accounts and the danger zone wait
// for their core commands.

import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import type { Channel, Member, ServerSummary } from '../../core';
import { Avatar, Dialog, InviteBox, Name } from '../../components/common';
import { GearIcon, HashIcon, PlusIcon } from '../../components/icons';
import { displayName, ProfileCard } from './MemberList';

export type ServerSettingsPage = 'general' | 'channels' | 'invite' | 'members';
const PAGES: ServerSettingsPage[] = ['general', 'channels', 'invite', 'members'];

const NO_CHANNELS: Channel[] = [];
const NO_MEMBERS: Member[] = [];

export function ServerSettings({
  server,
  open,
  onClose,
  onCreateChannel,
  onEditChannel,
}: {
  server: ServerSummary;
  open: boolean;
  onClose: () => void;
  onCreateChannel: () => void;
  onEditChannel: (channel: Channel) => void;
}) {
  const intl = useIntl();
  const [page, setPage] = useState<ServerSettingsPage>('general');
  const navRef = useRef<HTMLDivElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const moved = useRef(false);

  // Every opening starts on "General", with focus on the chosen tab.
  useEffect(() => {
    if (!open) return;
    setPage('general');
    moved.current = false;
    navRef.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
  }, [open]);

  // After a page switch, focus goes to the page's heading.
  useEffect(() => {
    if (moved.current) headingRef.current?.focus();
  }, [page]);

  const choose = (p: ServerSettingsPage) => {
    moved.current = true;
    setPage(p);
  };

  const onNavKey = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const tabs = [...(navRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])];
    const i = tabs.indexOf(document.activeElement as HTMLElement);
    tabs[(i + (e.key === 'ArrowDown' ? 1 : -1) + tabs.length) % tabs.length]?.focus();
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      className="settings-dialog server-settings"
      title={
        <span className="server-settings-title" title={server.name}>
          <span className="server-settings-icon" style={{ background: server.color }} aria-hidden="true">
            <bdi>{server.initials}</bdi>
          </span>
          <Name>{server.name}</Name>
        </span>
      }
    >
      <div className="settings">
        <div
          ref={navRef}
          className="settings-nav"
          role="tablist"
          aria-orientation="vertical"
          aria-label={intl.formatMessage({ id: 'server.settings.open' })}
          onKeyDown={onNavKey}
        >
          <ul role="presentation">
            {PAGES.map((p) => (
              <li key={p} role="presentation">
                <button
                  type="button"
                  role="tab"
                  id={`server-tab-${p}`}
                  aria-selected={page === p}
                  aria-controls="server-settings-panel"
                  tabIndex={page === p ? 0 : -1}
                  className={page === p ? 'active' : ''}
                  onClick={() => choose(p)}
                >
                  <FormattedMessage id={`server.settings.nav.${p}`} />
                </button>
              </li>
            ))}
          </ul>
        </div>
        <section id="server-settings-panel" className="settings-panel" role="tabpanel" aria-labelledby={`server-tab-${page}`}>
          <h3 ref={headingRef} tabIndex={-1}>
            <FormattedMessage id={`server.settings.nav.${page}`} />
          </h3>
          {page === 'general' && <GeneralPage server={server} />}
          {page === 'channels' && <ChannelsPage server={server} onCreate={onCreateChannel} onEdit={onEditChannel} />}
          {page === 'invite' && <InviteBox link={server.inviteLink} />}
          {page === 'members' && <MembersPage server={server} />}
        </section>
      </div>
    </Dialog>
  );
}

function GeneralPage({ server }: { server: ServerSummary }) {
  const age = server.ageRestriction === 'none' ? 'none' : server.ageRestriction.slice(0, 2);
  return (
    <>
      <dl className="server-facts">
        <dt>
          <FormattedMessage id="server.settings.name" />
        </dt>
        <dd>
          <Name>{server.name}</Name>
        </dd>
        <dt>
          <FormattedMessage id="server.settings.age" />
        </dt>
        <dd>
          <bdi>
            <FormattedMessage id={`age.${age}`} />
          </bdi>
        </dd>
      </dl>
      <p className="muted small">
        <FormattedMessage id="server.settings.readOnly" />
      </p>
    </>
  );
}

function ChannelsPage({ server, onCreate, onEdit }: { server: ServerSummary; onCreate: () => void; onEdit: (c: Channel) => void }) {
  const store = useAppStore();
  const channels = useStore(store, (s) => s.channels[server.id] ?? NO_CHANNELS);
  const intl = useIntl();
  return (
    <div className="stack">
      <div>
        <button type="button" className="secondary small" onClick={onCreate}>
          <PlusIcon /> <FormattedMessage id="channel.create" />
        </button>
      </div>
      <ul className="settings-list">
        {channels.map((c) => (
          <li key={c.id}>
            <HashIcon />
            <bdi className="grow">{c.name}</bdi>
            {c.adult && (
              <span className="adult-tag">
                18+
              </span>
            )}
            <button
              type="button"
              className="icon-button small"
              onClick={() => onEdit(c)}
              aria-label={`${intl.formatMessage({ id: 'channel.edit' })}: ${c.name}`}
              title={intl.formatMessage({ id: 'channel.edit' })}
            >
              <GearIcon />
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function MembersPage({ server }: { server: ServerSummary }) {
  const store = useAppStore();
  const members = useStore(store, (s) => s.members[server.id] ?? NO_MEMBERS);
  const myId = useStore(store, (s) => s.identity?.id);
  const [kick, setKick] = useState<Member | null>(null);
  return (
    <>
      <ul className="settings-list">
        {members.map((m) => (
          <li key={m.id}>
            <Avatar name={displayName(m)} color={m.avatarColor} size={32} presence={m.presence} />
            <Name className="name grow">{displayName(m)}</Name>
            {m.isOwner && (
              <span className="owner-tag">
                <FormattedMessage id="members.owner" />
              </span>
            )}
            {m.id !== myId && !m.isOwner && (
              <button type="button" className="danger small" onClick={() => setKick(m)}>
                <FormattedMessage id="profile.kick" />
              </button>
            )}
          </li>
        ))}
      </ul>
      {kick && <ProfileCard member={kick} server={server} isMe={false} startKick onClose={() => setKick(null)} />}
    </>
  );
}

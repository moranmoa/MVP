// B11 server settings (spec docs/ux/specs/B11-server-settings.md): one screen
// for the owner, opened from the gear by the server name, the server's menu in
// the rail, or the channel list. Only what the core API supports today is here:
// the name (rename, #261), channels, the invite link, members and the danger
// zone (close the server, #261). Changing the age limit and removed accounts
// wait for their core commands.

import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useCore, useCoreEvent, type Channel, type Invite, type Member, type ServerSummary } from '../../core';
import { Avatar, Dialog, InviteBox, Name } from '../../components/common';
import { GearIcon, HashIcon, PlusIcon } from '../../components/icons';
import { displayName, ProfileCard } from './MemberList';
import { ClosePanel, RenameForm } from './ServerLifecycle';

export type ServerSettingsPage = 'general' | 'channels' | 'invite' | 'members' | 'danger';
const PAGES: ServerSettingsPage[] = ['general', 'channels', 'invite', 'members', 'danger'];

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
                  className={[page === p ? 'active' : '', p === 'danger' ? 'danger-tab' : ''].filter(Boolean).join(' ')}
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
          {page === 'invite' && <InvitePage server={server} />}
          {page === 'members' && <MembersPage server={server} />}
          {page === 'danger' && <ClosePanel server={server} />}
        </section>
      </div>
    </Dialog>
  );
}

function GeneralPage({ server }: { server: ServerSummary }) {
  const age = server.ageRestriction === 'none' ? 'none' : server.ageRestriction.slice(0, 2);
  return (
    <div className="stack">
      <RenameForm server={server} />
      <dl className="server-facts">
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
        <FormattedMessage id="server.settings.ageReadOnly" />
      </p>
    </div>
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
            <Avatar name={displayName(m)} color={m.color ?? m.avatarColor} src={m.avatarDataUrl} size={32} presence={m.presence} />
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

const HOUR = 3_600_000;
const EXPIRY = { never: 0, hour: HOUR, day: 24 * HOUR, week: 7 * 24 * HOUR } as const;
type Expiry = keyof typeof EXPIRY;
const MAX_USES = [0, 1, 5, 10, 25, 100];

// Invites (#214): the current link, making a new one with an expiry and a use
// limit, and the list of links with Revoke. The core sends server.updated after
// a create or a revoke, and the list is read again then.
function InvitePage({ server }: { server: ServerSummary }) {
  const core = useCore();
  const intl = useIntl();
  const members = useStore(useAppStore(), (s) => s.members[server.id] ?? NO_MEMBERS);
  const [invites, setInvites] = useState<Invite[] | null>(null);
  const [expiry, setExpiry] = useState<Expiry>('never');
  const [maxUses, setMaxUses] = useState(0);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const [confirm, setConfirm] = useState<string | null>(null);

  const load = () =>
    core.listInvites(server.id).then(setInvites, () => {
      setInvites([]);
      setFailed(true);
    });
  useEffect(() => {
    void load();
  }, [server.id]);
  useCoreEvent('server.updated', (s) => {
    if (s.id === server.id) void load();
  }, [server.id]);

  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setFailed(false);
    try {
      await action();
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
    await load();
  };

  const create = () =>
    run(() =>
      core.createInvite(server.id, {
        ...(expiry !== 'never' ? { expiresMs: Date.now() + EXPIRY[expiry] } : {}),
        ...(maxUses ? { maxUses } : {}),
      }),
    );

  const now = Date.now();
  const status = (i: Invite) =>
    i.revoked ? 'revoked' : i.expiresAt !== undefined && i.expiresAt <= now ? 'expired' : i.maxUses && i.uses >= i.maxUses ? 'usedUp' : null;
  const author = (id: string) => {
    const m = members.find((x) => x.id === id);
    return m ? displayName(m) : '…';
  };

  return (
    <div className="stack">
      <h4>
        <FormattedMessage id="invite.current" />
      </h4>
      {server.inviteLink ? (
        <InviteBox link={server.inviteLink} />
      ) : (
        <p className="muted small">
          <FormattedMessage id="invite.none" />
        </p>
      )}

      <h4>
        <FormattedMessage id="invite.create" />
      </h4>
      <div className="invite-form">
        <label className="field">
          <span>
            <FormattedMessage id="invite.expires" />
          </span>
          <select value={expiry} onChange={(e) => setExpiry(e.target.value as Expiry)}>
            {(Object.keys(EXPIRY) as Expiry[]).map((k) => (
              <option key={k} value={k}>
                {intl.formatMessage({ id: `invite.expires.${k}` })}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>
            <FormattedMessage id="invite.maxUses" />
          </span>
          <select value={maxUses} onChange={(e) => setMaxUses(Number(e.target.value))}>
            {MAX_USES.map((n) => (
              <option key={n} value={n}>
                {n ? intl.formatNumber(n) : intl.formatMessage({ id: 'invite.maxUses.unlimited' })}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="primary" disabled={busy} onClick={() => void create()}>
          <FormattedMessage id="invite.createButton" />
        </button>
      </div>
      {failed && (
        <p className="error small" role="alert">
          <FormattedMessage id="invite.failed" />
        </p>
      )}

      <h4>
        <FormattedMessage id="invite.list" />
      </h4>
      {invites === null ? (
        <p className="muted small">
          <FormattedMessage id="common.loading" />
        </p>
      ) : (
        <ul className="settings-list invite-list">
          {[...invites].reverse().map((i) => {
            const st = status(i);
            return (
              <li key={i.id} className={st ? 'inactive' : undefined}>
                <div className="grow">
                  <div>
                    <FormattedMessage id="invite.by" values={{ name: author(i.createdBy) }} />
                  </div>
                  <div className="muted small">
                    {i.maxUses ? (
                      <FormattedMessage id="invite.usesOf" values={{ count: i.uses, max: i.maxUses }} />
                    ) : (
                      <FormattedMessage id="invite.uses" values={{ count: i.uses }} />
                    )}
                    {' · '}
                    {i.expiresAt === undefined ? (
                      <FormattedMessage id="invite.neverExpires" />
                    ) : (
                      <>
                        <FormattedMessage id="invite.expiresOn" />{' '}
                        {intl.formatDate(i.expiresAt, { dateStyle: 'medium', timeStyle: 'short' })}
                      </>
                    )}
                  </div>
                </div>
                {st ? (
                  <span className="muted small">
                    <FormattedMessage id={`invite.status.${st}`} />
                  </span>
                ) : confirm === i.id ? (
                  <span className="invite-confirm" role="group" aria-label={intl.formatMessage({ id: 'invite.revokeConfirm' })}>
                    <button type="button" className="danger small" autoFocus disabled={busy} onClick={() => void run(() => core.revokeInvite(server.id, i.id)).then(() => setConfirm(null))}>
                      <FormattedMessage id="invite.revoke" />
                    </button>
                    <button type="button" className="secondary small" onClick={() => setConfirm(null)}>
                      <FormattedMessage id="common.cancel" />
                    </button>
                  </span>
                ) : (
                  <button type="button" className="secondary small" onClick={() => setConfirm(i.id)}>
                    <FormattedMessage id="invite.revoke" />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

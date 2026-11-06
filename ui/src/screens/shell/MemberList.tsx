import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useCore, type Member, type ServerSummary } from '../../core';
import type { Connection } from '../../core/types';
import { Avatar, Code, Dialog, Name } from '../../components/common';
import { textDir, useUiDir } from '../../app/textDir';
import { CloseIcon, CloudIcon, LinkIcon } from '../../components/icons';
import { useContextMenu, type MenuItem } from '../../components/ContextMenu';
import { copyText } from '../../app/clipboard';
import { nameSkeleton } from '../../core/sanitize';

const NO_MEMBERS: Member[] = [];

export function displayName(m: { displayName: string; shortCode: string }): string {
  return m.displayName || m.shortCode;
}

// Names that pass for a role. Compared by skeleton, so "Αdmin" (Greek Alpha) counts too.
const ROLE_NAMES = ['admin', 'administrator', 'owner', 'moderator', 'mod', 'system', 'support', 'staff', 'בעלים', 'מנהל', 'מנהלת', 'הנהלה', 'מערכת', 'תמיכה'].map(nameSkeleton);

/**
 * Members whose name looks like the owner's name or like a role. Owners are
 * never flagged. The identity code next to every name is the real check; this
 * only points at the people worth checking.
 */
export function lookalikeIds(members: Member[]): Set<string> {
  const taken = new Set(ROLE_NAMES);
  for (const m of members) if (m.isOwner && m.displayName) taken.add(nameSkeleton(m.displayName));
  const out = new Set<string>();
  for (const m of members) {
    const s = m.displayName && nameSkeleton(m.displayName);
    if (s && !m.isOwner && taken.has(s)) out.add(m.id);
  }
  return out;
}

export function LookalikeTag() {
  const intl = useIntl();
  return (
    <span className="lookalike-tag" title={intl.formatMessage({ id: 'members.lookalikeHint' })}>
      <FormattedMessage id="members.lookalike" />
    </span>
  );
}

/**
 * Direct or through the relay (T20). In the list it is the icon alone with a
 * tooltip (the row's aria-label says it in words); `withLabel` is the
 * profile card's one-line summary, with the longer explanation as its tooltip.
 */
export function ConnectionIcon({ connection, withLabel }: { connection: Connection; withLabel?: boolean }) {
  const intl = useIntl();
  const Glyph = connection === 'direct' ? LinkIcon : CloudIcon;
  const title = intl.formatMessage({ id: withLabel ? `connection.${connection}.title` : `connection.${connection}` });
  return (
    <span className={`connection-icon ${connection}`} title={title}>
      <Glyph />
      {withLabel && <FormattedMessage id={`connection.${connection}`} />}
    </span>
  );
}

/** `onClose` is set on phones, where the list covers the chat and needs its own close button. */
export function MemberList({ server, onClose }: { server: ServerSummary; onClose?: () => void }) {
  const store = useAppStore();
  const members = useStore(store, (s) => s.members[server.id] ?? NO_MEMBERS);
  const myId = useStore(store, (s) => s.identity?.id);
  const intl = useIntl();
  const core = useCore();
  const [openId, setOpenId] = useState<string | null>(null);
  const [kickId, setKickId] = useState<string | null>(null);
  const { bind, menu } = useContextMenu();
  const owner = server.ownerId === myId;
  // B25 §4: the member menu. Every item is also on the profile card.
  const memberItems = (m: Member): MenuItem[] => [
    { key: 'profile', label: intl.formatMessage({ id: 'menu.profile' }), onSelect: () => setOpenId(m.id) },
    {
      key: 'code',
      label: intl.formatMessage({ id: 'menu.copyCode' }),
      onSelect: async () => store.showFlash((await copyText(m.shortCode)) ? 'common.copied' : 'menu.copyFailed'),
    },
    { kind: 'separator' },
    ...(m.id === myId
      ? []
      : [{ key: 'block', label: intl.formatMessage({ id: m.blocked ? 'profile.unblock' : 'profile.block' }), onSelect: () => void core.blockUser(m.id) }]),
    ...(owner && m.id !== myId && !m.isOwner
      ? [
          {
            key: 'kick',
            label: intl.formatMessage({ id: 'profile.kick' }),
            danger: true,
            onSelect: () => {
              setKickId(m.id);
              setOpenId(m.id);
            },
          },
        ]
      : []),
  ];

  const [online, offline] = useMemo(() => {
    const collator = new Intl.Collator(intl.locale);
    const sorted = [...members].sort((a, b) => collator.compare(displayName(a), displayName(b)));
    return [sorted.filter((m) => m.presence === 'online'), sorted.filter((m) => m.presence === 'offline')];
  }, [members, intl.locale]);

  const open = members.find((m) => m.id === openId);
  const flagged = useMemo(() => lookalikeIds(members), [members]);
  const headingId = useId();
  const closeRef = useRef<HTMLButtonElement>(null);
  const isSheet = !!onClose;
  useEffect(() => {
    if (isSheet) closeRef.current?.focus();
  }, [isSheet]);

  const group = (list: Member[], titleId: string) =>
    list.length > 0 && (
      <section>
        <h2 className="member-group">
          <FormattedMessage id={titleId} values={{ count: list.length }} />
        </h2>
        <ul>
          {list.map((m) => (
            <li key={m.id}>
              <button
                type="button"
                className={`member ${m.presence}`}
                {...bind(intl.formatMessage({ id: 'menu.forMember' }, { name: displayName(m) }), () => memberItems(m))}
                onClick={() => setOpenId(m.id)}
                aria-label={[
                  intl.formatMessage({ id: 'members.open' }, { name: displayName(m) }),
                  m.connection ? intl.formatMessage({ id: `connection.${m.connection}` }) : null,
                ]
                  .filter(Boolean)
                  .join(', ')}
              >
                <Avatar name={displayName(m)} color={m.avatarColor} size={32} presence={m.presence} />
                {m.connection && <ConnectionIcon connection={m.connection} />}
                <Name>{displayName(m)}</Name>
                {m.id === myId && (
                  <span className="muted small">
                    <FormattedMessage id="members.me" />
                  </span>
                )}
                {flagged.has(m.id) && <LookalikeTag />}
                {m.isOwner && (
                  <span className="owner-tag" title={intl.formatMessage({ id: 'members.owner' })}>
                    <FormattedMessage id="members.owner" />
                  </span>
                )}
              </button>
            </li>
          ))}
        </ul>
      </section>
    );

  return (
    <aside
      className="members"
      // On a phone the list covers the whole screen, so it behaves as a modal
      // dialog: labelled by its heading, focus moves in, Esc closes it (Shell
      // makes everything underneath inert and returns focus on close).
      {...(onClose
        ? {
            role: 'dialog',
            'aria-modal': true,
            'aria-labelledby': headingId,
            onKeyDown: (e: KeyboardEvent) => {
              if (e.key === 'Escape' && !open) {
                e.preventDefault();
                onClose();
              }
            },
          }
        : { 'aria-label': intl.formatMessage({ id: 'members.label' }) })}
    >
      {onClose && (
        <header className="members-header">
          <h2 id={headingId}>
            <FormattedMessage id="members.label" />
          </h2>
          <button
            ref={closeRef}
            type="button"
            className="icon-button"
            onClick={onClose}
            aria-label={intl.formatMessage({ id: 'members.close' })}
          >
            <CloseIcon />
          </button>
        </header>
      )}
      {group(online, 'members.online')}
      {group(offline, 'members.offline')}
      {open && (
        <ProfileCard
          key={`${open.id}-${kickId === open.id}`}
          member={open}
          server={server}
          isMe={open.id === myId}
          lookalike={flagged.has(open.id)}
          startKick={kickId === open.id}
          onClose={() => {
            setOpenId(null);
            setKickId(null);
          }}
        />
      )}
      {menu}
    </aside>
  );
}

export function ProfileCard({
  member,
  server,
  isMe,
  lookalike,
  startKick,
  onClose,
}: {
  member: Member;
  server: ServerSummary;
  isMe: boolean;
  lookalike?: boolean;
  /** Opens straight on the removal confirmation (from a menu or the settings screen). */
  startKick?: boolean;
  onClose: () => void;
}) {
  const core = useCore();
  const store = useAppStore();
  const intl = useIntl();
  const uiDir = useUiDir();
  const myId = useStore(store, (s) => s.identity?.id);
  const [confirmKick, setConfirmKick] = useState(!!startKick);
  const canKick = server.ownerId === myId && !isMe && !member.isOwner;
  const name = displayName(member);

  return (
    <Dialog open onClose={onClose} title={<Name>{name}</Name>} className="profile-dialog">
      <div className="profile-card">
        <Avatar name={name} color={member.avatarColor} size={80} presence={member.presence} />
        <div className="profile-meta">
          <span className={`presence-label ${member.presence}`}>
            <FormattedMessage id={`presence.${member.presence}`} />
          </span>
          {member.isOwner && (
            <span className="owner-tag">
              <FormattedMessage id="members.owner" />
            </span>
          )}
        </div>
        {member.connection && <ConnectionIcon connection={member.connection} withLabel />}
        {lookalike && (
          <p className="warning-text" role="note">
            <FormattedMessage id="members.lookalikeHint" />
          </p>
        )}
        {member.bio && (
          <p dir={textDir(member.bio, uiDir)} className="bio user-text">
            {member.bio}
          </p>
        )}
        <div className="code-box">
          <span>
            <FormattedMessage id="profile.code" />
          </span>
          <Code value={member.shortCode} />
          <p className="muted small">
            <FormattedMessage id="profile.codeHint" />
          </p>
        </div>
        {!isMe && !confirmKick && (
          <div className="actions">
            <button type="button" className="secondary" onClick={() => void core.blockUser(member.id)}>
              <FormattedMessage id={member.blocked ? 'profile.unblock' : 'profile.block'} />
            </button>
            {canKick && (
              <button type="button" className="danger" onClick={() => setConfirmKick(true)}>
                <FormattedMessage id="profile.kick" />
              </button>
            )}
          </div>
        )}
        {!isMe && !member.blocked && !confirmKick && (
          <p className="muted small">
            <FormattedMessage id="profile.blockHint" />
          </p>
        )}
        {confirmKick && (
          <div className="confirm" role="alertdialog" aria-label={intl.formatMessage({ id: 'profile.kick' })}>
            <p>
              <FormattedMessage id="profile.kickConfirm" values={{ name: <Name>{name}</Name> }} />
            </p>
            <div className="actions">
              <button
                type="button"
                className="danger"
                autoFocus
                onClick={async () => {
                  await core.kickMember(server.id, member.id);
                  onClose();
                }}
              >
                <FormattedMessage id="profile.kickConfirmButton" />
              </button>
              <button type="button" className="secondary" onClick={() => setConfirmKick(false)}>
                <FormattedMessage id="common.cancel" />
              </button>
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}

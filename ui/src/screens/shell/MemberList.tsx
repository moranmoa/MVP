import { useMemo, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useCore, type Member, type ServerSummary } from '../../core';
import { Avatar, Code, Dialog, Name } from '../../components/common';
import { CloseIcon } from '../../components/icons';
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

/** `onClose` is set on phones, where the list covers the chat and needs its own close button. */
export function MemberList({ server, onClose }: { server: ServerSummary; onClose?: () => void }) {
  const store = useAppStore();
  const members = useStore(store, (s) => s.members[server.id] ?? NO_MEMBERS);
  const myId = useStore(store, (s) => s.identity?.id);
  const intl = useIntl();
  const [openId, setOpenId] = useState<string | null>(null);

  const [online, offline] = useMemo(() => {
    const collator = new Intl.Collator(intl.locale);
    const sorted = [...members].sort((a, b) => collator.compare(displayName(a), displayName(b)));
    return [sorted.filter((m) => m.presence === 'online'), sorted.filter((m) => m.presence === 'offline')];
  }, [members, intl.locale]);

  const open = members.find((m) => m.id === openId);
  const flagged = useMemo(() => lookalikeIds(members), [members]);

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
                onClick={() => setOpenId(m.id)}
                aria-label={intl.formatMessage({ id: 'members.open' }, { name: displayName(m) })}
              >
                <Avatar name={displayName(m)} color={m.avatarColor} size={32} presence={m.presence} />
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
    <aside className="members" aria-label={intl.formatMessage({ id: 'members.label' })}>
      {onClose && (
        <header className="members-header">
          <h2>
            <FormattedMessage id="members.label" />
          </h2>
          <button type="button" className="icon-button" onClick={onClose} aria-label={intl.formatMessage({ id: 'members.close' })}>
            <CloseIcon />
          </button>
        </header>
      )}
      {group(online, 'members.online')}
      {group(offline, 'members.offline')}
      {open && <ProfileCard member={open} server={server} isMe={open.id === myId} lookalike={flagged.has(open.id)} onClose={() => setOpenId(null)} />}
    </aside>
  );
}

export function ProfileCard({
  member,
  server,
  isMe,
  lookalike,
  onClose,
}: {
  member: Member;
  server: ServerSummary;
  isMe: boolean;
  lookalike?: boolean;
  onClose: () => void;
}) {
  const core = useCore();
  const store = useAppStore();
  const intl = useIntl();
  const myId = useStore(store, (s) => s.identity?.id);
  const [confirmKick, setConfirmKick] = useState(false);
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
        {lookalike && (
          <p className="warning-text" role="note">
            <FormattedMessage id="members.lookalikeHint" />
          </p>
        )}
        {member.bio && (
          <p dir="auto" className="bio">
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

import { useMemo, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useCore, type Member, type ServerSummary } from '../../core';
import { Avatar, Code, Dialog, Name } from '../../components/common';

const NO_MEMBERS: Member[] = [];

export function displayName(m: { displayName: string; shortCode: string }): string {
  return m.displayName || m.shortCode;
}

export function MemberList({ server }: { server: ServerSummary }) {
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
      {group(online, 'members.online')}
      {group(offline, 'members.offline')}
      {open && <ProfileCard member={open} server={server} isMe={open.id === myId} onClose={() => setOpenId(null)} />}
    </aside>
  );
}

export function ProfileCard({
  member,
  server,
  isMe,
  onClose,
}: {
  member: Member;
  server: ServerSummary;
  isMe: boolean;
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

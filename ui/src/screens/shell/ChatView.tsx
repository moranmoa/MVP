import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { isCoreError, useCore, type Member, type Message, type ServerSummary } from '../../core';
import { Dialog, Name, RICH } from '../../components/common';
import { BackIcon, CloseIcon, EditIcon, HashIcon, ReplyIcon, SendIcon, UsersIcon, WifiOffIcon } from '../../components/icons';
import { useUserTextProps } from '../../app/textDir';
import { BODY_MAX, BODY_MAX_BYTES } from '../../core/sanitize';

/** The character counter shows from here on (B6/QA-17 acceptance: 3,800). */
const COUNTER_FROM = 3800;
const utf8 = new TextEncoder();
import { MessageList } from './MessageList';

const NO_MEMBERS: Member[] = [];

export function ChatView({
  server,
  channelId,
  membersOpen,
  onToggleMembers,
  onBack,
}: {
  server: ServerSummary;
  channelId: string;
  membersOpen: boolean;
  onToggleMembers: () => void;
  /** Phone layout only: back to the channel list. */
  onBack?: () => void;
}) {
  const store = useAppStore();
  const core = useCore();
  const intl = useIntl();
  const channel = useStore(store, (s) => s.channels[server.id]?.find((c) => c.id === channelId));
  const members = useStore(store, (s) => s.members[server.id] ?? NO_MEMBERS);
  const myId = useStore(store, (s) => s.identity!.id);
  useStore(store, (s) => s.messagesVersion); // re-render when messages change
  const win = store.window(channelId);

  const [replyTo, setReplyTo] = useState<Message | null>(null);
  const [editing, setEditing] = useState<Message | null>(null);
  const [toDelete, setToDelete] = useState<Message | null>(null);
  const [stickToken, setStickToken] = useState(0);

  const memberMap = useMemo(() => new Map(members.map((m) => [m.id, m])), [members]);
  const nobodyOnline = members.length > 0 && members.every((m) => m.id === myId || m.presence === 'offline');
  const isOwner = server.ownerId === myId;

  // Reading the channel clears its unread dot and mention badge.
  useEffect(() => {
    if (channel?.unread || channel?.mentions) void core.markRead(channelId);
  }, [channel?.unread, channel?.mentions, channelId, core]);

  const onReply = useCallback((m: Message) => {
    setEditing(null);
    setReplyTo(m);
  }, []);
  const onEdit = useCallback((m: Message) => {
    setReplyTo(null);
    setEditing(m);
  }, []);
  const onDelete = useCallback((m: Message) => setToDelete(m), []);
  const onRetry = useCallback((m: Message) => void core.retryMessage(m.id).catch(() => {}), [core]);

  const editLast = () => {
    for (let i = win.count - 1; i >= Math.max(0, win.count - 50); i--) {
      const m = win.get(i);
      if (m && m.authorId === myId && !m.deleted) return onEdit(m);
    }
  };

  const channelName = channel?.name ?? '';

  return (
    <section className="chat" aria-label={intl.formatMessage({ id: 'chat.messages' }, { channel: channelName })}>
      <header className="chat-header">
        {onBack && (
          <button
            type="button"
            className="icon-button back-button"
            onClick={onBack}
            aria-label={intl.formatMessage({ id: 'chat.back' })}
            title={intl.formatMessage({ id: 'chat.back' })}
          >
            <BackIcon />
          </button>
        )}
        <HashIcon />
        <h2 className="chat-title" tabIndex={-1}>
          <bdi>{channelName}</bdi>
        </h2>
        {channel?.adult && (
          <span className="adult-tag" title={intl.formatMessage({ id: 'channel.adult' })}>
            18+
          </span>
        )}
        <span className="spacer" />
        <button
          type="button"
          className={`icon-button members-button ${membersOpen ? 'pressed' : ''}`}
          aria-pressed={membersOpen}
          onClick={onToggleMembers}
          aria-label={intl.formatMessage({ id: 'chat.toggleMembers' })}
          title={intl.formatMessage({ id: 'chat.toggleMembers' })}
        >
          <UsersIcon />
        </button>
      </header>
      {nobodyOnline && (
        <div className="offline-banner" role="status">
          <WifiOffIcon />
          <span>
            <FormattedMessage id="chat.offline" />
          </span>
        </div>
      )}
      <MessageList
        win={win}
        channelName={channelName}
        memberMap={memberMap}
        myId={myId}
        isOwner={isOwner}
        stickToken={stickToken}
        onReply={onReply}
        onEdit={onEdit}
        onDelete={onDelete}
        onRetry={onRetry}
      />
      <Composer
        channelName={channelName}
        replyTo={replyTo}
        replyName={replyTo ? (memberMap.get(replyTo.authorId)?.displayName ?? '') : ''}
        editing={editing}
        onCancel={() => {
          setReplyTo(null);
          setEditing(null);
        }}
        onEditLast={editLast}
        onSubmit={async (body) => {
          if (editing) {
            if (body !== editing.body) await core.editMessage(editing.id, body);
            setEditing(null);
          } else {
            const reply = replyTo?.id;
            setReplyTo(null);
            setStickToken((t) => t + 1);
            await core.sendMessage(channelId, body, reply);
          }
        }}
      />
      <Dialog open={!!toDelete} onClose={() => setToDelete(null)} title={intl.formatMessage({ id: 'chat.delete' })}>
        <p>
          <FormattedMessage id="chat.deleteConfirm" />
        </p>
        <div className="actions">
          <button
            type="button"
            className="danger"
            autoFocus
            onClick={() => {
              // The real core can't delete yet (not-implemented); nothing changes then.
              if (toDelete) core.deleteMessage(toDelete.id).catch(() => {});
              setToDelete(null);
            }}
          >
            <FormattedMessage id="chat.delete" />
          </button>
          <button type="button" className="secondary" onClick={() => setToDelete(null)}>
            <FormattedMessage id="common.cancel" />
          </button>
        </div>
      </Dialog>
    </section>
  );
}

function Composer({
  channelName,
  replyTo,
  replyName,
  editing,
  onCancel,
  onEditLast,
  onSubmit,
}: {
  channelName: string;
  replyTo: Message | null;
  replyName: string;
  editing: Message | null;
  onCancel: () => void;
  onEditLast: () => void;
  onSubmit: (body: string) => Promise<void>;
}) {
  const intl = useIntl();
  const [text, setText] = useState('');
  // Empty: the interface direction (cursor and placeholder on the right in Hebrew).
  const textProps = useUserTextProps(text);
  const draft = useRef('');
  const ref = useRef<HTMLTextAreaElement>(null);

  // Entering edit mode swaps the draft out and the message text in.
  useEffect(() => {
    if (editing) {
      draft.current = text;
      setText(editing.body);
    } else if (draft.current) {
      setText(draft.current);
      draft.current = '';
    }
    if (editing || replyTo) ref.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing, replyTo]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = '';
    if (!text) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  }, [text]);

  // QA-10: no silent cut. Past the limit the counter turns red and sending
  // stops, so the person can shorten the text themselves.
  // The limit is in code points (QA-17): an emoji is one character, not two
  // UTF-16 units. A string never has more code points than units, so the
  // cheap length check skips the count for normal messages.
  const chars = text.length >= COUNTER_FROM ? [...text].length : text.length;
  const nearLimit = chars >= COUNTER_FROM;
  const tooLong = nearLimit && (chars > BODY_MAX || utf8.encode(text).length > BODY_MAX_BYTES);
  // Why the last send failed: no channel key yet (`not-allowed`: a moderator
  // has to renew it), a feature the core doesn't have yet, or anything else.
  const [failed, setFailed] = useState<'no-key' | 'not-yet' | 'other' | null>(null);

  const submit = async () => {
    const body = text.trim();
    if (!body || tooLong) return;
    setFailed(null);
    if (!editing) setText('');
    try {
      await onSubmit(body);
      if (editing) setText('');
    } catch (e) {
      // The core refused it: give the text back instead of losing it.
      setText((t) => t || body);
      const code = isCoreError(e) ? e.code : null;
      setFailed(code === 'not-allowed' ? 'no-key' : code === 'not-implemented' ? 'not-yet' : 'other');
    }
  };

  return (
    <div className="composer">
      {(replyTo || editing) && (
        <div className="composer-context">
          {editing ? <EditIcon /> : <ReplyIcon />}
          <span>
            {editing ? (
              <FormattedMessage id="chat.editing" />
            ) : (
              <FormattedMessage id="chat.replyingTo" values={{ name: <Name>{replyName}</Name> }} />
            )}
          </span>
          {editing && (
            <span className="muted small">
              <FormattedMessage id="chat.editHint" values={RICH} />
            </span>
          )}
          <span className="spacer" />
          <button
            type="button"
            className="icon-button"
            onClick={onCancel}
            aria-label={intl.formatMessage({ id: editing ? 'common.cancel' : 'chat.cancelReply' })}
          >
            <CloseIcon />
          </button>
        </div>
      )}
      <form
        className="composer-row"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <textarea
          ref={ref}
          {...textProps}
          aria-invalid={tooLong || undefined}
          aria-describedby={nearLimit || failed ? 'composer-status' : undefined}
          rows={1}
          value={text}
          placeholder={intl.formatMessage({ id: 'chat.composer' }, { channel: channelName })}
          aria-label={intl.formatMessage({ id: 'chat.composer' }, { channel: channelName })}
          onChange={(e) => {
            setFailed(null);
            setText(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void submit();
            } else if (e.key === 'Escape' && (replyTo || editing)) {
              e.preventDefault();
              onCancel();
            } else if (e.key === 'ArrowUp' && !text) {
              e.preventDefault();
              onEditLast();
            }
          }}
        />
        <button type="submit" className="icon-button send" aria-label={intl.formatMessage({ id: 'chat.send' })} disabled={!text.trim() || tooLong}>
          <SendIcon />
        </button>
      </form>
      {(nearLimit || failed) && (
        <p id="composer-status" className={`composer-status ${tooLong || failed ? 'error' : 'muted'}`} role={tooLong || failed ? 'alert' : undefined}>
          {failed ? (
            <FormattedMessage id={failed === 'no-key' ? 'chat.cantWriteYet' : failed === 'not-yet' ? 'chat.notYet' : 'chat.sendFailed'} />
          ) : (
            <>
              {tooLong && (
                <>
                  <FormattedMessage id="chat.tooLong" values={{ max: BODY_MAX }} />{' '}
                </>
              )}
              <span className="char-count" dir="ltr">
                <FormattedMessage id="chat.charCount" values={{ count: chars, max: BODY_MAX }} />
              </span>
            </>
          )}
        </p>
      )}
    </div>
  );
}

// Virtualised message list: only the rows on screen (plus a small overscan)
// exist in the DOM, so a channel with 100,000 messages scrolls like one with 50.

import { useVirtualizer } from '@tanstack/react-virtual';
import { memo, useCallback, useMemo, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import type { MessageWindow } from '../../app/store';
import type { Member, Message } from '../../core';
import { Avatar, Code, Name } from '../../components/common';
import {
  AlertIcon,
  ArrowDownIcon,
  CheckIcon,
  ClockIcon,
  DoubleCheckIcon,
  EditIcon,
  ReplyIcon,
  TrashIcon,
} from '../../components/icons';
import { paragraphs, textDir, useUiDir } from '../../app/textDir';
import { displayName, lookalikeIds, LookalikeTag } from './MemberList';
import { useContextMenu, type MenuItem } from '../../components/ContextMenu';
import { copyText } from '../../app/clipboard';
import { useAppStore } from '../../app/App';

const GROUP_GAP_MS = 5 * 60_000;
const BOTTOM_SLACK_PX = 48;

interface Handlers {
  onReply: (m: Message) => void;
  onEdit: (m: Message) => void;
  onDelete: (m: Message) => void;
  onRetry: (m: Message) => void;
}

export function MessageList({
  win,
  channelName,
  memberMap,
  myId,
  isOwner,
  stickToken,
  ...handlers
}: {
  win: MessageWindow;
  channelName: string;
  memberMap: Map<string, Member>;
  myId: string;
  isOwner: boolean;
  /** Changes when I send a message: jump to the bottom even if scrolled up. */
  stickToken: number;
} & Handlers) {
  const intl = useIntl();
  const scrollRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const [showJump, setShowJump] = useState(false);
  const [active, setActive] = useState<number | null>(null);
  const [revealed, setRevealed] = useState<Set<string>>(() => new Set());
  const [flash, setFlash] = useState<number | null>(null);
  const focusPending = useRef(false);
  const count = win.count;
  const flagged = useMemo(() => lookalikeIds([...memberMap.values()]), [memberMap]);
  const store = useAppStore();
  const { bind, menu } = useContextMenu();
  // B25 §2: the message menu, the same actions as the row's toolbar. Deleted
  // messages and hidden (blocked) ones have no items, so no menu opens.
  const authorName = (m: Message) => {
    const a = memberMap.get(m.authorId);
    return a ? displayName(a) : '…';
  };
  const messageItems = (m: Message): MenuItem[] =>
    m.deleted || (memberMap.get(m.authorId)?.blocked && !revealed.has(m.authorId))
      ? []
      : [
          { key: 'reply', label: intl.formatMessage({ id: 'chat.reply' }), onSelect: () => handlers.onReply(m) },
          {
            key: 'copy',
            label: intl.formatMessage({ id: 'menu.copyText' }),
            // The stored body: the direction marks are added only when it is shown.
            onSelect: async () => store.showFlash((await copyText(m.body)) ? 'common.copied' : 'menu.copyFailed'),
          },
          ...(m.authorId === myId ? [{ key: 'edit', label: intl.formatMessage({ id: 'chat.edit' }), onSelect: () => handlers.onEdit(m) }] : []),
          { kind: 'separator' },
          ...(m.authorId === myId || isOwner
            ? [{ key: 'delete', label: intl.formatMessage({ id: 'chat.delete' }), danger: true, onSelect: () => handlers.onDelete(m) }]
            : []),
        ];

  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 56,
    overscan: 10,
    getItemKey: (i) => i,
  });
  const items = virtualizer.getVirtualItems();
  const first = items[0]?.index ?? 0;
  const last = items[items.length - 1]?.index ?? 0;

  // Fetch the visible range (plus a margin) from the core.
  useEffect(() => {
    if (count) win.ensure(first - 40, last + 40);
  }, [win, first, last, count]);

  // Pin to the bottom while the reader is there: new messages and late row
  // measurements must not leave a gap under the last message.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && atBottom.current && el.scrollHeight - el.scrollTop - el.clientHeight > 1) {
      el.scrollTop = el.scrollHeight;
    }
  });

  useLayoutEffect(() => {
    if (stickToken === 0) return;
    atBottom.current = true;
    virtualizer.scrollToIndex(count - 1, { align: 'end' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stickToken]);

  // Move DOM focus after keyboard navigation once the row is rendered.
  useEffect(() => {
    if (!focusPending.current || active === null) return;
    const row = scrollRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`);
    if (row) {
      row.focus({ preventScroll: true });
      focusPending.current = false;
    }
  });

  const onScroll = () => {
    const el = scrollRef.current!;
    atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < BOTTOM_SLACK_PX;
    setShowJump(!atBottom.current);
  };

  const goTo = (index: number, align: 'start' | 'center' | 'end' | 'auto' = 'auto') => {
    const i = Math.max(0, Math.min(count - 1, index));
    atBottom.current = i === count - 1;
    virtualizer.scrollToIndex(i, { align });
    setActive(i);
    focusPending.current = true;
  };

  const goToRef = useRef(goTo);
  goToRef.current = goTo;
  const jumpToReply = useCallback((index: number) => {
    goToRef.current(index, 'center');
    setFlash(index);
    setTimeout(() => setFlash((f) => (f === index ? null : f)), 1600);
  }, []);
  const reveal = useCallback((authorId: string) => setRevealed((s) => new Set(s).add(authorId)), []);

  const onKeyDown = (e: KeyboardEvent) => {
    const row = (e.target as HTMLElement).closest<HTMLElement>('[data-index]');
    if (!row || e.target !== row) return;
    const i = Number(row.dataset.index);
    const page = Math.max(1, last - first - 2);
    const m = win.get(i);
    switch (e.key) {
      case 'ArrowUp':
        goTo(i - 1);
        break;
      case 'ArrowDown':
        goTo(i + 1);
        break;
      case 'PageUp':
        goTo(i - page);
        break;
      case 'PageDown':
        goTo(i + page);
        break;
      case 'Home':
        goTo(0, 'start');
        break;
      case 'End':
        goTo(count - 1, 'end');
        break;
      case 'r':
        if (m && !m.deleted) handlers.onReply(m);
        break;
      case 'e':
        if (m && !m.deleted && m.authorId === myId) handlers.onEdit(m);
        break;
      case 'Delete':
        if (m && !m.deleted && (m.authorId === myId || isOwner)) handlers.onDelete(m);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const tabRow = active ?? count - 1;

  return (
    <div className="message-area">
      <div
        ref={scrollRef}
        className="message-scroll"
        onScroll={onScroll}
        onKeyDown={onKeyDown}
        role="feed"
        aria-busy={!win.ready}
        aria-label={intl.formatMessage({ id: 'chat.messages' }, { channel: channelName })}
      >
        {win.ready && count === 0 && (
          <p className="chat-empty">
            <FormattedMessage id="chat.empty" values={{ channel: channelName }} />
          </p>
        )}
        <div className="message-canvas" style={{ height: virtualizer.getTotalSize() }}>
          {items.map((item) => {
            const m = win.get(item.index);
            const prev = item.index > 0 ? win.get(item.index - 1) : undefined;
            const grouped =
              !!m &&
              !!prev &&
              !prev.deleted &&
              !m.replyTo &&
              prev.authorId === m.authorId &&
              m.createdAt - prev.createdAt < GROUP_GAP_MS;
            return (
              <div
                key={item.key}
                data-index={item.index}
                ref={virtualizer.measureElement}
                className={`row ${flash === item.index ? 'flash' : ''}`}
                style={{ transform: `translateY(${item.start}px)` }}
                tabIndex={item.index === tabRow ? 0 : -1}
                role="article"
                aria-posinset={item.index + 1}
                aria-setsize={count}
                onFocus={(e) => {
                  if (e.target === e.currentTarget) setActive(item.index);
                }}
                {...(m && !m.deleted
                  ? bind(intl.formatMessage({ id: 'menu.forMessage' }, { name: authorName(m) }), () => messageItems(m))
                  : {})}
              >
                {m ? (
                  <MessageRow
                    message={m}
                    author={memberMap.get(m.authorId)}
                    replyAuthor={m.replyTo ? memberMap.get(m.replyTo.authorId) : undefined}
                    grouped={grouped}
                    lookalike={flagged.has(m.authorId)}
                    mine={m.authorId === myId}
                    canDelete={m.authorId === myId || isOwner}
                    actionsTabbable={item.index === tabRow}
                    revealed={revealed.has(m.authorId)}
                    onReveal={reveal}
                    onJumpToReply={jumpToReply}
                    {...handlers}
                  />
                ) : (
                  <div className="msg skeleton" aria-hidden="true">
                    <span className="sk-avatar" />
                    <span className="sk-lines">
                      <span />
                      <span />
                    </span>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
      {showJump && (
        <button type="button" className="jump-latest" onClick={() => goTo(count - 1, 'end')}>
          <ArrowDownIcon />
          <FormattedMessage id="chat.jumpToLatest" />
        </button>
      )}
      {menu}
    </div>
  );
}

const MessageRow = memo(function MessageRow({
  message: m,
  author,
  replyAuthor,
  grouped,
  lookalike,
  mine,
  canDelete,
  actionsTabbable,
  revealed,
  onReveal,
  onJumpToReply,
  onReply,
  onEdit,
  onDelete,
  onRetry,
}: {
  message: Message;
  author?: Member;
  replyAuthor?: Member;
  grouped: boolean;
  lookalike: boolean;
  mine: boolean;
  canDelete: boolean;
  actionsTabbable: boolean;
  revealed: boolean;
  onReveal: (authorId: string) => void;
  onJumpToReply: (index: number) => void;
} & Handlers) {
  const intl = useIntl();
  const uiDir = useUiDir();
  const lines = useMemo(() => paragraphs(m.body, uiDir), [m.body, uiDir]);
  const name = author ? displayName(author) : '…';
  const tab = actionsTabbable ? 0 : -1;
  const time = new Date(m.createdAt);
  const sameDay = new Date().toDateString() === time.toDateString();
  const timeLabel = sameDay
    ? intl.formatTime(time, { hour: '2-digit', minute: '2-digit' })
    : intl.formatDate(time, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
  const fullTime = intl.formatDate(time, { dateStyle: 'full', timeStyle: 'short' });

  if (m.deleted) {
    return (
      <div className="msg deleted">
        <TrashIcon />
        <span className="muted small">
          <FormattedMessage id="chat.deleted" />
        </span>
      </div>
    );
  }

  if (author?.blocked && !revealed) {
    return (
      <div className="msg blocked">
        <span className="muted small">
          <FormattedMessage id="chat.blocked" />
        </span>
        <button type="button" className="link small" tabIndex={tab} onClick={() => onReveal(m.authorId)}>
          <FormattedMessage id="chat.showAnyway" />
        </button>
      </div>
    );
  }

  return (
    <div className={`msg ${grouped ? 'grouped' : ''} ${m.mentionsMe ? 'mention' : ''} ${m.sendState === 'failed' ? 'failed' : ''}`}>
      {m.replyTo && (
        <button
          type="button"
          className="reply-ref"
          tabIndex={tab}
          onClick={() => onJumpToReply(m.replyTo!.index)}
          aria-label={intl.formatMessage({ id: 'chat.repliedMessage' })}
        >
          <ReplyIcon />
          <Name>{replyAuthor ? displayName(replyAuthor) : '…'}</Name>
          <span dir={textDir(m.replyTo.excerpt, uiDir)} className="reply-excerpt user-text">
            {m.replyTo.excerpt || intl.formatMessage({ id: 'chat.deleted' })}
          </span>
        </button>
      )}
      <div className="msg-main">
        <div className="msg-gutter">
          {grouped ? (
            <time className="hover-time" dateTime={time.toISOString()} title={fullTime}>
              {intl.formatTime(time, { hour: '2-digit', minute: '2-digit' })}
            </time>
          ) : (
            <Avatar name={name} color={author?.avatarColor ?? '#888'} size={40} />
          )}
        </div>
        <div className="msg-content">
          {!grouped && (
            <div className="msg-head">
              <Name className="author">{name}</Name>
              {lookalike && <LookalikeTag />}
              {author && (
                <Code value={author.shortCode} label={intl.formatMessage({ id: 'chat.code' }, { code: author.shortCode })} />
              )}
              <time dateTime={time.toISOString()} title={fullTime}>
                {timeLabel}
              </time>
            </div>
          )}
          <p className="msg-body">
            {lines.map((l, i) => (
              <span key={i} className="msg-line user-text" dir={l.dir}>
                {l.text}
                {m.editedAt && i === lines.length - 1 && (
                  <>
                    {' '}
                    <span className="edited" title={intl.formatDate(m.editedAt, { dateStyle: 'full', timeStyle: 'short' })}>
                      <FormattedMessage id="chat.edited" />
                    </span>
                  </>
                )}
              </span>
            ))}
          </p>
          {m.clipped && (
            <p className="muted small clipped-note">
              <FormattedMessage id="chat.clipped" />
            </p>
          )}
          {mine && m.sendState && <SendStateView message={m} onRetry={onRetry} tab={tab} />}
        </div>
      </div>
      <div className="msg-actions" role="toolbar" aria-label={name}>
        <button type="button" tabIndex={tab} onClick={() => onReply(m)} aria-label={intl.formatMessage({ id: 'chat.reply' })} title={intl.formatMessage({ id: 'chat.reply' })}>
          <ReplyIcon />
        </button>
        {mine && (
          <button type="button" tabIndex={tab} onClick={() => onEdit(m)} aria-label={intl.formatMessage({ id: 'chat.edit' })} title={intl.formatMessage({ id: 'chat.edit' })}>
            <EditIcon />
          </button>
        )}
        {canDelete && (
          <button
            type="button"
            tabIndex={tab}
            className="danger-icon"
            onClick={() => onDelete(m)}
            aria-label={intl.formatMessage({ id: 'chat.delete' })}
            title={intl.formatMessage({ id: 'chat.delete' })}
          >
            <TrashIcon />
          </button>
        )}
      </div>
    </div>
  );
});

function SendStateView({ message, onRetry, tab }: { message: Message; onRetry: (m: Message) => void; tab: number }) {
  const intl = useIntl();
  switch (message.sendState) {
    case 'pending':
      return (
        <span className="send-state pending">
          <ClockIcon />
          <FormattedMessage id="chat.state.pending" />
        </span>
      );
    case 'sent':
      return (
        <span className="send-state sent" role="img" aria-label={intl.formatMessage({ id: 'chat.state.sent' })} title={intl.formatMessage({ id: 'chat.state.sent' })}>
          <CheckIcon />
        </span>
      );
    case 'received':
      return (
        <span
          className="send-state received"
          role="img"
          aria-label={intl.formatMessage({ id: 'chat.state.received' })}
          title={intl.formatMessage({ id: 'chat.state.received' })}
        >
          <DoubleCheckIcon />
        </span>
      );
    case 'failed':
      return (
        <span className="send-state failed">
          <AlertIcon />
          <FormattedMessage id="chat.state.failed" />
          <button type="button" className="link small" tabIndex={tab} onClick={() => onRetry(message)}>
            <FormattedMessage id="common.retry" />
          </button>
        </span>
      );
    default:
      return null;
  }
}

// Voice rooms, 0.4.3 (docs/ux/specs/voice-rooms.md, "Join to create"). The
// pinned "Join chat" row opens a new room with me as its host; the live rooms
// sit under it, oldest first, each with who is in it and a crown on the host.
// Rooms are soft state from the core: nothing here is stored.

import { useEffect, useId, useRef, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { Avatar, Dialog, Name } from '../../components/common';
import { useContextMenu, type MenuItem } from '../../components/ContextMenu';
import { CrownIcon, MicOffIcon, SpeakerIcon } from '../../components/icons';
import { isCoreError, useCore, type Id, type Member, type ServerSummary, type VoiceRoom } from '../../core';
import { VOICE_ROOM_LIMIT_MAX, VOICE_ROOM_NAME_MAX, VOICE_ROOMS_MAX } from '../../core/sanitize';
import { CallError, codeOf, FAILURE_TEXT, ipNoticeSeen, rememberIpNotice, type CallEngine } from './CallBar';

const NO_MEMBERS: Member[] = [];
const NO_ROOMS: VoiceRoom[] = [];
/** Spec §2: rooms past this many collapse under "N more rooms". */
const SHOWN_ROOMS = 8;
const LIMITS = [2, 3, 4, 5].filter((n) => n <= VOICE_ROOM_LIMIT_MAX);

/** What the IP notice is in front of: opening a room, or joining one. */
type Pending = { kind: 'create' } | { kind: 'join'; roomId: Id };

export function VoiceRooms({ server, myId, engine }: { server: ServerSummary; myId: Id; engine: CallEngine }) {
  const store = useAppStore();
  const core = useCore();
  const intl = useIntl();
  const serverId = server.id;
  const rooms = useStore(store, (s) => s.voiceRooms[serverId] ?? NO_ROOMS);
  const members = useStore(store, (s) => s.members[serverId] ?? NO_MEMBERS);
  const relayOnly = useStore(store, (s) => s.relayOnly);
  const callError = useStore(store, (s) => s.callError);
  const callRoomId = useStore(store, (s) => s.call?.channelId ?? null);
  const { bind, menu } = useContextMenu();
  const [pending, setPending] = useState<Pending | null>(null);
  const [renaming, setRenaming] = useState<Id | null>(null);
  const [limitFor, setLimitFor] = useState<Id | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [announce, setAnnounce] = useState('');
  const rootRef = useRef<HTMLButtonElement>(null);
  const okRef = useRef<HTMLButtonElement>(null);
  const returnTo = useRef<HTMLElement | null>(null);
  const bodyId = useId();
  const hintId = useId();
  const reasonId = useId();

  // Events are hints: re-read the rooms whenever this server comes on screen.
  useEffect(() => {
    void store.loadVoiceRooms(serverId);
  }, [store, serverId]);
  // Spec §3 (voice-ip-notice): focus starts on "Got it".
  useEffect(() => {
    if (pending) okRef.current?.focus();
  }, [pending]);

  const byId = new Map(members.map((m) => [m.id, m]));
  const nameOfMember = (id: Id) => byId.get(id)?.displayName || codeOf(id);
  const roomName = (r: VoiceRoom) => r.name ?? intl.formatMessage({ id: 'voiceRooms.defaultName' }, { name: nameOfMember(r.masterId) });
  const owner = server.ownerId === myId;

  // Spec §4: when the host of my room changes, say so to the people in it.
  const masters = useRef(new Map<Id, Id>());
  useEffect(() => {
    const mine = rooms.find((r) => r.roomId === callRoomId);
    const before = mine ? masters.current.get(mine.roomId) : undefined;
    if (mine && before && before !== mine.masterId) {
      setAnnounce(intl.formatMessage({ id: 'voiceRooms.newMaster' }, { name: nameOfMember(mine.masterId) }));
    }
    masters.current = new Map(rooms.map((r) => [r.roomId, r.masterId]));
    // nameOfMember reads the latest members; re-running on members alone isn't news.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rooms, callRoomId, intl]);

  const capped = rooms.length >= VOICE_ROOMS_MAX;
  const rootBlocked = relayOnly ? 'call.error.relayOnly' : capped ? 'voiceRooms.capReached' : null;

  const start = (action: Pending, from: HTMLElement | null) => {
    returnTo.current = from;
    // In a call, computers connect directly: say so before the first create or join.
    if (!ipNoticeSeen()) setPending(action);
    else run(action);
  };
  const run = (action: Pending) => (action.kind === 'create' ? store.createVoiceRoom(serverId) : store.joinCall(action.roomId));

  const edit = (room: VoiceRoom, change: { name?: string; limit?: number }) =>
    core.editVoiceRoom(room.roomId, change.name, change.limit).catch((e) => {
      store.showFlash(isCoreError(e) && e.code === 'not-allowed' ? 'voiceRooms.notAllowed' : 'voiceRooms.editFailed');
    });

  const items = (room: VoiceRoom, blocked: boolean, inRoom: boolean, row: () => HTMLElement | null): MenuItem[] => {
    const canEdit = room.masterId === myId || owner;
    return [
      inRoom
        ? { key: 'leave', label: intl.formatMessage({ id: 'voiceRooms.leave' }), onSelect: () => void store.leaveCall() }
        : blocked
          ? { kind: 'separator' as const }
          : { key: 'join', label: intl.formatMessage({ id: 'voiceRooms.join' }), onSelect: () => start({ kind: 'join', roomId: room.roomId }, row()) },
      {
        key: 'copy',
        label: intl.formatMessage({ id: 'voiceRooms.copyName' }),
        onSelect: () => void navigator.clipboard?.writeText(roomName(room)).catch(() => {}),
      },
      ...(canEdit
        ? [
            { kind: 'separator' as const },
            { key: 'rename', label: intl.formatMessage({ id: 'voiceRooms.rename' }), onSelect: () => setRenaming(room.roomId) },
            {
              key: 'limit',
              label: intl.formatMessage({ id: 'voiceRooms.limit' }),
              onSelect: () => {
                returnTo.current = row();
                setLimitFor(room.roomId);
              },
            },
          ]
        : []),
    ];
  };

  const shown = expanded || rooms.length <= SHOWN_ROOMS ? rooms : rooms.slice(0, SHOWN_ROOMS);
  const limitRoom = rooms.find((r) => r.roomId === limitFor) ?? null;

  return (
    <div className="voice-room voice-rooms">
      <button
        type="button"
        ref={rootRef}
        className="channel voice-room-entry voice-root"
        aria-label={intl.formatMessage({ id: 'call.room' })}
        aria-describedby={[rooms.length ? hintId : null, rootBlocked ? reasonId : null].filter(Boolean).join(' ') || undefined}
        aria-disabled={rootBlocked ? true : undefined}
        onClick={(e) => !rootBlocked && start({ kind: 'create' }, e.currentTarget)}
      >
        <SpeakerIcon />
        <span className="voice-root-text">
          <span className="channel-name">
            <FormattedMessage id="call.room" />
          </span>
          {rooms.length > 0 && (
            <span className="voice-root-hint" id={hintId}>
              <FormattedMessage id="voiceRooms.createHint" />
            </span>
          )}
        </span>
      </button>
      {rootBlocked && (
        <p id={reasonId} className="voice-room-reason">
          <FormattedMessage id={rootBlocked} />
          {relayOnly && (
            <>
              {' '}
              <button type="button" className="link small" onClick={() => store.openSettings('privacy')}>
                <FormattedMessage id="privacy.relayOnly.open" />
              </button>
            </>
          )}
        </p>
      )}
      {!rootBlocked && callError && <CallError />}

      {rooms.length > 0 && (
        <ul className="voice-room-list">
          {shown.map((room) => {
            const inRoom = room.roomId === callRoomId || room.members.some((p) => p.userId === myId);
            const full = !inRoom && room.members.length >= room.limit;
            const blocked = relayOnly || full;
            const name = roomName(room);
            const label = [
              intl.formatMessage({ id: 'voiceRooms.roomLabel' }, { name, count: room.members.length, max: room.limit }),
              inRoom ? intl.formatMessage({ id: 'call.roomIn' }) : full ? intl.formatMessage({ id: 'call.roomFull' }) : null,
            ]
              .filter(Boolean)
              .join(', ');
            const rowId = `voice-room-${room.roomId}`;
            const row = () => document.getElementById(rowId);
            const menuLabel = intl.formatMessage({ id: 'menu.forChannel' }, { channel: name });
            const callHere = room.roomId === callRoomId;
            return (
              <li key={room.roomId} className={`voice-room-item ${inRoom ? 'in-room' : ''}`}>
                {renaming === room.roomId ? (
                  <RenameField
                    room={room}
                    placeholder={intl.formatMessage({ id: 'voiceRooms.defaultName' }, { name: nameOfMember(room.masterId) })}
                    label={intl.formatMessage({ id: 'voiceRooms.rename' })}
                    onDone={(value) => {
                      setRenaming(null);
                      setTimeout(() => row()?.focus());
                      if (value !== null && value !== (room.name ?? '')) void edit(room, { name: value });
                    }}
                  />
                ) : (
                  <button
                    type="button"
                    id={rowId}
                    className={`channel voice-room-entry voice-room-row ${inRoom ? 'in-room' : ''}`}
                    aria-label={label}
                    aria-disabled={blocked || inRoom ? true : undefined}
                    aria-describedby={relayOnly ? reasonId : undefined}
                    title={full ? intl.formatMessage({ id: 'call.roomFull' }) : undefined}
                    {...bind(menuLabel, () => items(room, blocked, inRoom, row))}
                    onClick={(e) => !blocked && !inRoom && start({ kind: 'join', roomId: room.roomId }, e.currentTarget)}
                  >
                    <SpeakerIcon />
                    <bdi className="channel-name">{name}</bdi>
                    <span className={`voice-room-count ${room.members.length >= room.limit ? 'full' : ''}`} aria-hidden="true">
                      <bdi dir="ltr">
                        {room.members.length}/{room.limit}
                      </bdi>
                    </span>
                  </button>
                )}
                <ul className="voice-room-people" aria-label={intl.formatMessage({ id: 'call.roomPeople' })}>
                  {room.members.map((p) => {
                    const m = byId.get(p.userId);
                    const who = nameOfMember(p.userId);
                    const failure = callHere && p.userId !== myId ? engine.peerFailed[p.userId] : undefined;
                    const [failText, failHint] = failure ? (FAILURE_TEXT[failure] ?? FAILURE_TEXT.failed) : ['', ''];
                    return (
                      <li key={p.userId} className={callHere && engine.speaking[p.userId] ? 'speaking' : undefined}>
                        <Avatar name={who} color={m?.color ?? m?.avatarColor ?? '#888'} src={m?.avatarDataUrl} size={20} />
                        <span className="call-person">
                          <Name>{who}</Name>
                          {failure && (
                            <span className={`call-peer-failed ${failure}`} title={intl.formatMessage({ id: failHint })}>
                              <FormattedMessage id={failText} />
                            </span>
                          )}
                        </span>
                        {p.userId === room.masterId && (
                          <span className="voice-crown" role="img" aria-label={intl.formatMessage({ id: 'voiceRooms.master' })} title={intl.formatMessage({ id: 'voiceRooms.master' })}>
                            <CrownIcon />
                          </span>
                        )}
                        {p.muted && (
                          <span className="call-muted" role="img" aria-label={intl.formatMessage({ id: 'call.micOff' })}>
                            <MicOffIcon />
                          </span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </li>
            );
          })}
        </ul>
      )}
      {!expanded && rooms.length > SHOWN_ROOMS && (
        <button type="button" className="link small voice-rooms-more" onClick={() => setExpanded(true)}>
          <FormattedMessage id="voiceRooms.more" values={{ count: rooms.length - SHOWN_ROOMS }} />
        </button>
      )}
      <span className="visually-hidden" aria-live="polite">
        {announce}
      </span>
      {menu}

      <Dialog
        open={!!pending}
        onClose={() => {
          setPending(null);
          returnTo.current?.focus();
        }}
        title={intl.formatMessage({ id: 'call.ipNotice.title' })}
        describedBy={bodyId}
        className="ip-notice"
      >
        <p id={bodyId}>
          <FormattedMessage id="call.ipNotice.body" />
        </p>
        <div className="actions">
          <button
            type="button"
            className="primary"
            ref={okRef}
            onClick={async () => {
              const action = pending;
              rememberIpNotice();
              setPending(null);
              if (action) await run(action);
              document.querySelector<HTMLButtonElement>('.call-bar .call-mute')?.focus();
            }}
          >
            <FormattedMessage id="call.ipNotice.ok" />
          </button>
          <button
            type="button"
            className="secondary"
            onClick={() => {
              setPending(null);
              returnTo.current?.focus();
            }}
          >
            <FormattedMessage id="common.cancel" />
          </button>
        </div>
      </Dialog>

      <Dialog
        open={!!limitRoom}
        onClose={() => {
          setLimitFor(null);
          returnTo.current?.focus();
        }}
        title={intl.formatMessage({ id: 'voiceRooms.limit' })}
        className="voice-limit-dialog"
      >
        {limitRoom && (
          <div className="voice-limit-options" role="group" aria-label={roomName(limitRoom)}>
            {LIMITS.map((n) => (
              <button
                key={n}
                type="button"
                className={n === limitRoom.limit ? 'primary' : 'secondary'}
                aria-pressed={n === limitRoom.limit}
                // Spec §3: never below the people already inside.
                disabled={n < limitRoom.members.length}
                onClick={() => {
                  setLimitFor(null);
                  returnTo.current?.focus();
                  if (n !== limitRoom.limit) void edit(limitRoom, { limit: n });
                }}
              >
                <FormattedMessage id="voiceRooms.limitValue" values={{ n }} />
              </button>
            ))}
          </div>
        )}
      </Dialog>
    </div>
  );
}

/** Spec §3: inline rename. Enter saves (empty = the default name), Esc or leaving the field cancels. */
function RenameField({ room, placeholder, label, onDone }: { room: VoiceRoom; placeholder: string; label: string; onDone: (value: string | null) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (value: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(value);
  };
  return (
    <input
      ref={ref}
      className="voice-room-rename"
      defaultValue={room.name ?? ''}
      placeholder={placeholder}
      aria-label={label}
      maxLength={VOICE_ROOM_NAME_MAX}
      dir="auto"
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          finish(e.currentTarget.value.trim());
        } else if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          finish(null);
        }
      }}
      onBlur={() => finish(null)}
    />
  );
}

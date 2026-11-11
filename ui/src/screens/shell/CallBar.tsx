// Voice v1, piece E (#223) and E2 (voice-room amendment): the UI around a call.
// Join the server's voice room from the channel sidebar, see who is in it,
// mute, leave. The audio itself (microphone, WebRTC, speaking levels) is the
// voice engine's, piece D in ui/src/voice.

import { useEffect, useId, useRef, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { Avatar, Dialog, Name } from '../../components/common';
import { AlertIcon, HangUpIcon, MicIcon, MicOffIcon, PhoneIcon, SpeakerIcon } from '../../components/icons';
import { useCore, type Id, type Member } from '../../core';
import { useVoiceEngine, type PeerFailure, type VoiceEngineView } from '../../voice';

/**
 * The voice engine (piece D, ui/src/voice) as the call UI uses it. `peerFailed`
 * comes from the engine (#245): `failed` (the connection failed) or `refused` (a
 * security check failed: fingerprint or unverified signal).
 */
export interface CallEngine extends VoiceEngineView {
  /** No microphone, denied, or one picked after joining without one: listen-only. */
  noMic: boolean;
}

/**
 * The one engine for my call. Call it once (the channel sidebar does) and pass
 * it down: each useVoiceEngine makes its own WebRTC engine.
 *
 * For demos and QA with the mock, `?voiceSim=noMic|rejoin|peerFailed|peerRefused`
 * acts out the engine's failure states, on top of what the engine reports.
 */
export function useCallEngine(myId: Id): CallEngine {
  const store = useAppStore();
  const core = useCore();
  const call = useStore(store, (s) => s.call);
  const stunUrl = useStunUrl();
  const view = useVoiceEngine(call, myId, { stunUrl });
  const sim = typeof location !== 'undefined' ? new URLSearchParams(location.search).get('voiceSim') : null;
  const error = call && sim === 'noMic' ? 'no-device' : call && sim === 'rejoin' ? 'rejoin-to-talk' : view.error;
  const other = call?.participants.find((p) => p.identity !== myId)?.identity;
  // No microphone (none, permission denied, or unplugged mid-call): stay in the
  // call to listen, and show the others I'm muted (#232 V5.7/V5.8). rejoin-to-talk
  // is still listen-only: the new mic only works after a rejoin.
  const noMic = !!call && (error === 'no-device' || error === 'mic-denied' || error === 'rejoin-to-talk');
  const meMuted = !!call?.participants.find((p) => p.identity === myId)?.muted;
  useEffect(() => {
    if (noMic && !meMuted) void core.setMuted(true).catch(() => {});
  }, [noMic, meMuted, core]);
  const simFailure: PeerFailure | null = sim === 'peerFailed' ? 'failed' : sim === 'peerRefused' ? 'refused' : null;
  const peerFailed = simFailure && other ? { ...view.peerFailed, [other]: simFailure } : view.peerFailed;
  return { ...view, error, noMic, peerFailed };
}

/**
 * Our STUN server (#222): the desktop shell puts it on every core.status (the
 * latest one reaches late listeners too). Null without one: calls then work
 * on the same network only.
 */
function useStunUrl(): string | null {
  const core = useCore();
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => core.on('core.status', (s) => setUrl(s.stunUrl ?? null)), [core]);
  return url;
}

/** The short code the app shows for someone not (yet) in the member list. */
function codeOf(id: Id): string {
  const s = id.replace(/[^0-9a-z]/gi, '').slice(0, 8).toUpperCase();
  return `${s.slice(0, 4)}-${s.slice(4)}`;
}

const ERROR_TEXT: Record<string, string> = {
  'call-full': 'call.error.full',
  'relay-only': 'call.error.relayOnly',
  'in-call-elsewhere': 'call.error.inCallElsewhere',
  'not-implemented': 'call.error.notImplemented',
  'not-allowed': 'call.error.notAllowed',
};

const NO_MEMBERS: Member[] = [];
const FAILURE_TEXT: Record<PeerFailure, [text: string, hint: string]> = {
  failed: ['call.peerFailed', 'call.peerFailedHint'],
  refused: ['call.peerRefused', 'call.peerRefusedHint'],
};
const ROOM_MAX = 5;

/** docs/ux/specs/voice-ip-notice.md: shown once per computer, before the first join. */
const IP_NOTICE_KEY = 'mvs.notices.voiceIp.v1';
function ipNoticeSeen(): boolean {
  try {
    return localStorage.getItem(IP_NOTICE_KEY) === '1';
  } catch {
    return false; // no storage: explain every time rather than join unexplained
  }
}
function rememberIpNotice(): void {
  try {
    localStorage.setItem(IP_NOTICE_KEY, '1');
  } catch {
    // Shown again next time.
  }
}

/**
 * The server's voice room in the channel sidebar: "Join chat" with a speaker
 * icon, and who is inside under it, for every member. Disabled, with the reason
 * under it, when the room is full or calls are off (relay-only).
 */
export function VoiceRoomEntry({ serverId, myId, engine }: { serverId: Id; myId: Id; engine: CallEngine }) {
  const store = useAppStore();
  const intl = useIntl();
  const room = useStore(store, (s) => s.voiceRooms[serverId]);
  const members = useStore(store, (s) => s.members[serverId] ?? NO_MEMBERS);
  const relayOnly = useStore(store, (s) => s.relayOnly);
  const callError = useStore(store, (s) => s.callError);
  const callHere = useStore(store, (s) => !!s.call && s.call.channelId === s.voiceRooms[serverId]?.roomId);
  const [notice, setNotice] = useState(false);
  const entryRef = useRef<HTMLButtonElement>(null);
  const okRef = useRef<HTMLButtonElement>(null);
  // Spec §3: focus starts on "Got it" (the dialog opens in its own effect, which runs first).
  useEffect(() => {
    if (notice) okRef.current?.focus();
  }, [notice]);
  const bodyId = useId();
  // Events are hints: re-read the room whenever this server comes on screen.
  useEffect(() => {
    void store.loadVoiceRoom(serverId);
  }, [store, serverId]);

  const people = room?.participants ?? [];
  const inRoom = people.some((p) => p.userId === myId);
  const full = !inRoom && people.length >= ROOM_MAX;
  const blocked = relayOnly ? 'call.error.relayOnly' : full ? 'call.error.full' : null;
  const title = inRoom ? 'call.roomIn' : full ? 'call.roomFull' : 'call.room';
  const count = intl.formatMessage({ id: 'call.roomCount' }, { count: people.length, max: ROOM_MAX });
  const byId = new Map(members.map((m) => [m.id, m]));
  const reasonId = `voice-room-reason-${serverId}`;
  const label = [intl.formatMessage({ id: title }), people.length ? count : null].filter(Boolean).join(', ');

  return (
    <div className="voice-room">
      <button
        type="button"
        ref={entryRef}
        className={`channel voice-room-entry ${inRoom ? 'in-room' : ''}`}
        aria-label={label}
        aria-disabled={blocked || inRoom ? true : undefined}
        aria-describedby={blocked ? reasonId : undefined}
        disabled={!room}
        onClick={() => {
          if (!room || blocked || inRoom) return;
          // In a call, computers connect directly: say so before the first join.
          if (!ipNoticeSeen()) setNotice(true);
          else void store.joinCall(room.roomId);
        }}
      >
        <SpeakerIcon />
        <span className="channel-name">
          <FormattedMessage id={title} />
        </span>
        {people.length > 0 && (
          // The button's label already says the count; this is its short form.
          <span className="voice-room-count" aria-hidden="true" title={count}>
            <bdi dir="ltr">
              {people.length}/{ROOM_MAX}
            </bdi>
          </span>
        )}
      </button>
      {blocked && (
        <p id={reasonId} className="voice-room-reason">
          <FormattedMessage id={blocked} />
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
      {!blocked && callError && <CallError />}
      <Dialog
        open={notice}
        onClose={() => {
          setNotice(false);
          entryRef.current?.focus();
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
              rememberIpNotice();
              setNotice(false);
              if (room) await store.joinCall(room.roomId);
              document.querySelector<HTMLButtonElement>('.call-bar .call-mute')?.focus();
            }}
          >
            <FormattedMessage id="call.ipNotice.ok" />
          </button>
          <button
            type="button"
            className="secondary"
            onClick={() => {
              setNotice(false);
              entryRef.current?.focus();
            }}
          >
            <FormattedMessage id="common.cancel" />
          </button>
        </div>
      </Dialog>
      {people.length > 0 && (
        <ul className="voice-room-people" aria-label={intl.formatMessage({ id: 'call.roomPeople' })}>
          {people.map((p) => {
            const m = byId.get(p.userId);
            const name = m?.displayName || codeOf(p.userId);
            return (
              <li key={p.userId} className={callHere && engine.speaking[p.userId] ? 'speaking' : undefined}>
                <Avatar name={name} color={m?.color ?? m?.avatarColor ?? '#888'} src={m?.avatarDataUrl} size={20} />
                <span className="call-person">
                  <Name>{name}</Name>
                  {callHere && p.userId !== myId && engine.peerFailed[p.userId] && (
                    <span
                      className={`call-peer-failed ${engine.peerFailed[p.userId]}`}
                      title={intl.formatMessage({ id: (FAILURE_TEXT[engine.peerFailed[p.userId]] ?? FAILURE_TEXT.failed)[1] })}
                    >
                      <FormattedMessage id={(FAILURE_TEXT[engine.peerFailed[p.userId]] ?? FAILURE_TEXT.failed)[0]} />
                    </span>
                  )}
                </span>
                {p.muted && (
                  <span className="call-muted" role="img" aria-label={intl.formatMessage({ id: 'call.micOff' })}>
                    <MicOffIcon />
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** Why joining failed, under the voice room entry, until dismissed or retried. */
export function CallError() {
  const store = useAppStore();
  const intl = useIntl();
  const code = useStore(store, (s) => s.callError);
  if (!code) return null;
  return (
    <div className="offline-banner call-blocked" role="status">
      <AlertIcon />
      <span>
        <FormattedMessage id={ERROR_TEXT[code] ?? 'call.error.other'} />
      </span>
      <button type="button" className="link small" onClick={() => store.clearCallError()} aria-label={intl.formatMessage({ id: 'common.close' })}>
        <FormattedMessage id="common.close" />
      </button>
    </div>
  );
}

/**
 * My call: where, mute and leave. In the channel sidebar, above my panel. Who
 * is in it shows once, in the room entry's roster.
 */
export function CallBar({ myId, engine }: { myId: Id; engine: CallEngine }) {
  const store = useAppStore();
  const core = useCore();
  const intl = useIntl();
  const call = useStore(store, (s) => s.call);
  // The call may be in another server's room than the one on screen.
  const serverId = useStore(store, (s) => Object.values(s.voiceRooms).find((r) => r.roomId === s.call?.channelId)?.serverId);
  const serverName = useStore(store, (s) => s.servers.find((x) => x.id === serverId)?.name ?? '');
  if (!call) return null;

  const me = call.participants.find((p) => p.identity === myId);
  const noMic = engine.noMic;
  const noMicText = intl.formatMessage({ id: engine.error === 'rejoin-to-talk' ? 'call.rejoinToTalk' : 'call.noMic' });
  const toggleMute = () => {
    if (noMic) return;
    const muted = !me?.muted;
    engine.setMicMuted(muted);
    void core.setMuted(muted).catch(() => {});
  };

  return (
    <section className="call-bar" aria-label={intl.formatMessage({ id: 'call.label' })}>
      <p className="call-where">
        <PhoneIcon />
        <FormattedMessage id="call.inCall" values={{ server: serverName }} />
      </p>
      {noMic && (
        <p id="call-no-mic" className="call-no-mic" role="status">
          <MicOffIcon />
          {noMicText}
        </p>
      )}
      <div className="call-actions">
        <button
          type="button"
          className={`icon-button call-mute ${me?.muted ? 'muted' : ''}`}
          aria-pressed={!!me?.muted}
          aria-disabled={noMic || undefined}
          aria-describedby={noMic ? 'call-no-mic' : undefined}
          aria-label={intl.formatMessage({ id: 'call.mute' })}
          title={noMic ? noMicText : intl.formatMessage({ id: 'call.mute' })}
          onClick={toggleMute}
        >
          {me?.muted ? <MicOffIcon /> : <MicIcon />}
        </button>
        <button
          type="button"
          className="icon-button call-leave"
          aria-label={intl.formatMessage({ id: 'call.leave' })}
          title={intl.formatMessage({ id: 'call.leave' })}
          onClick={() => void store.leaveCall()}
        >
          <HangUpIcon />
        </button>
      </div>
    </section>
  );
}

// Voice v1, piece E (#223) and E2 (voice-room amendment): the UI around a call.
// Join the server's voice room from the channel sidebar, see who is in it,
// mute, leave. The audio itself (microphone,
// WebRTC, speaking levels) is the voice engine's, piece D in ui/src/voice.

import { useEffect } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { Avatar, Name } from '../../components/common';
import { AlertIcon, HangUpIcon, MicIcon, MicOffIcon, PhoneIcon, SpeakerIcon } from '../../components/icons';
import { useCore, type CallState, type Id, type Member } from '../../core';

/**
 * Until piece D (ui/src/voice, dev-voice-shell) lands: the engine interface the
 * call bar uses, as agreed. Replace with `useVoiceEngine(call)` from '../../voice'.
 */
type AudioDevice = { deviceId: string; label: string };
interface VoiceEngine {
  ready: boolean;
  /** `rejoin-to-talk`: the call started without a mic; a newly picked one needs a rejoin (#231). */
  error?: 'mic-denied' | 'no-device' | 'failed' | 'rejoin-to-talk';
  speaking: Record<Id, boolean>;
  setMicMuted: (muted: boolean) => void;
  // Device choice: a settings surface, later.
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  inputId?: string;
  outputId?: string;
  setInput: (id: string) => void;
  setOutput: (id: string) => void;
  /** People the engine couldn't reach directly (its per-peer `peer-failed`). Asked of piece D. */
  peerFailed: Record<Id, boolean>;
}
/**
 * The stub has no audio. For demos and QA it can act out the engine's failure
 * states: `?voiceSim=noMic` (no microphone), `?voiceSim=rejoin` (a mic picked
 * after joining without one) or `?voiceSim=peerFailed` (the first other person
 * can't be reached directly).
 */
function useVoiceEngine(call: CallState | null, myId: Id): VoiceEngine {
  const sim = typeof location !== 'undefined' ? new URLSearchParams(location.search).get('voiceSim') : null;
  const other = call?.participants.find((p) => p.identity !== myId)?.identity;
  return {
    ready: true,
    error: call && sim === 'noMic' ? 'no-device' : call && sim === 'rejoin' ? 'rejoin-to-talk' : undefined,
    speaking: {},
    setMicMuted: () => {},
    inputs: [],
    outputs: [],
    setInput: () => {},
    setOutput: () => {},
    peerFailed: sim === 'peerFailed' && other ? { [other]: true } : {},
  };
}

/** The short code the app shows for someone not (yet) in the member list. */
function codeOf(id: Id): string {
  const s = id.replace(/[^0-9a-z]/gi, '').slice(0, 8).toUpperCase();
  return `${s.slice(0, 4)}-${s.slice(4)}`;
}

const ERROR_TEXT: Record<string, string> = {
  'call-full': 'call.error.full',
  'relay-only': 'call.error.relayOnly',
  'not-implemented': 'call.error.notImplemented',
  'not-allowed': 'call.error.notAllowed',
};

const NO_MEMBERS: Member[] = [];
const ROOM_MAX = 5;

/**
 * The server's voice room in the channel sidebar: "Join chat" with a speaker
 * icon, and who is inside under it, for every member. Disabled, with the reason
 * under it, when the room is full or calls are off (relay-only).
 */
export function VoiceRoomEntry({ serverId, myId }: { serverId: Id; myId: Id }) {
  const store = useAppStore();
  const intl = useIntl();
  const room = useStore(store, (s) => s.voiceRooms[serverId]);
  const members = useStore(store, (s) => s.members[serverId] ?? NO_MEMBERS);
  const relayOnly = useStore(store, (s) => s.relayOnly);
  const callError = useStore(store, (s) => s.callError);
  // Events are hints: re-read the room whenever this server comes on screen.
  useEffect(() => {
    void store.loadVoiceRoom(serverId);
  }, [store, serverId]);

  const people = room?.participants ?? [];
  const inRoom = people.some((p) => p.userId === myId);
  const blocked = relayOnly ? 'call.error.relayOnly' : !inRoom && people.length >= ROOM_MAX ? 'call.error.full' : null;
  const byId = new Map(members.map((m) => [m.id, m]));
  const reasonId = `voice-room-reason-${serverId}`;
  const label = [
    intl.formatMessage({ id: inRoom ? 'call.roomIn' : 'call.room' }),
    people.length ? intl.formatMessage({ id: 'call.roomCount' }, { count: people.length, max: ROOM_MAX }) : null,
  ]
    .filter(Boolean)
    .join(', ');

  return (
    <div className="voice-room">
      <button
        type="button"
        className={`channel voice-room-entry ${inRoom ? 'in-room' : ''}`}
        aria-label={label}
        aria-disabled={blocked || inRoom ? true : undefined}
        aria-describedby={blocked ? reasonId : undefined}
        disabled={!room}
        onClick={() => {
          if (room && !blocked && !inRoom) void store.joinCall(room.roomId);
        }}
      >
        <SpeakerIcon />
        <span className="channel-name">
          <FormattedMessage id={inRoom ? 'call.roomIn' : 'call.room'} />
        </span>
        {people.length > 0 && (
          <span className="voice-room-count" aria-hidden="true">
            {people.length}/{ROOM_MAX}
          </span>
        )}
      </button>
      {blocked && (
        <p id={reasonId} className="voice-room-reason">
          <FormattedMessage id={blocked} />
        </p>
      )}
      {!blocked && callError && <CallError />}
      {people.length > 0 && (
        <ul className="voice-room-people" aria-label={intl.formatMessage({ id: 'call.roomPeople' })}>
          {people.map((p) => {
            const m = byId.get(p.userId);
            const name = m?.displayName || codeOf(p.userId);
            return (
              <li key={p.userId}>
                <Avatar name={name} color={m?.avatarColor ?? '#888'} size={20} />
                <Name>{name}</Name>
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

/** My call: where, who, mute and leave. In the channel sidebar, above my panel. */
export function CallBar({ myId }: { myId: Id }) {
  const store = useAppStore();
  const core = useCore();
  const intl = useIntl();
  const call = useStore(store, (s) => s.call);
  // The call may be in another server's room than the one on screen.
  const serverId = useStore(store, (s) => Object.values(s.voiceRooms).find((r) => r.roomId === s.call?.channelId)?.serverId);
  const members = useStore(store, (s) => (serverId && s.members[serverId]) || NO_MEMBERS);
  const serverName = useStore(store, (s) => s.servers.find((x) => x.id === serverId)?.name ?? '');
  const engine = useVoiceEngine(call, myId);
  // No microphone (none, permission denied, or unplugged mid-call): stay in the
  // call to listen, and show the others I'm muted (#232 V5.7/V5.8).
  // rejoin-to-talk is still listen-only: the new mic only works after a rejoin.
  const noMic = !!call && (engine.error === 'no-device' || engine.error === 'mic-denied' || engine.error === 'rejoin-to-talk');
  const meMuted = !!call?.participants.find((p) => p.identity === myId)?.muted;
  useEffect(() => {
    if (noMic && !meMuted) void core.setMuted(true).catch(() => {});
  }, [noMic, meMuted, core]);
  if (!call) return null;

  const me = call.participants.find((p) => p.identity === myId);
  const byId = new Map(members.map((m) => [m.id, m]));
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
      <ul className="call-people">
        {call.participants.map((p) => {
          const m = byId.get(p.identity);
          const name = m?.displayName || codeOf(p.identity);
          return (
            <li key={p.identity} className={engine.speaking[p.identity] ? 'speaking' : undefined}>
              <Avatar name={name} color={m?.avatarColor ?? '#888'} size={24} />
              <span className="call-person">
                <Name>{name}</Name>
                {engine.peerFailed[p.identity] && p.identity !== myId && (
                  <span className="call-peer-failed">
                    <FormattedMessage id="call.peerFailed" />
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
      {noMic && (
        <p id="call-no-mic" className="call-no-mic" role="status">
          <MicOffIcon />
          <FormattedMessage id={engine.error === 'rejoin-to-talk' ? 'call.rejoinToTalk' : 'call.noMic'} />
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
          title={intl.formatMessage({ id: 'call.mute' })}
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

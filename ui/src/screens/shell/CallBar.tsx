// Voice v1, piece E (#223) and E2 (voice-room amendment): the UI around a call.
// The call bar (mute, leave) and what the voice rooms share (VoiceRooms.tsx):
// the IP notice, the join error line, the roster notes. The audio itself (microphone, WebRTC, speaking levels) is the
// voice engine's, piece D in ui/src/voice.

import { useEffect, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { AlertIcon, HangUpIcon, MicIcon, MicOffIcon, PhoneIcon } from '../../components/icons';
import { useCore, type Id } from '../../core';
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
export function codeOf(id: Id): string {
  const s = id.replace(/[^0-9a-z]/gi, '').slice(0, 8).toUpperCase();
  return `${s.slice(0, 4)}-${s.slice(4)}`;
}

const ERROR_TEXT: Record<string, string> = {
  'call-full': 'call.error.full',
  'relay-only': 'call.error.relayOnly',
  'in-call-elsewhere': 'call.error.inCallElsewhere',
  'room-cap': 'voiceRooms.capReached',
  'not-implemented': 'call.error.notImplemented',
  'not-allowed': 'call.error.notAllowed',
};

export const FAILURE_TEXT: Record<PeerFailure, [text: string, hint: string]> = {
  failed: ['call.peerFailed', 'call.peerFailedHint'],
  refused: ['call.peerRefused', 'call.peerRefusedHint'],
};

/** docs/ux/specs/voice-ip-notice.md: shown once per computer, before the first join. */
const IP_NOTICE_KEY = 'mvs.notices.voiceIp.v1';
export function ipNoticeSeen(): boolean {
  try {
    return localStorage.getItem(IP_NOTICE_KEY) === '1';
  } catch {
    return false; // no storage: explain every time rather than join unexplained
  }
}
export function rememberIpNotice(): void {
  try {
    localStorage.setItem(IP_NOTICE_KEY, '1');
  } catch {
    // Shown again next time.
  }
}

/** Why joining or opening a room failed, under the "Join chat" row, until dismissed or retried. */
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
  const serverId = useStore(store, (s) => Object.entries(s.voiceRooms).find(([, rooms]) => rooms.some((r) => r.roomId === s.call?.channelId))?.[0]);
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

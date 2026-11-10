// Voice v1, piece E (#223): the UI around a call. Join from a channel's
// header, see who is in it, mute, leave. The audio itself (microphone,
// WebRTC, speaking levels) is the voice engine's, piece D in ui/src/voice.

import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { Avatar, Name } from '../../components/common';
import { MicOffIcon, PhoneIcon } from '../../components/icons';
import { useCore, type CallState, type Id, type Member } from '../../core';

/**
 * Until piece D (ui/src/voice, dev-voice-shell) lands: the engine interface the
 * call bar uses, as agreed. Replace with `useVoiceEngine(call)` from '../../voice'.
 */
type AudioDevice = { deviceId: string; label: string };
interface VoiceEngine {
  ready: boolean;
  error?: 'mic-denied' | 'no-device' | 'failed';
  speaking: Record<Id, boolean>;
  setMicMuted: (muted: boolean) => void;
  // Device choice: a settings surface, later.
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  inputId?: string;
  outputId?: string;
  setInput: (id: string) => void;
  setOutput: (id: string) => void;
}
function useVoiceEngine(_call: CallState | null): VoiceEngine {
  return { ready: true, speaking: {}, setMicMuted: () => {}, inputs: [], outputs: [], setInput: () => {}, setOutput: () => {} };
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

/** "Join call" in a channel's header; hidden while I'm in this channel's call. */
export function JoinCallButton({ channelId }: { channelId: Id }) {
  const store = useAppStore();
  const intl = useIntl();
  const inThis = useStore(store, (s) => s.call?.channelId === channelId);
  if (inThis) return null;
  return (
    <button
      type="button"
      className="icon-button join-call"
      onClick={() => void store.joinCall(channelId)}
      aria-label={intl.formatMessage({ id: 'call.join' })}
      title={intl.formatMessage({ id: 'call.join' })}
    >
      <PhoneIcon />
    </button>
  );
}

/** Why joining failed, under the channel header, until dismissed or retried. */
export function CallError() {
  const store = useAppStore();
  const intl = useIntl();
  const code = useStore(store, (s) => s.callError);
  if (!code) return null;
  return (
    <div className="call-error" role="alert">
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
const NO_MEMBERS: Member[] = [];

export function CallBar({ myId }: { myId: Id }) {
  const store = useAppStore();
  const core = useCore();
  const intl = useIntl();
  const call = useStore(store, (s) => s.call);
  // The call may be in another server than the one on screen.
  const members = useStore(store, (s) => {
    const id = s.call?.channelId;
    const serverId = Object.keys(s.channels).find((k) => s.channels[k].some((c) => c.id === id));
    return (serverId && s.members[serverId]) || NO_MEMBERS;
  });
  const channelName = useStore(store, (s) => {
    const id = s.call?.channelId;
    for (const list of Object.values(s.channels)) {
      const c = list.find((x) => x.id === id);
      if (c) return c.name;
    }
    return '';
  });
  const engine = useVoiceEngine(call);
  if (!call) return null;

  const me = call.participants.find((p) => p.identity === myId);
  const byId = new Map(members.map((m) => [m.id, m]));
  const toggleMute = () => {
    const muted = !me?.muted;
    engine.setMicMuted(muted);
    void core.setMuted(muted).catch(() => {});
  };

  return (
    <section className="call-bar" aria-label={intl.formatMessage({ id: 'call.label' })}>
      <p className="call-where">
        <PhoneIcon />
        <FormattedMessage id="call.inCall" values={{ channel: channelName }} />
      </p>
      <ul className="call-people">
        {call.participants.map((p) => {
          const m = byId.get(p.identity);
          const name = m?.displayName || codeOf(p.identity);
          return (
            <li key={p.identity} className={engine.speaking[p.identity] ? 'speaking' : undefined}>
              <Avatar name={name} color={m?.avatarColor ?? '#888'} size={24} />
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
      <div className="call-actions">
        <button type="button" className="secondary small" aria-pressed={!!me?.muted} onClick={toggleMute}>
          <FormattedMessage id={me?.muted ? 'call.unmute' : 'call.mute'} />
        </button>
        <button type="button" className="danger small" onClick={() => void store.leaveCall()}>
          <FormattedMessage id="call.leave" />
        </button>
      </div>
    </section>
  );
}

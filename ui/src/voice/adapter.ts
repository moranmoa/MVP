// Voice v1, piece D (#223): the one place the voice engine touches the core
// api. Everything else in ui/src/voice talks to `VoiceCore`, so a change to the
// core contract (core/api/CONFORMANCE.md, "Voice calls") lands here only.

import type { CoreApi } from '../core/api';
import type { CallSignal, CallState, Id } from '../core/types';

export type SignalKind = CallSignal['kind'];

/** What the engine needs from the core. */
export interface VoiceCore {
  /** Rejects not-found, not-allowed, call-full, relay-only, not-implemented. */
  joinCall(channelId: Id): Promise<CallState>;
  /** Idempotent. */
  leaveCall(): Promise<unknown>;
  /** My mute flag as the others see it (presence), not the microphone itself. */
  setMuted(muted: boolean): Promise<unknown>;
  /** Payload at most 12 KiB; an offer or answer must carry `a=fingerprint`. */
  sendSignal(to: Id, kind: SignalKind, payload: string): Promise<unknown>;
  onState(handler: (state: CallState) => void): () => void;
  onSignal(handler: (signal: CallSignal) => void): () => void;
  /**
   * G-81: short-lived TURN credentials `{ urls, username, credential, ttl,
   * expiresAt }`, minted by the relay. Rejects relay-only, not-implemented,
   * rate-limited. Absent until the core api has it; the engine then goes on
   * without TURN. Whatever comes back is cleaned (cleanTurnCredentials).
   */
  getTurnCredentials?(): Promise<unknown>;
}

export function fromCoreApi(api: CoreApi): VoiceCore {
  // Not in CoreApi yet (G-81 step 4); bound only when the core has it.
  const turn = (api as Partial<{ getTurnCredentials(): Promise<unknown> }>).getTurnCredentials;
  return {
    ...(typeof turn === 'function' ? { getTurnCredentials: () => turn.call(api) } : {}),
    joinCall: (channelId) => api.joinCall(channelId),
    leaveCall: () => api.leaveCall(),
    setMuted: (muted) => api.setMuted(muted),
    sendSignal: (to, kind, payload) => api.sendSignal(to, kind, payload),
    onState: (handler) => api.on('call.state', handler),
    onSignal: (handler) => api.on('call.signal', handler),
  };
}

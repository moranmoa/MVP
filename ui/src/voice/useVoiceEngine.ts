// The React side of the voice engine, in the shape the call bar (piece E,
// ui/src/screens/shell/CallBar.tsx) agreed on. The app store joins and
// leaves through the core; this hook starts and stops the audio to match.

import { useCallback, useEffect, useRef, useState } from 'react';
import { useCore } from '../core';
import type { CallState, Id } from '../core/types';
import { fromCoreApi } from './adapter';
import {
  createVoiceEngine,
  type AudioDevice,
  type VoiceEngine,
  type VoiceEngineOptions,
  type VoiceError,
  type VoiceParticipant,
} from './engine';

export interface VoiceEngineView {
  ready: boolean;
  /** `rejoin-to-talk`: the call started without a mic; a new device needs a rejoin. */
  error?: 'mic-denied' | 'no-device' | 'failed' | 'rejoin-to-talk';
  speaking: Record<Id, boolean>;
  /**
   * People in the call I have no voice connection with: the engine gave up
   * (peer-failed) or refused them (a security check). Cleared when that
   * person connects again, and when the call ends.
   */
  peerFailed: Id[];
  setMicMuted: (muted: boolean) => void;
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  inputId?: string;
  outputId?: string;
  setInput: (id: string) => void;
  setOutput: (id: string) => void;
}

/**
 * `call` is the store's current call (null outside one), `myId` my identity.
 * `options.stunUrl` is the config's STUN URL from the desktop shell (piece C);
 * without it, only host candidates are used.
 */
/** The next peerFailed list after an engine event. Pure, for tests. */
export function nextPeerFailed(
  prev: Id[],
  event: { error: VoiceError } | { participants: VoiceParticipant[] },
): Id[] {
  if ('error' in event) {
    const id = event.error.peer;
    return id && !prev.includes(id) ? [...prev, id] : prev;
  }
  if (!event.participants.length) return prev.length ? [] : prev;
  const connected = new Set(event.participants.filter((p) => p.state === 'connected').map((p) => p.identity));
  const next = prev.filter((id) => !connected.has(id));
  return next.length === prev.length ? prev : next;
}

export function useVoiceEngine(call: CallState | null, myId: Id, options: Pick<VoiceEngineOptions, 'stunUrl'> = {}): VoiceEngineView {
  const core = useCore();
  const engineRef = useRef<VoiceEngine | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<VoiceEngineView['error']>();
  const [speaking, setSpeaking] = useState<Record<Id, boolean>>({});
  const [peerFailed, setPeerFailed] = useState<Id[]>([]);
  const [inputs, setInputs] = useState<AudioDevice[]>([]);
  const [outputs, setOutputs] = useState<AudioDevice[]>([]);
  const [inputId, setInputId] = useState<string>();
  const [outputId, setOutputId] = useState<string>();

  const stunUrl = options.stunUrl ?? null;
  useEffect(() => {
    const engine = createVoiceEngine(fromCoreApi(core), { stunUrl });
    engineRef.current = engine;
    const offs = [
      engine.on('speaking', setSpeaking),
      engine.on('participants', (participants) => setPeerFailed((prev) => nextPeerFailed(prev, { participants }))),
      engine.on('error', (e) => {
        setPeerFailed((prev) => nextPeerFailed(prev, { error: e }));
        if (e.code === 'mic-denied' || e.code === 'no-device' || e.code === 'failed' || e.code === 'rejoin-to-talk') setError(e.code);
      }),
    ];
    return () => {
      offs.forEach((off) => off());
      engine.dispose();
      engineRef.current = null;
    };
  }, [core, stunUrl]);

  const callKey = call && call.participants.length ? call.channelId : null;
  useEffect(() => {
    const engine = engineRef.current;
    if (!engine || !callKey || !call) return;
    let live = true;
    setReady(false);
    setError(undefined);
    void engine.attach(call, myId).then(async () => {
      if (!live) return;
      setReady(true);
      // Device labels are only filled in after the microphone permission.
      const [ins, outs] = await Promise.all([engine.listInputDevices(), engine.listOutputDevices()]);
      if (!live) return;
      setInputs(ins);
      setOutputs(outs);
    });
    return () => {
      live = false;
      engine.detach();
      setReady(false);
      setSpeaking({});
      setPeerFailed([]);
    };
    // Re-attach only when the call itself changes, not on every participant update:
    // the engine follows call.state on its own.
  },[callKey, myId, core, stunUrl]);

  const setMicMuted = useCallback((muted: boolean) => engineRef.current?.setMicMuted(muted), []);
  const setInput = useCallback((id: string) => {
    setInputId(id);
    void engineRef.current?.setInputDevice(id).catch(() => setError('no-device'));
  }, []);
  const setOutput = useCallback((id: string) => {
    setOutputId(id);
    void engineRef.current?.setOutputDevice(id);
  }, []);

  return { ready, error, speaking, peerFailed, setMicMuted, inputs, outputs, inputId, outputId, setInput, setOutput };
}

// Voice v1, piece D (#223): the WebRTC mesh engine. Ported from the spike's
// browser client (spikes/voice/web/voice.js, PR #59).
//
// - One RTCPeerConnection per other participant (the core caps a call at 5),
//   Opus audio only, with echo cancellation, noise suppression and AGC on.
// - Signalling goes through the core (`sendSignal` / `call.signal`), which
//   signs every envelope with the identity key; the SDP is sent whole, with
//   its candidates (no trickle), as in the spike.
// - Glare: of two participants, the one with the lower identity offers; an
//   offer from the higher one is ignored.
// - Fail closed:
//   - a `call.signal` without `fingerprintVerified` is never used, and the
//     connection to that peer is dropped;
//   - the remote SDP must name exactly one DTLS fingerprint, and what the
//     connection applied must equal the verified one;
//   - once connected, the certificate the peer actually presented (getStats)
//     must match it too;
//   - only then is the peer's audio played.
//   A peer dropped for any of these is not retried until the call ends.
// - Lost offers: the offerer re-offers (a fresh connection) when the peer is
//   not connected within `answerTimeoutMs`, doubling it each time, up to
//   OFFER_ATTEMPTS offers; then the peer is dropped as `peer-failed`. This
//   covers a newcomer whose engine wasn't attached yet when the first offer
//   came. The answerer drops a connection still not up after
//   `connectTimeoutMs` (`peer-failed`); the offerer's next offer starts over.
// - ICE servers come from the config's optional STUN URL only (`stunUrl`),
//   never a public default.
// - Speaking: an AnalyserNode per stream, level over a threshold.

import type { CallSignal, CallState, Id } from '../core/types';
import type { VoiceCore } from './adapter';
import { iceServersFor, normalizeFingerprint, soleFingerprint } from './sdp';

export type PeerConnectionState = 'connecting' | 'connected' | 'failed';

export interface VoiceParticipant {
  identity: Id;
  /** Me: whether the microphone works. Others: the WebRTC link. */
  state: PeerConnectionState;
}

export type VoiceErrorCode =
  | 'mic-denied'
  | 'no-device'
  | 'failed'
  | 'unverified-signal'
  | 'fingerprint-mismatch'
  | 'peer-failed'
  /** The call started without a microphone; sending needs a rejoin. */
  | 'rejoin-to-talk';

export interface VoiceError {
  code: VoiceErrorCode;
  /** The participant it concerns, for the per-peer codes. */
  peer?: Id;
}

export interface VoiceEvents {
  participants: VoiceParticipant[];
  /** Who is speaking now, me included, by identity. */
  speaking: Record<Id, boolean>;
  error: VoiceError;
}

export interface AudioDevice {
  deviceId: string;
  label: string;
}

/** The browser APIs the engine uses; tests pass fakes. */
export interface VoiceEnv {
  createPeerConnection(config: RTCConfiguration): RTCPeerConnection;
  getUserMedia(constraints: MediaStreamConstraints): Promise<MediaStream>;
  enumerateDevices(): Promise<MediaDeviceInfo[]>;
  /** null where Web Audio is missing: no speaking indicator then. */
  createAudioContext(): AudioContext | null;
  createAudioElement(): HTMLAudioElement;
  /** The Opus entries of the audio receive capabilities; null if unknown. */
  opusCodecs(): RTCRtpCodec[] | null;
}

export interface VoiceEngineOptions {
  /** The config's STUN URL (`stun:host:port`), if any. */
  stunUrl?: string | null;
  env?: VoiceEnv;
  /** How long to wait for ICE gathering before sending the SDP anyway. */
  gatherTimeoutMs?: number;
  /** The offerer's first wait for a connection; doubles per retry. Default 5 s. */
  answerTimeoutMs?: number;
  /** The answerer's wait for a connection. Default 30 s. */
  connectTimeoutMs?: number;
  /** RMS level above which someone counts as speaking (0..1). */
  speakingThreshold?: number;
}

export interface VoiceEngine {
  /** Joins through the core, then starts the microphone and the mesh. */
  join(channelId: Id): Promise<CallState>;
  /** Stops all audio and leaves through the core. */
  leave(): Promise<void>;
  /**
   * Starts the microphone and the mesh for a call someone else joined (the
   * app store does). `me` is my identity. Resolves once the mic is settled.
   */
  attach(state: CallState, me: Id): Promise<void>;
  /** Stops all audio without telling the core (the caller left already). */
  detach(): void;
  /** Mutes the microphone and tells the core (presence). */
  setMuted(muted: boolean): Promise<void>;
  /** Mutes the microphone only; for callers that tell the core themselves. */
  setMicMuted(muted: boolean): void;
  setInputDevice(deviceId: string): Promise<void>;
  setOutputDevice(deviceId: string): Promise<void>;
  listInputDevices(): Promise<AudioDevice[]>;
  listOutputDevices(): Promise<AudioDevice[]>;
  on<E extends keyof VoiceEvents>(event: E, cb: (payload: VoiceEvents[E]) => void): () => void;
  /** Leaves the audio and unsubscribes from the core for good. */
  dispose(): void;
}

const MAX_PEERS = 4; // 5 in a call, me included
/** Trickled candidates accepted from one peer, in total. */
const MAX_TRICKLED_ICE = 32;
const OFFER_ATTEMPTS = 3;
const SPEAKING_POLL_MS = 100;
const SPEAKING_HOLD_MS = 300;

interface Peer {
  id: Id;
  pc: RTCPeerConnection;
  offerer: boolean;
  /** From the verified signal; null until one arrives. */
  fingerprint: string | null;
  pendingIce: RTCIceCandidateInit[];
  iceReceived: number;
  stream: MediaStream | null;
  audio: HTMLAudioElement | null;
  meter: Meter | null;
  state: PeerConnectionState;
  /** Media plays only after the certificate check. */
  verified: boolean;
  closed: boolean;
  /** Offerer: which offer this is (1-based). */
  attempt: number;
  watchdog: ReturnType<typeof setTimeout> | null;
}

interface Meter {
  source: MediaStreamAudioSourceNode;
  analyser: AnalyserNode;
  buffer: Float32Array<ArrayBuffer>;
  lastLoud: number;
}

export function browserEnv(): VoiceEnv {
  return {
    createPeerConnection: (config) => new RTCPeerConnection(config),
    getUserMedia: (c) => navigator.mediaDevices.getUserMedia(c),
    enumerateDevices: () => navigator.mediaDevices?.enumerateDevices?.() ?? Promise.resolve([]),
    createAudioContext: () => (typeof AudioContext === 'function' ? new AudioContext({ latencyHint: 'interactive' }) : null),
    createAudioElement: () => new Audio(),
    opusCodecs: () => {
      try {
        const caps = RTCRtpReceiver.getCapabilities?.('audio');
        return caps ? caps.codecs.filter((c) => /^audio\/opus$/i.test(c.mimeType)) : null;
      } catch {
        return null;
      }
    },
  };
}

function micError(e: unknown): VoiceErrorCode {
  const name = (e as { name?: string })?.name;
  if (name === 'NotAllowedError' || name === 'SecurityError') return 'mic-denied';
  if (name === 'NotFoundError' || name === 'OverconstrainedError') return 'no-device';
  return 'failed';
}

export function createVoiceEngine(core: VoiceCore, options: VoiceEngineOptions = {}): VoiceEngine {
  const env = options.env ?? browserEnv();
  const gatherTimeoutMs = options.gatherTimeoutMs ?? 3000;
  const threshold = options.speakingThreshold ?? 0.02;
  const answerTimeoutMs = options.answerTimeoutMs ?? 5000;
  const connectTimeoutMs = options.connectTimeoutMs ?? 30_000;
  const iceServers = iceServersFor(options.stunUrl);

  const listeners: { [E in keyof VoiceEvents]: Set<(p: VoiceEvents[E]) => void> } = {
    participants: new Set(),
    speaking: new Set(),
    error: new Set(),
  };
  const emit = <E extends keyof VoiceEvents>(event: E, payload: VoiceEvents[E]) => {
    for (const cb of listeners[event]) {
      try {
        cb(payload);
      } catch {
        // A listener's bug must not break the mesh.
      }
    }
  };

  // ---- call state -----------------------------------------------------------
  let active = false;
  /** Bumped on every start/stop so late async work can tell it is stale. */
  let generation = 0;
  let me: Id | null = null;
  let channelId: Id | null = null;
  let members = new Set<Id>();
  const peers = new Map<Id, Peer>();
  /** Dropped for a security reason; not reconnected until the call ends. */
  const refused = new Set<Id>();
  let local: MediaStream | null = null;
  let micMuted = false;
  let micState: PeerConnectionState = 'connecting';
  let inputId: string | undefined;
  let outputId: string | undefined;
  let ctx: AudioContext | null = null;
  let localMeter: Meter | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;
  let speaking: Record<Id, boolean> = {};

  const unsubscribe = [core.onState((s) => onState(s)), core.onSignal((s) => void onSignal(s))];

  function emitParticipants() {
    if (!active || !me) return;
    const list: VoiceParticipant[] = [{ identity: me, state: micState }];
    for (const p of peers.values()) list.push({ identity: p.id, state: p.state });
    emit('participants', list);
  }

  // ---- microphone ------------------------------------------------------------
  async function openMic(deviceId: string | undefined): Promise<MediaStream> {
    return env.getUserMedia({
      audio: {
        deviceId: deviceId ? { exact: deviceId } : undefined,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
        channelCount: 1,
      },
      video: false,
    });
  }

  function micTrack(): MediaStreamTrack | null {
    return local?.getAudioTracks()[0] ?? null;
  }

  function applyMute() {
    for (const t of local?.getAudioTracks() ?? []) t.enabled = !micMuted;
  }

  // ---- audio analysis --------------------------------------------------------
  function audioContext(): AudioContext | null {
    if (!ctx) {
      try {
        ctx = env.createAudioContext();
        void ctx?.resume?.().catch(() => {});
      } catch {
        ctx = null;
      }
    }
    return ctx;
  }

  function makeMeter(stream: MediaStream): Meter | null {
    const c = audioContext();
    if (!c) return null;
    try {
      const source = c.createMediaStreamSource(stream);
      const analyser = c.createAnalyser();
      analyser.fftSize = 512;
      source.connect(analyser); // not to the destination: the <audio> element plays it
      return { source, analyser, buffer: new Float32Array(analyser.fftSize), lastLoud: -Infinity };
    } catch {
      return null;
    }
  }

  function dropMeter(m: Meter | null) {
    try {
      m?.source.disconnect();
    } catch {
      // already gone
    }
  }

  function loud(m: Meter, now: number): boolean {
    m.analyser.getFloatTimeDomainData(m.buffer);
    let sum = 0;
    for (const v of m.buffer) sum += v * v;
    if (Math.sqrt(sum / m.buffer.length) > threshold) m.lastLoud = now;
    return now - m.lastLoud < SPEAKING_HOLD_MS;
  }

  function pollSpeaking() {
    if (!active || !me) return;
    const now = Date.now();
    const next: Record<Id, boolean> = {};
    next[me] = !micMuted && !!localMeter && loud(localMeter, now);
    for (const p of peers.values()) next[p.id] = p.verified && !!p.meter && loud(p.meter, now);
    const keys = new Set([...Object.keys(next), ...Object.keys(speaking)]);
    if ([...keys].some((k) => next[k] !== speaking[k])) {
      speaking = next;
      emit('speaking', { ...next });
    }
  }

  // ---- peers -----------------------------------------------------------------
  function createPeer(id: Id, offerer: boolean, attempt = 1): Peer {
    const pc = env.createPeerConnection({
      iceServers,
      bundlePolicy: 'max-bundle',
      rtcpMuxPolicy: 'require',
    });
    const peer: Peer = {
      id,
      pc,
      offerer,
      fingerprint: null,
      pendingIce: [],
      iceReceived: 0,
      stream: null,
      audio: null,
      meter: null,
      state: 'connecting',
      verified: false,
      closed: false,
      attempt,
      watchdog: null,
    };
    peers.set(id, peer);
    peer.watchdog = setTimeout(
      () => {
        peer.watchdog = null;
        if (!peer.closed && !peer.verified) retryOrFail(peer);
      },
      offerer ? answerTimeoutMs * 2 ** (attempt - 1) : connectTimeoutMs,
    );

    // The offerer creates the audio m-line. The answerer must not: a
    // transceiver added before the offer is applied is not bound to the
    // offer's m-line in Chromium, and the answer then goes out recvonly
    // (QA-21: one-way audio). It takes the offer's transceiver instead, in
    // bindAnswerTransceiver.
    if (offerer) {
      const track = micTrack();
      const transceiver = track
        ? pc.addTransceiver(track, { direction: 'sendrecv', streams: local ? [local] : [] })
        : pc.addTransceiver('audio', { direction: 'recvonly' });
      preferOpus(transceiver);
    }

    pc.ontrack = (e) => {
      if (peer.closed) return;
      peer.stream = e.streams[0] ?? new MediaStream([e.track]);
      if (peer.verified) playPeer(peer);
    };
    pc.onconnectionstatechange = () => {
      if (peer.closed) return;
      const s = pc.connectionState;
      if (s === 'connected') void checkCertificate(peer);
      else if (s === 'failed') retryOrFail(peer);
    };
    emitParticipants();
    return peer;
  }

  /** The offerer offers again on a fresh connection while it has tries left. */
  function retryOrFail(peer: Peer) {
    if (peer.closed) return; // already dropped or retried (stale async work)
    const next = peer.attempt + 1;
    if (peer.offerer && next <= OFFER_ATTEMPTS && active && members.has(peer.id) && !refused.has(peer.id)) {
      dropPeer(peer);
      void offerTo(peer.id, next);
    } else {
      dropPeer(peer, 'peer-failed');
    }
  }

  function dropPeer(peer: Peer, reason?: VoiceErrorCode) {
    if (peer.closed) return;
    peer.closed = true;
    if (peer.watchdog) clearTimeout(peer.watchdog);
    peer.watchdog = null;
    peer.state = 'failed';
    try {
      peer.pc.close();
    } catch {
      // closing twice is harmless
    }
    dropMeter(peer.meter);
    if (peer.audio) {
      peer.audio.pause();
      peer.audio.srcObject = null;
    }
    peers.delete(peer.id);
    if (reason === 'fingerprint-mismatch' || reason === 'unverified-signal') refused.add(peer.id);
    if (reason) emit('error', { code: reason, peer: peer.id });
    if (speaking[peer.id] !== undefined) {
      const { [peer.id]: _gone, ...rest } = speaking;
      speaking = rest;
      emit('speaking', { ...rest });
    }
    emitParticipants();
  }

  /** The certificate the peer presented must be the one its verified SDP named. */
  // Fails closed: no stats, no certificate, or any certificate that differs
  // drops the peer. (Chromium reports remoteCertificateId once DTLS is up.)
  async function checkCertificate(peer: Peer) {
    const seen: string[] = [];
    try {
      const stats = await peer.pc.getStats();
      if (peer.closed) return;
      const byId = new Map<string, Record<string, unknown>>();
      stats.forEach((r: Record<string, unknown>) => byId.set(r.id as string, r));
      for (const r of byId.values()) {
        if (r.type !== 'transport' || typeof r.remoteCertificateId !== 'string') continue;
        const cert = byId.get(r.remoteCertificateId);
        const ok = cert && typeof cert.fingerprint === 'string' && typeof cert.fingerprintAlgorithm === 'string';
        seen.push(ok ? (normalizeFingerprint(cert.fingerprintAlgorithm as string, cert.fingerprint as string) ?? '') : '');
      }
    } catch {
      if (peer.closed) return;
      return dropPeer(peer, 'fingerprint-mismatch');
    }
    if (!peer.fingerprint || !seen.length || seen.some((f) => f !== peer.fingerprint)) {
      return dropPeer(peer, 'fingerprint-mismatch');
    }
    peer.verified = true;
    peer.state = 'connected';
    if (peer.watchdog) clearTimeout(peer.watchdog);
    peer.watchdog = null;
    if (peer.stream) playPeer(peer);
    emitParticipants();
  }

  function playPeer(peer: Peer) {
    if (peer.audio || !peer.stream) return;
    // Chromium feeds remote WebRTC audio to Web Audio only while an element
    // also plays it (spike finding), so the element both plays and enables
    // the meter.
    const audio = env.createAudioElement();
    audio.autoplay = true;
    audio.srcObject = peer.stream;
    if (outputId && 'setSinkId' in audio) void audio.setSinkId(outputId).catch(() => {});
    void audio.play?.()?.catch?.(() => {});
    peer.audio = audio;
    peer.meter = makeMeter(peer.stream);
  }

  /** Waits for ICE gathering so the SDP carries every candidate (no trickle). */
  function gathered(pc: RTCPeerConnection): Promise<void> {
    if (pc.iceGatheringState === 'complete') return Promise.resolve();
    return new Promise((resolve) => {
      const done = () => {
        if (pc.iceGatheringState !== 'complete') return;
        pc.removeEventListener('icegatheringstatechange', done);
        clearTimeout(t);
        resolve();
      };
      const t = setTimeout(() => {
        pc.removeEventListener('icegatheringstatechange', done);
        resolve();
      }, gatherTimeoutMs);
      pc.addEventListener('icegatheringstatechange', done);
    });
  }

  async function sendDescription(peer: Peer, kind: 'offer' | 'answer') {
    const gen = generation;
    const pc = peer.pc;
    const desc = kind === 'offer' ? await pc.createOffer() : await pc.createAnswer();
    if (peer.closed || gen !== generation) return;
    await pc.setLocalDescription(desc);
    await gathered(pc);
    if (peer.closed || gen !== generation) return;
    const sdp = pc.localDescription?.sdp;
    if (!sdp) return dropPeer(peer, 'failed');
    await core.sendSignal(peer.id, kind, sdp);
  }

  async function offerTo(id: Id, attempt = 1) {
    const peer = createPeer(id, true, attempt);
    try {
      await sendDescription(peer, 'offer');
    } catch {
      retryOrFail(peer);
    }
  }

  /** Applies a verified offer or answer; false (and the peer dropped) on a fingerprint problem. */
  function preferOpus(transceiver: RTCRtpTransceiver) {
    const opus = env.opusCodecs();
    if (!opus?.length) return;
    try {
      transceiver.setCodecPreferences(opus);
    } catch {
      // the browser negotiates its default (Opus first in Chromium)
    }
  }

  /**
   * Answerer, after the offer is applied: the transceiver of the offer's audio
   * m-line sends my mic (sendrecv), or only receives without one. False when
   * the offer has no audio m-line.
   */
  async function bindAnswerTransceiver(peer: Peer): Promise<boolean> {
    const transceiver = peer.pc
      .getTransceivers()
      .find((t) => t.mid !== null && t.currentDirection !== 'stopped' && t.receiver.track?.kind === 'audio');
    if (!transceiver) return false;
    const track = micTrack();
    if (track) {
      await transceiver.sender.replaceTrack(track);
      if (local) transceiver.sender.setStreams?.(local);
      transceiver.direction = 'sendrecv';
    } else {
      transceiver.direction = 'recvonly';
    }
    preferOpus(transceiver);
    return true;
  }

  async function applyRemote(peer: Peer, type: 'offer' | 'answer', sdp: string): Promise<boolean> {
    const fingerprint = soleFingerprint(sdp);
    if (!fingerprint) {
      dropPeer(peer, 'fingerprint-mismatch');
      return false;
    }
    peer.fingerprint = fingerprint;
    await peer.pc.setRemoteDescription({ type, sdp });
    if (peer.closed) return false;
    // What the connection applied must commit to the verified certificate.
    const applied = soleFingerprint(peer.pc.remoteDescription?.sdp ?? '');
    if (applied !== fingerprint) {
      dropPeer(peer, 'fingerprint-mismatch');
      return false;
    }
    for (const c of peer.pendingIce.splice(0)) await peer.pc.addIceCandidate(c).catch(() => {});
    return true;
  }

  async function onSignal(signal: CallSignal) {
    if (!active || !me || signal.from === me || !members.has(signal.from)) return;
    const existing = peers.get(signal.from);
    if (signal.fingerprintVerified !== true) {
      // The core never sends these (it fails closed); if one shows up anyway,
      // nothing from that peer is trusted for the rest of the call.
      if (existing) dropPeer(existing, 'unverified-signal');
      else {
        refused.add(signal.from);
        emit('error', { code: 'unverified-signal', peer: signal.from });
      }
      return;
    }
    if (refused.has(signal.from)) return;
    const iOffer = me < signal.from;
    try {
      if (signal.kind === 'offer') {
        if (iOffer) return; // glare: the lower identity offers, so mine wins
        // A new offer from the same peer (it restarted): start over.
        if (existing) dropPeer(existing);
        if (peers.size >= MAX_PEERS) return;
        const peer = createPeer(signal.from, false);
        if (!(await applyRemote(peer, 'offer', signal.payload))) return;
        if (peer.closed) return;
        if (!(await bindAnswerTransceiver(peer))) return dropPeer(peer, 'peer-failed');
        if (peer.closed) return;
        await sendDescription(peer, 'answer');
      } else if (signal.kind === 'answer') {
        if (!existing || !existing.offerer || existing.pc.signalingState !== 'have-local-offer') return;
        await applyRemote(existing, 'answer', signal.payload);
      } else if (signal.kind === 'ice') {
        if (!existing) return;
        let init: RTCIceCandidateInit;
        try {
          const parsed = JSON.parse(signal.payload) as RTCIceCandidateInit;
          if (typeof parsed?.candidate !== 'string') return;
          init = { candidate: parsed.candidate, sdpMid: parsed.sdpMid ?? null, sdpMLineIndex: parsed.sdpMLineIndex ?? null };
        } catch {
          return;
        }
        // A cap on everything a peer trickles, so a member can't make us send
        // checks to any number of addresses (we don't trickle; normally 0).
        if (existing.iceReceived >= MAX_TRICKLED_ICE) return;
        existing.iceReceived++;
        if (existing.pc.remoteDescription) await existing.pc.addIceCandidate(init).catch(() => {});
        else existing.pendingIce.push(init);
      }
    } catch {
      const peer = peers.get(signal.from);
      if (peer) retryOrFail(peer);
    }
  }

  function onState(state: CallState) {
    if (!active || !me) return;
    if (!state.participants.length || (channelId && state.channelId !== channelId)) {
      // Left, or the core moved me to another call: the mesh is over.
      if (!state.participants.length) stop();
      else {
        for (const p of [...peers.values()]) dropPeer(p);
        refused.clear();
        channelId = state.channelId;
        reconcile(state);
      }
      return;
    }
    reconcile(state);
  }

  function reconcile(state: CallState) {
    const others = state.participants.map((p) => p.identity).filter((id) => id !== me);
    members = new Set(others.slice(0, MAX_PEERS));
    for (const p of [...peers.values()]) if (!members.has(p.id)) dropPeer(p);
    for (const id of members) {
      if (peers.has(id) || refused.has(id)) continue;
      if (me! < id) void offerTo(id);
    }
    emitParticipants();
  }

  // ---- lifecycle -------------------------------------------------------------
  async function start(state: CallState, myId: Id) {
    if (active) stop();
    active = true;
    const gen = ++generation;
    me = myId;
    channelId = state.channelId;
    micState = 'connecting';
    speaking = {};
    try {
      local = await openMic(inputId);
      if (gen !== generation) {
        for (const t of local.getTracks()) t.stop();
        return;
      }
      applyMute();
      localMeter = makeMeter(local);
      micState = 'connected';
    } catch (e) {
      if (gen !== generation) return;
      local = null;
      micState = 'failed';
      emit('error', { code: micError(e) }); // still listen
    }
    timer = setInterval(pollSpeaking, SPEAKING_POLL_MS);
    reconcile(state);
  }

  function stop() {
    if (!active) return;
    active = false;
    generation++;
    for (const p of [...peers.values()]) dropPeer(p);
    for (const t of local?.getTracks() ?? []) t.stop();
    local = null;
    dropMeter(localMeter);
    localMeter = null;
    if (timer) clearInterval(timer);
    timer = null;
    void ctx?.close?.().catch(() => {});
    ctx = null;
    refused.clear();
    members = new Set();
    me = null;
    channelId = null;
    if (Object.keys(speaking).length) emit('speaking', {});
    speaking = {};
    emit('participants', []);
  }

  async function listDevices(kind: MediaDeviceKind): Promise<AudioDevice[]> {
    const all = await env.enumerateDevices().catch(() => [] as MediaDeviceInfo[]);
    return all.filter((d) => d.kind === kind).map((d, i) => ({ deviceId: d.deviceId, label: d.label || `${kind} ${i + 1}` }));
  }

  return {
    async join(id) {
      const state = await core.joinCall(id);
      const mine = state.participants[0]?.identity; // the core lists me first
      if (mine) await start(state, mine);
      return state;
    },
    async leave() {
      stop();
      await core.leaveCall();
    },
    attach: (state, myId) => start(state, myId),
    detach: () => stop(),
    async setMuted(muted) {
      micMuted = muted;
      applyMute();
      await core.setMuted(muted);
    },
    setMicMuted(muted) {
      micMuted = muted;
      applyMute();
    },
    async setInputDevice(deviceId) {
      inputId = deviceId || undefined;
      if (!active) return;
      // Connections made without a microphone are receive-only; sending would
      // need a renegotiation the answerer can't start (glare rule). v1: rejoin.
      if (!local) {
        emit('error', { code: 'rejoin-to-talk' });
        return;
      }
      const gen = generation;
      const next = await openMic(inputId);
      if (gen !== generation) {
        for (const t of next.getTracks()) t.stop();
        return;
      }
      const track = next.getAudioTracks()[0] ?? null;
      for (const p of peers.values()) {
        for (const s of p.pc.getSenders()) if (!s.track || s.track.kind === 'audio') await s.replaceTrack(track);
      }
      for (const t of local?.getTracks() ?? []) t.stop();
      dropMeter(localMeter);
      local = next;
      applyMute();
      localMeter = makeMeter(next);
      micState = 'connected';
      emitParticipants();
    },
    async setOutputDevice(deviceId) {
      outputId = deviceId || undefined;
      await Promise.all(
        [...peers.values()].map((p) =>
          p.audio && 'setSinkId' in p.audio ? p.audio.setSinkId(deviceId).catch(() => {}) : undefined,
        ),
      );
    },
    listInputDevices: () => listDevices('audioinput'),
    listOutputDevices: () => listDevices('audiooutput'),
    on(event, cb) {
      listeners[event].add(cb);
      return () => listeners[event].delete(cb);
    },
    dispose() {
      stop();
      for (const u of unsubscribe) u();
      for (const set of Object.values(listeners)) set.clear();
    },
  };
}

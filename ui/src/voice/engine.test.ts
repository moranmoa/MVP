// Voice v1, piece D: the mesh engine against fake WebRTC and a fake core.

import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CallSignal, CallState, Id } from '../core/types';
import type { SignalKind, VoiceCore } from './adapter';
import { createVoiceEngine, type VoiceEngineOptions, type VoiceEnv, type VoiceError, type VoiceParticipant } from './engine';
import { fingerprints, iceServersFor, normalizeFingerprint, soleFingerprint } from './sdp';

const A: Id = 'a'.repeat(64);
const B: Id = 'b'.repeat(64);
const C: Id = 'c'.repeat(64);
const CH: Id = 'f'.repeat(64);

// ---- fakes -------------------------------------------------------------------

let fpCounter = 0;
const newFingerprint = () => {
  const n = (++fpCounter).toString(16).padStart(2, '0');
  return Array.from({ length: 32 }, (_, i) => (i === 0 ? n : 'AB')).join(':');
};

class FakeTrack {
  kind = 'audio';
  enabled = true;
  stopped = false;
  stop() {
    this.stopped = true;
  }
}

class FakeStream {
  constructor(readonly tracks: FakeTrack[] = [new FakeTrack()]) {}
  getAudioTracks() {
    return this.tracks;
  }
  getTracks() {
    return this.tracks;
  }
}

/** Every fake connection, so a test can look at them and at who presented what certificate. */
const allPcs: FakePC[] = [];

class FakePC {
  readonly fingerprint = newFingerprint();
  /** What getStats reports as the peer's certificate; by default, the real peer's. */
  presentedFingerprint: string | null = null;
  /** 'none': no transport stats; 'throw': getStats rejects. */
  statsMode: 'ok' | 'none' | 'throw' = 'ok';
  /** Rewrites what setRemoteDescription applies (a misbehaving stack). */
  rewriteRemote: ((sdp: string) => string) | null = null;
  localDescription: RTCSessionDescriptionInit | null = null;
  remoteDescription: RTCSessionDescriptionInit | null = null;
  signalingState: RTCSignalingState = 'stable';
  connectionState: RTCPeerConnectionState = 'new';
  iceGatheringState: RTCIceGatheringState = 'complete';
  closed = false;
  candidates: RTCIceCandidateInit[] = [];
  senders: { track: FakeTrack | null; replaceTrack: (t: FakeTrack | null) => Promise<void> }[] = [];
  transceivers: { direction: string; codecs: unknown }[] = [];
  ontrack: ((e: { track: FakeTrack; streams: FakeStream[] }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;

  constructor(readonly config: RTCConfiguration) {
    allPcs.push(this);
  }
  addTransceiver(trackOrKind: FakeTrack | string, init: { direction: string }) {
    const t = { direction: init.direction, codecs: null as unknown, setCodecPreferences: (c: unknown) => (t.codecs = c) };
    this.transceivers.push(t);
    const sender = {
      track: typeof trackOrKind === 'string' ? null : trackOrKind,
      replaceTrack: async (nt: FakeTrack | null) => void (sender.track = nt),
    };
    this.senders.push(sender);
    return t;
  }
  getSenders() {
    return this.senders;
  }
  private sdp(type: string) {
    return `v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\na=fingerprint:sha-256 ${this.fingerprint}\r\na=setup:${type === 'offer' ? 'actpass' : 'active'}\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n`;
  }
  async createOffer() {
    return { type: 'offer' as const, sdp: this.sdp('offer') };
  }
  async createAnswer() {
    return { type: 'answer' as const, sdp: this.sdp('answer') };
  }
  async setLocalDescription(d: RTCSessionDescriptionInit) {
    this.localDescription = d;
    this.signalingState = d.type === 'offer' ? 'have-local-offer' : 'stable';
    this.maybeConnect();
  }
  async setRemoteDescription(d: RTCSessionDescriptionInit) {
    this.remoteDescription = { type: d.type, sdp: this.rewriteRemote ? this.rewriteRemote(d.sdp!) : d.sdp };
    this.signalingState = d.type === 'offer' ? 'have-remote-offer' : 'stable';
    this.maybeConnect();
  }
  async addIceCandidate(c: RTCIceCandidateInit) {
    this.candidates.push(c);
  }
  addEventListener() {}
  removeEventListener() {}
  async getStats() {
    if (this.statsMode === 'throw') throw new Error('stats unavailable');
    if (this.statsMode === 'none') return new Map<string, Record<string, unknown>>([['X', { id: 'X', type: 'codec' }]]);
    const remote = this.presentedFingerprint ?? soleFingerprint(this.remoteDescription?.sdp ?? '')?.split(' ')[1];
    return new Map<string, Record<string, unknown>>([
      ['T1', { id: 'T1', type: 'transport', remoteCertificateId: 'CR' }],
      ['CR', { id: 'CR', type: 'certificate', fingerprint: remote?.toUpperCase(), fingerprintAlgorithm: 'sha-256' }],
    ]);
  }
  close() {
    this.closed = true;
    this.connectionState = 'closed';
  }
  private maybeConnect() {
    if (this.signalingState !== 'stable' || !this.localDescription || !this.remoteDescription) return;
    setTimeout(() => {
      if (this.closed) return;
      this.ontrack?.({ track: new FakeTrack(), streams: [new FakeStream()] });
      this.connectionState = 'connected';
      this.onconnectionstatechange?.();
    }, 0);
  }
}

class FakeAnalyser {
  fftSize = 512;
  level = 0;
  getFloatTimeDomainData(buf: Float32Array) {
    buf.fill(this.level);
  }
}

class FakeAudioContext {
  analysers: FakeAnalyser[] = [];
  closed = false;
  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }
  createAnalyser() {
    const a = new FakeAnalyser();
    this.analysers.push(a);
    return a;
  }
  async resume() {}
  async close() {
    this.closed = true;
  }
}

interface Harness {
  env: VoiceEnv;
  mics: FakeStream[];
  contexts: FakeAudioContext[];
  audios: { srcObject: unknown; paused: boolean }[];
  pcs: FakePC[];
}

function fakeEnv(opts: { micError?: string } = {}): Harness {
  const h: Harness = { mics: [], contexts: [], audios: [], pcs: [], env: null as unknown as VoiceEnv };
  h.env = {
    createPeerConnection: (config) => {
      const pc = new FakePC(config);
      h.pcs.push(pc);
      return pc as unknown as RTCPeerConnection;
    },
    getUserMedia: async () => {
      if (opts.micError) throw Object.assign(new Error('no'), { name: opts.micError });
      const s = new FakeStream();
      h.mics.push(s);
      return s as unknown as MediaStream;
    },
    enumerateDevices: async () =>
      [
        { kind: 'audioinput', deviceId: 'mic1', label: 'Mic' },
        { kind: 'audiooutput', deviceId: 'spk1', label: '' },
      ] as MediaDeviceInfo[],
    createAudioContext: () => {
      const c = new FakeAudioContext();
      h.contexts.push(c);
      return c as unknown as AudioContext;
    },
    createAudioElement: () => {
      const a = { srcObject: null as unknown, paused: true, autoplay: false, play: async () => void (a.paused = false), pause: () => void (a.paused = true) };
      h.audios.push(a);
      return a as unknown as HTMLAudioElement;
    },
    opusCodecs: () => [{ mimeType: 'audio/opus', clockRate: 48000, channels: 2 }],
  };
  return h;
}

class FakeCore implements VoiceCore {
  stateHandlers = new Set<(s: CallState) => void>();
  signalHandlers = new Set<(s: CallSignal) => void>();
  sent: { to: Id; kind: SignalKind; payload: string }[] = [];
  muted: boolean[] = [];
  left = 0;
  /** Where sendSignal delivers (another engine's core), if wired. */
  wire: ((to: Id, kind: SignalKind, payload: string) => void) | null = null;
  constructor(readonly me: Id, readonly participants: Id[]) {}
  state(): CallState {
    return { channelId: CH, participants: this.participants.map((identity) => ({ identity, muted: false })), full: false };
  }
  async joinCall() {
    return this.state();
  }
  async leaveCall() {
    this.left++;
    return null;
  }
  async setMuted(m: boolean) {
    this.muted.push(m);
    return null;
  }
  async sendSignal(to: Id, kind: SignalKind, payload: string) {
    this.sent.push({ to, kind, payload });
    this.wire?.(to, kind, payload);
    return null;
  }
  onState(h: (s: CallState) => void) {
    this.stateHandlers.add(h);
    return () => this.stateHandlers.delete(h);
  }
  onSignal(h: (s: CallSignal) => void) {
    this.signalHandlers.add(h);
    return () => this.signalHandlers.delete(h);
  }
  emitState(s: CallState) {
    for (const h of this.stateHandlers) h(s);
  }
  emitSignal(s: CallSignal) {
    for (const h of this.signalHandlers) h(s);
  }
}

const flush = async (n = 10) => {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};

/** Two engines whose cores pass signals to each other, as two verifying cores would. */
function pair(ids: [Id, Id] = [A, B], opts: Omit<VoiceEngineOptions, 'env'> = {}) {
  const [x, y] = ids;
  const hx = fakeEnv();
  const hy = fakeEnv();
  const cx = new FakeCore(x, [x, y]);
  const cy = new FakeCore(y, [y, x]);
  cx.wire = (to, kind, payload) => to === y && setTimeout(() => cy.emitSignal({ from: x, kind, payload, fingerprintVerified: true }), 0);
  cy.wire = (to, kind, payload) => to === x && setTimeout(() => cx.emitSignal({ from: y, kind, payload, fingerprintVerified: true }), 0);
  const ex = createVoiceEngine(cx, { env: hx.env, stunUrl: 'stun:stun.example.test:3478', ...opts });
  const ey = createVoiceEngine(cy, { env: hy.env, stunUrl: 'stun:stun.example.test:3478', ...opts });
  return { hx, hy, cx, cy, ex, ey };
}

function record<T>(on: (cb: (p: T) => void) => unknown): T[] {
  const out: T[] = [];
  on((p) => out.push(p));
  return out;
}

afterEach(() => {
  vi.useRealTimers();
  allPcs.length = 0;
});

// ---- tests -------------------------------------------------------------------

describe('sdp helpers', () => {
  it('reads and normalises fingerprints, and wants exactly one', () => {
    expect(fingerprints('a=fingerprint:SHA-256 AB:CD\r\nx\na=fingerprint:sha-512 0a:0b')).toEqual(['sha-256 ab:cd', 'sha-512 0a:0b']);
    expect(soleFingerprint('v=0\r\na=fingerprint:sha-256 AB:CD\r\n')).toBe('sha-256 ab:cd');
    expect(soleFingerprint('v=0\r\n')).toBeNull();
    expect(soleFingerprint('a=fingerprint:sha-256 AB:CD\na=fingerprint:sha-256 AB:CE')).toBeNull();
    expect(soleFingerprint('a=fingerprint:md5 AB:CD')).toBeNull();
    // sha-256 or stronger only.
    expect(normalizeFingerprint('sha-1', 'AB:CD')).toBeNull();
    expect(normalizeFingerprint('sha-224', 'AB:CD')).toBeNull();
    expect(normalizeFingerprint('sha-384', 'AB:CD')).toBe('sha-384 ab:cd');
    expect(soleFingerprint('a=fingerprint:sha-1 AB:CD')).toBeNull();
    expect(soleFingerprint('a=fingerprint:sha-256 AB:CD extra')).toBeNull();
  });

  it('takes ICE servers from the configured STUN URL only, never a default', () => {
    expect(iceServersFor(undefined)).toEqual([]);
    expect(iceServersFor(null)).toEqual([]);
    expect(iceServersFor('')).toEqual([]);
    expect(iceServersFor('stun:stun.example.test:3478')).toEqual([{ urls: 'stun:stun.example.test:3478' }]);
    expect(iceServersFor('turn:turn.example.test')).toEqual([]);
    expect(iceServersFor('stun:user@host')).toEqual([]);
    expect(iceServersFor('https://example.test')).toEqual([]);
  });
});

describe('voice engine', () => {
  it('connects two peers: only the lower identity offers, media plays after the checks', async () => {
    const { hx, hy, cx, cy, ex, ey } = pair();
    const partsX = record<VoiceParticipant[]>((cb) => ex.on('participants', cb));
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await flush();

    expect(cx.sent.map((s) => s.kind)).toEqual(['offer']);
    expect(cy.sent.map((s) => s.kind)).toEqual(['answer']);
    expect(hx.pcs).toHaveLength(1);
    expect(hy.pcs).toHaveLength(1);
    // Opus only, with the microphone, through the configured STUN server only.
    expect(hx.pcs[0]!.config.iceServers).toEqual([{ urls: 'stun:stun.example.test:3478' }]);
    expect(hx.pcs[0]!.transceivers[0]).toMatchObject({ direction: 'sendrecv', codecs: [{ mimeType: 'audio/opus' }] });
    expect(hx.audios).toHaveLength(1);
    expect(hy.audios).toHaveLength(1);
    expect(hx.audios[0]!.paused).toBe(false);
    expect(partsX.at(-1)).toEqual([
      { identity: A, state: 'connected' },
      { identity: B, state: 'connected' },
    ]);
  });

  it('glare: an offer from the higher identity is ignored by the lower one', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), A);
    await flush();
    expect(core.sent.map((s) => [s.to, s.kind])).toEqual([[B, 'offer']]);

    // B offers too (it got the rule wrong, or both raced): A keeps its own offer.
    core.emitSignal({ from: B, kind: 'offer', payload: `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`, fingerprintVerified: true });
    await flush();
    expect(core.sent.map((s) => s.kind)).toEqual(['offer']);
    expect(h.pcs).toHaveLength(1);
    expect(h.pcs[0]!.signalingState).toBe('have-local-offer');
  });

  it('the higher identity waits for the offer instead of sending one', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A, C]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), B);
    await flush();
    // B < C, so B offers to C; A < B, so B waits for A.
    expect(core.sent.map((s) => [s.to, s.kind])).toEqual([[C, 'offer']]);
  });

  it('drops a peer whose signal is not fingerprint-verified', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), B);

    core.emitSignal({ from: A, kind: 'offer', payload: `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`, fingerprintVerified: false });
    await flush();
    expect(h.pcs).toHaveLength(0);
    expect(core.sent).toEqual([]);
    expect(errors).toEqual([{ code: 'unverified-signal', peer: A }]);

    // Refused for the rest of the call, even with a verified one later.
    core.emitSignal({ from: A, kind: 'offer', payload: `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`, fingerprintVerified: true });
    await flush();
    expect(h.pcs).toHaveLength(0);
  });

  it('an unverified signal closes an existing connection to that peer', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), A);
    await flush();
    expect(h.pcs).toHaveLength(1);
    core.emitSignal({ from: B, kind: 'ice', payload: '{"candidate":"candidate:1 1 udp 1 192.0.2.1 9 typ host"}', fingerprintVerified: false });
    await flush();
    expect(h.pcs[0]!.closed).toBe(true);
    expect(h.pcs[0]!.candidates).toEqual([]);
  });

  it('ignores signals from someone not in the call', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), B);
    core.emitSignal({ from: '0'.repeat(64), kind: 'offer', payload: `a=fingerprint:sha-256 ${newFingerprint()}`, fingerprintVerified: true });
    await flush();
    expect(h.pcs).toHaveLength(0);
  });

  it('drops a peer whose SDP names no single fingerprint, before applying it', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), B);
    const twoFps = `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`;
    core.emitSignal({ from: A, kind: 'offer', payload: twoFps, fingerprintVerified: true });
    await flush();
    expect(h.pcs[0]!.closed).toBe(true);
    expect(h.pcs[0]!.remoteDescription).toBeNull();
    expect(core.sent).toEqual([]);
    expect(errors).toEqual([{ code: 'fingerprint-mismatch', peer: A }]);
  });

  it('drops a peer when the applied SDP differs from the verified fingerprint', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), B);
    const swap = newFingerprint();
    const realCreate = h.env.createPeerConnection;
    h.env.createPeerConnection = (c) => {
      const pc = realCreate(c) as unknown as FakePC;
      pc.rewriteRemote = (sdp) => sdp.replace(/a=fingerprint:sha-256 \S+/, `a=fingerprint:sha-256 ${swap}`);
      return pc as unknown as RTCPeerConnection;
    };
    core.emitSignal({ from: A, kind: 'offer', payload: `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`, fingerprintVerified: true });
    await flush();
    expect(h.pcs[0]!.closed).toBe(true);
    expect(core.sent).toEqual([]);
    expect(errors).toEqual([{ code: 'fingerprint-mismatch', peer: A }]);
  });

  it('drops a peer that presents a different DTLS certificate, before playing its audio', async () => {
    const { hx, hy, cx, cy, ex, ey } = pair();
    const errors = record<VoiceError>((cb) => ey.on('error', cb));
    const realCreate = hy.env.createPeerConnection;
    hy.env.createPeerConnection = (c) => {
      const pc = realCreate(c) as unknown as FakePC;
      pc.presentedFingerprint = newFingerprint(); // a man in the middle's certificate
      return pc as unknown as RTCPeerConnection;
    };
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await flush();
    expect(hy.pcs[0]!.closed).toBe(true);
    expect(hy.audios).toEqual([]); // nothing from that peer was ever played
    expect(errors).toEqual([{ code: 'fingerprint-mismatch', peer: A }]);
    expect(hx.audios).toHaveLength(1); // A's side checks on its own
  });

  for (const mode of ['none', 'throw'] as const) {
    it(`fails closed when the certificate can't be read (getStats: ${mode})`, async () => {
      const { hy, cx, cy, ex, ey } = pair();
      const errors = record<VoiceError>((cb) => ey.on('error', cb));
      const realCreate = hy.env.createPeerConnection;
      hy.env.createPeerConnection = (c) => {
        const pc = realCreate(c) as unknown as FakePC;
        pc.statsMode = mode;
        return pc as unknown as RTCPeerConnection;
      };
      await ex.attach(cx.state(), A);
      await ey.attach(cy.state(), B);
      await flush();
      expect(hy.pcs[0]!.closed).toBe(true);
      expect(hy.audios).toEqual([]);
      expect(errors).toEqual([{ code: 'fingerprint-mismatch', peer: A }]);
    });
  }

  it('caps the trickled ICE candidates a peer can send, in total', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env, answerTimeoutMs: 60_000 });
    await engine.attach(core.state(), A);
    await flush();
    const pc = h.pcs[0]!;
    const ice = (i: number) =>
      core.emitSignal({
        from: B,
        kind: 'ice',
        payload: JSON.stringify({ candidate: `candidate:${i} 1 udp 1 192.0.2.${i % 250} 9 typ host`, sdpMid: '0' }),
        fingerprintVerified: true,
      });
    for (let i = 0; i < 20; i++) ice(i); // before the answer: queued
    await flush();
    core.emitSignal({ from: B, kind: 'answer', payload: `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`, fingerprintVerified: true });
    await flush();
    for (let i = 20; i < 100; i++) ice(i); // after: applied, up to the cap
    await flush();
    expect(pc.candidates).toHaveLength(32);
    engine.dispose();
  });

  it('leave closes every connection, stops the mic and tells the core', async () => {
    const { hx, cx, cy, ex, ey } = pair();
    const parts = record<VoiceParticipant[]>((cb) => ex.on('participants', cb));
    await ex.join(CH);
    await ey.attach(cy.state(), B);
    await flush();
    expect(hx.pcs).toHaveLength(1);

    await ex.leave();
    expect(cx.left).toBe(1);
    expect(hx.pcs.every((pc) => pc.closed)).toBe(true);
    expect(hx.mics[0]!.tracks.every((t) => t.stopped)).toBe(true);
    expect(hx.audios.every((a) => a.paused)).toBe(true);
    expect(hx.contexts.every((c) => c.closed)).toBe(true);
    expect(parts.at(-1)).toEqual([]);

    // Signals after leaving do nothing.
    cx.emitSignal({ from: B, kind: 'offer', payload: `a=fingerprint:sha-256 ${newFingerprint()}`, fingerprintVerified: true });
    await flush();
    expect(hx.pcs).toHaveLength(1);
  });

  it('follows call.state: a peer who leaves is closed, an empty state stops everything', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B, C]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), A);
    await flush();
    expect(h.pcs).toHaveLength(2);
    core.emitState({ channelId: CH, participants: [{ identity: A, muted: false }, { identity: C, muted: false }], full: false });
    expect(h.pcs[0]!.closed).toBe(true);
    expect(h.pcs[1]!.closed).toBe(false);
    core.emitState({ channelId: CH, participants: [], full: false });
    expect(h.pcs[1]!.closed).toBe(true);
    expect(h.mics[0]!.tracks[0]!.stopped).toBe(true);
  });

  it('mute disables the mic track and tells the core; setMicMuted only the track', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), A);
    const track = h.mics[0]!.tracks[0]!;
    await engine.setMuted(true);
    expect(track.enabled).toBe(false);
    expect(core.muted).toEqual([true]);
    engine.setMicMuted(false);
    expect(track.enabled).toBe(true);
    expect(core.muted).toEqual([true]);
  });

  it('a changed input device keeps the mute state and replaces the sent track', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), A);
    await flush();
    engine.setMicMuted(true);
    await engine.setInputDevice('mic1');
    const [old, next] = h.mics;
    expect(old!.tracks[0]!.stopped).toBe(true);
    expect(next!.tracks[0]!.enabled).toBe(false);
    expect(h.pcs[0]!.senders[0]!.track).toBe(next!.tracks[0]);
    expect(await engine.listInputDevices()).toEqual([{ deviceId: 'mic1', label: 'Mic' }]);
    expect(await engine.listOutputDevices()).toEqual([{ deviceId: 'spk1', label: 'audiooutput 1' }]);
  });

  it('without a microphone it reports why and still listens', async () => {
    const h = fakeEnv({ micError: 'NotAllowedError' });
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), A);
    await flush();
    expect(errors).toEqual([{ code: 'mic-denied' }]);
    expect(h.pcs[0]!.transceivers[0]!.direction).toBe('recvonly');
  });

  it('speaking: a level over the threshold, held briefly; never while muted', async () => {
    vi.useFakeTimers();
    const h = fakeEnv();
    const core = new FakeCore(A, [A]);
    const engine = createVoiceEngine(core, { env: h.env });
    const events = record<Record<Id, boolean>>((cb) => engine.on('speaking', cb));
    await engine.attach(core.state(), A);
    const mic = h.contexts[0]!.analysers[0]!;

    mic.level = 0.2;
    await vi.advanceTimersByTimeAsync(100);
    expect(events.at(-1)).toEqual({ [A]: true });
    mic.level = 0;
    await vi.advanceTimersByTimeAsync(100);
    expect(events.at(-1)).toEqual({ [A]: true }); // held
    await vi.advanceTimersByTimeAsync(400);
    expect(events.at(-1)).toEqual({ [A]: false });

    mic.level = 0.2;
    engine.setMicMuted(true);
    await vi.advanceTimersByTimeAsync(300);
    expect(events.at(-1)).toEqual({ [A]: false });
  });

  it('a lost first offer: the answerer attaches 1 s late and the pair still connects', async () => {
    vi.useFakeTimers();
    const { hx, hy, cx, cy, ex, ey } = pair();
    const errors = record<VoiceError>((cb) => ex.on('error', cb));
    await ex.attach(cx.state(), A);
    await vi.advanceTimersByTimeAsync(10);
    expect(cx.sent.map((s) => s.kind)).toEqual(['offer']); // lost: B isn't attached yet
    expect(cy.sent).toEqual([]);
    await vi.advanceTimersByTimeAsync(1_000);
    await ey.attach(cy.state(), B);
    await vi.advanceTimersByTimeAsync(4_500); // past the 5 s answer timeout
    expect(cx.sent.map((s) => s.kind)).toEqual(['offer', 'offer']);
    expect(cy.sent.map((s) => s.kind)).toEqual(['answer']);
    expect(hx.pcs).toHaveLength(2);
    expect(hx.pcs[0]!.closed).toBe(true);
    expect(hx.pcs[1]!.connectionState).toBe('connected');
    expect(hx.audios).toHaveLength(1);
    expect(hy.audios).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(60_000); // connected: no more offers, no failure
    expect(cx.sent).toHaveLength(2);
    expect(errors).toEqual([]);
    ex.dispose();
    ey.dispose();
  });

  it('gives up after 3 offers with backoff, as peer-failed', async () => {
    vi.useFakeTimers();
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), A);
    await vi.advanceTimersByTimeAsync(4_900);
    expect(core.sent).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(200); // 5 s: second offer
    expect(core.sent).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(9_800);
    expect(core.sent).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(300); // +10 s: third offer
    expect(core.sent).toHaveLength(3);
    expect(errors).toEqual([]);
    await vi.advanceTimersByTimeAsync(20_000); // +20 s: give up
    expect(core.sent).toHaveLength(3);
    expect(errors).toEqual([{ code: 'peer-failed', peer: B }]);
    expect(h.pcs.every((pc) => pc.closed)).toBe(true);
    engine.dispose();
  });

  it('the answerer drops a connection that stays connecting, as peer-failed', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env, connectTimeoutMs: 300 });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), B);
    const realCreate = h.env.createPeerConnection;
    h.env.createPeerConnection = (c) => {
      const pc = realCreate(c) as unknown as FakePC;
      pc.setLocalDescription = async (d) => void (pc.localDescription = d); // never connects
      return pc as unknown as RTCPeerConnection;
    };
    core.emitSignal({ from: A, kind: 'offer', payload: `v=0
a=fingerprint:sha-256 ${newFingerprint()}
`, fingerprintVerified: true });
    await flush();
    expect(h.pcs[0]!.closed).toBe(false);
    await new Promise((r) => setTimeout(r, 400));
    expect(h.pcs[0]!.closed).toBe(true);
    expect(errors).toEqual([{ code: 'peer-failed', peer: A }]);
    engine.dispose();
  });

  it('a mic chosen after starting without one asks for a rejoin', async () => {
    const h = fakeEnv({ micError: 'NotFoundError' });
    const core = new FakeCore(A, [A, B]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), A);
    await engine.setInputDevice('mic1');
    expect(errors).toEqual([{ code: 'no-device' }, { code: 'rejoin-to-talk' }]);
    engine.dispose();
  });

  it('dispose unsubscribes from the core', async () => {
    const core = new FakeCore(A, [A]);
    const engine = createVoiceEngine(core, { env: fakeEnv().env });
    expect(core.stateHandlers.size).toBe(1);
    engine.dispose();
    expect(core.stateHandlers.size).toBe(0);
    expect(core.signalHandlers.size).toBe(0);
  });
});

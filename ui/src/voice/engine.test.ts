// Voice v1, piece D: the mesh engine against fake WebRTC and a fake core.

import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CallSignal, CallState, Id } from '../core/types';
import type { SignalKind, VoiceCore } from './adapter';
import { createVoiceEngine, type VoiceEngineOptions, type VoiceEnv, type VoiceError, type VoiceParticipant } from './engine';
import { cleanTurnCredentials, fingerprints, iceServersFor, normalizeFingerprint, soleFingerprint } from './sdp';

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

/** An offer from a peer with a mic: one audio m-line, sendrecv. */
const audioOffer = (fp = newFingerprint()) =>
  `v=0\r\na=fingerprint:sha-256 ${fp}\r\na=setup:actpass\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=sendrecv\r\n`;

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

interface FakeSender {
  track: FakeTrack | null;
  streams: FakeStream[];
  replaceTrack: (t: FakeTrack | null) => Promise<void>;
  setStreams: (...s: FakeStream[]) => void;
}

interface FakeTransceiver {
  mid: string | null;
  direction: string;
  stopped: boolean;
  codecs: unknown;
  sender: FakeSender;
  receiver: { track: { kind: string } };
  setCodecPreferences: (c: unknown) => void;
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
  senders: FakeSender[] = [];
  transceivers: FakeTransceiver[] = [];
  ontrack: ((e: { track: FakeTrack; streams: FakeStream[] }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;

  /** setConfiguration calls (G-81 refresh) and restartIce calls. */
  configs: RTCConfiguration[] = [];
  restarts = 0;
  /** Candidate types of the selected pair, for connectionTypes. */
  pairTypes: [string, string] = ['host', 'host'];
  getConfiguration() {
    return this.config;
  }
  setConfiguration(c: RTCConfiguration) {
    this.configs.push(c);
    this.config = c;
  }
  restartIce() {
    this.restarts++;
  }
  constructor(public config: RTCConfiguration) {
    allPcs.push(this);
  }
  private transceiver(direction: string, track: FakeTrack | null, mid: string | null): FakeTransceiver {
    const sender: FakeSender = {
      track,
      streams: [],
      replaceTrack: async (nt) => void (sender.track = nt),
      setStreams: (...s) => void (sender.streams = s),
    };
    const t: FakeTransceiver = {
      mid,
      direction,
      stopped: false,
      codecs: null,
      sender,
      receiver: { track: { kind: 'audio' } },
      setCodecPreferences: (c) => void (t.codecs = c),
    };
    this.transceivers.push(t);
    this.senders.push(sender);
    return t;
  }
  addTransceiver(trackOrKind: FakeTrack | string, init: { direction: string }) {
    return this.transceiver(init.direction, typeof trackOrKind === 'string' ? null : trackOrKind, null);
  }
  getTransceivers() {
    return this.transceivers;
  }
  getSenders() {
    return this.senders;
  }
  /** The transceiver of the one audio m-line ("0"), once negotiated. */
  bound(): FakeTransceiver | undefined {
    return this.transceivers.find((t) => t.mid === '0');
  }
  private sdp(type: string) {
    const direction = this.bound()?.direction ?? 'inactive';
    return `v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\na=fingerprint:sha-256 ${this.fingerprint}\r\na=setup:${type === 'offer' ? 'actpass' : 'active'}\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=${direction}\r\n`;
  }
  async createOffer() {
    // The first transceiver without a mid gets the m-line.
    const t = this.transceivers.find((x) => x.mid === null);
    if (t && !this.bound()) t.mid = '0';
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
    // Like Chromium: a transceiver added with addTransceiver before the offer
    // is NOT bound to the offer's m-line; a new receive-only one is (QA-21).
    // One per audio/video m-line, in order, mid = its index (m=application
    // is SCTP: no transceiver).
    if (d.type === 'offer' && !this.bound()) {
      const kinds = [...(d.sdp ?? '').matchAll(/\r\nm=(\w+) /g)].map((m) => m[1]!);
      kinds.forEach((kind, i) => {
        if (kind !== 'audio' && kind !== 'video') return;
        this.transceiver('recvonly', null, String(i)).receiver.track.kind = kind;
      });
    }
    this.maybeConnect();
  }
  /** Whether the remote side sends audio, from its m-line direction. */
  private remoteSends(): boolean {
    return /a=(sendrecv|sendonly)\r\n/.test(this.remoteDescription?.sdp ?? '');
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
      ['T1', { id: 'T1', type: 'transport', remoteCertificateId: 'CR', selectedCandidatePairId: 'P' }],
      ['P', { id: 'P', type: 'candidate-pair', localCandidateId: 'L', remoteCandidateId: 'R', state: 'succeeded', nominated: true }],
      ['L', { id: 'L', type: 'local-candidate', candidateType: this.pairTypes[0], address: '192.0.2.10', port: 5000 }],
      ['R', { id: 'R', type: 'remote-candidate', candidateType: this.pairTypes[1], address: '198.51.100.20', port: 6000 }],
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
      if (this.remoteSends()) this.ontrack?.({ track: new FakeTrack(), streams: [new FakeStream()] });
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
  /** G-81: mocked TURN credentials (the core api does not have it yet). */
  getTurnCredentials?: () => Promise<unknown>;
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

  it('QA-21: the answerer sends its mic on the offer\'s m-line (sendrecv), so audio flows both ways', async () => {
    const { hx, hy, cx, cy, ex, ey } = pair();
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await flush();
    const answerer = hy.pcs[0]!;
    // One transceiver only: bound to m-line "0", sending the mic, Opus.
    expect(answerer.transceivers).toHaveLength(1);
    expect(answerer.transceivers[0]).toMatchObject({ mid: '0', direction: 'sendrecv', codecs: [{ mimeType: 'audio/opus' }] });
    expect(answerer.transceivers[0]!.sender.track).toBe(hy.mics[0]!.tracks[0]);
    expect(answerer.transceivers[0]!.sender.streams).toEqual([hy.mics[0]]);
    expect(cy.sent[0]!.payload).toContain('a=sendrecv');
    // Each side plays the other: the offerer hears the answerer too.
    expect(hx.audios).toHaveLength(1);
    expect(hy.audios).toHaveLength(1);
  });

  it('an answerer without a mic only receives, and a mic chosen later asks for a rejoin', async () => {
    const hx = fakeEnv();
    const hy = fakeEnv({ micError: 'NotAllowedError' });
    const cx = new FakeCore(A, [A, B]);
    const cy = new FakeCore(B, [B, A]);
    cx.wire = (to, kind, payload) => to === B && setTimeout(() => cy.emitSignal({ from: A, kind, payload, fingerprintVerified: true }), 0);
    cy.wire = (to, kind, payload) => to === A && setTimeout(() => cx.emitSignal({ from: B, kind, payload, fingerprintVerified: true }), 0);
    const ex = createVoiceEngine(cx, { env: hx.env });
    const ey = createVoiceEngine(cy, { env: hy.env });
    const errors = record<VoiceError>((cb) => ey.on('error', cb));
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await flush();
    expect(hy.pcs[0]!.transceivers).toHaveLength(1);
    expect(hy.pcs[0]!.transceivers[0]).toMatchObject({ mid: '0', direction: 'recvonly' });
    expect(hy.pcs[0]!.transceivers[0]!.sender.track).toBeNull();
    expect(hy.audios).toHaveLength(1); // B hears A
    expect(hx.audios).toHaveLength(0); // A gets nothing from B, as expected
    await ey.setInputDevice('mic1');
    expect(errors).toEqual([{ code: 'mic-denied' }, { code: 'rejoin-to-talk' }]);
  });

  it('a new input device replaces the track the answerer sends', async () => {
    const { hy, cx, cy, ex, ey } = pair();
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await flush();
    await ey.setInputDevice('mic1');
    expect(hy.pcs[0]!.transceivers[0]!.sender.track).toBe(hy.mics[1]!.tracks[0]);
  });

  it('the answerer sets any extra offered m-lines (video, a 2nd audio) inactive and closes data channels', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env });
    await engine.attach(core.state(), B);
    const extra =
      `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\na=setup:actpass\r\n` +
      'm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:0\r\na=sendrecv\r\n' +
      'm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:1\r\na=sendrecv\r\n' +
      'm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:2\r\na=sendrecv\r\n' +
      'm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:3\r\n';
    core.emitSignal({ from: A, kind: 'offer', payload: extra, fingerprintVerified: true });
    await flush();
    const pc = h.pcs[0]!;
    expect(core.sent.map((s) => s.kind)).toEqual(['answer']);
    expect(pc.transceivers.map((t) => [t.mid, t.receiver.track.kind, t.direction])).toEqual([
      ['0', 'video', 'inactive'],
      ['1', 'audio', 'sendrecv'],
      ['2', 'audio', 'inactive'],
    ]);
    expect(pc.transceivers[1]!.sender.track).toBe(h.mics[0]!.tracks[0]);
    expect(pc.transceivers.filter((t) => t.sender.track).length).toBe(1); // the mic goes out once
    // A data channel the peer opens is closed at once.
    const channel = { closed: false, close() { this.closed = true; } };
    (pc as unknown as { ondatachannel: (e: { channel: typeof channel }) => void }).ondatachannel({ channel });
    expect(channel.closed).toBe(true);
    engine.dispose();
  });

  it('the answerer drops an offer without an audio m-line', async () => {
    const h = fakeEnv();
    const core = new FakeCore(B, [B, A]);
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), B);
    core.emitSignal({ from: A, kind: 'offer', payload: `v=0\r\na=fingerprint:sha-256 ${newFingerprint()}\r\n`, fingerprintVerified: true });
    await flush();
    expect(h.pcs[0]!.closed).toBe(true);
    expect(core.sent).toEqual([]);
    expect(errors).toEqual([{ code: 'peer-failed', peer: A }]);
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
    core.emitSignal({ from: A, kind: 'offer', payload: audioOffer(), fingerprintVerified: true });
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

  it('a failed offer send is retried, not the end of the re-offers', async () => {
    const { hx, cx, cy, ex, ey } = pair();
    const errors = record<VoiceError>((cb) => ex.on('error', cb));
    const realSend = cx.sendSignal.bind(cx);
    let failures = 1;
    cx.sendSignal = async (to, kind, payload) => {
      if (failures-- > 0) throw Object.assign(new Error('busy'), { code: 'internal' });
      return realSend(to, kind, payload);
    };
    await ey.attach(cy.state(), B);
    await ex.attach(cx.state(), A);
    await flush();
    expect(cx.sent.map((s) => s.kind)).toEqual(['offer']); // the 2nd attempt; the 1st threw
    expect(hx.pcs).toHaveLength(2);
    expect(hx.pcs[0]!.closed).toBe(true);
    expect(hx.pcs[1]!.connectionState).toBe('connected');
    expect(errors).toEqual([]);
  });

  it('an answer that fails to apply leads to a re-offer', async () => {
    const { hx, cx, cy, ex, ey } = pair();
    const errors = record<VoiceError>((cb) => ex.on('error', cb));
    const realCreate = hx.env.createPeerConnection;
    let first = true;
    hx.env.createPeerConnection = (c) => {
      const pc = realCreate(c) as unknown as FakePC;
      if (first) {
        first = false;
        pc.setRemoteDescription = async () => {
          throw new Error('bad answer');
        };
      }
      return pc as unknown as RTCPeerConnection;
    };
    await ey.attach(cy.state(), B);
    await ex.attach(cx.state(), A);
    await flush(20);
    expect(cx.sent.map((s) => s.kind)).toEqual(['offer', 'offer']);
    expect(hx.pcs[1]!.connectionState).toBe('connected');
    expect(errors).toEqual([]);
  });

  it('gives up as peer-failed when every offer send fails', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    core.sendSignal = async () => {
      throw new Error('down');
    };
    const engine = createVoiceEngine(core, { env: h.env });
    const errors = record<VoiceError>((cb) => engine.on('error', cb));
    await engine.attach(core.state(), A);
    await flush();
    expect(h.pcs).toHaveLength(3);
    expect(h.pcs.every((pc) => pc.closed)).toBe(true);
    expect(errors).toEqual([{ code: 'peer-failed', peer: B }]);
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

// ---- TURN (G-81) ------------------------------------------------------------------

const TURN_HOST = 'turn.example.test';
/** Credentials as the relay would mint them (mocked until the core api exists). */
function turnCreds(ttl = 600, now = Date.now(), tag = 'abcdEFGH1234') {
  return {
    urls: [`turn:${TURN_HOST}:3478?transport=udp`, `turn:${TURN_HOST}:3478?transport=tcp`, `turns:${TURN_HOST}:5349`],
    username: `${Math.floor(now / 1000) + ttl}:${tag}`,
    credential: `c2VjcmV0${tag}=`,
    ttl,
    expiresAt: now + ttl * 1000,
  };
}
const turnServer = (c: { username: string }) => (pc: FakePC) =>
  (pc.config.iceServers ?? []).find((s) => (s as RTCIceServer).username === c.username);

describe('TURN credentials (cleanTurnCredentials)', () => {
  const now = Date.now();
  const ok = turnCreds(600, now);
  const clean = (patch: Record<string, unknown>, host: string | null = TURN_HOST) => cleanTurnCredentials({ ...ok, ...patch }, host, now);

  it('accepts well-formed credentials for the configured host', () => {
    expect(cleanTurnCredentials(ok, TURN_HOST, now)).toEqual(ok);
    expect(cleanTurnCredentials(ok, 'TURN.Example.Test', now)).toEqual(ok); // host case does not matter
  });

  it('pins the URLs: exact host, allowed ports, transport= only', () => {
    expect(clean({}, null)).toBeNull(); // no configured relay host: no TURN
    expect(clean({ urls: ['turn:evil.example.test:3478'] })).toBeNull();
    expect(clean({ urls: [`turn:${TURN_HOST}.evil.test:3478`] })).toBeNull();
    expect(clean({ urls: [`turn:x.${TURN_HOST}:3478`] })).toBeNull();
    expect(clean({ urls: [`turn:${TURN_HOST}:3479`] })).toBeNull();
    expect(clean({ urls: [`turn:${TURN_HOST}`] })).toBeNull(); // the port is required
    expect(clean({ urls: [`turn:${TURN_HOST}:3478?transport=udp&x=1`] })).toBeNull();
    expect(clean({ urls: [`turn:${TURN_HOST}:3478?transport=sctp`] })).toBeNull();
    expect(clean({ urls: [`stun:${TURN_HOST}:3478`] })).toBeNull();
    expect(clean({ urls: [`turn:user@${TURN_HOST}:3478`] })).toBeNull();
    expect(clean({ urls: [] })).toBeNull();
    expect(clean({ urls: Array(5).fill(`turn:${TURN_HOST}:3478`) })).toBeNull();
    expect(clean({ urls: [`turns:${TURN_HOST}:443?transport=tcp`] })?.urls).toEqual([`turns:${TURN_HOST}:443?transport=tcp`]);
  });

  it('wants <expiry>:<random> (never a peer id), a plain credential, a sane ttl and expiry', () => {
    expect(clean({ username: 'a'.repeat(64) })).toBeNull();
    expect(clean({ username: `1800000000:${'a'.repeat(64)}:x` })).toBeNull();
    expect(clean({ username: 'x'.repeat(300) })).toBeNull();
    expect(clean({ credential: '' })).toBeNull();
    expect(clean({ credential: 'has space' })).toBeNull();
    expect(clean({ credential: 'x'.repeat(257) })).toBeNull();
    expect(clean({ ttl: 30 })).toBeNull();
    expect(clean({ ttl: 100_000 })).toBeNull();
    expect(clean({ ttl: 600.5 })).toBeNull();
    expect(clean({ expiresAt: now - 1 })).toBeNull();
    expect(clean({ expiresAt: now + 3_600_000 })).toBeNull(); // beyond its own ttl
    expect(cleanTurnCredentials(null, TURN_HOST, now)).toBeNull();
    expect(cleanTurnCredentials('turn:x', TURN_HOST, now)).toBeNull();
  });

  it('iceServersFor: STUN, then TURN with its credentials', () => {
    expect(iceServersFor('stun:stun.example.test:3478', ok)).toEqual([
      { urls: 'stun:stun.example.test:3478' },
      { urls: ok.urls, username: ok.username, credential: ok.credential },
    ]);
    expect(iceServersFor(null, ok)).toEqual([{ urls: ok.urls, username: ok.username, credential: ok.credential }]);
  });
});

describe('voice engine with TURN', () => {
  it('off by default: never asks the core, STUN only', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const ask = vi.fn(async () => turnCreds());
    core.getTurnCredentials = ask;
    const engine = createVoiceEngine(core, { env: h.env, stunUrl: 'stun:stun.example.test:3478', turnHost: TURN_HOST });
    await engine.attach(core.state(), A);
    await flush();
    expect(ask).not.toHaveBeenCalled();
    expect(h.pcs[0]!.config.iceServers).toEqual([{ urls: 'stun:stun.example.test:3478' }]);
    engine.dispose();
  });

  it('on: the first connection already has TURN', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const c = turnCreds();
    core.getTurnCredentials = async () => c;
    const engine = createVoiceEngine(core, { env: h.env, stunUrl: 'stun:stun.example.test:3478', enableTurn: true, turnHost: TURN_HOST });
    await engine.attach(core.state(), A);
    await flush();
    expect(h.pcs).toHaveLength(1);
    expect(h.pcs[0]!.config.iceServers).toEqual([
      { urls: 'stun:stun.example.test:3478' },
      { urls: c.urls, username: c.username, credential: c.credential },
    ]);
    engine.dispose();
  });

  const failures: [string, () => Promise<unknown>][] = [
    ['rejects', async () => Promise.reject(Object.assign(new Error('x'), { code: 'not-implemented' }))],
    ['never answers (2 s, then without)', () => new Promise<never>(() => {})],
    ['answers for another host', async () => ({ ...turnCreds(), urls: ['turn:evil.example.test:3478'] })],
  ];
  for (const [name, ask] of failures) {
    it(`goes on without TURN when the core ${name}`, async () => {
      vi.useFakeTimers();
      const h = fakeEnv();
      const core = new FakeCore(A, [A, B]);
      core.getTurnCredentials = ask;
      const engine = createVoiceEngine(core, { env: h.env, enableTurn: true, turnHost: TURN_HOST });
      const errors = record<VoiceError>((cb) => engine.on('error', cb));
      const attached = engine.attach(core.state(), A);
      await vi.advanceTimersByTimeAsync(2_100);
      await attached;
      expect(h.pcs).toHaveLength(1);
      expect(h.pcs[0]!.config.iceServers).toEqual([]);
      expect(errors).toEqual([]);
      engine.dispose();
    });
  }

  it('caches the credentials until 80% of the ttl: a second call does not ask again', async () => {
    const h = fakeEnv();
    const core = new FakeCore(A, [A, B]);
    const c = turnCreds(600);
    const ask = vi.fn(async () => c);
    core.getTurnCredentials = ask;
    const engine = createVoiceEngine(core, { env: h.env, enableTurn: true, turnHost: TURN_HOST });
    await engine.attach(core.state(), A);
    engine.detach();
    await engine.attach(core.state(), A);
    await flush();
    expect(ask).toHaveBeenCalledTimes(1);
    expect(turnServer(c)(h.pcs.at(-1)!)).toBeTruthy();
    engine.dispose();
  });

  it('refresh at 80% of the ttl: new servers on every connection, only the offerer restarts ICE, same connection', async () => {
    vi.useFakeTimers();
    const { hx, hy, cx, cy, ex, ey } = pair([A, B], { enableTurn: true, turnHost: TURN_HOST });
    let n = 0;
    const issued: ReturnType<typeof turnCreds>[] = [];
    const mint = async () => {
      const c = turnCreds(100, Date.now(), `tag${++n}xxxxxx`);
      issued.push(c);
      return c;
    };
    cx.getTurnCredentials = mint;
    cy.getTurnCredentials = mint;
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await vi.advanceTimersByTimeAsync(50);
    expect(hx.pcs).toHaveLength(1);
    expect(hy.pcs).toHaveLength(1);
    expect(hx.audios).toHaveLength(1);
    expect(issued).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(80_000); // 80% of 100 s
    expect(issued).toHaveLength(4);
    const offerer = hx.pcs[0]!;
    const answerer = hy.pcs[0]!;
    const fresh = (pc: FakePC) => !!(turnServer(issued[2]!)(pc) ?? turnServer(issued[3]!)(pc));
    expect(fresh(offerer)).toBe(true);
    expect(fresh(answerer)).toBe(true);
    expect(offerer.configs).toHaveLength(1);
    expect(answerer.configs).toHaveLength(1);
    expect(offerer.restarts).toBe(1);
    expect(answerer.restarts).toBe(0);
    // The offerer renegotiated on the same connection; the answerer answered on it.
    expect(cx.sent.map((s) => s.kind)).toEqual(['offer', 'offer']);
    expect(cy.sent.map((s) => s.kind)).toEqual(['answer', 'answer']);
    expect(hx.pcs).toHaveLength(1);
    expect(hy.pcs).toHaveLength(1);
    expect(offerer.closed || answerer.closed).toBe(false);
    expect(hx.audios).toHaveLength(1);
    ex.dispose();
    ey.dispose();
  });

  it('never puts the credentials in an event, even when fetching fails', async () => {
    vi.useFakeTimers();
    const { cx, cy, ex, ey } = pair([A, B], { enableTurn: true, turnHost: TURN_HOST });
    const c = turnCreds(100);
    cx.getTurnCredentials = async () => c;
    cy.getTurnCredentials = async () => Promise.reject(new Error(`${c.username} ${c.credential}`));
    const seen: unknown[] = [];
    for (const e of [ex, ey]) {
      e.on('participants', (p) => seen.push(p));
      e.on('speaking', (p) => seen.push(p));
      e.on('error', (p) => seen.push(p));
    }
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await vi.advanceTimersByTimeAsync(81_000);
    const text = JSON.stringify(seen) + JSON.stringify(await ex.connectionTypes());
    expect(seen.length).toBeGreaterThan(0);
    expect(text).not.toContain(c.credential);
    expect(text).not.toContain(c.username);
    ex.dispose();
    ey.dispose();
  });

  it('connectionTypes: the selected pair candidate types, never addresses', async () => {
    const { hx, cx, cy, ex, ey } = pair();
    await ex.attach(cx.state(), A);
    await ey.attach(cy.state(), B);
    await flush();
    hx.pcs[0]!.pairTypes = ['relay', 'srflx'];
    const types = await ex.connectionTypes();
    expect(types).toEqual({ [B]: { local: 'relay', remote: 'srflx' } });
    expect(JSON.stringify(types)).not.toMatch(/\d+\.\d+\.\d+\.\d+/);
  });
});

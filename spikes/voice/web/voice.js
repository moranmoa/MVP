// Browser / Electron variant of the voice spike: a full-mesh group call over
// the browser's own WebRTC stack (Chromium's in Electron).
//
// URL parameters (all optional):
//   signal=ws://host:8080  room=test  name=alice
//   autojoin=1             join without a click
//   src=mic|pulses         pulses = synthetic 30 ms tone at every wall-clock
//                          second, used to measure mouth-to-ear latency
//   aec=0 ns=0 agc=0       turn the browser's audio processing off
//   duration=60            leave and report after N seconds (bench)
//
// Latency method: the sender starts a tone burst when the wall clock crosses
// a whole second; the receiver detects the burst onset in the decoded remote
// audio. Both peers read the same clock (same machine or same Docker host),
// so onset time minus the second boundary is the one-way latency through
// encoder, network, jitter buffer and decoder. Sound-card buffers on both
// ends are NOT included (add ~10-40 ms on real hardware).

'use strict';

const params = new URLSearchParams(location.search);
// Which WebRTC stack this is: Chromium (Electron, Chrome, Edge, WebView2) or
// WebKit (WebKitGTK / WKWebView, i.e. Tauri on Linux and macOS, Safari).
const ENGINE = /Chrome\//.test(navigator.userAgent) ? 'chromium' : 'webkit';
const $ = (id) => document.getElementById(id);
const log = (...a) => {
  const line = `${new Date().toISOString().slice(11, 23)} ${a.join(' ')}`;
  console.log(line);
  $('log').textContent += line + '\n';
};

const state = {
  id: params.get('name') || `p-${Math.random().toString(36).slice(2, 7)}`,
  ws: null,
  ctx: null,
  local: null, // MediaStream we send
  peers: new Map(), // id -> {pc, kind, audio, stream, latency:[], level, stats, samples:[]}
  startedAt: 0,
  cpuStart: null,
  iceServers: [],
  kinds: new Map(), // id -> kind, from the signalling server
  gone: new Map(), // peers that left; kept for the report
};

// ---------------------------------------------------------------- worklets
// Inline so the page also works from file:// (Electron) without a server.
const WORKLETS = `
// Offset between Date.now() and this context's frame clock, estimated as
// the smallest (wall - frameTime) seen: render quanta are computed in
// batches ahead of time, never late, so the minimum tracks the true clock.
// The minimum is re-taken every second: under CPU load the audio thread can
// slip (underrun) and the frame clock then falls behind the wall clock.
class Clock {
  constructor() { this.offset = Infinity; this.window = Infinity; this.frames = 0; this.n = 0; }
  tick(n) {
    const t = this.frames / sampleRate * 1000;
    const o = Date.now() - t;
    if (o < this.window) this.window = o;
    if (this.offset === Infinity || ++this.n % 375 === 0) { this.offset = this.window; this.window = Infinity; }
    this.frames += n;
    return Math.min(this.offset, this.window) + t; // wall time of the first sample of this quantum
  }
}

class Pulser extends AudioWorkletProcessor {
  constructor() { super(); this.clock = new Clock(); this.phase = 0; }
  process(_in, outputs) {
    const out = outputs[0][0];
    const wall0 = this.clock.tick(out.length);
    for (let i = 0; i < out.length; i++) {
      const ms = (wall0 + i / sampleRate * 1000) % 1000;
      this.phase += 2 * Math.PI * 1000 / sampleRate;
      // 30 ms burst, plus very quiet noise so the encoder never idles.
      out[i] = (ms < 30 ? 0.5 * Math.sin(this.phase) : 0) + (Math.random() - 0.5) * 0.002;
    }
    for (let c = 1; c < outputs[0].length; c++) outputs[0][c].set(out);
    return true;
  }
}

class Detector extends AudioWorkletProcessor {
  constructor() { super(); this.clock = new Clock(); this.quiet = 0; this.peak = 0; this.n = 0; }
  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    const len = ch ? ch.length : 128;
    const wall0 = this.clock.tick(len);
    if (!ch) return true;
    for (let i = 0; i < len; i++) {
      const v = Math.abs(ch[i]);
      if (v > this.peak) this.peak = v;
      if (v > 0.1) {
        if (this.quiet > sampleRate * 0.3) {
          const wall = wall0 + i / sampleRate * 1000;
          const ms = wall % 1000;
          // Bursts start on whole seconds; anything later than 900 ms is noise.
          if (ms < 900) this.port.postMessage({ latency: ms });
        }
        this.quiet = 0;
      } else {
        this.quiet++;
      }
    }
    if (++this.n % 40 === 0) { this.port.postMessage({ level: this.peak }); this.peak = 0; }
    return true;
  }
}
registerProcessor('pulser', Pulser);
registerProcessor('detector', Detector);
`;

async function audioContext() {
  if (state.ctx) return state.ctx;
  const ctx = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' });
  const url = URL.createObjectURL(new Blob([WORKLETS], { type: 'text/javascript' }));
  await ctx.audioWorklet.addModule(url);
  await ctx.resume();
  state.ctx = ctx;
  return ctx;
}

// ------------------------------------------------------------- capabilities
async function capabilities() {
  const md = navigator.mediaDevices;
  const supported = md && md.getSupportedConstraints ? md.getSupportedConstraints() : {};
  const caps = {
    userAgent: navigator.userAgent,
    RTCPeerConnection: typeof RTCPeerConnection === 'function',
    getUserMedia: !!(md && md.getUserMedia),
    enumerateDevices: !!(md && md.enumerateDevices),
    selectAudioOutput: !!(md && md.selectAudioOutput),
    setSinkId: typeof HTMLMediaElement !== 'undefined' && 'setSinkId' in HTMLMediaElement.prototype,
    audioWorklet: typeof AudioWorkletNode === 'function',
    echoCancellation: !!supported.echoCancellation,
    noiseSuppression: !!supported.noiseSuppression,
    autoGainControl: !!supported.autoGainControl,
    insertableStreams: typeof RTCRtpScriptTransform === 'function' || 'createEncodedStreams' in (window.RTCRtpSender?.prototype || {}),
    opus: false,
  };
  try {
    caps.opus = !!RTCRtpSender.getCapabilities('audio')?.codecs.some((c) => /opus/i.test(c.mimeType));
  } catch {}
  return caps;
}

async function fillDevices() {
  if (!navigator.mediaDevices?.enumerateDevices) return;
  const devices = await navigator.mediaDevices.enumerateDevices();
  for (const [sel, kind] of [[$('input'), 'audioinput'], [$('output'), 'audiooutput']]) {
    const keep = sel.value;
    sel.replaceChildren(...devices.filter((d) => d.kind === kind).map((d, i) => new Option(d.label || `${kind} ${i + 1}`, d.deviceId)));
    if (keep) sel.value = keep;
  }
  if (!('setSinkId' in HTMLMediaElement.prototype)) $('sinkNote').textContent = '(לא נתמך כאן – ברירת המחדל של המערכת)';
}

// ------------------------------------------------------------- local media
async function localStream() {
  const ctx = await audioContext();
  if ((params.get('src') || 'mic') === 'pulses') {
    const node = new AudioWorkletNode(ctx, 'pulser', { outputChannelCount: [1] });
    const dest = ctx.createMediaStreamDestination();
    node.connect(dest);
    return dest.stream;
  }
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      deviceId: $('input').value ? { exact: $('input').value } : undefined,
      echoCancellation: $('aec').checked,
      noiseSuppression: $('ns').checked,
      autoGainControl: $('agc').checked,
    },
  });
  const settings = stream.getAudioTracks()[0].getSettings();
  log('mic settings', JSON.stringify(settings));
  meter(stream, (v) => ($('myLevel').style.width = `${Math.min(200, v * 400)}px`));
  await fillDevices(); // labels appear only after permission
  return stream;
}

function meter(stream, onLevel, onLatency) {
  const ctx = state.ctx;
  const src = ctx.createMediaStreamSource(stream);
  const det = new AudioWorkletNode(ctx, 'detector');
  const mute = ctx.createGain();
  mute.gain.value = 0;
  src.connect(det).connect(mute).connect(ctx.destination);
  det.port.onmessage = (e) => {
    if (e.data.level !== undefined) onLevel(e.data.level);
    if (e.data.latency !== undefined && onLatency) onLatency(e.data.latency);
  };
  return det;
}

// ---------------------------------------------------------------- the mesh
function send(msg) {
  state.ws.send(JSON.stringify(msg));
}

function peer(id, kind) {
  if (state.peers.has(id)) return state.peers.get(id);
  const pc = new RTCPeerConnection({ iceServers: state.iceServers });
  const p = { pc, kind: state.kinds.get(id) || kind, audio: null, latency: [], level: 0, samples: [], last: null };
  state.peers.set(id, p);
  for (const t of state.local.getAudioTracks()) pc.addTrack(t, state.local);
  pc.ontrack = (e) => {
    const stream = e.streams[0] || new MediaStream([e.track]);
    // Chromium only feeds remote WebRTC audio into Web Audio when an element
    // also plays it; in bench mode that element stays muted.
    const audio = new Audio();
    audio.srcObject = stream;
    audio.muted = params.get('src') === 'pulses';
    const out = $('output').value;
    if (out && audio.setSinkId) audio.setSinkId(out).catch((err) => log('setSinkId', err.message));
    audio.play().catch((err) => log('play', err.message));
    p.audio = audio;
    meter(stream, (v) => (p.level = v), (ms) => p.latency.push(ms));
  };
  pc.onconnectionstatechange = () => log(id, 'connection', pc.connectionState);
  return p;
}

/** Waits for ICE gathering so the SDP carries every candidate (no trickle). */
function gathered(pc) {
  if (pc.iceGatheringState === 'complete') return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => pc.iceGatheringState === 'complete' && resolve();
    pc.addEventListener('icegatheringstatechange', done);
    setTimeout(resolve, 3000);
  });
}

async function offer(id, kind) {
  const { pc } = peer(id, kind);
  await pc.setLocalDescription(await pc.createOffer());
  await gathered(pc);
  send({ type: 'signal', to: id, data: { sdp: pc.localDescription.toJSON() } });
}

async function onSignal(from, data) {
  const p = peer(from, 'unknown');
  if (!data.sdp) return;
  await p.pc.setRemoteDescription(data.sdp);
  if (data.sdp.type === 'offer') {
    await p.pc.setLocalDescription(await p.pc.createAnswer());
    await gathered(p.pc);
    send({ type: 'signal', to: from, data: { sdp: p.pc.localDescription.toJSON() } });
  }
}

async function join() {
  $('join').disabled = true;
  try {
    const signal = $('signal').value || `ws://${location.host || 'localhost:8080'}`;
    try {
      const cfg = await (await fetch(signal.replace(/^ws/, 'http') + '/config.json')).json();
      state.iceServers = cfg.iceServers || [];
    } catch {
      state.iceServers = [];
    }
    state.local = await localStream();
    state.ws = new WebSocket(signal);
    state.ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      if (msg.type === 'peers') msg.peers.forEach((p) => state.kinds.set(p.id, p.kind) && offer(p.id, p.kind)); // the newcomer offers
      else if (msg.type === 'joined') state.kinds.set(msg.id, msg.kind) && log('joined', msg.id, msg.kind);
      else if (msg.type === 'signal') onSignal(msg.from, msg.data).catch((err) => log('signal error', err.message));
      else if (msg.type === 'left') drop(msg.id);
    };
    await new Promise((resolve, reject) => {
      state.ws.onopen = resolve;
      state.ws.onerror = () => reject(new Error(`cannot reach ${signal}`));
    });
    send({ type: 'join', room: $('room').value, id: state.id, kind: ENGINE });
    state.startedAt = Date.now();
    $('leave').disabled = false;
    $('report').disabled = false;
    log('joined room', $('room').value, 'as', state.id);
  } catch (err) {
    log('join failed:', err.message);
    $('join').disabled = false;
  }
}

function drop(id) {
  const p = state.peers.get(id);
  if (!p) return;
  p.pc.close();
  p.audio?.pause();
  state.peers.delete(id);
  state.gone.set(id, p);
  log('left', id);
}

function leave() {
  for (const id of [...state.peers.keys()]) drop(id);
  state.ws?.close();
  state.local?.getTracks().forEach((t) => t.stop());
  $('join').disabled = false;
  $('leave').disabled = true;
}

// ------------------------------------------------------------------- stats
async function poll() {
  for (const [id, p] of state.peers) {
    const stats = await p.pc.getStats();
    const s = { t: Date.now() };
    stats.forEach((r) => {
      if (r.type === 'inbound-rtp' && r.kind === 'audio') {
        Object.assign(s, {
          jitter: r.jitter * 1000,
          packetsLost: r.packetsLost,
          packetsReceived: r.packetsReceived,
          jbDelay: r.jitterBufferDelay,
          jbEmitted: r.jitterBufferEmittedCount,
          concealed: r.concealedSamples,
          samples: r.totalSamplesReceived,
          codec: r.codecId,
        });
      }
      if (r.type === 'candidate-pair' && r.nominated && r.state === 'succeeded') s.rtt = r.currentRoundTripTime * 1000;
      if (r.type === 'outbound-rtp' && r.kind === 'audio') s.bytesSent = r.bytesSent;
    });
    p.samples.push(s);
    p.last = s;
  }
  render();
}

function summarize(p) {
  const first = p.samples.find((s) => s.packetsReceived !== undefined) || {};
  const last = p.last || {};
  const recv = (last.packetsReceived || 0) - (first.packetsReceived || 0);
  const lost = (last.packetsLost || 0) - (first.packetsLost || 0);
  const jitters = p.samples.map((s) => s.jitter).filter((x) => x !== undefined);
  const rtts = p.samples.map((s) => s.rtt).filter((x) => x !== undefined);
  const lat = p.latency.slice(2); // skip the first bursts while the jitter buffer settles
  return {
    kind: p.kind,
    latencyMs: stat(lat),
    jitterMs: stat(jitters),
    rttMs: stat(rtts),
    lossPct: recv + lost > 0 ? (100 * lost) / (recv + lost) : null,
    jitterBufferMs:
      last.jbEmitted > first.jbEmitted ? (1000 * (last.jbDelay - first.jbDelay)) / (last.jbEmitted - first.jbEmitted) : null,
    concealedPct: last.samples > first.samples ? (100 * (last.concealed - first.concealed)) / (last.samples - first.samples) : null,
  };
}

function stat(xs) {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  const q = (f) => s[Math.min(s.length - 1, Math.floor(f * s.length))];
  return { n: s.length, median: round(q(0.5)), p95: round(q(0.95)), min: round(s[0]), max: round(s[s.length - 1]) };
}
const round = (x) => Math.round(x * 10) / 10;

function render() {
  const rows = [];
  for (const [id, p] of state.peers) {
    const sum = summarize(p);
    const td = (x) => {
      const cell = document.createElement('td');
      cell.textContent = x ?? '–';
      return cell;
    };
    const tr = document.createElement('tr');
    tr.append(
      td(`${id} (${p.kind})`),
      td(p.last?.rtt?.toFixed(0)),
      td(p.last?.jitter?.toFixed(1)),
      td(sum.lossPct?.toFixed(1)),
      td(sum.jitterBufferMs?.toFixed(0)),
      td(sum.latencyMs?.median),
      td((p.level * 100).toFixed(0)),
    );
    rows.push(tr);
  }
  $('peers').replaceChildren(...rows);
}

async function report() {
  const data = {
    id: state.id,
    variant: ENGINE,
    room: $('room').value,
    src: params.get('src') || 'mic',
    processing: { aec: $('aec').checked, ns: $('ns').checked, agc: $('agc').checked },
    seconds: (Date.now() - state.startedAt) / 1000,
    capabilities: await capabilities(),
    peers: Object.fromEntries([...state.gone, ...state.peers].map(([id, p]) => [id, summarize(p)])),
  };
  if (state.ws?.readyState === WebSocket.OPEN) send({ type: 'report', data });
  window.__report = data;
  log('report', JSON.stringify(data.peers));
  return data;
}
window.voiceReport = report;

// -------------------------------------------------------------------- boot
(async () => {
  $('name').value = state.id;
  $('signal').value = params.get('signal') || (location.protocol.startsWith('http') ? `ws://${location.host}` : '');
  $('room').value = params.get('room') || 'test';
  for (const k of ['aec', 'ns', 'agc']) if (params.get(k) === '0') $(k).checked = false;
  $('join').onclick = join;
  $('leave').onclick = leave;
  $('report').onclick = report;
  const caps = await capabilities();
  window.__caps = caps;
  $('caps').textContent = JSON.stringify(caps, null, 2);
  await fillDevices().catch((err) => log('devices', err.message));
  navigator.mediaDevices?.addEventListener?.('devicechange', fillDevices);
  setInterval(() => poll().catch((err) => log('stats', err.message)), 1000);
  if (params.get('autojoin') === '1') await join();
  const duration = Number(params.get('duration') || 0);
  if (duration) {
    setTimeout(async () => {
      await report();
      window.__done = true;
    }, duration * 1000);
  }
})();

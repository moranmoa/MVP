//! Voice spike, Tauri variant: a full-mesh group call where WebRTC runs in
//! Rust (str0m) and audio goes straight to the sound card (cpal), so the
//! webview's own WebRTC (weak in WebKitGTK) is never used. Joins the same
//! rooms as the browser / Electron client through spikes/voice/signal.
//!
//!   str0m-peer --signal ws://host:8080 --room test [--name alice]
//!              [--src mic|pulses] [--input NAME] [--output NAME]
//!              [--duration SECS] [--results DIR] [--warmup SECS]
//!   str0m-peer --list-devices
//!
//! `--src pulses` sends a 30 ms tone at every wall-clock second and measures
//! the one-way latency of the bursts it receives, like voice.js does.

mod apm;
mod audio;
mod jitter;
mod net;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

use jitter::{JitterBuffer, FRAME};

struct Args {
    signal: String,
    room: String,
    name: String,
    pulses: bool,
    input: Option<String>,
    output: Option<String>,
    duration: Option<u64>,
    warmup: u64,
    results: Option<String>,
}

fn args() -> Args {
    let mut a = Args {
        signal: "ws://localhost:8080".into(),
        room: "test".into(),
        name: format!("r-{:05x}", std::process::id() ^ (now_ms() as u32 & 0xfffff)),
        pulses: false,
        input: None,
        output: None,
        duration: None,
        warmup: 10,
        results: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--signal" => a.signal = v(),
            "--room" => a.room = v(),
            "--name" => a.name = v(),
            "--src" => a.pulses = v() == "pulses",
            "--input" => a.input = Some(v()),
            "--output" => a.output = Some(v()),
            "--duration" => a.duration = v().parse().ok(),
            "--warmup" => a.warmup = v().parse().unwrap_or(10),
            "--results" => a.results = Some(v()),
            "--list-devices" => {
                audio::list_devices();
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument {other}; see the top of src/main.rs");
                std::process::exit(2);
            }
        }
    }
    a
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
}

// ------------------------------------------------------------------ peers

struct Conn {
    rtc: Rtc,
    mid: Option<Mid>,
    pt: Option<Pt>,
    pending: Option<SdpPendingOffer>,
    connected: bool,
    next_timeout: Instant,
}

#[derive(Default)]
struct Stats {
    latency: Vec<f64>,
    rtt: Vec<f64>,
    jitter: Vec<f64>,
    quiet: usize,
    level: f32,
}

struct Peer {
    kind: String,
    conn: Mutex<Conn>,
    socket: UdpSocket,
    jb: Mutex<JitterBuffer>,
    stats: Mutex<Stats>,
    alive: AtomicBool,
}

impl Peer {
    fn new(kind: &str, stun: &[SocketAddr]) -> std::io::Result<Arc<Peer>> {
        let socket = UdpSocket::bind((net::local_ip(), 0))?;
        let mut rtc = RtcConfig::new()
            .set_stats_interval(Some(Duration::from_secs(1)))
            // str0m holds 15 audio packets (300 ms) back for reordering by
            // default, stalling playout after every loss; jitter.rs reorders.
            .set_reordering_size_audio(0)
            .build(Instant::now());
        let local = socket.local_addr()?;
        rtc.add_local_candidate(Candidate::host(local, "udp").expect("host candidate"));
        for server in stun {
            if let Some(public) = net::stun_binding(&socket, *server) {
                if public != local {
                    if let Ok(c) = Candidate::server_reflexive(public, local, "udp") {
                        rtc.add_local_candidate(c);
                    }
                }
                break;
            }
        }
        Ok(Arc::new(Peer {
            kind: kind.into(),
            conn: Mutex::new(Conn { rtc, mid: None, pt: None, pending: None, connected: false, next_timeout: Instant::now() }),
            socket,
            jb: Mutex::new(JitterBuffer::new()),
            stats: Mutex::new(Stats::default()),
            alive: AtomicBool::new(true),
        }))
    }

    /// Drain str0m's output; must follow every mutation of the Rtc.
    fn drain(&self, c: &mut Conn) {
        loop {
            match c.rtc.poll_output() {
                Ok(Output::Timeout(t)) => {
                    c.next_timeout = t;
                    return;
                }
                Ok(Output::Transmit(t)) => {
                    let _ = self.socket.send_to(&t.contents, t.destination);
                }
                Ok(Output::Event(e)) => match e {
                    Event::Connected => c.connected = true,
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        self.alive.store(false, Ordering::Relaxed);
                    }
                    Event::MediaAdded(m) if m.kind == MediaKind::Audio => {
                        c.mid.get_or_insert(m.mid);
                    }
                    Event::MediaData(d) => {
                        self.jb.lock().unwrap().push(d.time.numer(), &d.data, now_ms());
                    }
                    Event::MediaIngressStats(s) => {
                        let mut st = self.stats.lock().unwrap();
                        if let Some(rtt) = s.rtt {
                            st.rtt.push(rtt.as_secs_f64() * 1000.0);
                        }
                    }
                    Event::MediaEgressStats(s) => {
                        if let Some(rtt) = s.rtt {
                            self.stats.lock().unwrap().rtt.push(rtt.as_secs_f64() * 1000.0);
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    eprintln!("rtc: {e}");
                    self.alive.store(false, Ordering::Relaxed);
                    return;
                }
            }
        }
    }

    fn write(&self, rtp_time: MediaTime, data: &[u8]) {
        let mut c = self.conn.lock().unwrap();
        if !c.connected {
            return;
        }
        let Some(mid) = c.mid else { return };
        if c.pt.is_none() {
            c.pt = c.rtc.writer(mid).and_then(|w| {
                w.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt())
            });
        }
        let Some(pt) = c.pt else { return };
        if let Some(w) = c.rtc.writer(mid) {
            if let Err(e) = w.write(pt, Instant::now(), rtp_time, data.to_vec()) {
                eprintln!("write: {e}");
            }
        }
        // The network thread may be asleep in recv for a while; give str0m's
        // pacer a tick now so the packet leaves immediately.
        self.drain(&mut c);
        let _ = c.rtc.handle_input(Input::Timeout(Instant::now()));
        self.drain(&mut c);
    }

    /// Network thread: socket -> str0m.
    fn run(self: Arc<Self>) {
        let mut buf = vec![0u8; 2000];
        let local = self.socket.local_addr().unwrap();
        while self.alive.load(Ordering::Relaxed) {
            let wait = {
                let c = self.conn.lock().unwrap();
                c.next_timeout.saturating_duration_since(Instant::now())
            };
            self.socket.set_read_timeout(Some(wait.clamp(Duration::from_millis(1), Duration::from_millis(20)))).ok();
            let input = self.socket.recv_from(&mut buf);
            let mut c = self.conn.lock().unwrap();
            let res = match input {
                Ok((n, source)) => match buf[..n].try_into() {
                    Ok(contents) => c.rtc.handle_input(Input::Receive(
                        Instant::now(),
                        Receive { proto: Protocol::Udp, source, destination: local, contents },
                    )),
                    Err(_) => Ok(()), // e.g. a late STUN reply from our own binding request
                },
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    c.rtc.handle_input(Input::Timeout(Instant::now()))
                }
                Err(e) => {
                    eprintln!("socket: {e}");
                    break;
                }
            };
            if let Err(e) = res {
                eprintln!("rtc input: {e}");
            }
            self.drain(&mut c);
        }
    }
}

fn sdp_msg(kind: &str, sdp: String) -> Value {
    json!({ "sdp": { "type": kind, "sdp": sdp } })
}

// ------------------------------------------------------------------- main

fn main() {
    str0m::crypto::from_feature_flags().install_process_default();
    let a = args();
    let stun = net::stun_servers(&a.signal);
    let (in_tx, in_rx) = mpsc::channel();
    let out = net::connect(&a.signal, in_tx).unwrap_or_else(|e| {
        eprintln!("cannot reach {}: {e}", a.signal);
        std::process::exit(1)
    });
    out.send(json!({ "type": "join", "room": a.room, "id": a.name, "kind": "str0m" })).unwrap();
    eprintln!("{} joined {} via {} (stun: {stun:?})", a.name, a.room, a.signal);

    let peers: Arc<Mutex<HashMap<String, Arc<Peer>>>> = Default::default();
    let started = Instant::now();

    // Microphone and speaker; the bench (pulses) needs neither.
    let mic: audio::Ring = Default::default();
    let speaker: audio::Ring = Default::default();
    let _streams = if a.pulses {
        vec![]
    } else {
        let mut s = vec![];
        match audio::start_input(a.input.as_deref(), mic.clone()) {
            Ok(st) => s.push(st),
            Err(e) => eprintln!("microphone: {e}"),
        }
        match audio::start_output(a.output.as_deref(), speaker.clone()) {
            Ok(st) => s.push(st),
            Err(e) => eprintln!("speaker: {e}"),
        }
        s
    };

    let apm = apm::Apm::new().map(Arc::new);
    eprintln!("audio processing (AEC/NS/AGC): {}", if apm.is_some() { "on" } else { "off (build with --features apm)" });
    spawn_sender(peers.clone(), a.pulses, mic, apm.clone());
    spawn_playout(peers.clone(), speaker, a.pulses, apm);

    let cpu_start: Mutex<Option<(f64, f64)>> = Mutex::new(None);
    let deadline = a.duration.map(|d| started + Duration::from_secs(d));
    let mut kinds: HashMap<String, String> = HashMap::new();
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        if cpu_start.lock().unwrap().is_none() && started.elapsed() >= Duration::from_secs(a.warmup) {
            *cpu_start.lock().unwrap() = cpu_now().map(|c| (c, now_ms()));
        }
        let Ok(msg) = in_rx.recv_timeout(Duration::from_millis(200)) else { continue };
        match msg["type"].as_str() {
            Some("peers") => {
                // The newcomer offers to everybody already in the room.
                for p in msg["peers"].as_array().into_iter().flatten() {
                    let id = p["id"].as_str().unwrap_or_default().to_string();
                    let kind = p["kind"].as_str().unwrap_or("?").to_string();
                    let Ok(peer) = Peer::new(&kind, &stun) else { continue };
                    let offer = {
                        let mut c = peer.conn.lock().unwrap();
                        let mut api = c.rtc.sdp_api();
                        let mid = api.add_media(MediaKind::Audio, Direction::SendRecv, None, None, None);
                        let (offer, pending) = api.apply().expect("an offer");
                        c.mid = Some(mid);
                        c.pending = Some(pending);
                        peer.drain(&mut c);
                        offer.to_sdp_string()
                    };
                    out.send(json!({ "type": "signal", "to": id, "data": sdp_msg("offer", offer) })).ok();
                    start_peer(&peers, id, peer);
                }
            }
            Some("joined") => {
                kinds.insert(msg["id"].as_str().unwrap_or_default().into(), msg["kind"].as_str().unwrap_or("?").into());
            }
            Some("signal") => {
                let from = msg["from"].as_str().unwrap_or_default().to_string();
                let sdp = &msg["data"]["sdp"];
                let text = sdp["sdp"].as_str().unwrap_or_default();
                match sdp["type"].as_str() {
                    Some("offer") => {
                        let Ok(offer) = SdpOffer::from_sdp_string(text) else { continue };
                        let kind = kinds.get(&from).cloned().unwrap_or_else(|| "?".into());
                        let Ok(peer) = Peer::new(&kind, &stun) else { continue };
                        let answer = {
                            let mut c = peer.conn.lock().unwrap();
                            let answer = c.rtc.sdp_api().accept_offer(offer);
                            peer.drain(&mut c);
                            answer
                        };
                        match answer {
                            Ok(ans) => {
                                out.send(json!({ "type": "signal", "to": from, "data": sdp_msg("answer", ans.to_sdp_string()) })).ok();
                                start_peer(&peers, from, peer);
                            }
                            Err(e) => eprintln!("offer from {from}: {e}"),
                        }
                    }
                    Some("answer") => {
                        let Some(peer) = peers.lock().unwrap().get(&from).cloned() else { continue };
                        let Ok(answer) = SdpAnswer::from_sdp_string(text) else { continue };
                        let mut c = peer.conn.lock().unwrap();
                        if let Some(pending) = c.pending.take() {
                            if let Err(e) = c.rtc.sdp_api().accept_answer(pending, answer) {
                                eprintln!("answer from {from}: {e}");
                            }
                            peer.drain(&mut c);
                        }
                    }
                    _ => {}
                }
            }
            Some("left") => {
                // Kept in the map so the report still has its numbers.
                if let Some(p) = peers.lock().unwrap().get(msg["id"].as_str().unwrap_or_default()) {
                    p.alive.store(false, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }

    let report = report(&a, &peers, *cpu_start.lock().unwrap(), started);
    if let Some(dir) = &a.results {
        let dir = format!("{dir}/{}", a.room);
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(format!("{dir}/{}.json", a.name), serde_json::to_string_pretty(&report).unwrap()).ok();
    } else {
        out.send(json!({ "type": "report", "data": report })).ok();
        std::thread::sleep(Duration::from_millis(300));
    }
    println!("{} cpu {}% {}", a.name, report["cpuPctOfOneCore"], report["peers"]);
}

fn start_peer(peers: &Arc<Mutex<HashMap<String, Arc<Peer>>>>, id: String, peer: Arc<Peer>) {
    peers.lock().unwrap().insert(id, peer.clone());
    std::thread::spawn(move || peer.run());
}

/// Sender clock: every 20 ms, one Opus frame to every connected peer.
fn spawn_sender(peers: Arc<Mutex<HashMap<String, Arc<Peer>>>>, pulses: bool, mic: audio::Ring, apm: Option<Arc<apm::Apm>>) {
    std::thread::spawn(move || {
        let mut enc = opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).expect("opus encoder");
        // Same settings Chromium uses by default for voice.
        enc.set_bitrate(opus::Bitrate::Bits(32_000)).ok();
        enc.set_inband_fec(true).ok();
        enc.set_packet_loss_perc(10).ok();
        let mut packet = vec![0u8; 1500];
        let start = Instant::now();
        let rtp_base: u64 = (now_ms() as u64 * 48) & 0xffff_ffff;
        let mut phase = 0f32;
        for k in 1u64.. {
            let frame_wall = now_ms();
            let mut pcm = [0f32; FRAME];
            if pulses {
                for (i, s) in pcm.iter_mut().enumerate() {
                    phase += 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0;
                    let ms = (frame_wall + i as f64 / 48.0) % 1000.0;
                    *s = if ms < 30.0 { 0.5 * phase.sin() } else { 0.0 };
                }
            } else {
                let mut r = mic.lock().unwrap();
                for s in pcm.iter_mut() {
                    *s = r.pop_front().unwrap_or(0.0);
                }
            }
            if let Some(apm) = &apm {
                apm.capture(&mut pcm);
            }
            if let Ok(n) = enc.encode_float(&pcm, &mut packet) {
                let time = MediaTime::new(rtp_base + (k - 1) * FRAME as u64, Frequency::FORTY_EIGHT_KHZ);
                let list: Vec<Arc<Peer>> = peers.lock().unwrap().values().cloned().collect();
                for p in list {
                    p.write(time, &packet[..n]);
                }
            }
            let next = start + Duration::from_millis(20 * k);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    });
}

/// Playout clock: every 20 ms, pull one frame per peer from its jitter
/// buffer, look for latency bursts, mix to the speaker.
fn spawn_playout(peers: Arc<Mutex<HashMap<String, Arc<Peer>>>>, speaker: audio::Ring, pulses: bool, apm: Option<Arc<apm::Apm>>) {
    std::thread::spawn(move || {
        let start = Instant::now() + Duration::from_millis(10);
        for k in 0u64.. {
            let next = start + Duration::from_millis(20 * k);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            let wall = now_ms();
            let mut mix = [0f32; FRAME];
            let list: Vec<Arc<Peer>> = peers.lock().unwrap().values().cloned().collect();
            for p in list {
                let frame = p.jb.lock().unwrap().pop();
                let mut st = p.stats.lock().unwrap();
                let jitter = p.jb.lock().unwrap().jitter_ms;
                if k % 50 == 0 {
                    st.jitter.push(jitter);
                }
                let Some(frame) = frame else { continue };
                for (i, &s) in frame.iter().enumerate() {
                    let v = s as f32 / 32768.0;
                    mix[i] += v;
                    st.level = st.level.max(v.abs());
                    if v.abs() > 0.1 {
                        if st.quiet > 48_000 * 3 / 10 {
                            let ms = (wall + i as f64 / 48.0) % 1000.0;
                            if ms < 900.0 {
                                st.latency.push(ms);
                            }
                        }
                        st.quiet = 0;
                    } else {
                        st.quiet += 1;
                    }
                }
            }
            if let Some(apm) = &apm {
                apm.render(&mix);
            }
            if !pulses {
                let mut r = speaker.lock().unwrap();
                r.extend(mix.iter().map(|v| v.clamp(-1.0, 1.0)));
                while r.len() > 48_000 / 5 {
                    r.pop_front();
                }
            }
        }
    });
}

// ----------------------------------------------------------------- report

/// CPU seconds used so far: the container's cgroup when there is one (one
/// participant per container in the bench), else this process.
fn cpu_now() -> Option<f64> {
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat") {
        let us: f64 = s.lines().find_map(|l| l.strip_prefix("usage_usec "))?.trim().parse().ok()?;
        return Some(us / 1e6);
    }
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let f: Vec<&str> = stat.rsplit(')').next()?.split_whitespace().collect();
    Some((f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?) / 100.0)
}

fn stat(xs: &[f64]) -> Value {
    if xs.is_empty() {
        return Value::Null;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |f: f64| (s[((f * s.len() as f64) as usize).min(s.len() - 1)] * 10.0).round() / 10.0;
    json!({ "n": s.len(), "median": q(0.5), "p95": q(0.95), "min": q(0.0), "max": q(1.0) })
}

fn report(a: &Args, peers: &Arc<Mutex<HashMap<String, Arc<Peer>>>>, cpu: Option<(f64, f64)>, started: Instant) -> Value {
    let mut out = serde_json::Map::new();
    for (id, p) in peers.lock().unwrap().iter() {
        let st = p.stats.lock().unwrap();
        let jb = p.jb.lock().unwrap();
        let lat: Vec<f64> = st.latency.iter().skip(2).copied().collect();
        out.insert(
            id.clone(),
            json!({
                "kind": p.kind,
                "latencyMs": stat(&lat),
                "jitterMs": stat(&st.jitter),
                "rttMs": stat(&st.rtt),
                "lossPct": jb.loss_pct(),
                "jitterBufferMs": jb.avg_delay_ms(),
                "concealedPct": (jb.played > 0).then(|| 100.0 * jb.concealed as f64 / jb.played as f64),
            }),
        );
    }
    let cpu_pct = match (cpu, cpu_now()) {
        (Some((c0, t0)), Some(c1)) => Some(((c1 - c0) / ((now_ms() - t0) / 1000.0) * 1000.0).round() / 10.0),
        _ => None,
    };
    let mem = std::fs::read_to_string("/sys/fs/cgroup/memory.current").ok().and_then(|m| m.trim().parse::<f64>().ok());
    json!({
        "id": a.name,
        "variant": "str0m",
        "room": a.room,
        // Processing reshapes the test tone, so latency is not read from those runs.
        "src": match (a.pulses, cfg!(feature = "apm")) { (true, false) => "pulses", (true, true) => "pulses+apm", _ => "mic" },
        "processing": { "aec": cfg!(feature = "apm"), "ns": cfg!(feature = "apm"), "agc": cfg!(feature = "apm") },
        "seconds": started.elapsed().as_secs_f64().round(),
        "netem": std::env::var("NETEM").unwrap_or_else(|_| "none".into()),
        "cpuPctOfOneCore": cpu_pct,
        "memMiB": mem.map(|m| (m / 1048576.0).round()),
        "peers": out,
    })
}

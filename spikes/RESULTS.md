# Spike results: experiment 4 (voice) and 6 (footprint) – Tauri or Electron

Decision 7 in docs/spec.md: Tauri (provisional) or Electron (fallback),
decided by **voice quality on Linux** and **memory use**.

## Recommendation: Electron for the desktop shell

Switch the provisional choice to Electron, and keep the str0m voice engine
as a later option (it can run inside Electron too).

1. **Tauri cannot do voice in its webview on Linux.** WebKitGTK, which Tauri
   uses there, ships without WebRTC in every mainstream distribution we
   checked (no `RTCPeerConnection` at all, see below). A Tauri app therefore
   needs its own native voice engine from day one: WebRTC (str0m), jitter
   buffer, loss concealment, echo cancellation, TURN, device handling, and
   later video and screen sharing. Electron has all of this, mature, on
   Windows, macOS and Linux.
2. **Memory is not a Tauri win.** With the same UI and mock core, idle
   memory is about equal (Windows: Tauri 110–129 MiB, Electron 102 MiB;
   Linux: Tauri ~400 MiB PSS, Electron ~332 MiB). With a 10k-message
   channel scrolled, Tauri is lower on Windows (219–264 against 307 MiB) and
   equal on Linux (475–505 against 468–481 MiB).
3. **Where Tauri clearly wins is download and disk size:** the Windows
   installer is 1.4 MiB against 115 MiB, installed size 3.6 MiB against
   368 MiB. On Linux, counting the libraries each package pulls in, they
   are equal (446 against 421 MiB).
4. **The native engine is much cheaper on CPU** (≈8% against ≈32% of a core
   per participant in a 6-person call on this Windows PC). That is worth
   having for gamers, but it does not need Tauri: the engine is a Rust
   library that can sit in the Rust core under Electron (napi-rs or a
   sidecar process). With Electron we can ship voice in phase 2 on
   Chromium's WebRTC and move to the native engine later if CPU matters.
   With Tauri, that engine becomes a phase 2 dependency.

**What would flip this:** if we decide to build the native engine anyway
(for CPU, or one engine on every platform including mobile), Tauri becomes
the better shell. The cost is roughly: a NetEQ-class jitter buffer (ours
conceals 14% of frames under 10 ms jitter, Chromium 1.9%), a TURN client
(str0m has none), echo cancellation tuned on real devices, device hot-plug
and Bluetooth switching, and later native screen capture and video
encoding.

### Risks of Electron, and what to do about them

| Risk | Mitigation |
| --- | --- |
| 115 MiB installer, 368 MiB installed; every update is large | Delta / differential updates in the updater (spike 7); updates only between calls (already in the spec) |
| Chromium security fixes: Electron must be updated about every 8 weeks | Make the Electron bump a routine release task; CI check on the Electron version age |
| The Rust core must be reached from Node | napi-rs native module (MIT) or the core as a sidecar process over IPC; `CoreApi` in `ui/src/core/api.ts` stays the boundary |
| Mobile (phase 3) needs another shell | `ui/` is a plain web app: Tauri 2 mobile or Capacitor can host the same UI; Android WebView and iOS WKWebView do have WebRTC |
| CPU per voice participant (~32% of a core on this PC, 6-person mesh) | Native engine inside the core later; it interoperates with Chromium peers already (mixed rooms below) |
| Licences | Electron is MIT; Chromium is BSD plus third-party; `ffmpeg.dll` is LGPL, dynamically linked and unmodified, which the policy allows |
| The electron-builder `.deb` misses `libasound2t64` on Debian 13 (the app does not start without it) | Add it to `linux.debian.depends` when the real package is made |

## What was measured and what was simulated

| Measured on real hardware (this Windows 11 PC, Ryzen 7 3700X) | Simulated / synthetic |
| --- | --- |
| Footprint of both shells: installer, installed size, start time, memory | Linux: Docker (WSL2 kernel) with Xvfb, software rendering, no GPU |
| A 6-person call: 3 Electron + 3 native peers over loopback, CPU per participant, latency without network delay | Network conditions: `tc netem` inside Docker containers |
| The native peer listing this PC's WASAPI microphones and speakers | Audio: a synthetic 1 kHz tone (or Chromium's fake microphone), never a real microphone or speaker |
| | Echo cancellation and noise suppression: only CPU cost measured; how well they cancel echo needs people (spikes/voice/TESTING.md) |

Other caveats:

- Latency is from the sender's audio graph to the receiver's decoded audio
  (encoder, network, jitter buffer, decoder). Sound-card buffers on both
  ends are not included; add roughly 10–40 ms on real devices.
- Both sides read the same clock (same Docker host or same PC), so one-way
  latency needs no clock sync. On Chromium the tone is timed in an
  AudioWorklet, accurate to about ±10 ms.
- Other sessions were using the same Docker host during the runs. CPU is
  container CPU time, so it holds up under contention, but small differences
  (±1–2 points) are noise. An earlier run under heavier load is not reported
  (its Chromium audio clock slipped; the method now re-anchors every second).
- macOS was not tested (no Mac here). WKWebView on macOS does have WebRTC,
  so Tauri on macOS could use the webview, but that would be a second voice
  stack next to the native one Linux needs.

## Experiment 4 – voice

### Prototype

- `voice/web/` – the browser client: full mesh, one `RTCPeerConnection` per
  peer, Opus, device selection (`enumerateDevices` + `setSinkId`), echo
  cancellation / noise suppression / gain control switches, live stats.
- `voice/electron/` – that client in Electron 44.7.0 (Chromium WebRTC).
- `voice/str0m-peer/` – the Tauri-side engine: str0m 0.24.1 (pure-Rust
  crypto), libopus, cpal 0.16 for devices, our own jitter buffer with Opus
  FEC and loss concealment, and WebRTC's AudioProcessing module behind
  `--features apm`. It joins the same rooms as the browser client.
- `voice/signal/` – a small WebSocket signalling server (relays SDP only).

A mesh of 6 means each participant sends 5 copies of its audio
(5 × ~32 kbit/s Opus ≈ 160 kbit/s up) and keeps 5 connections. That is fine
for voice. Selective forwarding (the spec's "elected SFU", phase 4) is
needed when rooms grow past ~6–8, or for video and screen sharing, where
5 upstream copies of a 1–3 Mbit/s stream do not fit a home uplink.

### Docker bench, 6 participants per room (Linux, netem on every container)

`one-way latency` = median over all 30 links of each link's median; `p95` =
worst link's 95th percentile.

| Room | netem | Direction | Latency median ms | p95 ms | Jitter ms | Loss % | Jitter buffer ms | Concealed % |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| c6-clean | none | Chromium → Chromium | 77 | 118 | 1.0 | 0.0 | 36 | 0.0 |
| c6-lossy | 50 ms, 2% loss | Chromium → Chromium | 120 | 156 | 1.0 | 2.1 | 31 | 1.9 |
| c6-jitter | 50 ± 10 ms, 2% loss | Chromium → Chromium | 154 | 194 | 10.0 | 2.0 | 62 | 1.9 |
| s6-clean | none | str0m → str0m | 18 | 37 | 0.1 | 0.0 | 20 | 0.2 |
| s6-lossy | 50 ms, 2% loss | str0m → str0m | 68 | 76 | 0.0 | 2.0 | 20 | 2.1 |
| s6-jitter | 50 ± 10 ms, 2% loss | str0m → str0m | 70 | 117 | 11.3 | 1.9 | 24 | **14.1** |
| mix-lossy (3+3) | 50 ms, 2% loss | Chromium → Chromium | 126 | 142 | 1.0 | 2.0 | 37 | 1.9 |
| mix-lossy | | Chromium → str0m | 119 | 136 | 0.4 | 0.0* | 45 | 2.2 |
| mix-lossy | | str0m → Chromium | 78 | 101 | 0.0 | 2.1 | 15 | 1.4 |
| mix-lossy | | str0m → str0m | 68 | 91 | 0.1 | 2.1 | 20 | 2.3 |

\* Not understood yet: the str0m receiver counted no missing sequence
numbers on these links although 2.2% of frames were concealed. Either
lost packets were retransmitted or the counter is wrong; the concealment
figure is the one to trust.

Reading it: everything is far below the spec's 200 ms target. Chromium adds
about 25–50 ms on the sending side (compare str0m → Chromium with
Chromium → Chromium) (Web Audio → WebRTC capture path) and
keeps a larger, adaptive jitter buffer; its NetEQ keeps concealment at the
loss rate even with jitter. Our jitter buffer is smaller and faster but
conceals 14% of frames under jitter – that is the part a production native
engine would have to replace.

CPU and memory per participant (container cgroup, steady state, 6 per room):

| Room | Variant | Source | CPU % of one core (median) | Memory MiB |
| --- | --- | --- | --- | --- |
| c6-clean | Chromium (headless) | tone | 40.4 | 154 |
| c6-lossy | Chromium | tone | 40.9 | 159 |
| c6-mic-raw | Chromium | fake mic, processing off | 32.7 | 154 |
| c6-mic-apm | Chromium | fake mic, AEC + NS + AGC on | 36.5 | 156 |
| s6-clean | str0m | tone | 5.6 | 4 |
| s6-lossy | str0m | tone | 4.3 | 5 |
| s6-apm | str0m + WebRTC APM | tone, AEC + NS + AGC on | 6.5 | 5 |

The Chromium figure is a whole headless browser (renderer, audio service,
Web Audio meters); the str0m figure is the engine alone, without any UI.
Echo cancellation + noise suppression + gain control cost about 2–4 points
in both.

### This Windows PC, 3 Electron + 3 native peers, loopback

| Direction | Latency median ms | p95 ms | Concealed % |
| --- | --- | --- | --- |
| Electron → Electron | 69 | 82 | 0.0 |
| Electron → str0m | 44 | 49 | 0.3 |
| str0m → Electron | 44 | 55 | 0.0 |
| str0m → str0m | 17 | 22 | 0.5 |

CPU per participant (whole process tree): Electron 32.0 / 32.2 / 32.7%,
native str0m 8.3 / 7.8 / 8.5% of one core.

### Echo cancellation, noise suppression and device selection

| | Electron (Chromium) | Tauri webview on Linux (WebKitGTK) | Tauri webview on Windows (WebView2) | Native engine (str0m + cpal) |
| --- | --- | --- | --- | --- |
| WebRTC (`RTCPeerConnection`) | yes | **no** | yes (Chromium) | yes (str0m) |
| Echo cancellation | yes (AEC3) | constraint only, nothing to send it over | yes | yes, with `--features apm` (same AEC3 code, BSD-3) |
| Noise suppression / gain control | yes | **no** | yes | yes, with `--features apm` |
| Choose microphone | yes (`deviceId`) | yes (`getUserMedia`) | yes | yes (cpal) |
| Choose speaker | yes (`setSinkId`) | **no** (`setSinkId` missing) | yes | yes (cpal) |
| TURN (relay when direct fails) | yes | – | yes | **no**: str0m has no TURN client |
| Linux audio | PulseAudio / PipeWire natively | – | – | ALSA only in cpal 0.16; PulseAudio / PipeWire through their ALSA plugins |

WebKitGTK 4.1 (the API Tauri 2 uses), checked by running the client in
Debian 13 and by looking for the JS WebRTC bindings in each distribution's
library:

| Distribution | WebKitGTK | `RTCPeerConnection` |
| --- | --- | --- |
| Debian 13 | 2.54.0 | no (also at runtime, with `enable-webrtc` set) |
| Ubuntu 24.04 | 2.52.6 | no |
| Fedora 43 | 2.54.2 | no |
| Arch | 2.54.2 | no |

str0m notes: by default it holds 15 audio packets (300 ms) to deliver them
in order, which stalls playout after every loss; the spike sets
`set_reordering_size_audio(0)` and reorders in its own jitter buffer.

## Experiment 6 – footprint

The same `ui/` build (mock core, `?demo=1`, 100k-message demo channel) in
each shell. `spikes/shell/bench.js` is injected identically into both: it
marks *rendered* when the first message rows are on screen, waits 15 s
(idle sample), then scrolls back through 10,000 messages and marks
*scrolled*. Start time = process launch to *rendered*.

Versions: Tauri 2.12.5 (wry 0.57.1, tauri-cli 2.12.4), Windows build
cross-compiled with mingw-w64; Electron 44.7.0, electron-builder 26.17.0.
WebView2 runtime 156 on this PC.

### Windows 11 (real hardware), per-user NSIS install

| | Tauri | Electron |
| --- | --- | --- |
| Installer | **1.4 MiB** | 114.8 MiB |
| Installed size | **3.6 MiB** (+ WebView2, part of Windows 11) | 368.2 MiB |
| Start, first launch after install | 1.8 s / 0.9 s (two installs) | 12.9 s (Defender scanning the fresh install) |
| Start, later launches | 0.58–0.87 s | **0.37–0.40 s** |
| Memory idle (private working set, all processes) | 110–129 MiB | **102 MiB** |
| Memory after 10k messages | **219–264 MiB** | 307 MiB |

### Linux (Debian 13 in Docker, Xvfb, software rendering), .deb install

| | Tauri | Electron |
| --- | --- | --- |
| `.deb` download | **1.6 MB** | 99 MB |
| AppImage | 107 MB (bundles WebKitGTK) | 125 MB |
| Installed, own files | **3.7 MiB** | 283 MiB |
| Installed on a clean system, with every library it pulls in | 446 MiB | 421 MiB |
| Start (5 launches, two series) | 0.83–2.73 s | **0.36–1.22 s** |
| Memory idle (PSS, all processes) | 400 / 403 MiB | **331 / 333 MiB** |
| Memory after 10k messages | 475 / 505 MiB | 468 / 481 MiB |

Most of Tauri's Linux memory is the WebKit web process (267 MiB PSS idle).
On a desktop with a GPU both numbers will be lower.

## Reproduce

Everything builds in Docker; only the Windows measurements run on the host.
Run from the repository root unless stated.

Build the UI once (own volume, so it does not touch ui/'s dev volume):

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/ui":/app -v shell-ui-node-modules:/app/node_modules -w /app node:22.23.4-bookworm sh -c "npm ci && npm run build"
```

Voice bench (~25 minutes, writes `spikes/voice/bench/results/` and `summary.md`):

```bash
spikes/voice/bench/matrix.sh
```

One room on its own: `spikes/voice/bench/run.sh <room> <chromium> <str0m> ["netem rule"] [seconds]`,
e.g. `spikes/voice/bench/run.sh mix 3 3 "delay 50ms loss 2%"`. `STR0M=str0m-apm`
uses the build with audio processing.

WebKitGTK check (Debian 13; `BASE=ubuntu:24.04` for Ubuntu):

```bash
MSYS_NO_PATHCONV=1 docker build -f spikes/voice/bench/Dockerfile.webkit --build-arg BASE=debian:trixie -t voicebench-webkit:trixie spikes/voice
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm voicebench-webkit:trixie sh -c 'grep -c -a JSRTCPeerConnection /usr/lib/*/libwebkit2gtk-4.1.so.0'
```

Tauri packages (Linux .deb/AppImage and Windows NSIS; copies them to `spikes/shell/out/`):

```bash
MSYS_NO_PATHCONV=1 docker build -t shell-tauri-builder spikes/shell/tauri
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD":/repo -v shell-tauri-target:/repo/spikes/shell/tauri/target -w /repo/spikes/shell/tauri shell-tauri-builder sh -c 'convert -size 1024x1024 xc:"#5865f2" /tmp/i.png && cargo tauri icon /tmp/i.png -o icons && APPIMAGE_EXTRACT_AND_RUN=1 cargo tauri build -b deb,appimage && cargo tauri build --target x86_64-pc-windows-gnu -b nsis && mkdir -p ../out/linux ../out/windows && cp target/release/bundle/deb/*.deb ../out/linux/ && cp target/x86_64-pc-windows-gnu/release/bundle/nsis/*.exe ../out/windows/'
```

Electron packages (`electronuserland/builder:wine`, digest `sha256:f3a8d451994c881d0972e7b1846ee4d4fdc15eb41fc6b1f350b38cbf99ca90fe`):

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD":/repo -v shell-electron-node-modules:/repo/spikes/shell/electron/node_modules -w /repo/spikes/shell/electron electronuserland/builder:wine sh -c 'npm install && npx electron-builder --linux deb --win nsis && cp dist/*.deb ../out/linux/ && cp dist/*.exe ../out/windows/'
```

Linux footprint (one clean image per package):

```bash
cd spikes/shell && MSYS_NO_PATHCONV=1 docker build -f Dockerfile.measure --build-arg PKG=ShellSpikeTauri_0.1.0_amd64.deb -t shell-measure-tauri . && docker run --rm shell-measure-tauri
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm --security-opt seccomp=unconfined --security-opt apparmor=unconfined --shm-size 512m shell-measure-tauri measure-linux.sh shell-spike-tauri tauri 5
```

The same for Electron with `--build-arg PKG=shell-spike-electron_0.1.0_amd64.deb --build-arg EXTRA=libasound2t64`
and the command `/opt/ShellSpikeElectron/shell-spike-electron`. The seccomp /
AppArmor options let the apps' own sandboxes (bubblewrap, Chromium's) work
inside the container; they are not switched off.

Windows footprint (installs per user, measures, uninstalls):

```powershell
powershell -ExecutionPolicy Bypass -File spikes\shell\measure-windows.ps1 -Installer "spikes\shell\out\windows\ShellSpikeTauri_0.1.0_x64-setup.exe" -Exe shell-spike-tauri.exe
```

```powershell
powershell -ExecutionPolicy Bypass -File spikes\shell\measure-windows.ps1 -Installer "spikes\shell\out\windows\ShellSpikeElectron Setup 0.1.0.exe" -Exe ShellSpikeElectron.exe
```

Windows voice run: build `str0m-peer.exe` (mingw, in `shell-tauri-builder`
with `cargo build --release --target x86_64-pc-windows-gnu`) and the
Electron voice app (`npx electron-builder --win dir` in `spikes/voice/electron`),
start the signal server with
`docker run -d -p 127.0.0.1:8080:8080 -e RESULTS_DIR=/results -v <dir>:/results voicebench-signal`, then:

```powershell
powershell -ExecutionPolicy Bypass -File spikes\voice\bench\run-windows.ps1 -Electron <VoiceSpike.exe> -Str0m <str0m-peer.exe> -Results <dir>
```

Licences: `cargo deny check licenses` with `core/deny.toml` passes for
`spikes/voice/str0m-peer` (all features) and `spikes/shell/tauri`.

A 6-person test with real people, microphones and networks: spikes/voice/TESTING.md (Hebrew).

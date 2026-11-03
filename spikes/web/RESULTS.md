# Spike experiment 5: a browser peer on the libp2p network: results

**Verdict: it works.** A browser peer built from rust-libp2p 0.54 compiled to
WebAssembly joins the same network as the native peers. Headless Chromium,
Firefox and WebKit all passed these checks:

- connect to the relay over WebSocket;
- reach an **unmodified core/sync node** through the relay circuit;
- reach a native peer **directly over WebRTC** (webrtc-direct);
- exchange blobs both ways in core/sync's wire format, ending with identical
  state hashes on both sides.

Every connect attempt succeeded (10/10 per kind, 3 runs × 2 browsers + 1
WebKit run).

Two things must change before production. Both are covered below:

1. **The relay needs settings and three code changes** (specified below for
   devops; the harness uses a patched copy in `relay/`). Without
   `EXTERNAL_ADDRS`, circuits fail silently. libp2p's default circuit limits
   (128 KiB / 2 min per circuit, 16 circuits in total) cut off relayed browser
   sessions. With Nagle's algorithm on, every relayed message waited about
   40 ms per hop (turning it off took the relayed sync round trip from 90 ms
   to 3 ms).
2. **WebRTC in libp2p 0.54 is alpha and has a real bug.** A browser cannot
   send a blob of ~8 KB or more to a native peer over WebRTC (the native side
   fails to decode it). The first request a native peer opens on a fresh
   WebRTC connection also hangs for 10 s. Small messages, and everything from
   native to browser, work. This needs a libp2p upgrade before WebRTC carries
   real traffic; until then, the relay path is the reliable one.

Measured on 2026-11-03 on the owner's PC (Docker Desktop, Windows 11),
against core/sync and infra/relay as they are on main today (the hardened
sync with recomputed ids, size limits and pull rate limits). All peers share
one Docker bridge network: no NAT, no WAN latency (see "What this does not
show").

Reproduce: [README.md](README.md). Per-run tables: [results/runs.md](results/runs.md).

## What was built

| Piece | Where | What |
|---|---|---|
| Browser peer | `browser/` | rust-libp2p 0.54 → wasm32 via wasm-bindgen. Transports: WebSocket (`websocket-websys`), relay circuit client, webrtc-direct (`libp2p-webrtc-websys`). Behaviours: identify, ping, gossipsub, request-response (CBOR), same as core/sync. Blobs live in memory. |
| Wire crate | `wire/` | core/sync's current wire format (`/mvs/sync/0.1.0`: blob = dep count, sorted deps, data; id recomputed from log, deps and data; same size limits; content-based gossipsub message ids), `SyncRequest`/`SyncResponse`, and an in-memory log with the same pull rules (have-markers, missing-since with byte-capped pages, state hash). A test checks it against `sync::Blob` and `sync::Store`: same ids, same bytes, same markers, same pages. |
| Native WebRTC peer | `native/` | `web-native`: core/sync's SQLite `Store` and protocols plus a webrtc-direct listener (`libp2p-webrtc` 0.8.0-alpha). core/sync's `Node` has no WebRTC, and core/ was off-limits for this spike. |
| Harness | `harness/` | Docker build, a docker-compose network (relay built from `relay/`, a patched copy of `infra/relay`, core/sync's own `sync-peer`, `web-native`, Playwright 1.56.1) and the test script. |

The browser joins as follows. It dials the relay over WebSocket, then dials
`<relay>/p2p-circuit/p2p/<native>`. For the direct path it learns the native
peer's `/udp/…/webrtc-direct/certhash/…` address through identify on the
relayed connection, dials it, then closes the relayed connection. A test also
dials that address with no relay at all. Blobs flow as in core/sync: pull on
connect (CBOR request-response) and gossipsub push on `log/<name>`.

## Numbers

Raised relay limits, relay with Nagle off. Each cell is the p50 range across
3 runs; p95 values are in `results/runs.md`. WebKit is from one run. All
times are in ms.

| | Chromium 141 | Firefox 142 | WebKit 26 |
|---|---|---|---|
| wasm instantiate (page already loaded) | 41–74 | 97–318 | 61 |
| **Connect** | | | |
| browser → relay, WebSocket | 8–16 | 20–27 | 6 |
| browser → native, relay circuit (incl. WebSocket) | 13–22 | 56–152 | 16–17 |
| browser → native, webrtc-direct | 135–136 | 226–236 | 112 |
| **Success rate**, every kind | 30/30 each | 30/30 each | 10/10 each |
| **Blob delivery, write → stored** (50 each way) | | | |
| relayed, native → browser / browser → native | 1.2–3.2 / 0–1.5 | 3.8–5.7 / 1.3–3.8 | 1.1 / 2.0 |
| direct, native → browser / browser → native | 1.3–3.4 / 0–1.8 | 4.3–6.4 / 0.8–3.5 | 1.2 / 1.9 |
| **Round trip** | | | |
| sync request → empty page, relayed | 2.7–3.9 | 5–8 | 4 |
| sync request → empty page, direct (WebRTC) | 4.0–5.0 | 6–7 | 4 |
| libp2p ping, relayed | 1.2–2.1 | 2–4 | 2 |
| **Joining peer pulls full history** | | | |
| from core/sync node, relayed | 29–80 (280 blobs) | 256–367 (520 blobs) | 33 (280 blobs) |
| from web-native, WebRTC only | 59–114 (280 blobs) | 191–222 (520 blobs) | 84 (280 blobs) |
| 320 KB pushed over one circuit | 27 | 27–30 | 26 |
| State hash identical after exchange | yes | yes | yes |

**WASM bundle** (`web_peer_bg.wasm`, release, `opt-level = "z"`, LTO,
`wasm-opt -Oz`):

| | Raw | gzip -9 | brotli -11 |
|---|---|---|---|
| wasm | 1,729 KB | 628 KB | **480 KB** |
| JS glue | 33 KB | 6 KB | |

Notes on reading the table:

- "0" latencies happen because the browser and the containers share one
  clock to the millisecond; the sync round trip is the clock-free measure.
- Firefox's ranges are wider because its first run had a slow start (wasm
  init 318 ms, circuit connect 152 ms; later runs 97–103 and 56–67 ms).
  WebKit was as fast as Chromium on the relayed path and fastest on
  WebRTC.
- webrtc-direct connect is slower than relayed in every browser (ICE + DTLS +
  Noise ≈ 3 round trips plus certificate work). That is not a problem for a
  long-lived connection, but it does not make a cheap "first contact" path.

## What we fixed along the way (and what it bought)

| Problem | Effect | Fix | Before → after (Chromium p50) |
|---|---|---|---|
| `libp2p-websocket-websys` 0.4 completes a flush only when a 100 ms interval timer sees the send buffer empty | every handshake step waits for a tick: **~400 ms** per WebSocket connect, in every browser | `EagerFlush` wrapper in `browser/src/lib.rs` (flush returns at once; writes still respect the buffer limit) | 401 → 137 ms |
| Relay TCP and WebSocket sockets had Nagle's algorithm on (libp2p-tcp's default) | small relayed messages wait for delayed ACKs, about 40 ms per hop | `nodelay(true)` for TCP and WebSocket in the relay (spec R2, R3 below) | WebSocket connect 137–146 → 8–16 ms; circuit connect 151–208 → 13–22 ms; relayed sync round trip 88–97 → 2.7–3.9 ms |

The first fix belongs upstream (or comes with a libp2p upgrade). The second
also speeds up native peers that are relayed through the relay today.

## Direct versus relayed

- **In this test both always worked** (10/10 per attempt kind, every run,
  every browser). That is expected on a flat Docker network. It does not
  predict real-world rates.
- **What webrtc-direct needs:** the native peer must be reachable on a UDP
  port from the browser. That means a public IP or port forward, or the same
  LAN. The browser cannot hole-punch with webrtc-direct. rust-libp2p 0.54 has
  no private-to-private WebRTC (`/webrtc`, signalled over the relay), which is
  what browser ↔ NAT'd-PC would need; js-libp2p has it. So **for typical home
  users behind NAT, a browser will talk to them through the relay.** Direct
  WebRTC will be the exception (peers with UPnP/port forwarding, and the
  owner's server).
- The relay therefore carries most web traffic. Size it for that (next
  section).

## Bugs and limits found

1. **Relay without `EXTERNAL_ADDRS` → silent circuit failure.** The relay
   logs `ReservationReqAccepted`. The client then drops the reservation
   because it contains no addresses, and denies every incoming circuit with
   `NO_RESERVATION`. Dialers see `UNEXPECTED_MESSAGE`. Production sets
   `EXTERNAL_ADDRS` in `infra/.env`; keep it set, and alert if it is empty.
2. **Default circuit limits cut browser sessions.** libp2p's defaults are
   16 circuits in total, 4 per peer, and 2 min or 128 KiB per circuit.
   Measured with defaults: a 320 KB push over a circuit was cut after 9 of
   40 blobs. A joining Firefox peer's history pull was cut mid-transfer. An
   early harness version, which left test connections open, hit the
   16-circuit cap after 16 connections. Spec R1 below makes the limits configurable.
3. **WebRTC, browser → native, ≥ ~8 KB fails** (libp2p-webrtc 0.8.0-alpha on
   the native side, libp2p-webrtc-websys 0.4.0-alpha in the browser). The
   native peer pulls from a browser that is connected only over WebRTC.
   - Blobs of 100 B, 1 KB and 4 KB arrive.
   - With 8 KB blobs, every pull fails with `IO error on outbound stream:
     Failed to encode/decode message`. Nothing arrives, in all three
     browsers.
   - native → browser works at any size (a 520-blob history including
     320 KB of bulk blobs).
   - The native log also shows `webrtc_sctp … ErrChunk` warnings. So the
     likely culprit is message fragmentation between the browser's SCTP and
     webrtc-rs 0.9. Not confirmed.
   - The harness keeps this case as a reported known issue.
4. **WebRTC, first native-initiated request hangs** (seen in Chromium). On a
   fresh webrtc-direct connection, the native peer's first sync request to the browser times out
   after 10 s. The next one, 5 s later, works. Pulls on connect are therefore
   10–15 s late when WebRTC is the only connection.
5. **Firefox + page served from `localhost`:** libp2p-webrtc-websys refuses to
   dial, because Firefox blocks WebRTC to loopback. This only affects local
   development; the harness serves the page from a non-localhost name.
6. **Build gotchas for the WASM peer** (all pinned and commented in
   `browser/Cargo.toml`):
   - libp2p 0.54's matching `libp2p-webrtc-websys` is `0.4.0-alpha`. It only
     compiles with `wasm-bindgen` ≤ 0.2.92, because newer web-sys removed an
     enum variant it matches on. That in turn needs `yamux` 0.13.3: newer
     versions pull `getrandom` 0.3, which needs a newer wasm-bindgen.
   - gossipsub 0.47 needs `instant/wasm-bindgen`, or the bundle imports a
     missing `env.now` and fails to load.
   - All of these go away with a libp2p upgrade. The updater session already
     flagged libp2p 0.54 for RustSec advisories, so the upgrade is due
     anyway.
7. **Stopped peers log `closure invoked recursively or after being dropped`**
   (JS callbacks of the websys transports firing after the swarm is dropped).
   Harmless here, but noisy in a real app's console.
8. **core/sync's wire types are private**, and the crate pulls SQLite and
   tokio, so it cannot build for wasm32. The spike therefore duplicates
   `Blob` and the sync messages in `wire/`, with a test that keeps them
   byte-identical. Suggestion for core/sync: move `Blob`, `SyncRequest`,
   `SyncResponse` and the protocol constants into a small dependency-free
   crate (or behind a `default-features = false` split). The web client can
   then share them for real.

## What the relay needs in production

- **wss via the reverse proxy.** infra/ now ships this: the `caddy` service
  (`--profile proxy`, `RELAY_DOMAIN`) terminates TLS with an automatic
  Let's Encrypt certificate and proxies the subdomain to the relay's port
  4002. What the browser side needs from it:
  - A page served over https may only open `wss://`: browsers block `ws://`
    from https pages as mixed content. So wss is required, not optional.
  - Advertise `/dns4/<RELAY_DOMAIN>/tcp/443/wss` in `EXTERNAL_ADDRS` (as
    `.env.example` does). The browser dials
    `/dns4/<RELAY_DOMAIN>/tcp/443/wss/p2p/<relay id>`, exactly the ws address
    used here with `wss` and the domain.
  - Caddy proxies WebSocket upgrades with no idle timeout by default, so
    nothing to set. With another proxy (nginx: `proxy_read_timeout` 60 s),
    keep its timeout above libp2p's ping interval (15 s).
  - Not tested end to end: there is no domain yet. Only plain `ws` was
    measured.
- **Ports.** 443/tcp (+ 80 for certificate issuance) on Caddy; 4001/tcp+udp
  as today for native peers. 4002 stays on localhost behind the proxy.
- **Resources.** The relay container is capped at 1 CPU / 256 MB. That was
  sized for bootstrap plus occasional circuits. Once it relays web clients'
  traffic, check CPU (Noise for every relayed byte) and memory per circuit
  under load.
- **`EXTERNAL_ADDRS` must be set** (bug 1).
- **Raise the circuit limits** (bug 2). As a starting point: `MAX_CIRCUITS`
  sized to concurrent web users (hundreds), `MAX_CIRCUITS_PER_PEER` around
  16–64, `MAX_CIRCUIT_SECS` of an hour or more, and `MAX_CIRCUIT_BYTES` in
  the hundreds of MB. Watch bandwidth: the relay now carries the web
  clients' traffic.
- **Nagle off** (spec R2, R3).
- **Optional, cert-free alternative:** the relay could also listen on
  webrtc-direct. Browsers then reach it with only the certificate hash in
  the address, with no domain or TLS certificate. Its certificate must
  persist across restarts, or the advertised address changes. Not done here
  (relay changes were kept minimal), and bugs 3–4 argue for waiting for a
  libp2p upgrade.

## Relay changes for devops (spec)

`infra/relay` is not changed by this spike. The harness builds `relay/`, a
copy of `infra/relay` (as on main, 2026-11-03) with the three changes below;
`relay/relay.patch` is the exact diff. All measurements above used it.

- **R1. Circuit limits from the environment.** Read four optional variables
  and apply them to `relay::Config`; unset or empty keeps libp2p's default,
  so behaviour doesn't change until `.env` sets them:

  | Variable | `relay::Config` field | libp2p default |
  |---|---|---|
  | `MAX_CIRCUITS` | `max_circuits` | 16 |
  | `MAX_CIRCUITS_PER_PEER` | `max_circuits_per_peer` | 4 |
  | `MAX_CIRCUIT_SECS` | `max_circuit_duration` (seconds) | 120 |
  | `MAX_CIRCUIT_BYTES` | `max_circuit_bytes` | 131072 |

  Pass them through `infra/docker-compose.yml` (`${VAR:-}`) and list them,
  empty, in `.env.example`. A non-numeric value should fail startup with the
  variable's name. Starting values for production are under "What the relay
  needs in production". Acceptance: `bash spikes/web/harness/run.sh` with
  the variables unset shows "bulk 320 KB over circuit: cut after N blobs";
  with `MAX_CIRCUIT_BYTES=1073741824` (and the others raised) it shows "ok".
- **R2. Nagle off on the TCP transport:** `tcp::Config::default().nodelay(true)`
  in `.with_tcp(...)`.
- **R3. Nagle off on the WebSocket transport.** `SwarmBuilder::with_websocket`
  has no TCP config, so build the same thing with `with_other_transport`
  before `with_dns`: `websocket::WsConfig::new(tcp::tokio::Transport::new(
  tcp::Config::default().nodelay(true)))`, upgraded with `Version::V1Lazy`,
  Noise and yamux. Listening addresses and the `/ws` port are unchanged.
  Acceptance for R2 + R3: in the harness, Chromium's `relay_ws` connect p50
  drops from ~140 ms to under 20 ms and the relayed sync round trip from
  ~90 ms to under 5 ms.

Ops item, no code: keep `EXTERNAL_ADDRS` non-empty (bug 1); an empty value
should probably log a warning at startup.

## Why rust-libp2p → WASM (and not js-libp2p)

The preferred option worked, so js-libp2p was not needed. The browser peer
uses the same libp2p version, protocols and wire code as core/. A web client
can share logic with the native client, at the cost of a ~480 KB (brotli)
bundle and the alpha WebRTC crates above. js-libp2p would add
private-to-private WebRTC (browser ↔ NAT'd peer without the relay), which
rust-libp2p lacks. That is the main reason to reconsider later.

## What this does not show

- **No NAT, no WAN.** One Docker bridge, sub-millisecond RTT. Real links add
  their RTT to every number above. Connect costs about 3–4 round trips
  relayed (WebSocket + Noise + yamux, then the circuit and a second Noise),
  and more for WebRTC (ICE + DTLS + Noise).
- **No wss through a real proxy or certificate**, no mobile browsers, no
  real Safari (WebKit via Playwright is close but not identical).
- **No long sessions.** `MAX_CIRCUIT_SECS` was raised to 1 h. A session
  longer than that is not tested, and the browser peer does not redial.
- **The browser peer has no anti-entropy timer** (core/sync pulls every
  5 s). It pulls on connect and when a pushed blob's deps are missing.
- **Licences:** cargo-deny with `core/deny.toml` passes for `spikes/web` and
  `relay/` (the patched relay; licenses, sources, bans).

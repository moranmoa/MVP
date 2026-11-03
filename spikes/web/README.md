# Spike 5: a browser peer on the libp2p network

Results and conclusions: [RESULTS.md](RESULTS.md).

| Folder | What |
|---|---|
| `wire/` | core/sync's wire format (blob encoding, sync request/response, an in-memory log with the same pull rules), buildable for wasm32 |
| `browser/` | the browser peer: rust-libp2p 0.54 compiled to wasm32 (WebSocket, relay circuit, webrtc-direct; gossipsub push + CBOR pull) |
| `native/` | `web-native`: a native peer with core/sync's SQLite store and protocols plus a webrtc-direct listener |
| `www/` | the page that loads the wasm bundle |
| `relay/` | copy of `infra/relay` with the relay changes specified in RESULTS.md (`relay.patch` is the diff); the harness builds it |
| `harness/` | Docker build, docker-compose (relay, core/sync node, web-native, Playwright) and the test script |

## Reproduce

Needs only Docker. From the repo root, in Git Bash (Windows) or any shell:

```bash
bash spikes/web/harness/build.sh
```

The measurements in RESULTS.md ran with raised relay circuit limits:

```bash
MAX_CIRCUITS=256 MAX_CIRCUITS_PER_PEER=64 MAX_CIRCUIT_SECS=3600 MAX_CIRCUIT_BYTES=1073741824 bash spikes/web/harness/run.sh
```

and once with libp2p's default limits (shows the circuit cut-off):

```bash
bash spikes/web/harness/run.sh
```

Tables from one or more runs (paths relative to `harness/`; copy each run's
`dist/results.json` aside first, since the next run overwrites it):

```bash
bash spikes/web/harness/summarize.sh dist/results.json
```

`build.sh` compiles `web-native`, core/sync's unmodified `sync-peer` and the
wasm bundle in a `rust:1.90-bookworm`-based image, runs the wire-format tests,
and writes `harness/dist/` (bundle sizes in `dist/www/pkg/sizes.json`).
`run.sh` starts its own relay (built from `relay/`, a patched copy of `infra/relay`; fresh key, own
Docker network; the `infra/` relay is not touched), the two native peers and
the Playwright container, which runs Chromium and Firefox headless. Results:
`harness/dist/results.json`; peer logs: `harness/dist/peers.log`. It exits
non-zero when a check fails (the one known WebRTC issue in RESULTS.md is
reported but not counted).

Options (environment variables for `run.sh`): `BROWSERS=chromium` (or
`firefox`, `webkit`), `ATTEMPTS=10` (connects per kind), `MESSAGES=50` (blobs
each way), `BULK_BLOBS=40` and `BLOB_BYTES=8192` (bulk tests), and the relay
limits `MAX_CIRCUITS`, `MAX_CIRCUITS_PER_PEER`, `MAX_CIRCUIT_SECS`,
`MAX_CIRCUIT_BYTES`.

To try the page by hand, serve `harness/dist/www` on localhost and, in the
console:

```js
const p = new WebPeer('general');
await p.dial('/ip4/127.0.0.1/tcp/4002/ws/p2p/<relay peer id>');
```

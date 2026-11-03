# Spike experiment 2: log sync — results

**Verdict: all four success criteria pass, with large margins, on a local
Docker network.** The next test should add real WAN latency and NAT (see
"What this does not show yet").

Measured on 2026-11-03 on the owner's PC (Docker Desktop, Windows 11): 4 peer
containers plus the existing bootstrap/relay container on the `infra_default`
network. Run it again with `bash core/sync/harness/run.sh`.

## Summary (4 runs)

| Criterion | Target | Run 1 | Run 2 | Run 3 | Run 4 | Pass |
|---|---|---|---|---|---|---|
| 1,000 concurrent messages (3 peers) → identical state hash | identical | ✅ 251 ms | ✅ 275 ms | ✅ 237 ms | ✅ 231 ms | ✅ |
| Catch-up of 10,000 missed messages after restart | < 10 s | 339 ms | 341 ms | 343 ms | 360 ms | ✅ |
| p95 delivery latency, live messages | < 1 s | 1.0 ms | 0.9 ms | 1.4 ms | 1.0 ms | ✅ |
| Join → first messages | < 10 s | 50 ms | 60 ms | 92 ms | 47 ms | ✅ |

Other numbers from the same runs:

- p95 latency during the 1,000-message burst (everyone writing at full
  speed): 106–148 ms.
- Join → full history (11,600 messages on an empty store): 343–418 ms.
- Discovery: a peer finds and connects to the others 20–110 ms after start.

## How it works

- **Store** (`src/store.rs`): SQLite (bundled). A blob is accepted only when
  all of its deps are already stored. Because of that rule, insertion order is
  a topological order of the DAG, and having a blob implies having all of its
  ancestors. Heads are kept in a table as blobs are added. Blobs are opaque:
  only `id`, `deps` and `data` are needed, so `core/records` can plug in later.
- **Pull** (`src/node.rs`): the puller sends `have` markers for the log. Those
  are its heads plus the blobs at distances 1, 2, 4, 8, … back in its local
  order, as git does in fetch negotiation. The other side takes the ancestor
  closure of the markers it knows (one recursive CTE) and returns every blob
  of the log outside that closure, in insertion order, in pages of 2,000.
  - With heads only, a peer that has blobs the other side has never seen
    would get the whole log back. With the markers, the extra data is at most
    about the size of the real gap.
  - Pulls run on connect, every 5 s (anti-entropy), and when a pushed blob
    arrives before its deps.
- **Push**: every new blob is published on the log's gossipsub topic. If its
  deps are missing, it waits in memory and the peer pulls from the sender.
- **Discovery**: dial the relay, then run a Kademlia `get_closest_peers` query
  against it (the relay runs Kademlia in server mode) and dial the peers it
  returns. Each peer also listens on a relay circuit as a fallback for NAT. In
  these runs every peer connection was direct TCP.

## Limits on peer input (added after the spike)

Everything a peer sends is untrusted. The node enforces these limits:

| What | Limit |
|---|---|
| Blob id | Never read from the wire. It is recomputed as BLAKE3 over the log name, deps and data, and checked again on every `append`. |
| Deps | Strictly ascending (one valid encoding per blob), and all in the same log as the blob. |
| Blob size | At most 64 deps and 32 KiB of data; larger blobs are rejected before allocation. |
| Log names | 1–128 printable ASCII characters. Peers' gossip subscriptions are tracked only for `log/<valid name>`, at most 1,024 topics per peer. |
| `have` list in a pull | Only the first 64 ids are used. |
| Pull response page | At most 2,000 blobs or 4 MiB, under the CBOR codec's 10 MiB cap. |
| Inbound pulls | Per peer: a burst of 30, then 10 per second; buckets survive reconnects (4,096 peers remembered). All peers together: a burst of 100, then 50 per second. |
| "More" in a response | Followed only if the page added something new. |
| Gossip messages | About 38 KiB max per frame (one maximal blob plus 4 KiB of framing). Invalid blobs are rejected and not forwarded. Messages are deduplicated by content. |
| Pushed blobs waiting for deps | At most 10,000 blobs or 16 MiB, expiring after 60 s; copies of one blob are kept once. Re-parking a blob counts against the same cap. |
| Events to the app | A 65,536-event buffer; when the app falls behind, new events are dropped (the blobs are still stored). |
| Connections | 256 incoming, 64 pending incoming, 128 outgoing, 384 in total, 2 per peer. |

What these limits do not cover yet: who may write. Until `core/records` is wired in, any peer can put well-formed blobs into any log it knows the name of. Signatures and roles are checked there, before `append`.

## Full report of run 4

### Discovery

All 3 peers were connected to each other 69 ms after the last one was
started. Start → first peer, per node: p1 76 ms, p2 27 ms, p3 67 ms.

### 1. Concurrent writes

1,000 messages were written at full speed across 3 peers (334/333/333). All
3 peers held the same state 231 ms after the writes began. The state hash,
`7ed9614cff0f6dc5`, was identical on all 3.

### 2. Delivery latency

This is the time from `append` on the writer until the blob is stored on
another peer. All peers share the host clock.

| Load | Deliveries | push / pull | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|---|
| live: 3 × 200 msgs, 20 msg/s per peer | 1200 | 1200 / 0 | 0.6 | 1.0 | 15.2 | 26.9 |
| burst: the 1,000 concurrent writes above | 2000 | 1228 / 772 | 50.0 | 123.7 | 135.7 | 166.9 |

In the burst, about 40% of deliveries arrived by pull. Some pushed blobs
reached a peer before their deps did, which triggered a pull. The pull won the
race for those blobs, and for others it reached first.

### 3. Offline peer catches up

p3 was stopped: store closed, all connections dropped. The other two wrote
10,000 messages, and both had all of them after 2,745 ms. Then p3 was
restarted and caught up:

| Measure | Time |
|---|---|
| restart → first peer connected | 52 ms |
| restart → first missed message stored | 88 ms |
| restart → last missed message stored | 360 ms |
| restart → harness saw identical state hash on all 3 | 360 ms |

### 4. New peer joins

p4 started with an empty store while the log held 11,600 messages.

| Measure | Time |
|---|---|
| join → first peer connected | 17 ms |
| join → first messages stored | 47 ms |
| join → full history stored | 343 ms |
| join → harness saw identical state hash on all 4 | 406 ms |

The final state hash, `03fdc0808fce1c10`, was identical on all 4 peers.

## What this does not show yet

- **No WAN, no NAT.** All containers share one Docker bridge: round trips are
  under 1 ms and connections are direct. Real home and mobile links add tens
  to hundreds of ms per round trip.
  - Catch-up of 10,000 messages takes 5 pages, so about 6 round trips plus
    transfer. It should stay well under 10 s on a normal link.
  - When the relay circuit is the only path, its default per-circuit limits
    (2 minutes, 128 KiB per circuit in libp2p's defaults) would cut a large
    catch-up short. That needs a test with spike 1's relayed pairs.
- **"Restart" is an in-process restart.** `stop` drops the node completely:
  swarm, connections and SQLite handle. `start` goes through the same path as
  a fresh process: open the store, load the key, dial the relay, discover
  peers, pull. The process itself and the container stay up.
- **One log.** The code handles any number of logs (one gossipsub topic and
  one pull per log, opened on demand), but the harness uses one channel log.
- **No signatures or validation.** Blobs are opaque here. Rejecting invalid
  records is `core/records`' job, and should happen before `append`.
- **Same clock for latency.** Fine on one host; a WAN test needs
  round-trip-based measurement.

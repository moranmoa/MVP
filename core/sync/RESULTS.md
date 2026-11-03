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

This describes the spike, where records were opaque blobs. Since B4-sync the unit is a signed `records::Record`; see "Ingest checks" below. The sync mechanics are unchanged.

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

## Ingest checks (B4-sync)

Every record from a peer passes these checks, in order, before it is stored, forwarded on gossip (`Accept`) or served to anyone:

1. `Record::decode`: size, canonical form, signature. The record id is computed from the signed bytes.
2. The log is derived from the record (`<server>/control` or `<server>/<channel>`), never from the gossip topic or the pull request. A record pushed on another log's topic is rejected; a pulled page may only contain the requested log.
3. `deps` are stored in the same log, and `auth_deps` in the same server's control log. Missing ones park the record and trigger a pull (of the control log, for `auth_deps`). Ones found in the wrong log reject it.
4. For **channel** records, the `Authorizer` (supplied by the integration layer) is a **storage gate that must be monotone in the stored control log**: whatever it accepts with control set S, it accepts with any superset (e.g. "the author is the owner or has a stored `Join`"). It sees the server's stored control log and runs inside the store's write transaction, only once everything the record relies on is present. Kicks, revokes, voids and the GAP-2 checks are *not* used here: they belong to display, forwarding to the app and keys, and never delete anything. A refused channel record is remembered (bounded) and listed in later pulls' `have`, so peers stop re-sending it; the list for a server is cleared whenever one of its control records is stored, since the gate may now open.

   **Control records are never refused at ingest.** Authority is a function of the whole set of well-formed control records, and whether one counts can change when a concurrent record arrives. Refusing them would make the stored set depend on arrival order, so every one that decodes and has its deps is stored and forwarded, and authority decides what it means. Likewise, voiding (e.g. by a later kick) is decided when displaying, never by deleting from the store.

Gossip outcomes: stored → `Accept`; duplicate, still waiting for deps, or refused by the gate → `Ignore` (a peer with more of the control log may rightly accept it, so the sender isn't blamed); undecodable, misfiled or cross-log → `Reject`.

### Harness with signed records (2 runs, 2026-11-03)

| Measure | Run 1 | Run 2 |
|---|---|---|
| 1,000 concurrent messages → identical state | 810 ms | 482 ms |
| Live p95 delivery latency | 10.3 ms | 2.4 ms |
| Catch-up of 10,000 missed messages | 1,208 ms | 1,356 ms |
| Join → first messages | 263 ms | 314 ms |
| Join → full history (11,600 messages) | 1,423 ms | 2,026 ms |

Every target still passes. Catch-up takes about 1 s longer than with unsigned blobs, which is the cost of checking 10,000 Ed25519 signatures. That work runs on blocking threads, not on the network task.

**Discovery.** The relay now publishes only global addresses (relay PR #30), so on the Docker network the peers would only meet through relay circuits, which are capped at 128 KiB and 2 minutes. The node therefore:
- dials `bootstrap` peers directly (the harness passes `p1..p4`);
- replaces a relayed connection with a direct one as soon as one exists;
- upgrades from what identify reports. Non-global addresses are dialled only with `allow_lan`, which is on in the harness and off for real users.

## Limits on peer input (added after the spike)

Everything a peer sends is untrusted. The node enforces these limits:

| What | Limit |
|---|---|
| Record id | Never read from the wire: `Record::decode` computes it from the signed bytes and checks the signature. |
| Deps | Strictly ascending (enforced by `records`), `deps` in the same log, `auth_deps` in the server's control log. |
| Record size | `records::MAX_RECORD_BYTES` (about 19 KiB), checked before parsing. |
| Log names | 1–160 printable ASCII characters. Peers' gossip subscriptions are tracked only for `log/<valid name>`, at most 1,024 topics per peer. |
| `have` list in a pull | Only the first 64 ids are used. |
| Pull response page | At most 2,000 records or 4 MiB, under the CBOR codec's 10 MiB cap. |
| Inbound pulls | Per peer: a burst of 30, then 10 per second; buckets survive reconnects (4,096 peers remembered). All peers together: a burst of 100, then 50 per second. Answered on a blocking thread with a read-only store connection, at most 8 at once, so a large log never stalls the network task. |
| Served pull bytes | Per peer: a 16 MiB burst, then 2 MiB/s. All peers together: a 32 MiB burst, then 8 MiB/s. A page is cut to the remaining budget; under 64 KiB the pull is dropped. |
| "More" in a response | Followed only if the page added something new. |
| Gossip messages | One maximal record plus 4 KiB of framing. Only records that passed the ingest checks are forwarded. Messages are deduplicated by content. |
| Pushed records waiting for deps | At most 10,000 records or 16 MiB, expiring after 60 s; copies of one record are kept once. Re-parking a record counts against the same cap. |
| Events to the app | A 65,536-event buffer; when the app falls behind, new events are dropped (the records are still stored) and a `Lagged { dropped }` event follows, telling the app to re-read the open logs from the store. |
| Connections | 256 incoming, 64 pending incoming, 128 outgoing, 384 in total, 2 per peer, 8 direct incoming per IP address. Idle connections close after 5 minutes. |
| Node key | `node.key` is created owner-only (Unix 0600; Windows a protected single-entry ACL for the current user, as for the updater's root keys) and never overwritten; an existing key is tightened to 0600 on Unix. |

Who may write is the `Authorizer`'s call. The harness uses `AcceptAll`, which lets anyone write; the integration layer (T1) supplies the real one. Serving only members (GAP-4) is not done yet: today anyone who knows a log's name can pull it.

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
- **No signatures or validation in the spike runs.** The 4 spike runs used opaque blobs. Since B4-sync every record is signed and checked at ingest (see "Ingest checks").
- **Same clock for latency.** Fine on one host; a WAN test needs
  round-trip-based measurement.

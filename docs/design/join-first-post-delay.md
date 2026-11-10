# Design note: the ~9.8 s first post after joining

Status: draft for the Tech Lead and the security lead. Authors: dev-identity (api) and dev-sync (sync). No code yet.

## Symptom
In the v0.2.5 smoke (and on 0.2.2), the joiner's first post waited about 9.8 s in 4 of 10 joins. The others were fast.

Until the joiner holds the channel key, its send fails closed: `seal` answers `not-allowed`, with `notice: rekey-waiting`. The post goes out only once the owner's `KeyRotation` record, which includes the joiner, is stored on the joiner's device.

## Where the time goes
A join is followed by these steps, each with a worst case:

| # | Step | Code | Worst case |
| --- | --- | --- | --- |
| 1 | The owner's node stores the `Join`, by gossip or by an anti-entropy pull | `core/sync/src/node.rs` `ANTI_ENTROPY` = 5 s | 0–5 s |
| 2 | The owner's core loop reads it and runs `rekey` | `core/api/src/service.rs` `run_sync_loop`, `SYNC_EVERY` = 2 s, woken early by a hint | 0–2 s |
| 3 | The join rotation is due, unless one ran in the last 12–18 s (GAP-8 batching) | `service.rs` `rekey`, `JOIN_REKEY_MIN_MS`, `rekey_after` | 0 s, or up to 18 s inside a batch window |
| 4 | `rotate_channel` signs the rotation and publishes it on the channel's gossip topic | `service.rs` `rotate_channel`; `node.rs` `append_local` | ms |
| 5 | The joiner receives it by gossip, or else on its next anti-entropy pull | `node.rs` `ANTI_ENTROPY` | 0–5 s |

- Likely cause, to be confirmed by the measurement below: gossip in step 5 misses when the joiner isn't yet in the owner's view of that channel topic. The joiner subscribes to channel topics only after its own join is accepted and `open_server` runs, which races the owner's rotation.
- `append_local` treats "no subscribed peers" as fine, because they'll pull. So the record waits for the joiner's next 5 s pull.
- The Tech Lead's smoke log shows the rotation arriving on a pull. That fits steps 1 + 2 + 5 with a missed gossip: about 5 + 2 + 3 ≈ 10 s.
- Step 3 can add more when joins come close together, as they did in the 10-join smoke.

## Proposed fix
The goal: a new member gets the key without waiting for any 5 s pull.

1. **sync: eager pull while waiting for a key** (dev-sync).
   - Add a port `Network::want(server)` / `unwant(server)`, which makes the node pull that server's logs from every connected peer each `ACK_PULL` (1 s).
   - Cap it at 30 s per call, so it ends even if nobody rotates.
   - It reuses the pull path and its rate limits (`PULL_RATE` 10/s per peer), so it adds no new protocol message.
2. **sync: pull on subscribe** (dev-sync).
   - When gossipsub reports that a connected peer subscribed to a topic of an open log, pull that log from that peer at once.
   - This closes the race in step 5 in both directions, and also helps the owner's step 1.
3. **api: ask for it** (dev-identity).
   - After my join is accepted, and whenever `rekey` raises `rekey-waiting` for a channel without a key, call `want(server)`.
   - Call `unwant` once every known channel encrypts (the probe in `rekey`).
   - A stored record already wakes the core loop by a hint, so the key applies within ms.
4. **api: no extra wait from batching** (dev-identity).
   - Keep the GAP-8 batch, which the security lead's DoS bound relies on.
   - But when a join is deferred, wake exactly when its window ends, instead of on the next idle 2 s round.
   - The first join after a quiet period still rotates at once, as it does today.

Expected result: step 5 drops from 0–5 s to ≤ 1 s, and step 1 improves too. A join outside a batch window should post in about 1–3 s.

## Before coding: measure
Add debug logs (`RUST_LOG=api=debug,sync=debug`, from #204) with millisecond timestamps for each of these:
- `Join` stored at the owner;
- `rekey` run, and whether it was deferred;
- the rotation published;
- the rotation stored at the joiner.

Rerun the 10-join smoke on a throwaway relay. If step 3 turns out to dominate, piece 4 comes first.

## Files
- **sync:**
  - `core/sync/src/node.rs`: the `want` and `unwant` commands, a timer driving `ACK_PULL`, and pull on gossipsub `Subscribed`, which `node.rs` doesn't handle today.
  - `core/sync/src/lib.rs`: the API.
  - Tests in `core/sync/tests/`: a rotation published before the peer subscribes arrives within about 1 s.
- **api:**
  - `core/api/src/ports.rs`: `Network::want` and `unwant`, which default to doing nothing.
  - `core/api/src/syncnet.rs`.
  - `core/api/src/service.rs`: the join path, `rekey`, and a deadline-aware `run_sync_loop` timeout.
  - `core/api/tests/flows.rs`: a joiner calls `want` and then `unwant` once keyed.
- **QA:** the 10-join smoke with a first-post p95 under 3 s as the pass bar.

## Not changing
- Keys still travel only inside signed `KeyRotation` records.
- The gossip and pull checks (`Authorizer`, sealing) stay as they are.
- No new wire message, so there's no compatibility concern: old peers simply answer the extra pulls.

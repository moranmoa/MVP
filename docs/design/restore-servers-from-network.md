# Restoring servers from the network

- **Status:** proposal, by dev-identity (2026-11-10), for review by the Tech Lead and Security Lead.
- **Owner direction:** "the whole principle is that there's no server but a network of peers, so this should be saved in the distributed network", and "it doesn't have to keep copies everywhere in the network, but it must be stable enough to always work".
- **Goal:** after a restore from the 12 words on a clean computer, with all my other devices gone, my servers come back from the network by themselves.
  - No central server.
  - Nobody else learns which servers I'm in.

## 0. Today, and the interim

- **Today:** a restore rebuilds only the identity. The server list lives in the local `core.db`.
- **Fix 2 (#290):** rejoining by an invite link as a member I already am:
  - signs no new join;
  - posts no empty name;
  - posts no age claim without a birth date;
  - isn't age-blocked when the birth date is unknown.
  - `restoreIdentity(words, input?)` also takes the country and birth date that the UI asks again.
- **Fix 1 (dev-ui):** the restore flow asks the agreement, country and birth date before the words, and the name after. It then says that servers come back by invite link until this design ships.
- **Owner workaround once fix 2 is merged:** on the main PC, copy the server's invite link. Open it on the laptop while the main PC is online.
  - The laptop returns as the owner: same member, history readable, name and age claim untouched.
  - Calls from one machine at a time.

## 1. The personal backup ("roster")

**Keys** come from the identity seed, which the 12 words give, with BLAKE3 `derive_key`, in `core/identity`:

| Value | Derivation | Use |
|---|---|---|
| `k` | `derive_key("mvs/roster-key/v1", seed)` | XChaCha20-Poly1305 key for the roster |
| `w` | Ed25519 from `derive_key("mvs/roster-sign/v1", seed)` | signs every roster version |
| `tag` | `blake3("mvs/roster-tag/v1" ‖ w.public)` | where the roster lives |

- **Unlinkable:** the tag can't be computed from my public identity key. Nobody can look up a named person's roster, or link a tag to an identity, from the tag alone.
- **Can't be overwritten by others:** a holder accepts a blob for `tag` only if it's signed by the key whose hash is `tag`.

**The blob:** `{v: 1, tag, w.public, version: u64, ts}`, then the ciphertext, then a signature by `w`.
- **Plaintext:**
  - `servers: [{ server_id, sync_secret, state: member | left | closed, changed_at }]`;
  - `profile: { name, country, birth_date, locale }`.
- **Padding:** to 4 KiB steps, at most 64 KiB, so the size doesn't reveal the number of servers.

**GAP-4:** the sync secrets sit only inside the ciphertext, and only the seed opens it.

## 2. Where it lives: k = 5 copies, not everywhere

| # | Holder | Count | Kept for | Notes |
|---|---|---|---|---|
| 1 | each of my own devices | 1 per device | while installed | it is the writer |
| 2 | relay / bootstrap mailbox | 1 per bootstrap (1 today) | 365 days since the last refresh | opaque store keyed by `tag`, ≤ 64 KiB, rate-limited |
| 3 | DHT record under `tag` | the K closest DHT servers | republished every 24 h | today the DHT servers are the bootstraps (clients are in client mode, GAP-6), so this grows with GAP-9 |
| 4 | "roster buddies": online co-members, picked at random across all my servers | 3 | 180 days since the last refresh | each node holds at most 8 others' blobs (64 KiB each), dropping the least recently refreshed; it announces itself as a DHT *provider* of `tag` (no blob in the DHT) |

**Target:** at least 3 reachable holders besides my own devices.

**The GAP-4 objection, answered.** I said earlier that members can't hold the backup, because a clean device can't reach a server's members without that server's secret. That's true for *server* traffic. The roster uses a separate protocol, `/mvs/roster/1`, keyed only by `tag`, outside any server.
- **Finding holders:** a restoring device finds them through the relay mailbox, the DHT record, and DHT provider records for `tag`. It dials them over relay circuits like any peer.
- **No secret needed:** a restore needs no server secret; the secrets come out of the blob.
- **Who may ask a node to hold a blob:** only a bound member of a server we share. That stops spam.
- **Who may fetch:** anyone who presents `tag`, rate-limited. Only I can compute the tag.

## 3. What each party learns

- **Relay and DHT nodes:** that peer P stores, or is asked about, an opaque blob of a given size step under `tag`. Not who I am, and not which servers.
- **A buddy:** that a co-member it already knows (it shares a server with me) keeps a blob with it. It can link `tag` to my identity, but it can't read the server list or its size beyond the padding step.
- **Timing:** a write follows a join or leave. Writes are delayed by a random 30 s to 10 min, and every login republishes anyway, which blurs the timing.
- **The server list:** readable only with the seed.

## 4. Writes, refresh and offline holders

- **Writes:** on create server, join, leave, close, being kicked, and a profile or birth-date change.
  - `version = max(seen) + 1`, merged first (see §6).
  - Then sent to every holder.
- **Republish:** on every login, and every 24 h while online.
  - The writer counts the holders that answered with the latest version.
  - If fewer than 3 answered, it picks new buddies (replacing any not seen for 7 days) and pushes.
  - So copies heal whenever any of my devices is online.
- **Holders:** keep only the highest valid `(version, hash)` per tag, and drop a blob after its retention runs out without a refresh.
- **Rollback:** a stale holder can serve an old version. A restore asks every reachable holder and takes the highest valid version.
  - An old version can't put me back into a server I left, was kicked from, or that was closed. A restore re-adds a server only if *its own log* still lists me as a member (§5).

## 5. Restore

1. The UI runs the agreement, country, birth date and words, then calls `restoreIdentity(words, input)`.
2. The core derives `tag`, `k` and `w`, and asks the holders: relay mailbox, DHT record, DHT providers.
   - It runs in the background, retrying for up to 24 h.
   - It emits a new `restore.progress { found, servers }` event.
3. It verifies, decrypts, and merges with anything already local.
4. For each entry in `member` state, it opens the server with its secret and fetches.
   - It adds the server locally **without posting** (the fix 2 path) if the log lists me as a member who isn't kicked and the server isn't closed.
   - Otherwise it marks the entry `left` or `closed`.
5. The profile and birth date fill the empty profile. If no blob was found, the UI's answers stand.
6. This device becomes a writer and republishes.

## 6. Two devices with one identity

- **Roster:** merged, not replaced.
  - Entries are keyed by `server_id`, last-writer-wins on `changed_at`; the profile is last-writer-wins too.
  - A device that sees a version with entries it lacks merges and republishes it as `version + 1`. Concurrent writes converge after one round.
- **Records:** both devices sign with the same key. That's valid: `seq` decides neither validity nor order (a min-ID topological sort).
- **Leave or close on one device:** the other sees it in the server log, drops the server, and marks the entry.
- **Calls: one device per call.** Sync routes voice to one peer per identity.
  - Follow-up: `joinCall` refuses while another device of mine is in the room.
- **Re-keys:** both devices may rotate on the same join. Both rotations are valid. Open question: check that groupkeys converges.
- **Per-device state, not synced:** relay-only, send states, unread, mute.
- **Real linked devices** (device keys signed by the identity, revocation) stay in phase 3 (spec).

## 7. Availability

- **When a restore works:** if any one holder with the latest version is reachable.
- **Alpha, one bootstrap:**
  - Every path goes through that relay, so availability is about the relay's: about 99% on the owner's machine.
  - Buddies protect against the relay *losing its store*, not against the relay being down.
- **After GAP-9, two or more bootstraps:** the DHT record and provider records live on each, and buddies are reachable through either.
  - With 2 bootstraps at 99% and 3 buddies, a restore succeeds unless every bootstrap is down, or every holder has dropped or lost the blob. Target: ≥ 99.9%.
- **Retention:** 365 days on bootstraps and 180 days on buddies. A user can be away for 6 months with no device and still restore from a buddy, or for a year from a bootstrap.

## 8. Pieces and files

| Piece | Owner | Files |
|---|---|---|
| Roster keys from the seed | dev-identity | `core/identity/src/lib.rs` (+ tests) |
| Roster format, padding, merge, encrypt and sign; write hooks; republish; restore flow and "re-add without posting"; `restore.progress` | dev-identity | new `core/api/src/roster.rs`, `service.rs`, `types.rs`, `ports.rs` (`Network::put_roster`, `fetch_roster`), `CONFORMANCE.md` |
| `/mvs/roster/1` store and fetch, holder store with quotas and expiry, DHT record and provider records, store only from bound co-members, fetch rate limit | dev-sync | `core/sync/src/node.rs`, new `core/sync/src/roster.rs`, `store.rs` |
| Bootstrap mailbox and DHT server role, retention, rate limits | devops | `infra/relay/**` |
| Restore steps, progress and copy | dev-ui | `ui/src/screens/onboarding/*`, `i18n` |
| Tests | each owner, then QA | api `flows.rs`: restore with every device gone, rollback, a left server not re-entered, two-device merge. Sync `tests/relay.rs`: fetch via DHT providers through a relay |

## 9. Questions for the reviewers

1. The tag, key and signing-key derivations, and whether the tag should rotate (e.g. per year) to limit how long a buddy can link it.
2. k, the retention periods, and the buddies' per-node quota.
3. Whether a bootstrap should also be a DHT *server* (today it is the only one).
4. Whether to include the profile and birth date in the roster: the spec's "encrypted identity backup".

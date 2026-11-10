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

## 1. The personal backup

**Keys** come from the BIP-39 seed, which the 12 words give. The seed carries 128 bits of entropy, so no slow KDF is needed. Each value uses BLAKE3 `derive_key` under its own frozen `mvs/` context, in `core/identity` (Security Lead):

| Value | Derivation | Use |
|---|---|---|
| `k_enc` | `derive_key("mvs/backup-key/v1", seed)` | XChaCha20-Poly1305, random 24-byte nonce, AAD = `"mvs/backup/v1" ‖ tag ‖ version` |
| `w` | Ed25519 from `derive_key("mvs/backup-write/v1", seed)` | a **separate write key**: it authorises every write |
| `tag` | `derive_key("mvs/backup-tag/v1", w_pub)` | where the backup lives; no rotation in v1 |

- **Unlinkable:** the tag isn't the identity key and isn't derived from it. Nobody can look up a named person's backup, or link a tag to an identity, from the tag alone.
- **No identity signature outside:** holders never see one. Writes are authorised by `w` only: a holder accepts a blob for `tag` only if it's signed by the `w` whose hash gives `tag`.
  - An identity signature visible to holders would link the tag to the identity key, and through it to every membership.
  - If devices want an identity signature for integrity between them, it goes *inside* the ciphertext. The AEAD already authenticates the content.
- **Writes don't use a bearer token** (Tech Lead's alternative). A buddy that learned a token could overwrite the backup at every other holder. A signature by `w` reveals nothing it could reuse.

**The blob:** `{v: 1, tag, w_pub, version: u64}`, then the ciphertext, then a signature by `w`.
- **Plaintext:**
  - `servers: [{ server_id, sync_secret, role_hint, joined_at, gone?: { left | kicked | closed, at } }]`;
  - `profile: { name, country, birth_date, locale }`;
  - `version`, repeated inside.
- **Kept out:** channel and group keys (re-fetched through rotations), messages, invite keys, the relay-only setting and the leave outbox.
- **Padding:** to 4 KiB steps, at most 64 KiB, so the size doesn't reveal the number of servers.
- **Rollback:** each device keeps the highest `version` it has seen and refuses older ones.
  - A *fresh* device can't detect a rolled-back blob. The harm is limited to missing recently joined servers, and §5 never re-enters a server I left.
  - This is accepted and documented (Security Lead).

**Sync secrets in the blob:** accepted by the Security Lead. Whoever holds the words can already sign as me, unwrap every channel key wrapped to me, and rejoin through any invite. The secrets only add finding my servers without an invite. They raise the impact of lost words from "identity taken" to "identity taken and every server immediately readable", and that's accepted because the words are the whole account.

## 2. Where it lives: k = 5 copies, not everywhere

| # | Holder | Count | Kept for | Notes |
|---|---|---|---|---|
| 1 | each of my own devices | 1 per device | while installed | it is the writer |
| 2 | relay / bootstrap mailbox (a `/mvs/backup/1` store) | 1 per bootstrap (1 today) | 13 months since the last write | quotas below. The relay is the only rendezvous a clean machine has (Tech Lead). devops, `infra/**`; the live relay only from main, with the manager's OK |
| 3 | DHT **provider records** for `tag` (never the blob: a 64 KiB blob can't be a kad record, Tech Lead) | the K closest DHT servers; the bootstrap is a DHT server (yes, Tech Lead and Security Lead) | re-announced every 12 h by each holder | they point to the stores that hold the blob; today the DHT servers are the bootstraps, so this grows with GAP-9 |
| 4 | **Phase B, after GAP-9:** "backup buddies", online co-members picked at random across all my servers | 3 | 180 days since the last refresh | each node holds at most 8 others' blobs (64 KiB each), dropping the least recently refreshed; **at most one tag per bound identity** (R1: `w` keys are free, so otherwise one co-member could evict everyone); each announces itself as a provider of `tag` |

**Phases (Tech Lead):**
- **Phase A:** my devices, the bootstrap mailbox, provider records, restore, and the merge.
- **Phase B, after GAP-9:** buddies.
- **Target:** 1 bootstrap in phase A; at least 3 reachable holders besides my own devices in phase B.

**Mailbox quotas, for devops** (constants; R2):
- **Per write:** validated *before* storing. The signature by `w`, `tag == derive_key("mvs/backup-tag/v1", w_pub)`, size ≤ 64 KiB and a multiple of 4 KiB, and a `version` above the stored one.
- **Per tag:** one blob, at most 12 writes an hour.
- **Per IP:** at most 30 writes an hour, 5 new tags a day, and 60 fetches an hour.
- **Total:** at most 50,000 tags (about 3.2 GB at the cap). When full, refuse new tags; never evict live ones.
- **Expiry:** 13 months after the last write.
- **Answers:** fetch by exact tag only, and a constant "not found" (R3).
- **Logs:** counts only; no tag-to-IP logs (R11).
- The DHT provider records get the same per-IP limits.

**The GAP-4 objection, answered.** I said earlier that members can't hold the backup, because a clean device can't reach a server's members without that server's secret. That's true for *server* traffic. The backup uses a separate protocol, `/mvs/backup/1`, keyed only by `tag`, outside any server.
- **Finding holders:** a restoring device finds them through the bootstrap mailbox and the DHT provider records for `tag`. It dials them **over relay circuits only** (R3), like any peer.
- **Untrusted answers (R4):** every answer is checked for `w`, the tag, the AEAD and the version before use.
- **No secret needed:** a restore needs no server secret; the secrets come out of the blob.
- **Who may ask a node to hold a blob:** only a bound member of a server we share. That stops spam.
- **Who may fetch:** anyone who presents `tag`, rate-limited. Only I can compute the tag.

## 3. What each party learns

- **Relay and DHT nodes:** that peer P stores, or is asked about, an opaque blob of a given size step under `tag`. Not who I am, and not which servers.
- **A buddy:** that a co-member it already knows (it shares a server with me) keeps a blob with it. It can link `tag` to my identity, but it can't read the server list or its size beyond the padding step.
- **Timing:** a write follows a join or leave. Writes are delayed by a random 30 s to 10 min, and every login republishes anyway, which blurs the timing.
- **The server list:** readable only with the seed.
- **Accepted risk (Security Lead):** the relay, and Cloudflare in front of it, see the tag, the size bucket, the update times and, at restore time, the fetching IP.

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
2. The core derives `tag`, `k` and `w`, and asks the holders: the bootstrap mailbox and the DHT providers.
   - It runs in the background, retrying for up to 24 h.
   - It emits a new `restore.progress { found, servers }` event.
3. It verifies, decrypts, and merges with anything already local.
4. For each entry with no tombstone, it opens the server with its secret and fetches.
   - It adds the server locally **without posting** (the fix 2 path) if the log lists me as a member who isn't kicked and the server isn't closed.
   - Otherwise it adds a tombstone: left, kicked or closed.
   - Tombstoned servers are never opened.
5. The profile and birth date fill the empty profile. If no blob was found, the UI's answers stand.
6. This device becomes a writer and republishes.

## 6. Two devices with one identity

- **Backup:** merged, not replaced, and **not last-writer-wins** (Security Lead). It's a set of servers with tombstones:
  - The union of both devices' entries, keyed by `server_id`, so neither device erases the other's joins.
  - A tombstone (left, kicked or closed) beats a membership with an equal or earlier `joined_at`.
  - A deliberate rejoin after a leave makes a new entry with a later `joined_at`. Closed and kicked tombstones are final.
  - On a kick, the next write drops the secret and keeps only the tombstone.
  - The profile is last-writer-wins on `changed_at`, in the local merge only (R5). Holders never compare contents: they keep the highest valid `version`.
  - A device that sees a version with entries it lacks merges and republishes it as `max(version) + 1`. Concurrent writes converge after one round.
- **Records:** both devices sign with the same key. That's valid: `seq` decides neither validity nor order (a min-ID topological sort).
- **Leave or close on one device:** the other sees it in the server log, drops the server, and marks the entry.
- **Calls: one device per call.** Sync routes voice to one peer per identity.
  - The core refuses `joinCall` while a hello from my own identity, from another peer, is in the room (Security Lead).
  - The 0.3.x release notes say "call from one machine only" until phase 3 (Tech Lead).
- **Re-keys:** both devices may rotate on the same join, and a restored device adds one extra re-key per channel because its `keyed_for` is empty. That's noise, not harm.
  - Both rotations are valid authority records wrapped to every current member. Messages sealed under either key stay readable, provided the cipher keeps every accepted rotation.
  - A test (in #290) checks it: one owner on two cores rotates concurrently, a third member reads messages sealed under both, and a later kick excludes the kicked member from both.
  - If they diverge, a restored device must not re-key before it has synced the control log, and it becomes a GAP-5 item.
- **Per-device state, not synced:** relay-only, send states, unread, mute.
- **Real linked devices** (device keys signed by the identity, revocation) stay in phase 3 (spec).

## 7. Availability

- **When a restore works:** if any one holder with the latest version is reachable.
- **Alpha, one bootstrap:**
  - Every path goes through that relay, so availability is about the relay's: about 99% on the owner's machine.
  - Buddies protect against the relay *losing its store*, not against the relay being down.
- **After GAP-9, two or more bootstraps:** provider records live on each bootstrap, and buddies (phase B) are reachable through either.
  - With 2 bootstraps at 99% and 3 buddies, a restore succeeds unless every bootstrap is down, or every holder has dropped or lost the blob. Target: ≥ 99.9%.
- **Retention:** 13 months on bootstraps and 180 days on buddies. A user can be away for 6 months with no device and still restore from a buddy, or for a year from a bootstrap.

## 8. Pieces and files

| Piece | Owner | Files |
|---|---|---|
| Backup keys from the seed | dev-identity | `core/identity/src/lib.rs` (+ tests) |
| Backup format, padding, merge, encrypt and sign; write hooks; republish; restore flow and "re-add without posting"; `restore.progress` | dev-identity | new `core/api/src/backup.rs`, `service.rs`, `types.rs`, `ports.rs` (`Network::put_backup`, `fetch_backup`), `CONFORMANCE.md` |
| `/mvs/backup/1` store and fetch, holder store with quotas and expiry, DHT provider records, store only from bound co-members (phase B), fetch rate limit | dev-sync | `core/sync/src/node.rs`, new `core/sync/src/backup.rs`, `store.rs` |
| Bootstrap mailbox and DHT server role, retention, rate limits | devops | `infra/relay/**` |
| Restore steps, progress and copy | dev-ui | `ui/src/screens/onboarding/*`, `i18n` |
| Tests | each owner, then QA | api `flows.rs`: restore with every device gone, rollback, a left server not re-entered, two-device merge. Sync `tests/relay.rs`: fetch via DHT providers through a relay |

## 9. Decisions (Tech Lead and Security Lead, on #291)

1. **Derivations:** as in §1, with the tag from `derive_key`. **No tag rotation in v1.**
2. **Numbers:** k = 5, the retention periods and the buddy quota are fine, as constants.
3. **DHT:** the bootstrap acts as a DHT server. The DHT carries provider records only.
4. **Contents:** the profile and birth date go in the backup. Sync secrets may too; channel keys, messages and invite keys stay out.
5. **Also decided:**
   - the relay mailbox is the home in phase A, and buddies come in phase B, after GAP-9;
   - a tombstone merge;
   - the `joinCall` refusal;
   - the concurrent re-key test (#290: it converges);
   - the Security Lead's conditions R1–R5, folded into §2 and §6.
6. **Owners:** the Security Lead reviews the protocol and derivations, and devops builds the infra side.

## 10. Future: email + password (owner roadmap, not now)

The owner wants users to be able to link an email and sign in with an email and password later. The design above stays compatible: the password opens the **seed**, and the seed opens the backup as before. Nothing in §1–§8 changes.

- **Lookup handle:** the email is never stored in clear.
  - `handle = derive_key("mvs/email-handle/v1", normalise(email))`, and the holders store under `H(handle)`.
  - A plain hash can be enumerated: anyone can test "does x@example.com have a backup" against a holder. Better: an **OPRF** (blinded hash) evaluated by the bootstraps, ideally split across them by threshold, so neither a holder nor an outsider can test emails offline. Holders also rate-limit lookups.
- **Password key:** `k_pw = Argon2id(password, salt = handle)` with a high cost (for example 256 MiB, t = 3).
  - The "password envelope" `AEAD(k_pw, seed)` is stored under the handle like a backup.
  - Writes are authorised by `w` (§1), so only the account can set or replace it.
  - Sign-in: email → handle → envelope → password → seed → the normal restore (§5).
- **Trade-offs:**
  - **Offline guessing:** anyone who can fetch the envelope (anyone who knows the email) can guess passwords offline. Argon2id only slows that down, and a weak password falls. The envelope opens the whole account.
  - **Mitigation:** make the password itself OPRF-hardened (OPAQUE-style), with the bootstraps as rate-limited online oracles. Guessing then needs the network, and it's throttled. That needs a running service (the bootstraps) but no account database.
  - The password stays optional, with a strength check. The 12 words remain the root.
  - **Email enumeration:** prevented by the OPRF plus rate limits, with a constant "not found".
- **Email verification:** sending mail needs a mail-sending service, which is the one central piece. Without verification, the email is just a lookup name the user picks:
  - Restoring still needs the password, so that's safe.
  - But someone could claim a handle for another person's email first, and the first `w` per handle wins.
  - A verification service run by the bootstraps (a signed one-time code by email) prevents that, at the cost of a service that learns emails.
  - **Password reset** without a custodian is impossible by design: the words are the only reset.

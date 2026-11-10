# Short invite links (T8-02)

Status: **design**. The Security Lead approved it with amendments S1–S6, folded in below (marked **[S1]**…**[S6]**). The Tech Lead's §8 decision is pending. No code yet.
Goal: an invite link of **at most 60 characters after `mvs://join/`**, in base58 with a checksum. This is a contract change (core ↔ UI ↔ desktop deep links).

## 1. Today's link

```
mvs://join/<server id>/<invite record id>/<server secret>
```

Each part is 32 bytes in lower-case hex (`core/api/src/model.rs` `invite_link` / `parse_invite_link`). That is 96 bytes, or **194 characters** after the prefix.

| Part | Used for |
|---|---|
| server ID | Which server; checked against the genesis (`ServerId::derive(owner, nonce)` in `core/records`), so a fake genesis can't claim it. |
| invite record ID | The `Join { invite }` action names it; authority checks expiry, max uses and revocation. |
| server secret | GAP-4: the topic handles (`mvs/topic/v1`) and gossip keys of every log. Without it nobody serves us the control log. One per server, shared by all of its invites. |

## 2. What can move out of the link

60 base58 characters hold about 351 bits (60 × log2 58), i.e. 43 bytes at most, so 96 bytes can't fit. The idea: the link carries only a short **invite key**, and the full data comes from the network after connecting, in a sealed envelope.

- **Server secret**: fetched, inside the envelope. 32 random bytes can't shrink, and the secret must stay what existing members already use.
- **Invite record ID**: fetched, inside the envelope.
- **Server ID**: fetched, inside the envelope. The link keeps a **16-byte prefix** of it, so whoever holds the link can't redirect a joiner to another server (§5).
- **Discovery**: unchanged. Today the joiner finds peers through the general DHT lookup and asks every connected peer. The envelope request goes the same way, and #215's retry covers "nobody online yet".
  - Later, the provider-key lookup from the `claude/join-hang-wip` branch, if it lands, could key on the envelope handle too.

## 3. New layout (version 1)

37 bytes, encoded as base58 with the Bitcoin alphabet: **at most 51 characters**. The first byte is non-zero, so there's no leading-`1` padding, and 9 characters are left for later versions.

```
offset  len  field
0       1    version = 0x01
1       16   invite key k        (random per invite, OsRng)
17      16   server ID prefix    (first 16 bytes of the ServerId)
33      4    checksum            first 4 bytes of BLAKE3 derive_key("mvs/invite-link-checksum/v1", bytes[0..33])
```

Link: `mvs://join/<base58(37 bytes)>`, with no `/` after the prefix. That's how a parser tells it from a v0 link.

Keys derived from k:
- `h = derive_key("mvs/invite-handle/v1", k)` names the envelope on the wire (32 bytes, opaque).
- `e = derive_key("mvs/invite-seal/v1", k)` seals it.

**Envelope**: XChaCha20-Poly1305 (the AEAD sync already uses for gossip) under `e`, with a random 24-byte nonce.
- AAD: `0x01 ‖ server ID prefix`.
- Plaintext: `server ID (32) ‖ invite record ID (32) ‖ server secret (32)`.
- 136 bytes in all.

The envelope and `h` travel **inside the invite's control record**: `Action::Invite` gains `handle: [u8; 32]` and `envelope: bytes` (≤ 256, opaque to authority). Every member that holds the control log can then serve the envelope, not just the inviter.

**[S6] Record rules:**
- `handle` and `envelope` are optional, but **both or neither**: decode rejects a record with one and not the other.
- The encoding is canonical.
- Each invite record grows by about 170 bytes (32 + 136 + framing), well inside `MAX_RECORD_BYTES`.
- Old records parse without them.

**[S1] Serve only live invites.** A member serves `h` only while authority says the invite is live: not revoked, not expired, uses left. It withdraws `h` on revocation, on expiry and when the last use is taken, and re-evaluates whenever authority's state changes. A peer never answers for an invite authority says is dead.

**[S5] Server secret rotation ends outstanding v1 links.** Envelopes seal the secret of the moment, so after a rotation (GAP-4b) members stop serving every invite whose envelope is stale. The admin creates new invites, with new links, for anyone still to join. This is deliberate: rotation is meant to cut off holders of the old secret. The UI says so when rotating.

**Versioning**: unknown versions are refused with a clear status ("this link needs a newer MVS"), never guessed. A new layout gets a new version byte and new KDF labels.

**Checksum**: typo detection only, not a MAC. A random mistyped link passes with probability 2^-32.

## 4. Join with a v1 link

1. Parse and check the checksum, then derive `h` and `e`.
2. Ask connected peers for the envelope named `h`. Retried with #215's backoff while nobody has it.
3. Open the envelope. Refuse it unless `server ID` starts with the link's prefix.
4. From here it's today's join: `open_server(server, secret)`, pull the control log, check the genesis against the server ID.
5. **[S3] Bind the link to its invite**, exactly:
   - The invite record must be the one the envelope names.
   - Authority must accept it: its signer is an admin at that point, and it is not voided, revoked or expired, with uses left.
   - Its `handle` must equal `h`.
   - Then **re-open `record.envelope` under `e`** and require byte-for-byte the same plaintext as the envelope that was served. So the secret actually used is the admin-signed one, not whatever a peer handed over.
   - Any failure: the join is refused as `invalid`.
   - The genesis check against the full ServerId (step 4) stays.
6. `Join { invite }` as today.

## 5. Security

What an attacker without the link can do:
- **Guess a link**: 128-bit k. Infeasible. The checksum doesn't help: it's public math, not a secret.
- **Learn anything from `h` or the envelope** (relay, any peer, a sniffer on the circuit): `h` is a one-way function of k, and the envelope is AEAD-sealed under another one. Both are opaque.
- **Enumerate envelopes** by asking for random handles: 2^-256 per try, and request rates are already capped per peer (`limits.rs`).

What a link holder (or a member) can do:
- **Read the server.** Same as today: the link is a bearer credential for the server secret.
- **Forge an envelope** under the same k and serve it to other joiners:
  - Pointing at **another server**: blocked by the 16-byte prefix. A matching ServerId needs about 2^128 grinding of `ServerId::derive`.
  - Pointing at **the same server with a fake secret**: the joiner pulls nothing. That's a denial of service only, and the joiner keeps asking other peers.
  - Pointing at **another invite** of the same server (to dodge an expiry or use limit): blocked by step 5. The named invite record must carry `h`, and only an admin can write one.
- **Brute-force k offline** with `h` and an envelope (members, or a peer that saw a request): 2^128 work, and N live invites only make it N·2^-128 per try.
- **Revocation** stops new envelopes from being served. Anyone who already opened one keeps the server secret. That's unchanged from today (one secret per server); re-keying on kick is a separate GAP item. A revoked or expired v1 link becomes useless even to a holder who hadn't opened it yet, which v0 links can't offer.
- **What a leaked link gives** is the same as v0: the server secret (control log, presence, gossip topics; GAP-4/4b), not message keys, which go only to members. The holder can join while the invite is live.

**[S2] Responder rules** (`/mvs/invite/1`; PR-CHECKLIST §4):
- Answer only an exact `h` match, with no listing or prefix search.
- "Not found" is one constant response, whether or not the peer is in that server.
- Per-peer and global request caps, as for pulls.
- Envelope at most 256 bytes.

Residual, recorded under GAP-4b: anyone who knows `h` (a link holder, or a non-member peer the joiner asked) can tell which connected peers are members of that server while the invite is live. That's the same exposure as topic handles today, and it ends with expiry or revocation.

**[S4] k at rest is as sensitive as the server secret.**
- It's stored encrypted under the identity storage key (GAP-11), never logged, `Zeroizing`, with a redacted `Debug`.
- The UI may display the link but must not persist it.

**Typos and case:** the checksum also catches a link whose case was changed. The UI then says "copy the link exactly", not just "invalid".

What changes for privacy:
- The link shows a 16-byte server ID prefix, not the full ID (today: the full ID and the secret in plain hex).
- The envelope moves the secret off the link. A link pasted into a chat is now worth exactly as much as before: it still opens the secret. It is not worth more.

k is **16 bytes** (sec-lead, §8 Q1): 128 bits matches the rest of the system (Ed25519, XChaCha). 20 bytes would add length and no meaningful margin.

## 6. Compatibility

- **Parse both.** Core, the UI (`cleanInviteLink`) and the desktop deep-link handler accept v0 (three 64-hex parts) and v1. v0 joins work exactly as today.
- **Create v1 only one release later.** Release N parses both and still creates v0. Release N+1 creates v1. Testers on N can then open N+1's links.
  - An older app given a v1 link reports `invalid`. The release notes say to update.
- Invites created before N+1 have no `handle`/`envelope`. Their v0 link stays valid. The owner can make a new invite to get a short link.
- **v0 parsing is dropped at the open alpha** (sec-lead, §8 Q3). v0 links carry the secret in plain hex and live on in chat history, so they shouldn't be supported for good.
- **Base58 is case-sensitive.** `cleanInviteLink` lower-cases today, and must not do that for v1. Deep links keep the path's case on Windows and Linux (Electron argv). Users must copy the link, not retype it.

## 7. Code pieces

| Area | File | Change |
|---|---|---|
| link format | `core/api/src/model.rs` | `InviteLink::{V0 {server, invite, secret}, V1 {key, prefix}}`; `invite_link`/`parse_invite_link` for both; base58 (the `bs58` crate, MIT/Apache, or ~30 lines in-house) and the checksum; unit tests with pinned vectors. |
| KDFs, envelope | `core/api/src/crypto/` (new `invite.rs`) | `h`, `e`, seal and open (XChaCha20-Poly1305, as `core/sync/src/access.rs` seals gossip); pinned-vector tests like `handles_and_keys_are_pinned_and_domain_separated`. |
| create | `core/api/src/service.rs` (the server summary's `invite_link`, about line 2051, and invite creation); `core/api/src/db.rs` | A fresh k per invite, kept in the inviter's db so the link can be shown again; `Action::Invite` with `handle` and `envelope`. |
| join | `core/api/src/service.rs` `join_server`/`try_join` | The v1 path (§4): fetch the envelope, prefix check, the invite-carries-`h` check; #215 retries it. |
| serve | `core/api/src/service.rs` (control-log load) | Tell sync to serve `h → envelope` for every live invite in our servers' control logs. Withdraw on revoke, expiry, uses running out and secret rotation; re-evaluate on every authority change (S1, S5). |
| port | `core/api/src/ports.rs`, `core/api/src/syncnet.rs` | `Network::serve_invite(h, envelope)`, `withdraw_invite(h)`, `fetch_invite(h) -> Option<Vec<u8>>`. |
| wire | `core/sync/src/node.rs` (+ its request/response types) | A `/mvs/invite/1` request/response: ask connected peers for `h`, answer from the served map. Exact match only, one constant "not found", per-peer and global caps, envelope ≤ 256 bytes (S2). No server secret is needed to ask or answer. |
| authority | `core/authority` (owner: core-5) | `Action::Invite` gains `handle` and `envelope` (optional, size-capped, opaque to the rules). Old records parse without them. |
| UI parser | `ui/src/core/sanitize.ts` `cleanInviteLink`, `ui/src/core/types.ts`, `ui/src/core/mock.ts` | Accept v0 (lower-cased as today) and v1 (`^mvs://join/[1-9A-HJ-NP-Za-km-z]{40,59}$`, case kept). The checksum is checked in core, not the UI. |
| UI tests | `ui/src/test/invite.test.tsx`, `ui/src/test/linkOpen.test.tsx`, `ui/src/core/ipc.test.ts` | v1 accepted with its case kept; v0 still accepted; too long, wrong alphabet and `/` inside refused. |
| deep links | `desktop/src/deeplink.js` (+ `desktop/test`) | The same two patterns. |
| join status | `core/api/src/types.rs` | `Invalid` gets a reason, or a new `update-needed` phase for an unknown version (dev-ui wording). |

Order: authority field → api crypto + model (pinned vectors) → sync wire → api join/serve → UI and desktop parsers (release N, still creating v0) → switch creation to v1 (N+1).

**[S6] Tests:**
- Pinned vectors for `h`, `e`, the checksum and base58.
- Hostile cases: wrong version, bad checksum, a `/` inside the payload, a link of 52+ characters, the wrong server-ID prefix, and a record whose `handle` doesn't match `h`.
- A mixed-case base58 link through `desktop/src/deeplink.js`.
- Use `bs58` if it passes `cargo deny`; otherwise an in-house version with the vectors.

## 8. Decisions

| Question | Sec-lead | Tech Lead |
|---|---|---|
| Q1. k: 16 or 20 bytes? | 16 bytes | pending |
| Q2. Envelope inside `Action::Invite`, or kept by the inviter only? | inside `Action::Invite`: joins work while the inviter is offline, and it's admin-signed, as S3 needs | pending |
| Q3. Keep v0 parsing for good? | no: drop it at the open alpha, and stop creating v0 at N+1 | pending |
| S5. Secret rotation? | rotation ends outstanding v1 links | pending |

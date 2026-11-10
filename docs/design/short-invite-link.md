# Short invite links (T8-02)

Status: **design, for review** (tech-lead, sec-lead). No code yet.
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

The envelope and `h` travel **inside the invite's control record**: `Action::Invite` gains `handle: [u8; 32]` and `envelope: bytes` (≤ 256, opaque to authority). Every member that holds the control log can then serve the envelope, not just the inviter. A revoked invite stops being served.

**Versioning**: unknown versions are refused with a clear status ("this link needs a newer MVS"), never guessed. A new layout gets a new version byte and new KDF labels.

**Checksum**: typo detection only, not a MAC. A random mistyped link passes with probability 2^-32.

## 4. Join with a v1 link

1. Parse and check the checksum, then derive `h` and `e`.
2. Ask connected peers for the envelope named `h`. Retried with #215's backoff while nobody has it.
3. Open the envelope. Refuse it unless `server ID` starts with the link's prefix.
4. From here it's today's join: `open_server(server, secret)`, pull the control log, check the genesis against the server ID.
5. **Bind the link to its invite**: the invite record named in the envelope must be in the control log (an admin signed it, as authority already checks) and must carry this same `h`. Otherwise the join is refused as `invalid`.
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
- **Revocation** stops new envelopes from being served. Anyone who already opened one keeps the server secret. That's unchanged from today (one secret per server); re-keying on kick is a separate GAP item.

What changes for privacy:
- The link shows a 16-byte server ID prefix, not the full ID (today: the full ID and the secret in plain hex).
- The envelope moves the secret off the link. A link pasted into a chat is now worth exactly as much as before: it still opens the secret. It is not worth more.

For the sec-lead to decide: 16 bytes of k (128-bit) against 20 (also fits: 41 bytes = at most 56 characters).

## 6. Compatibility

- **Parse both.** Core, the UI (`cleanInviteLink`) and the desktop deep-link handler accept v0 (three 64-hex parts) and v1. v0 joins work exactly as today.
- **Create v1 only one release later.** Release N parses both and still creates v0. Release N+1 creates v1. Testers on N can then open N+1's links.
  - An older app given a v1 link reports `invalid`. The release notes say to update.
- Invites created before N+1 have no `handle`/`envelope`. Their v0 link stays valid. The owner can make a new invite to get a short link.
- **Base58 is case-sensitive.** `cleanInviteLink` lower-cases today, and must not do that for v1. Deep links keep the path's case on Windows and Linux (Electron argv). Users must copy the link, not retype it.

## 7. Code pieces

| Area | File | Change |
|---|---|---|
| link format | `core/api/src/model.rs` | `InviteLink::{V0 {server, invite, secret}, V1 {key, prefix}}`; `invite_link`/`parse_invite_link` for both; base58 (the `bs58` crate, MIT/Apache, or ~30 lines in-house) and the checksum; unit tests with pinned vectors. |
| KDFs, envelope | `core/api/src/crypto/` (new `invite.rs`) | `h`, `e`, seal and open (XChaCha20-Poly1305, as `core/sync/src/access.rs` seals gossip); pinned-vector tests like `handles_and_keys_are_pinned_and_domain_separated`. |
| create | `core/api/src/service.rs` (the server summary's `invite_link`, about line 2051, and invite creation); `core/api/src/db.rs` | A fresh k per invite, kept in the inviter's db so the link can be shown again; `Action::Invite` with `handle` and `envelope`. |
| join | `core/api/src/service.rs` `join_server`/`try_join` | The v1 path (§4): fetch the envelope, prefix check, the invite-carries-`h` check; #215 retries it. |
| serve | `core/api/src/service.rs` (control-log load) | Tell sync to serve `h → envelope` for every live invite in our servers' control logs; withdraw on revoke. |
| port | `core/api/src/ports.rs`, `core/api/src/syncnet.rs` | `Network::serve_invite(h, envelope)`, `withdraw_invite(h)`, `fetch_invite(h) -> Option<Vec<u8>>`. |
| wire | `core/sync/src/node.rs` (+ its request/response types) | Ask connected peers for `h`, answer from the served map. Envelope ≤ 256 bytes; per-peer request caps as for pulls. No server secret is needed to ask or answer. |
| authority | `core/authority` (owner: core-5) | `Action::Invite` gains `handle` and `envelope` (optional, size-capped, opaque to the rules). Old records parse without them. |
| UI parser | `ui/src/core/sanitize.ts` `cleanInviteLink`, `ui/src/core/types.ts`, `ui/src/core/mock.ts` | Accept v0 (lower-cased as today) and v1 (`^mvs://join/[1-9A-HJ-NP-Za-km-z]{40,59}$`, case kept). The checksum is checked in core, not the UI. |
| UI tests | `ui/src/test/invite.test.tsx`, `ui/src/test/linkOpen.test.tsx`, `ui/src/core/ipc.test.ts` | v1 accepted with its case kept; v0 still accepted; too long, wrong alphabet and `/` inside refused. |
| deep links | `desktop/src/deeplink.js` (+ `desktop/test`) | The same two patterns. |
| join status | `core/api/src/types.rs` | `Invalid` gets a reason, or a new `update-needed` phase for an unknown version (dev-ui wording). |

Order: authority field → api crypto + model (pinned vectors) → sync wire → api join/serve → UI and desktop parsers (release N, still creating v0) → switch creation to v1 (N+1).

## 8. Open questions

1. k: 16 or 20 bytes (§5)?
2. Envelope inside `Action::Invite` (members serve it), or kept by the inviter only (no authority change, but no join while the inviter is offline)? This note proposes the former.
3. Keep v0 parsing for good, or drop it at the open alpha?

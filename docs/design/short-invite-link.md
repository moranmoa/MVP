# Short invite links (T8-02)

Status: **design, settled**.
- Tech Lead: changes requested (envelope off-log); folded in.
- Security Lead: approved with S1–S6, then with C1–C3 for the off-log design; folded in, marked **[S…]** / **[C…]**.

No code yet.
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

## 2. What moves out of the link

60 base58 characters hold about 351 bits (60 × log2 58), i.e. 43 bytes at most, so 96 bytes can't fit. The link therefore carries only a short **invite key** k and a server ID prefix. The full server ID, the invite record ID and the server secret come from the network after connecting, in a sealed **envelope** that the server's members serve.

- **Server secret, invite record ID, server ID**: fetched, inside the envelope. The link keeps a **16-byte prefix** of the server ID, so whoever holds the link can't redirect a joiner to another server (§5).
- **Discovery**: unchanged. Today the joiner finds peers through the general DHT lookup and asks every connected peer. The envelope request goes the same way, and #215's retry covers "nobody online yet".

## 3. Layout (version 1)

37 bytes, encoded as base58 with the Bitcoin alphabet: **at most 51 characters**. The first byte is non-zero, so there's no leading-`1` padding, and 9 characters are left for later versions.

```
offset  len  field
0       1    version = 0x01
1       16   invite key k        (random per invite, OsRng)
17      16   server ID prefix    (first 16 bytes of the ServerId)
33      4    checksum            first 4 bytes of BLAKE3 derive_key("mvs/invite-link-checksum/v1", bytes[0..33])
```

Link: `mvs://join/<base58(37 bytes)>`, with no `/` after the prefix. That's how a parser tells it from a v0 link.

k is **16 bytes** (§8): 128 bits matches the rest of the system (Ed25519, XChaCha), and an online guess also needs a peer that answers. 20 bytes would add length and no meaningful margin.

**Keys derived from k:**
- `h = derive_key("mvs/invite-handle/v1", k)` names the envelope on the wire (32 bytes, opaque).
- `e = derive_key("mvs/invite-seal/v1", k)` seals it.

**Envelope**: XChaCha20-Poly1305 from the `chacha20poly1305` crate already in the tree (as `core/sync/src/access.rs` uses it; no second copy) under `e`, with a random 24-byte nonce.
- AAD: `0x01 ‖ server ID prefix`.
- Plaintext: `server ID (32) ‖ invite record ID (32) ‖ server secret (32)`.
- 136 bytes in all.

**Versioning**: unknown versions are refused with a clear status ("this link needs a newer MVS"), never guessed. A new layout gets a new version byte and new KDF labels.

**Checksum**: typo detection only, not a MAC. A random mistyped link passes with probability 2^-32. It also catches a link whose case was changed; the UI then says "copy the link exactly", not just "invalid".

## 4. Where the envelope lives: an off-log publication

**Not in `Action::Invite`** (§8 Q2).
- Today's peers parse the Invite body as exactly 12 bytes (`core/authority/src/lib.rs:252`), and `Kind::Invite.max_body()` is 128 (`core/records/src/lib.rs:276`).
- A longer body would be malformed on older peers. Every join through that invite would then be invalid there only, and membership would split between versions.
- **No record or authority change.**

Instead, the inviter publishes an **invite publication** to the server's members over the member-gated sync stream, the way presence travels:

```
InvitePublication {
  invite:   RecordId (32)     the Invite record in the control log
  handle:   h (32)
  envelope: ≤ 256 bytes       (136 in v1)
  sig:      Ed25519 (64)      by the invite record's author, over
                              "mvs/invite-envelope/v1\0" ‖ invite ‖ handle ‖ envelope
}
```

**[C1] Members store and serve a publication only if:**
- the signature checks against the **author of the named invite** in their own control log, which must be in it;
- that invite is **live** in their own authority state: accepted, not voided, not revoked, not expired, uses left.

They re-check on every authority change and drop the publication the moment the invite dies (**[S1]**: revocation, expiry, last use taken). Without the signature, any member could plant mappings.

**[C2] Bounded:**
- At most one publication per invite ID. The first valid one wins, later ones are ignored.
- Envelope ≤ 256 bytes.
- At most 256 stored publications per server.
- Never forwarded to non-members.

**Catching up**: a member re-announces the live publications it holds whenever another bound member connects, so a member who was offline at creation catches up. The inviter can then be offline when someone joins.

Old peers don't know the new sync message and simply don't serve it. That's acceptable during release N.

**[S5] Server secret rotation ends outstanding v1 links.** Envelopes seal the secret of the moment, and members can't see inside them. So on a rotation (GAP-4b) every member drops all publications of that server. The admin creates new invites, with new links, for anyone still to join. This is deliberate: rotation is meant to cut off holders of the old secret. The UI says so when rotating.

**[S6] Canonical encoding** for the publication: fixed field order and lengths, rejected on any length mismatch. Stated size: 32 + 32 + 136 + 64 plus framing, about 270 bytes per publication, off-log, so nothing changes in `MAX_RECORD_BYTES`.

## 5. Join with a v1 link

1. Parse and check the checksum, then derive `h` and `e`.
2. Ask connected peers for `h` over `/mvs/invite/1`. **A non-member may ask**: the joiner isn't a member yet, and neither asking nor answering needs a server secret. Retried with #215's backoff while nobody has it.
3. **[C3] Answers are untrusted.** Collect them from several peers and try each in turn. The first one that opens under `e`, whose server ID matches the link's prefix, and that passes step 4 is used. A bad answer (say, junk a member planted under a real `h`) must not stop the join.
4. From here it's today's join: `open_server(server, secret)`, pull the control log, check the genesis against the full ServerId.
5. **Binding check (recommended, cheap).** The served answer carries the whole signed publication, so once the control log is in, the joiner checks C1's signature against the named invite's author, the same check members make. A failure moves on to the next answer (step 3).
6. `Join { invite }` as today. Authority enforces expiry, the use cap and revocation on the invite named.

**No mandatory link-to-invite binding** (the former S3).
- It protects nothing v0 doesn't already give away: a link holder has the server secret, can read every live invite ID in the control log, and can write a v0 link to any of them.
- Authority still enforces each invite's limits on the `Join`.
- **This returns as a must the day an invite carries anything beyond "may join"** (a role, channel grants, an 18+ attestation; GAP-22). Redirecting a joiner to another invite would then change what they get.
- Reconsider it as well when v0 parsing is dropped.

## 6. Security

What an attacker without the link can do:
- **Guess a link**: 128-bit k, and each online guess is a request to capped peers. Infeasible. The checksum doesn't help: it's public math, not a secret.
- **Learn anything from `h`, the envelope or the publication** (relay, any peer, a sniffer on the circuit): `h` is a one-way function of k, and the envelope is AEAD-sealed under another one.
- **Brute-force k offline** with `h` and an envelope (members, or a peer that saw a request): 2^128 work, and N live invites only make it N·2^-128 per try.
- **Enumerate envelopes** by asking for random handles: 2^-256 per try, under the rate caps below.

What a member can do:
- **Plant junk under a real `h`**: it fails C1 at other members, and at the joiner it fails to open or is skipped (C3).
- **Plant a mapping for a dead invite**: refused (C1 liveness).

What a link holder can do:
- **Read the server.** Same as today: the server secret opens the control log, presence and the gossip topics (GAP-4/4b), not message keys, which go only to members. They can join while the invite is live.
- **Forge an envelope** under the same k:
  - Pointing at **another server**: blocked by the 16-byte prefix (a matching ServerId needs about 2^128 grinding of `ServerId::derive`) and by the genesis check.
  - Pointing at **the same server with a fake secret**: the joiner pulls nothing and moves on to other answers. A denial of service at most.
  - Pointing at **another invite** of the same server: possible, as v0 already allows (§5). Fails the step-5 check when the joiner does it.
- **Revocation and expiry** stop the envelope from being served. Anyone who already opened one keeps the server secret, unchanged from today. Re-keying on kick is a separate GAP item. A dead v1 link becomes useless even to a holder who hadn't opened it yet, which v0 links can't offer.

**[S2] Responder rules** (`/mvs/invite/1`; PR-CHECKLIST §4):
- Answer only an exact `h` match, with no listing or prefix search.
- "Not found" is one constant response, whether or not the peer is in that server.
- **Rate caps**: 10 asks per minute per peer and 120 per minute per node in total, like pull limits. Over the cap, the constant "not found".
- Answers are at most 400 bytes (one publication).

Residual, recorded under GAP-4b: anyone who knows `h` (a link holder, or a non-member peer the joiner asked) can tell which connected peers are members of that server while the invite is live. That's the same exposure as topic handles today, and it ends with expiry or revocation.

**[S4] k at rest is as sensitive as the server secret.**
- It's stored encrypted under the identity storage key (GAP-11), never logged, `Zeroizing`, with a redacted `Debug`.
- The UI may display the link but must not persist it.

**Privacy:** the link shows a 16-byte server ID prefix, not the full ID (today: the full ID and the secret in plain hex).

## 7. Compatibility

- **Parse both.** Core, the UI (`cleanInviteLink`) and the desktop deep-link handler accept v0 (three 64-hex parts) and v1. v0 joins work exactly as today.
- **Create v1 only one release later.** Release N parses both and still creates v0. Release N+1 creates v1. Testers on N can then open N+1's links.
  - An older app given a v1 link reports `invalid`. The release notes say to update.
- Invites created before N+1 have no publication. Their v0 link stays valid. The owner can make a new invite to get a short link.
- **v0 parsing stays through the open alpha** (§8 Q3). Whether to drop it at 1.0 is decided then, with a release-notes line.
- **Base58 is case-sensitive.**
  - `cleanInviteLink` lower-cases today, and must not do that for v1.
  - The desktop deep-link handler (`desktop/src/deeplink.js`) must not lower-case the path either. Windows `argv` paths sometimes go through case-folding helpers, so this gets a test with mixed-case base58.
  - Users must copy the link, not retype it.

## 8. Code pieces

| Area | File | Change |
|---|---|---|
| link format | `core/api/src/model.rs` | `InviteLink::{V0 {server, invite, secret}, V1 {key, prefix}}`; `invite_link`/`parse_invite_link` for both; base58 with `bs58` (MIT/Apache, allowed by the licence policy, not a crypto crate) and the checksum. |
| KDFs, envelope, publication | `core/api/src/crypto/` (new `invite.rs`) | `h`, `e`; seal and open (`chacha20poly1305`, the crate and version already in the tree); sign and verify the publication under `mvs/invite-envelope/v1\0`; canonical encode/decode. |
| create | `core/api/src/service.rs` (the server summary's `invite_link`, about line 2051, and invite creation); `core/api/src/db.rs` | A fresh k per invite, kept encrypted in the inviter's db so the link can be shown again (S4); build and publish the publication. **No authority change.** |
| serve while live | `core/api/src/service.rs` (control-log load and every authority change) | C1: verify publications from members against the invite's author and liveness; tell sync to serve or drop. Drop all of a server's publications on secret rotation (S5). |
| join | `core/api/src/service.rs` `join_server`/`try_join` | The v1 path (§5): ask, try each answer (C3), prefix check, genesis check, signature check; #215 retries it. |
| port | `core/api/src/ports.rs`, `core/api/src/syncnet.rs` | `Network::publish_invite(pub)`, `serve_invite(pub)` / `drop_invite(h)`, `fetch_invite(h) -> Vec<publication bytes>` (several answers), and a node event for publications received from members. |
| member stream | `core/sync/src/node.rs` | Carry publications to bound members only (as presence travels), re-announce on member connect, per-server cap (C2). The publication is opaque to sync; the api decides what to serve. |
| wire | `core/sync/src/node.rs` (+ request/response types) | `/mvs/invite/1`: anyone may ask, including non-members; exact match only; constant "not found"; 10/min per peer and 120/min per node; answers ≤ 400 bytes (S2). No server secret is needed to ask or answer. |
| UI parser | `ui/src/core/sanitize.ts` `cleanInviteLink`, `ui/src/core/types.ts`, `ui/src/core/mock.ts` | Accept v0 (lower-cased as today) and v1 (`^mvs://join/[1-9A-HJ-NP-Za-km-z]{40,59}$`, case kept). The checksum is checked in core, not the UI. |
| UI tests | `ui/src/test/invite.test.tsx`, `ui/src/test/linkOpen.test.tsx`, `ui/src/core/ipc.test.ts` | v1 accepted with its case kept; v0 still accepted; too long, wrong alphabet and `/` inside refused. |
| deep links | `desktop/src/deeplink.js` (+ `desktop/test`) | The same two patterns; never lower-cases v1; a mixed-case test. |
| join status | `core/api/src/types.rs` | `Invalid` gets a reason (`copy-exactly` for a bad checksum, `update-needed` for an unknown version); dev-ui wording. |

**[S6] Tests:**
- Pinned vectors for `h`, `e`, the checksum, base58 and the publication signature.
- Hostile cases:
  - wrong version, bad checksum, a `/` inside the payload, a link of 52+ characters, the wrong server-ID prefix;
  - a publication with a bad signature, one for a dead invite, a second publication for the same invite, junk under a real `h` (the join still succeeds from another answer).

Order: api crypto + model (pinned vectors) → sync member stream and wire → api create/serve/join → UI and desktop parsers (release N, still creating v0) → switch creation to v1 (N+1).

## 9. Decisions

| Question | Decision |
|---|---|
| Q1. k: 16 or 20 bytes? | **16 bytes** (sec-lead; tech-lead agrees) |
| Q2. Where does the envelope live? | **Off-log, member-gated, signed by the inviter** (tech-lead; sec-lead accepts with C1–C3). Not in `Action::Invite`: older peers would split. |
| Q3. v0 parsing? | **Keep it through the open alpha**; decide on dropping it at 1.0. Creating v0 stops at N+1. |
| S3 (bind the link to the invite)? | **Not required now**; the joiner does the cheap signature check (§5 step 5). It becomes a must if invites ever carry more than "may join". |
| S5. Secret rotation? | **Rotation ends outstanding v1 links.** |

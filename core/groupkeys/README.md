# groupkeys: per-channel epoch keys

A channel's messages are encrypted under the key of the current **epoch**. A
moderator starts a new epoch with a **rotation**: a fresh random 32-byte key,
wrapped separately for every member allowed to read the channel. A kick (or a
revoke that takes away access) removes the member from the next rotation, so
they can't read anything sent after it. MLS was rejected (spec: it needs a
central order); this is the per-epoch wrapped-key design from the spec.

`records` is used for types only (`PublicKey`, `RecordId`, `ServerId`,
`ChannelId`, and `Kind` to read size limits). The caller puts a rotation body
into a signed record, and checks with `authority` that its author may rotate,
before calling `ChannelKeys::apply`.

## API in short

| Item | What it does |
| --- | --- |
| `ChannelKeys::new(server, channel, my_identity)` | One member's view of one channel |
| `apply(record_id, author, body)` | Take in a rotation (idempotent, any order). Returns whether we got a key |
| `head()` | The rotation new messages use |
| `rotation_need(members)` | `Initial`, `Required { removed }`, `Add { missing }` or `None` |
| `rotate(rotator, members, rng)` | Body for the next rotation (epoch = head + 1, prev = head) |
| `encrypt(members, plaintext, rng)` | Seal under the head key. Refuses with `RotationRequired` while a removed member still holds it |
| `decrypt(author, ciphertext)` | Open with whichever epoch key the header names |
| `Rotation::create / decode / unwrap_key`, `seal / open` | The lower layer, for other callers and tests |

## Crypto, and why

**Member keys: X25519 derived from the Ed25519 identity.** The public key maps
through the Edwards to Montgomery birational map (`VerifyingKey::to_montgomery`),
and the secret is the same clamped scalar (`SigningKey::to_scalar_bytes`), the
same conversion as libsodium's `crypto_sign_ed25519_pk_to_curve25519`. Why:
anyone can be added to a rotation knowing only their identity, with no extra
"encryption key" record to publish, sync and keep in step with the identity.
Using one key pair for both Ed25519 signatures and an X25519 KEM is covered by
Thormarker, *On using the same key pair for Ed25519 and an X25519 based KEM*
(2021). Small-order ("weak") identity keys are refused. The cost: the
encryption key can't be rotated on its own; per-device keys (spec: later stage)
can be bound to the identity when they arrive.

**Wrapping: DH-KEM + BLAKE3 KDF + XChaCha20-Poly1305, with one ephemeral key
per rotation.** For each recipient: `shared = X25519(ephemeral, recipient)`,
the all-zero result is rejected, then `wrap_key = BLAKE3.derive_key(ctx,
shared || ephemeral_pk || recipient_x25519 || AD)`. The AD binds the server, the
rotator (record author), the whole rotation header (channel, epoch, prev,
ephemeral key) and the recipient. The epoch key is sealed under `wrap_key` with
XChaCha20-Poly1305. Each `wrap_key` is used once, so the all-zero nonce is
safe, even if a rotator's RNG repeated an ephemeral key (the header still
differs).

Why not HPKE (RFC 9180, the `hpke` crate)? HPKE base mode is the same shape
(DH-KEM, KDF, AEAD), but every recipient carries their own 32-byte `enc`:
112 bytes per member instead of 80, so about 56 KB instead of 40 KB for 500
members (the spec budgets about 40 KB). Sharing one ephemeral key across
recipients is the standard multi-recipient KEM with randomness reuse; it is
safe because every wrap key also depends on that recipient's own DH output and
key. The primitives come from vetted crates: `chacha20poly1305` (RustCrypto,
audited by NCC Group) and `curve25519-dalek` / `ed25519-dalek`, with `blake3`
as the KDF (already used for record IDs). No primitive is implemented here.

**Messages: XChaCha20-Poly1305, random 24-byte nonce.** The key is
`BLAKE3.derive_key(ctx, epoch_key)`. A random 192-bit nonce makes collisions
negligible however many messages an epoch has. The AD is
`server || channel || epoch || rotation record ID || author || header`, so a
ciphertext opens only in the channel, epoch and author it was made for:
copied into another channel or server, re-posted under another author, or
with its epoch rewritten, it fails. `decrypt` also refuses an author who wasn't
a recipient of that epoch.

## Formats and size overhead

Rotation body (little-endian), carried in a signed record whose author is the
rotator:

```text
ver u8 | channel [32] | epoch u64 | prev tag u8 (+ RecordId [32]) | ephemeral [32]
| count u16 | count * (recipient Ed25519 key [32] | sealed epoch key [48])
```

Message ciphertext: `ver u8 | epoch u64 | key hint [8] | nonce [24] | sealed`.
The key hint is the first 8 bytes of the rotation's record ID, used to pick
the key (two concurrent rotations can share an epoch number); the full ID is
in the AD.

| Item | Bytes |
| --- | --- |
| Message overhead | **57** (1 + 8 + 8 + 24 + 16 tag) |
| Largest plaintext in a `Message` record (16,384 limit) | **16,327** |
| Rotation header (epoch > 0) | **108** (76 for epoch 0) |
| Per member in a rotation | **80** (32 key + 32 sealed key + 16 tag) |
| Members that fit 16,384 bytes | **203** |
| 500 members | **40,108** (spec estimate: about 40 KB) |

Tests check all of these: the largest ciphertext is signed into a real
`Message` record, and a 203-member rotation fits 16,384 bytes.

**Needs a decision (records owner):** `records` has no rotation kind yet, and
its largest body is 16 KiB, so a rotation tops out at 203 members per record.
The spec's 500-member server needs either a `KeyRotation` kind with a body
limit of about 48 KiB (`MAX_RECORD_BYTES` grows with it), or rotations split
over several records. Tests carry rotations in `Message` envelopes for now.

## Rules

- **Kick or revoke forces rotation.** `rotation_need(members)` compares the
  head's recipients with the current member set from `authority`. If anyone
  removed still holds the head key, the answer is `Required`, and `encrypt`
  refuses until a moderator rotates. New members only produce `Add`: sending
  goes on, and they read once a rotation includes them.
- **Concurrent rotations resolve deterministically.** A rotation is *linked*
  if it is epoch 0, or its `prev` is linked and exactly one epoch below. The
  head is the linked rotation with the highest epoch, and between rotations of
  the same epoch the **lowest record ID** wins, the tie-break `authority` uses.
  This depends only on the set of rotations held, so every peer converges
  (tested over every delivery order). If the winner was made before its author
  saw a concurrent kick, the kicked member is still in it, so `Required` fires
  and one more rotation settles it. The losing rotation's key is kept, so
  anything sent under it stays readable to its recipients.
- **Offline catch-up.** Every rotation wraps the key directly for each member;
  there's no chain to replay. A member back after several rotations applies
  them in any order (one whose `prev` hasn't arrived waits, then links) and
  holds the key of every epoch they were in.
- **New members can't read history from before they joined.** They have no
  wrap for earlier epochs. Their first readable epoch is the rotation that
  adds them.

## Where this differs from the spec, or the spec is unclear

1. **History for new members: unclear, and differs from the recommended
   default.** Open question 3 in `docs/spec.md` ("Does a new user see all the
   history?", status: under discussion) recommends a per-server setting with
   **full history by default**, and from-join-only for servers in the public
   index. This crate does from-join only, as this task asked. Full history
   would need a "history grant": a moderator re-wraps old epoch keys for the
   newcomer. That fits the same wrap format, but it's not built. Needs a
   product decision.
2. **Per channel, not per server.** The spec talks of one key per server that
   changes on every kick or ban. Keys here are per channel, which also allows
   private channels. A server-wide key is a single fixed `ChannelId`.
3. **"Concurrent bans merge into one epoch."** Concurrent rotations do collapse
   to one epoch (lowest ID wins), but if the winner still includes a member
   only the loser removed, one more rotation follows. A merge without the extra
   rotation (the key derived from all concurrent keys, so only members in every
   one of them can read) is possible later.
4. **Who may rotate** is left to the caller: moderators and up, matching
   kick. Whether an ordinary member may rotate to add a newcomer is open.

## Threat model

**Protects:**
- Message content from relays, DHT nodes, sync peers and anyone else who sees
  the records but isn't a member: they see only ciphertext.
- New messages from removed members: after the rotation that drops them, they
  hold no key for the new epoch. The check is enforced: `encrypt` refuses
  while a removed member still holds the head key.
- Integrity and context: any change to a ciphertext, its epoch, or the
  server, channel or author it's bound to makes decryption fail. A rotation
  body moved to another channel is refused, and one re-signed by another
  author opens for nobody.
- Malformed keys: small-order member keys and ephemeral keys are refused.

**Doesn't protect:**
- **What a member already saw.** A kicked member keeps every message and key
  from the epochs they were in (spec: "they keep the history they already
  downloaded"). Any member can copy plaintext, take screenshots or hand an
  epoch key to someone else; no cryptography stops a reader from leaking.
- **Authenticity inside the group.** Every member of an epoch holds its key,
  so any of them could produce a ciphertext naming another author. Authorship
  comes from the record signature, which the AD is tied to through the author
  field. `decrypt` also checks the author was a recipient.
- **Replay within the same context.** The same ciphertext reposted by its own
  author in a new record (same channel and epoch) still opens; the AD can't
  bind the record ID, which is a hash of the body. Deduplicate higher up if
  that matters.
- **Metadata.** Rotations list recipients in the clear, so anyone holding the
  records sees who is in a channel. Authors, timing, epoch numbers and message
  sizes are visible too; padding isn't done here.
- **A malicious moderator.** They can wrap the key for anyone, or put a broken
  wrap for someone (`Applied::WrapInvalid`, attributable to the signed
  rotation). Only `authority` limits who rotates.
- **Before a kick is known.** A member who hasn't yet received a kick record
  keeps sending under the old key, which the kicked member can still read.
  This window is inherent in a system without a central server.

**Forward secrecy limits:**
- No forward secrecy within an epoch: one leaked epoch key exposes every
  message of that epoch, past and future, until the next rotation.
- Rotations happen on membership change, not on a timer, so an epoch can last
  a long time. Periodic rotation would bound the exposure, at the cost of a
  rotation record of 80 bytes per member each time.
- The wrap uses a long-term key (derived from the identity). Whoever steals an
  identity secret key can unwrap **every** epoch that member was ever in, past
  and future, from the stored rotations, until that identity is removed. There
  is no post-compromise recovery short of kicking the stolen identity. Separate
  per-device keys that rotate would narrow this later.
- The rotator's ephemeral secret and the new epoch key come from its RNG
  (`OsRng` in production): a broken RNG on one moderator's device exposes the
  epochs that moderator created.
- Keys are zeroed on drop in memory (`zeroize`); storing them safely at rest
  is the caller's job.

## Tests

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm cargo test --release -p groupkeys -- --nocapture
```

Run from `core/`. `tests/scenarios.rs`: a kicked member can't decrypt the next
epoch; tampered ciphertext, AD or epoch is rejected; concurrent rotations
converge over every delivery order; offline catch-up in reverse order; a new
member can't read history; size limits. `tests/properties.rs` (proptest):
wrap/unwrap round trip for random member sets, a tampered rotation never yields
a different key, channel round trip, and any byte flip in a sealed message is
rejected.

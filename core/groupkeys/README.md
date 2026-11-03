# groupkeys: per-channel epoch keys

A channel's messages are encrypted under the key of the current **epoch**. A
moderator starts a new epoch with a **rotation**: a fresh random 32-byte key,
wrapped separately for every member allowed to read the channel. A kick (or a
revoke that takes away access) removes the member from the next rotation, so
they can't read anything sent after it. MLS was rejected (spec: it needs a
central order); this is the per-epoch wrapped-key design from the spec.

`records` is used for types only (`PublicKey`, `RecordId`, `ServerId`,
`ChannelId`, and `Kind` to read size limits). The caller puts a rotation body
into a signed record, applies **every** well-formed rotation record of the
channel with `ChannelKeys::apply`, and passes the ones `authority` **accepts**
to `set_accepted` whenever that set changes (for the integration:
`authority::State::accepted` intersected with this channel's rotation
records). **Deny by default:** a rotation authority hasn't ruled on (not
processed yet, pending on missing records, or a kind it doesn't know) can never
be the head, so nobody sends under it. Rotations that aren't accepted keep
their keys for reading; a message opened under one comes back with
`accepted: false`, and the caller must show it as untrusted. The result depends
only on the two sets, not on the order rotations and verdicts arrive in.

All domain-separation and KDF strings start with `mvs/`.

## API in short

| Item | What it does |
| --- | --- |
| `ChannelKeys::new(server, channel, my_identity)` | One member's view of one channel |
| `apply(record_id, author, body)` | Take in a rotation (idempotent, any order). Returns whether we got a key |
| `set_accepted(ids)`, `accept(id)` | The rotations `authority` accepts (allow-list); recomputes the head |
| `head()` | The rotation new messages use: linked, accepted, highest epoch, lowest ID |
| `rotation_need(members)` | `Initial`, `Required { removed }`, `Add { missing }` or `None` |
| `rotate(rotator, members, rng)` | Body for the next rotation (epoch = head + 1, prev = head) |
| `encrypt(members, plaintext, rng)` | Seal under the head key. Refuses with `RotationRequired` while a removed member still holds it |
| `decrypt(author, ciphertext)` | Open with whichever epoch key the header names |
| `decrypt_with_rotation(author, ciphertext)` | The same, and says which rotation and epoch opened it and whether it is accepted, so the caller can flag a message sent under a superseded or non-accepted key |
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

*Why not a dedicated subkey derived from the recovery words?* It would still
have to be published and signed before anyone could wrap to it, which is the
extra record this design avoids, and it wouldn't fix the real weakness: a
stolen identity secret opens every stored epoch, and so would a stolen subkey
derived from the same words. Only keys that change help, that is per-device
keys certified by the identity. The way there needs no format change: the wire
field stays "recipient Ed25519 key", and recipient → X25519 goes through one
function (`x25519_public`), so a certified device key can be a recipient
later.

**Wrapping: DH-KEM + BLAKE3 KDF + XChaCha20-Poly1305, with one ephemeral key
per rotation.** For each recipient: `shared = X25519(ephemeral, recipient)`,
the all-zero result is rejected, then `wrap_key = BLAKE3.derive_key(ctx,
shared || ephemeral_pk || recipient_x25519 || AD)`. The AD binds the server, the
rotator (record author), the whole rotation header (channel, epoch, prev,
ephemeral key) and the recipient. The epoch key is sealed under `wrap_key` with
XChaCha20-Poly1305. Each `wrap_key` is used once, so the all-zero nonce is
safe. If a rotator's RNG repeated an ephemeral key, the wrap keys still differ
as long as the channel, epoch or prev differ. If the RNG repeated completely,
the epoch key repeats too, so the same plaintext is sealed again and nothing
new leaks. That is the only case left, and it means the RNG is broken anyway.

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

`records` has no rotation kind yet, and its largest body is 16 KiB, so a
rotation tops out at 203 members per record. See decision B below. Tests carry
rotations in `Message` envelopes for now.

## Rules

- **Kick or revoke forces rotation.** `rotation_need(members)` compares the
  head's recipients with the current member set from `authority`. If anyone
  removed still holds the head key, the answer is `Required`, and `encrypt`
  refuses until a moderator rotates. New members only produce `Add`: sending
  goes on, and they read once a rotation includes them.
- **Concurrent rotations resolve deterministically.** A rotation is *linked*
  if it is epoch 0, or its `prev` is linked and exactly one epoch below. The
  head is the linked, accepted rotation with the highest epoch, and between
  rotations of the same epoch the **lowest record ID** wins, the tie-break
  `authority` uses. This depends only on the rotations held and the accepted
  set, so every peer converges (tested over every order of rotations and
  acceptances). A rotation that is not accepted still links the
  rotations built on it: linking only checks the epoch counter, and a later
  rotation's key is fresh, so it owes nothing to it. If the winner was made before its author
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

1. **History for new members.** Open question 3 in `docs/spec.md` recommends
   **full history by default** (from-join only for public-index servers). This
   crate does from-join only, as the task asked. See decision A below.
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

## Decisions for the owner

Nothing below is built yet; it waits for your decision. These are spec
decision log rows 30 and 31. The owner-facing version, in Hebrew with lines
ready to paste into the log, is in
[`DECISIONS-D30-D31.md`](DECISIONS-D30-D31.md).

### A. What new members see

| Option | New member sees | Privacy | Complexity | Cost |
| --- | --- | --- | --- | --- |
| 1. Encrypted, from join (built today) | Only what's sent after the rotation that adds them | Best. Content stays with whoever was a member when it was sent | Nothing more to build | Every join needs a rotation (80 B per member), so a moderator must be online |
| 2. **Encrypted with history** (the third option) | Everything since the server started | Relays and outsiders still see only ciphertext. But anyone who gets any later key, by joining or through a leak, can read all earlier history | Small. Each rotation also seals the previous epoch key under the new one (a "history link"), so a newcomer gets the current key and walks back. A join needs no rotation: any online member can wrap the current key for the newcomer | +48 B per rotation; a newcomer opens one link per past epoch (microseconds each) |
| 3. Public, unencrypted | Everything, and so does anyone who has the server ID, relays and DHT nodes included | None. Content is copied to peers and relays and can never be taken back | Fewest keys, but a second code path through sync, search and moderation | Cheapest |

- Option 3 contradicts the spec: "all server content is encrypted with a group key only members hold; relays see only encrypted bytes". If "public" means anyone can join and read, option 2 with open joining gives that and keeps relays blind.
- For option 2, a history link is cheaper than having moderators re-wrap every past epoch key for each newcomer: 48 B per rotation instead of 48 B × epochs per join, and no moderator needs to be online.
- A kick still works in option 2: links only point backwards, so a kicked member can't reach new epochs.

**Switching mode later:**

| Change | Existing messages | New messages |
| --- | --- | --- |
| 1 → 2 | Newcomers read back as far as the switch. A moderator who still holds older keys may also link further back, as a deliberate "open the past" step | Readable to every later joiner |
| 2 → 1 | Existing members keep everything. Newcomers get no link, so they can't read anything from before they joined | From join only |
| 1 or 2 → 3 | Stay encrypted, readable only by people who had the keys, unless moderators publish the old keys, which can't be undone | Plaintext, public |
| 3 → 1 or 2 | **Stay public forever**: they've already been copied to peers and relays. The UI has to say so | Encrypted |

**My pick: option 2 as the default for the Hebrew-gamers alpha, option 1 as a per-server switch for private groups, and no option 3 in the alpha.** Gamer communities coming from Discord expect to see channel history, and the spec recommends full history by default. Option 2 keeps the spec's promise that relays see only ciphertext, costs 48 B per rotation, and lets a newcomer read without waiting for a moderator, which matters on small servers with few moderators. Servers in the public index follow the spec and default to option 1. Building option 2 means a history-link field in the rotation and a small "add member" wrap record: about a day, plus tests.

### B. Channels bigger than 203 members

| Option | Security | Sync per rotation | Effort | Past 500 (e.g. 5,000) |
| --- | --- | --- | --- | --- |
| 1. `KeyRotation` kind with a ~48 KiB limit | Same as today | One 40 KB record to every member: about 20 MB in total across 500 members | Smallest: a kind and a limit in `records`. But `MAX_RECORD_BYTES` roughly triples for **every** record, so the worst-case memory of PR #7's pending queue (4,096 records) also roughly triples | No: tops out at about 613 members |
| 2. **One rotation split across records (shards)** | Same as today. Each shard is signed, and all carry the same header | Each member fetches only the shard holding their wrap (≤16 KB); 500 members = 3 shards, 5,000 = 25 (about 400 KB in total) | Medium: shard index and count in the body; shards 1..n name shard 0's record ID, which stays the rotation's ID (so lowest record ID still breaks ties, as in `authority`); a member whose shard is missing shows up as `Add` | Yes, linear. The rotator's cost at 5,000 members is about 0.25 s of X25519 |
| 3. Key tree (TreeKEM/LKH) | Strong; about log₂(n) wraps per rotation | About 1 KB per rotation at 5,000 | High. Every member must agree on the tree's state, which needs a total order: the reason MLS was rejected | Yes in theory, but a poor fit for a log with no central order |
| 4. Sender keys (Signal/WhatsApp groups) | Adds forward secrecy within each sender's chain | A kick makes every remaining member send a new key to every other: n² wraps, 250k at 500 and 25M at 5,000 | Medium-high | No |

**My pick: option 2, shards, with a dedicated `KeyRotation` kind kept at the current 16 KiB limit.** It's the only option that keeps working at 5,000 members without raising the size limit for every record, and members fetch only their own shard. For the alpha, servers under 200 members already fit in one record, so the shard change can land before open alpha. It's about 1–2 days with tests.

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

**Known gaps (tracked):**
- Non-accepted and orphan rotations are kept without a bound, so memory can
  grow (GAP-8, issue #19).
- Every `ChannelKeys` holds its own copy of the identity signing key (issue
  #20).
- The integration has to label messages opened under a non-accepted rotation
  as untrusted (issue #21).

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
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -w /work rust:1.90-bookworm cargo test --release -p groupkeys -- --nocapture
```

Run from `core/`. `tests/scenarios.rs`: a kicked member can't decrypt the next
epoch; tampered ciphertext, AD or epoch is rejected; concurrent rotations
converge over every delivery order; offline catch-up in reverse order; a new
member can't read history; size limits. `tests/properties.rs` (proptest):
wrap/unwrap round trip for random member sets, a tampered rotation never yields
a different key, channel round trip, and any byte flip in a sealed message is
rejected.
`tests/acceptance_head.rs`: the security review's PoC as a regression test (a
non-member's rotation wrapped for exactly the members never becomes head until
`authority` accepts it); no head while nothing is accepted; the same head in
every order of rotations and acceptances, also when authority withdraws an
acceptance; rotations built on a non-accepted one still link; non-accepted keys
still read, flagged `accepted: false`. `tests/format.rs`: strict decoding (huge
count, count mismatch, prev tag, version, duplicate or unsorted recipients) and
the v1 formats pinned with a fixed RNG.

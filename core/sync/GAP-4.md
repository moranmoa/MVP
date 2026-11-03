# GAP-4: members-only sync (design note)

**Status: approved by both leads (#96), built in core/sync. GAP-4 closes
once core/api switches to `open_server` (see "What api does next").**

**Names, not server IDs, on the wire.** Each server gets a 32-byte random
*server secret*, made by the owner at creation, kept in the local store and
carried in every invite link (`mvs://join/<server>/<invite>/<secret>`; the
link isn't frozen yet). It never goes into a record. Every log is
addressed on the wire by a *handle*,
`BLAKE3::derive_key("mvs/topic/v1", secret ‖ scope)`. That handle is both
the gossipsub topic and the `log` field of a pull request. A node answers
only handles it can map back to a log it holds, through the secrets it
knows. So an outsider who knows only the server ID can neither name a log
nor recognise one in the topic lists that gossipsub shows to connected
peers. Gossip payloads are also sealed with ChaCha20-Poly1305 under a key
derived from the secret (`mvs/gossip/v1`). A peer that sees a topic hash
and subscribes to it gets only ciphertext and can't inject. This adds
`chacha20poly1305` to sync; api, identity and groupkeys already use it.
`Node::open_server(server, secret)` replaces opening raw log names.

**Peer ID ↔ identity binding.** The pull request gets an optional `binding`
field. It is the identity key's Ed25519 signature over
`mvs/peer-binding/v1 ‖ peer_id ‖ expiry`, with an expiry of about 24 hours,
signed by `core/identity` through a callback in `NodeConfig`. The answerer
checks three things: the signature, the expiry, and that `peer_id` is the
connection's Noise-authenticated remote. It then caches the identity for
that connection. The binding is sent only in a pull, so only to a peer that
already knows the secret (it answered on the handle). It isn't announced at
connect, which would show who we are to the relay and to every DHT
neighbour. A pull for a channel log is answered only if the bound identity
passes a new `sync::ReadGate` trait. `core/api` implements that trait with
`MemberGate`'s rule: owner, or a stored `Join` whose invite is stored, the
same rule as the storage gate. **Bootstrap:** a pull for the *control* log
is answered for any bound requester that knows the secret, so that a
joiner can get the log it needs before its own `Join` exists. The control
log names members and roles but holds no messages (GAP-15 covers the
residual).

**What it doesn't stop, and the order of work.** A kicked member keeps the
secret: the rule is monotone, so they still read gossip and pull channels.
Pulls could use the non-monotone authority verdict, because serving isn't
storing. Gossip needs secret rotation on kick, which should be its own item
next to GAP-5 key rotation. A leaked invite link gives the control log
only. Order: (1) handles + sealed gossip, (2) binding + `ReadGate` + the
bootstrap rule, (3) harness and tests, in one PR from core/sync. A separate
small change to `core/api` (secret in the link and the store, the binding
callback, `MemberGate: ReadGate`), which I'd coordinate with whoever owns
api.

## Decisions (tech lead, #96)

1. The per-server secret goes in the invite link.
2. Kicked members are excluded at serve time: `ReadGate` (implemented in
   api) uses the current authority state and is **not** monotone. That is
   fine, because serving isn't storing. The storage gate stays monotone.
3. Gossip is sealed with the secret. Secret rotation on kick is a separate
   follow-up.

This PR is core/sync only. The api side (the link, the store, the binding
callback, `ReadGate`) goes to dev-identity. Building waits for the
security lead's OK too.

## As built (core/sync)

The security lead's amendments are all in:

- **Sealing:** XChaCha20-Poly1305 with a random 192-bit nonce; a key per
  log, `derive_key("mvs/gossip/v1", secret ‖ scope)`; AD = version ‖ handle.
  A message that doesn't open is `Reject`, and an opened record still goes
  through decode, the derived log and the store.
- **No handles in the DHT:** nothing publishes them there. If discovery
  ever needs a DHT key, it gets its own `mvs/dht/v1` derivation.
- **The binding signs the control handle:** `mvs/peer-binding/v1 ‖ len ‖
  peer id ‖ control handle ‖ expiry`. The expiry may be at most 24 h ahead,
  with ±10 min skew allowed. A verified binding is cached per peer and
  dropped when the peer disconnects.

The code:

- **`src/access.rs`:** `ServerSecret` (zeroized on drop, `Debug` redacted),
  `Handle` (`derive_key("mvs/topic/v1", secret ‖ scope)`, scope `0x00` or
  `0x01 ‖ channel`, pinned by a test), sealing, `Binding`, and two traits:
  - `Binder`: signs bindings; implemented for a plain `SigningKey`.
  - `ReadGate`: who may read channels; not monotone.
- **`Node::start_with(config, Access { binder, read_gate })`,
  `open_server(server, secret)` and `open_channel(server, channel)`:** a
  pull for a handle needs a valid binding from the Noise-authenticated
  peer. A channel pull additionally runs the `ReadGate`, on the answering
  thread over the read-only store. The control log is served to any bound
  peer.
- **The plain-name path is kept:** `Node::start` and `Node::open(log)` work
  as before, until api switches. A sealed log is never served, accepted or
  opened by its plain name.
- **The sync protocol is now `/mvs/sync/0.3.0`.**
- **The harness** syncs a sealed server, with a fixed test secret.

## What api does next (dev-identity)

- **Secret:** a random secret per server, made at creation, kept in the
  local store, and carried in the invite link (with dev-ui for the link
  format). Never logged, and never in an https URL.
- **Binder:** implemented over the identity key.
- **`ReadGate`:** implemented over the current authority `State`: a member,
  and not kicked.
- **Opening servers:** start the node with `Node::start_with`, and open
  servers with `open_server` / `open_channel` instead of `open(log)`.

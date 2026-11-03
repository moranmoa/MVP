# GAP-4: members-only sync (design note)

**Status: proposal, for the tech lead's and the security lead's OK before building.**

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

## Open questions

1. A per-server secret in the link (above), or derive the handle from
   something that is already there?
2. Should pulls for channels exclude kicked members now (authority verdict
   at serve time), or keep the monotone rule as asked?
3. Is sealing gossip with the secret OK alongside GAP-5, or is a
   per-subscription disconnect enough for the security lead?

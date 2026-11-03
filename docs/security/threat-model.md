# MVS system threat model

Status: v1, 2026-11-03. Owner: security lead. Covers `main` (`5ef10ff`) plus the
open PRs #5 (sync hardening), #7 (records/authority fixes), #8 (updater audit),
#9 (groupkeys), #10 (UI security) and #11 (identity). Where a mitigation lives
only in an open PR, the PR number is given. Each crate already has its own
audit (`core/updater/SECURITY.md`, `core/groupkeys/README.md` "Threat model",
`core/identity/README.md` "Security properties"). This document covers the
**seams between crates** and the system as a whole, and does not repeat those
audits.

Read [the prioritised GAP list](#gaps-prioritised) first if you only have five
minutes.

## The one-paragraph summary

Each crate is solid on its own terms: records verify signatures on decode,
authority resolves the control log deterministically and blocks backdating,
groupkeys gets the cryptography right, identity derives and stores keys
carefully, and the updater survives a hostile network. **But nothing yet
connects them.** There is no integration layer: sync stores and gossips opaque
blobs without ever calling `Record::decode`, no code checks that a channel
message's `auth_deps` grant posting rights, there is no membership record (any
key that knows a server ID counts as a member), sync answers any peer's pull for
any log, and the network layer publishes every user's IP address in the DHT.
These are the checks a central server would do. In MVS every peer must do them,
and right now no crate owns them.

## Scope and assumptions

- In scope: the Rust core (`core/`), the UI's boundary with the core (`ui/`),
  the relay/bootstrap node and TURN config (`infra/`), and the release/update
  chain.
- Out of scope: malware running as the user (it can already replace the app;
  see `core/updater/SECURITY.md`), physical attacks on a running unlocked
  device, and the web/mobile clients (stage 2+), except where a decision made
  now locks them in.
- The closed alpha is invite-only for known testers (spec, "MVP"). The open
  alpha adds open invite links and 2-of-3 update keys. The public index is
  stage 6.
- Cryptographic primitives (Ed25519, X25519, BLAKE3, XChaCha20-Poly1305,
  Argon2id, Noise) are assumed sound.

## Assets

| # | Asset | Why it matters | Where it lives |
| --- | --- | --- | --- |
| A1 | **Recovery words / identity secret** | Whoever has them *is* the user, forever: signs as them, unwraps every epoch key they ever got (groupkeys README). No revocation exists | OS keychain (`core/identity`, PR #11), optionally under the app lock |
| A2 | **Server owner key** | Same key as A1 for the owner. The owner can't be demoted or kicked (authority rules), so losing it loses the server; leaking it hands the server over for good | As A1 |
| A3 | Message content | Spec promise: "relays and DHT nodes see only encrypted bytes" | Record bodies (`Kind::Message`), SQLite store, UI memory |
| A4 | Epoch keys | Open a channel's messages for a whole epoch | groupkeys memory (zeroized); at rest: caller's job (unbuilt) |
| A5 | Server state integrity: roles, kicks, who may post | Wrong state = moderators lose control, kicked users return | Control log, resolved by `authority` |
| A6 | **Users' and the owner's home IP addresses** | Harassment, DDoS, physical location (spec risk table: "IP exposure between members") | libp2p listen addresses, DHT, relay logs, gossipsub |
| A7 | Social metadata: who is in which server, who talks to whom, when | Outing members of sensitive communities | Gossipsub topics/subscriptions, control log, rotation recipient lists, relay logs |
| A8 | Update root keys | Code execution on every client | Offline, owner's encrypted drive (`core/updater/SECURITY.md`) |
| A9 | Relay node key | Its peer ID is baked into every client as the bootstrap address; a thief can impersonate the entry point | Docker volume `relay-data`, `/data/node.key` |
| A10 | Availability: disk, CPU, bandwidth of peers and of the owner's home PC | P2P peers store and forward for each other | Everywhere |
| A11 | Minors' data (birth date, age gating) | Legal (spec: age restriction, 18+ servers) | Identity backup, profile |

## Actors

| Actor | Has | Wants |
| --- | --- | --- |
| **Outsider who knows a server ID** (from an invite link, a screenshot, a leaked link) | A fresh key pair, a libp2p node | Read the server, post into it, map its members |
| **Malicious member** | A valid identity that is a member; the current epoch key | Spam, impersonation, disk/CPU exhaustion of other members, harass, escalate privileges |
| **Malicious moderator / admin** | Kick, grant (below own rank), rotate | Abuse power: mass-kick, lock others out of reading, wrap keys for outsiders, backdate |
| **Kicked user** | Old keys and history, old control-log heads, a still-running node | Keep reading, keep posting, come back under a new key |
| **Sybil operator** | Unlimited free key pairs and peer IDs | Flood public servers, ban evasion, eclipse peers in the DHT |
| **Network attacker** (ISP, Wi-Fi, state) | Sees and alters packets between peers and to the relay | Read, inject, map who talks to whom, block |
| **Malicious relay / DHT node** (including a third-party relay in future, or our relay if the owner's PC is compromised) | Sees every connection, reservation and circuit it handles, answers DHT queries | Map the social graph, collect IPs, eclipse joiners, deny service |
| **Holder of a compromised update key** | One root key (closed alpha: that is all keys) | Ship malicious code to everyone |
| **Stolen or shared device** (thief, family member, repair shop) | The disk, maybe an unlocked OS session | Take over the identity, read stored messages |

## Architecture and trust boundaries

```text
             ┌──────────────────────────── user's device ────────────────────────────┐
             │                                                                       │
 12 words ──►│ identity ──signing key──► records ──Record──► authority (control log)  │
             │   │  (keychain, app lock)   ▲  sign/decode      │ State: roles, kicks  │
             │   │                         │                   ▼                      │
             │   └──X25519 (same key)──► groupkeys ◄── members, voided rotations      │
             │                             │  epoch keys, seal/open                   │
             │                             ▼                                          │
             │   [ NO INTEGRATION LAYER ] ◄─── blobs (opaque) ─── sync (SQLite store) │
             │                             │                        ▲   libp2p node  │
             │                             ▼                        │                 │
             │                        UI (React) ◄── guardCore sanitize (PR #10)      │
             │                                                                       │
             │   updater ◄── manifests + root chain (signed offline)                  │
             └──────────────────────┬────────────────────────────────────────────────┘
                     TB2 network    │ Noise (TCP), gossipsub, request-response, Kademlia
         ┌──────────────────────────┴───────────────┐
         ▼                                          ▼
   other peers (TB3: untrusted)            relay/bootstrap on owner's home PC (TB4)
                                           Docker: TCP/UDP 4001 public, WS 4002 localhost
```

Trust boundaries:

- **TB1, OS/keychain ↔ app.** Secrets cross here (identity). Anything running as
  the same user is trusted by the OS; the app lock narrows that.
- **TB2, device ↔ network.** Everything arriving from libp2p is hostile:
  framing, blobs, gossip, DHT results, identify info.
- **TB3, member ↔ member.** A record is authentic (signed) but not authorised
  until authority/groupkeys say so. *A valid signature is not permission.*
- **TB4, user ↔ helper servers.** The relay is trusted for availability only,
  never for content or integrity. It is the owner's PC, so it also holds the
  owner's IP and logs.
- **TB5, core ↔ UI.** UI renders text written by other people; it must never get
  secrets it doesn't need. PR #10's `guardCore` cleans everything crossing.
- **TB6, release signers ↔ clients.** Update manifests and root rotations.

### Data flows and where each check should happen

| # | Flow | Check that must happen | Who does it today |
| --- | --- | --- | --- |
| F1 | Words → identity key (HKDF) → `Draft::sign` | Key never leaves identity except as `&SigningKey` | identity (PR #11) ✔ |
| F2 | Peer → sync gossip/pull → SQLite `append` | (a) `Record::decode` (size, canonical form, signature); (b) log name **derived from the record** (`server`, `scope`), not from the topic; (c) record deps = blob deps | **GAP-1.** sync checks only its own BLAKE3 blob id; the `content_id` hook comment in `core/sync/src/lib.rs` (PR #5) describes exactly this but it isn't built |
| F3 | Store → gossip forward (`on_gossip` → `Accept`) | Forward only records that passed F2 *and* authority (otherwise we amplify junk) | **GAP-1.** `on_gossip` accepts anything that `Blob::decode`s |
| F4 | Control record → `authority::Resolver` | Signature already verified (type guarantees it), server matches genesis, permission rules | authority ✔ (PR #7 binds servers to their genesis) |
| F5 | Channel record → "is this message allowed?" | `auth_deps` resolve to control state where author is a non-kicked **member**, and a later kick voids it | **GAP-2, GAP-3.** No code reads `auth_deps` |
| F6 | Rotation record → groupkeys `apply` / `set_accepted` | Rotator may rotate (authority), rotation's recipients ⊆ members; **default deny**: a rotation authority hasn't accepted is never head (PR #9 review: the `set_voided` version was fail-open) | **GAP-5.** No `Kind::KeyRotation`; "who may rotate" left to the caller; no caller |
| F7 | Members set → `groupkeys::rotation_need` / `encrypt` | Member set comes from authority | **GAP-3.** authority has no member set (only roles and kicks) |
| F8 | Message plaintext → `encrypt` → record body | Body is ciphertext for every `Message` | **GAP-5.** Not wired; bodies are plaintext today |
| F9 | Core → UI | Sanitize names/text/colours/images; never pass keys | UI `guardCore` (PR #10) ✔ for text; Tauri IPC surface not built yet |
| F10 | Sync pull request from peer → `answer()` | Requesting peer is a member of that server (not kicked) | **GAP-4.** Anyone who names the log gets it |
| F11 | Node ↔ relay ↔ DHT | Publish only what's needed; filter private addresses; relay-only option | **GAP-6.** Clients are Kademlia servers and publish all listen addresses |
| F12 | Manifest / root chain → updater | Threshold signatures, freshness, rollback rules | updater ✔ (PR #8) |

## Threats by actor

Legend: ✔ mitigated (where) · ◐ partly · **GAP-n** see the prioritised list.

### Outsider who knows a server ID

| Threat | Status |
| --- | --- |
| Download the whole server (control log, channels) by subscribing to `log/<name>` or sending a pull | **GAP-4**: `answer()` in `core/sync/src/node.rs` serves any open log to any peer; gossipsub lets anyone subscribe |
| Read message content | **GAP-5** today (bodies plaintext). Once wired: ✔ groupkeys (only recipients of an epoch can open it) |
| Read the member list and roles | ◐ Control log is plaintext by design (needed to resolve); rotation recipients are in the clear (groupkeys README). Limited by fixing GAP-4. Residual: **GAP-15** |
| Post control `Post` records into any server | **GAP-3**: `permitted()` returns `true` for `Action::Post` from any non-kicked key, member or not (`core/authority/src/lib.rs`) |
| Post channel messages | **GAP-2/3**: nothing checks channel records at all |
| Forge a genesis for someone's server | ✔ `ServerId::derive(owner, nonce)` + `GenesisServerMismatch` (records, PR #7) |
| Learn which servers a given user is in | **GAP-6/15**: gossipsub announces each peer's topic subscriptions to every connected peer; topic name is the log name |

### Malicious member

| Threat | Status |
| --- | --- |
| Forge another member's record | ✔ Ed25519 `verify_strict` in `Record::decode`, ID = hash of signed bytes |
| Re-encode a record to get a second ID / bypass dedup | ✔ canonical encoding enforced, sig excluded from ID (records) |
| Move a ciphertext to another channel/server/author | ✔ AEAD AD binds server, channel, epoch, rotation, author (groupkeys) |
| Replay own ciphertext in a new record | ◐ Opens again (groupkeys README "Replay"). Dedupe by ciphertext hash at the integration layer: part of **GAP-1** |
| Grant themself a role | ✔ `permitted()` requires `author_role > role` |
| Fake timestamps to reorder chat | ◐ authority never uses `ts`. UI displays `ts` as given: **GAP-18** |
| Disk exhaustion: many valid records, huge DAG fan-out | ◐ size limits per record (records), dep limits, pending caps and pull rate limits (sync, PR #5). **GAP-8**: no per-log / per-author quota |
| CPU: records that force full re-resolution | ◐ incremental resolver with `MAX_PENDING` (authority, PR #7). Watch in perf tests. IDs ground to sort early make each incremental insert O(n) (`Vec::insert` + re-indexing): CPU only, noted in PR #7 |
| Unicode lookalike names, bidi overrides, zalgo, huge avatars | ✔ `guardCore` sanitize (UI, PR #10) |
| XSS / script via message text | ✔ React escaping + CSP meta (UI, PR #10). Recheck when Tauri IPC lands: **GAP-17** |
| Leak plaintext / epoch keys to outsiders | Not preventable (groupkeys README). Accepted |
| Rotate keys to exclude others (if members may rotate) | **GAP-5**: who may rotate is unchecked |

### Malicious moderator / admin

| Threat | Status |
| --- | --- |
| Kick or revoke someone of equal/higher rank | ✔ rank checks in `permitted()` |
| Backdate a kick after being demoted | ✔ revoke voids the target's concurrent privileged records (authority) |
| Mass-kick everyone below them | ◐ Allowed by design; owner can revoke them and their concurrent kicks are voided only if concurrent. **GAP-14**: no rate limit, no moderation log yet (spec stage 3) |
| **Choose what a kick voids**: sign a kick with stale deps (or, with GAP-2, a stale channel cut) to erase the target's recent history, posts and messages included | **GAP-2** design note below: voiding limited to records nobody else built on. Needs a spec decision before formats freeze (also listed unresolved in PR #7) |
| Wrap the epoch key for an outsider | ◐ groupkeys can't stop it; integration must refuse rotations whose recipients aren't members (**GAP-5**) |
| Rotation with deliberately broken wraps (lock someone out) | ◐ Detected and attributable (`Applied::WrapInvalid`, groupkeys); UI must surface it. **GAP-5** |
| Spam rotations (each up to 16 KiB to every member) | **GAP-8/14** |
| Take the server from the owner | ✔ owner can't be revoked or kicked |

### Kicked user

| Threat | Status |
| --- | --- |
| Read messages sent after the kick | ✔ once wired: `encrypt` refuses until a rotation drops them (groupkeys). Today **GAP-5** (plaintext) and **GAP-4** (sync serves them) |
| Keep posting in channels | **GAP-2**: channel records aren't checked against kicks |
| Backdate channel messages by pointing `auth_deps` at pre-kick control heads | **GAP-2**: needs a rule; see the design note there |
| Keep posting control records | ✔ `permitted()` refuses kicked authors; concurrent records voided |
| Keep downloading the server | **GAP-4** |
| Return under a new key | **GAP-3/13**: without membership/invites, a new key is a member immediately |
| Keep the history they already had | Accepted (spec) |

### Sybil operator / spammer

| Threat | Status |
| --- | --- |
| Unlimited identities | **GAP-13**: spec's identity proof-of-work isn't built; nothing costs anything |
| Flood a server with posts | **GAP-3/8/13**: no membership, no quota, no per-author rate limit at accept time (spec says spam rules are local per peer: not built) |
| Squat a peer's 256 incoming connection slots with free peer IDs (idle timeout 1 h, no per-IP cap) | **GAP-19** |
| Pull amplification: a ~100-byte request gets up to a 4 MiB page, 50 pulls/s across all peers, so up to ~200 MiB/s of upload from a home connection | **GAP-19** (GAP-4 removes strangers; a byte budget is still needed against members) |
| Eclipse a peer in the DHT (surround it with attacker nodes so it only syncs with them) | ◐ Content can't be forged, only withheld; the relay is a fixed bootstrap with a pinned peer ID. **GAP-6** (custom DHT protocol; no IPFS DHT bleed) |
| Ban evasion | Spec answer: invite-only / join approval mode, harder PoW. **GAP-3/13** |

### Network attacker

| Threat | Status |
| --- | --- |
| Read or alter peer traffic | ✔ Noise on every libp2p connection, also through relay circuits (end-to-end between peers) |
| Impersonate the bootstrap | ✔ peer ID pinned in the bootstrap multiaddr (`/p2p/<id>`); Noise authenticates it. Depends on relay key secrecy (A9) |
| See who connects to whom and when | ◐ Inherent to P2P. Relay-only mode (spec) narrows peers' view but moves it to the relay: **GAP-6** |
| Block the relay / the owner's home connection | Accepted for alpha (single bootstrap, spec). More bootstraps before open alpha: **GAP-9** note |
| Withhold updates / roll back clock | ✔ updater freshness and time floor (PR #8) |
| Replay old records | ✔ content-addressed, idempotent |

### Malicious relay / DHT node

| Threat | Status |
| --- | --- |
| Read relayed content | ✔ Noise end-to-end through the circuit; message bodies will also be group-encrypted (**GAP-5**) |
| Build a who-talks-to-whom graph from circuits and reservations | ◐ Inherent. Our relay logs every connection and relay event at `info` with peer IDs: **GAP-7** |
| Collect every online user's IP | **GAP-6**: clients publish listen addresses into Kademlia and answer `FIND_NODE` as servers; the relay adds every address identify reports, including LAN ones |
| Refuse service / drop circuits | Accepted (availability only) |
| Pollute the DHT store | ◐ Bounded `MemoryStore` defaults; we don't use DHT records at all. **GAP-9**: disable record storage |

### Compromised update key

| Threat | Status |
| --- | --- |
| Sign a malicious release | ◐ Closed alpha: one key = full compromise (accepted, documented in `core/updater/SECURITY.md`). Open alpha: 2-of-3 on hardware tokens: **GAP-12** |
| Fork clients onto an attacker root | ✔ `RootChain::fork_at` raises an alarm (PR #8) |
| Freeze clients on an old version | ✔ freshness, `Stale` reporting (PR #8) |
| Malicious artifact on the seeder | ✔ hash in signed manifest, `store_artifact` + `open_verified` (PR #8). Seeder itself not built |

### Stolen device

| Threat | Status |
| --- | --- |
| Read the recovery words from the keychain | ◐ Only with the app lock is it protected at rest (Argon2id, identity PR #11). App lock is optional: **GAP-11** |
| Read stored messages | **GAP-11**: SQLite store is plaintext; and bodies are plaintext today (**GAP-5**). identity already derives a `local-storage/key`, unused |
| Steal the libp2p peer key | ◐ `load_or_create_key` in `core/sync/src/node.rs` writes it with default permissions. Low impact (network identity only), fixed with **GAP-11** |
| Recover after theft | **GAP-10**: no identity revocation / "compromised, moved to new key" record; owner can't transfer ownership |

## Seam-by-seam answers

The questions asked for the cross-cutting review, answered directly.

1. **Does sync verify record signatures before authority sees them?** No. Sync
   never decodes records. A blob whose bytes aren't a valid record is stored,
   counted as a head, gossiped onwards and served to other peers. Authority
   itself is safe (it only takes `Record`, which can only come from
   `Record::decode`), so forged records can't change state, but junk spreads
   and fills disks, and a real record can be filed under a different log name.
   → GAP-1.
2. **Who checks that a channel message's `auth_deps` grant posting rights?**
   Nobody. records only enforces that control records have none and that there
   are at most 16. authority ignores every non-control record. → GAP-2.
3. **Metadata leakage to the relay and the DHT.** Relay sees every reservation
   and circuit (who reaches whom through it) and logs it. Gossipsub topic names
   are the log names in clear, and subscriptions are announced to every
   connected peer, so any connected peer learns which servers you're in.
   gossipsub `MessageAuthenticity::Signed` stamps each push with the sender's
   peer ID. Rotations list recipients in clear. → GAP-6, GAP-7, GAP-15.
4. **IP exposure.** *Owner:* the bootstrap domain resolves to the home IP, by
   decision (spec, decision 24); that can't be hidden while TCP/UDP is
   port-forwarded at home. *Users:* every client runs Kademlia in **server
   mode**, listens on `0.0.0.0`, and identify hands its full listen address list
   (LAN addresses included) to the relay, which adds them to its routing table
   and hands them to anyone asking `FIND_NODE`. So anyone who can reach the
   relay can enumerate every online user's IP without joining any server. The
   spec's "relay only" mode isn't built. → GAP-6.
5. **Spam and Sybil on public servers.** Keys are free, there is no membership
   record, any key can post control `Post`s, and channel messages are
   unchecked. → GAP-3, GAP-8, GAP-13.
6. **Key compromise and recovery.** Identity: no revocation, no successor
   record; a stolen identity also unwraps every epoch it was ever in. Owner:
   permanent and untransferable. Update key: single key in closed alpha, fork
   alarm exists. Relay key: in a Docker volume with default permissions. →
   GAP-10, GAP-11, GAP-12, GAP-9.

## Relay and TURN config review (`infra/`)

Reviewed `infra/docker-compose.yml`, `infra/relay/src/main.rs`,
`infra/relay/Dockerfile`, `infra/.env.example`, `infra/README.md`.

**Exposure**

| Port | Bound to | Verdict |
| --- | --- | --- |
| 4001/tcp, 4001/udp (QUIC) | `0.0.0.0` on the host, router port-forward | Intended. Noise-authenticated libp2p only |
| 4002/tcp (plain WebSocket) | `${WS_BIND:-127.0.0.1}` | ✔ Good default: only the local reverse proxy reaches it, TLS terminated there. Risk: setting `WS_BIND=0.0.0.0` exposes unencrypted WS (Noise still runs inside, but no TLS). README should say "never change unless the proxy is on another machine on a private network" |
| 3478 + 49160–49200 (TURN, `--profile voice`) | `0.0.0.0` | Phase 2. See TURN findings below before enabling |

**Findings**

| # | Finding | Severity | Fix |
| --- | --- | --- | --- |
| R1 | **DHT leaks every client's IP, including LAN addresses.** The relay adds every `listen_addr` from identify to Kademlia, unfiltered (private, loopback, link-local included), and serves them to anyone | High | Filter to global addresses; with client nodes in Kademlia client mode, keep only the relay's own routing (GAP-6) |
| R2 | **Default Kademlia protocol `/ipfs/kad/1.0.0`** (relay and `core/sync`). Our DHT can merge with the public IPFS DHT if any peer learns an IPFS address, exposing our peers there and pulling in millions of foreign peers | Medium | `kad::Config::new(StreamProtocol::new("/mvs/kad/1.0.0"))` on both sides |
| R3 | **No connection limits on the relay.** `relay::Config::default()` caps reservations (128) and circuits (16, 2 min, 128 KiB each) with per-peer/per-IP rate limits, which is good. But plain connections are unlimited: a flood exhausts file descriptors/memory on the owner's PC | Medium | Add `connection_limits::Behaviour` (e.g. 512 established total, 8 per peer, 64 pending incoming) and `memory_connection_limits` |
| R4 | **Relay accepts DHT `PUT_VALUE` / `ADD_PROVIDER`.** Bounded by `MemoryStore` defaults (~1,024 records of ≤65 KiB), so ≤~65 MiB, but we use no DHT records | Low | `StoreInserts::FilterBoth` and drop all inserts, or a 0-size store |
| R5 | **Relay logs a social graph.** `info` level logs every connection (peer ID) and every `relay::Event` (source and destination peer IDs of circuits). Docker's json-file log driver keeps it forever with no rotation | Medium (privacy) and Low (disk) | Log relay events at `debug`; default `RUST_LOG=warn,relay_node=info` printing only counts; add `logging: {driver: json-file, options: {max-size: 10m, max-file: "3"}}` |
| R6 | **Key file permissions.** `fs::write` creates `/data/node.key` with the umask default (0644). Only uid 10001 runs in the container, but anyone with Docker access on the host can read the volume | Low | Create with `OpenOptions::mode(0o600)`; refuse to start if the existing file is group/world-readable. Back the key up offline (losing it changes the bootstrap peer ID baked into clients) |
| R7 | **Unpinned build.** `infra/relay` has no committed `Cargo.lock`, the Dockerfile runs `cargo build --release` without `--locked`, and base images are tags, not digests. Each rebuild may pull different dependencies | Medium (supply chain) | Commit `Cargo.lock`, build with `--locked`, pin `rust:1.90-bookworm@sha256:…` and `debian:bookworm-slim@sha256:…`, add the relay to the cargo-deny job |
| R8 | **No container limits.** No `mem_limit`, `pids_limit`, `cpus`, `read_only`, `cap_drop` | Low | `read_only: true`, `cap_drop: [ALL]`, `security_opt: [no-new-privileges:true]`, `mem_limit: 512m`, `pids_limit: 256` |
| R9 | **libp2p 0.54** brings 8 RustSec advisories and `ring 0.16` (note from the updater/CI session) | Medium | Upgrade libp2p in relay and sync together (one version across the workspace) |
| R10 | Identify protocol string is `/descord-replacement/0.1.0` on the relay and `/mvs/0.1.0` on clients. Harmless today, but nothing filters peers by protocol | Info | Use `/mvs/…` everywhere; clients ignore peers that don't speak it |
| R11 | **Circuit limit vs sync.** 128 KiB / 2 min per circuit (defaults) cuts off relayed catch-up (sync RESULTS.md), and the sync node has no DCUtR, so relayed peers never upgrade. Users will be pushed to direct connections (and IP exposure) to make it work | Info (affects GAP-6 design) | Decide relay-only mode budgets explicitly: raise limits for reserved members, or add DCUtR and accept direct connections only when relay-only is off |
| T1 | **TURN can reach the owner's LAN.** coturn has no `--denied-peer-ip` ranges, so a TURN client can relay to `192.168.x.x`/`10.x` on the owner's home network (classic TURN SSRF) | High (phase 2) | Add `--no-multicast-peers`, `--denied-peer-ip=10.0.0.0-10.255.255.255`, `172.16.0.0-172.31.255.255`, `192.168.0.0-192.168.255.255`, `100.64.0.0-100.127.255.255`, `169.254.0.0-169.254.255.255`, `127.0.0.0-127.255.255.255`, `::1`, `fc00::-fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff`, `fe80::-febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff` (one `--denied-peer-ip` per range) |
| T2 | **TURN secret fails open.** `--static-auth-secret=${TURN_SECRET:-}` starts with an empty secret if unset; `.env.example` ships `change-me` | Medium (phase 2) | `${TURN_SECRET:?set TURN_SECRET}` and reject `change-me` in a start script; add `--total-quota`, `--user-quota`, `--max-bps` |

Good: non-root `USER relay` (uid 10001), multi-stage image, no secrets in git
(`.env` ignored, example uses RFC 5737 `203.0.113.10`), relay peer ID pinned in
client bootstrap addresses, WebSocket on localhost by default.

## GAPs, prioritised

**P0 = must fix before the closed alpha. P1 = before the open alpha. P2 = later.**
Owner is the crate or area that should do the work. The glue layer is `core/api` (board item T1, dev-identity): the one crate that owns the
pipeline sync → records → authority → groupkeys → UI. Owners below are the Tech
Lead's assignments (review of PR #15).

| ID | Pri | Gap | Concrete task | Owner |
| --- | --- | --- | --- | --- |
| GAP-1 | **P0** | Sync stores, serves and gossips blobs without verifying they're records | Build the `content_id` hook from `core/sync/src/lib.rs`: `Record::decode` at ingest, derive the log name from `(server, scope)`, require blob deps = record deps, cross-log `auth_deps` kept separately; `on_gossip` returns `Accept` only after decode (and, once GAP-2/3 land, after authorisation); dedupe message ciphertexts. Test: a peer pushing junk bytes, or a real record under another log name, never gets it stored or forwarded | dev-sync (B4-sync) + `core/api` (T1) |
| GAP-2 | **P0** | Nothing checks channel messages: `auth_deps` unread; kicked users can post; backdating via stale `auth_deps` | Add `authority::check_channel(record, &Resolver) -> Verdict`: accept iff every `auth_deps` entry is a known control record, and the author is a non-kicked member in the state at those heads. **Backdating needs a format decision now:** a kick should name the kicker's channel heads (a small "cut" in the kick body, ≤16 ids like `auth_deps`), and every message of the target not an ancestor of that cut is voided, like control-log voiding. Without it, a kicked user can always sign messages pointing at old heads. A stale cut lets a moderator erase history, so the cut needs the bound in [the GAP-2 design note](#gap-2-design-note-bounding-what-a-kick-voids). Decide before formats freeze. **Signed off for B4 (b4-plan.md):**
- The author is judged at the linear position after the latest `auth_dep`. Any pending `auth_dep` gives `Pending`, and a Pending message is never shown or forwarded.
- Kick, Leave **and Revoke** carry the cut, as up to 64 `(ChannelId, RecordId)` pairs. A Revoke voids the author's `KeyRotation`s that aren't ancestors of its cut, because a rotation is a privileged record.
- T1 fills the cut with channels where the target has records first. Channels beyond the 64-pair cap are not voided.
- The voiding rule stays swappable until the owner chooses. | B4 (records/authority) |
| GAP-3 | **P0** | No membership: any key that knows a server ID is a `Member`; control `Post` allowed from non-members; groupkeys has no member set | Add `Kind::Invite` (signed, expiry, max uses, revocable: spec "invite link is a signed record with expiry, not the key itself") and `Kind::Join` (references an invite). `State` gets `members`; `role_of` returns Member only for joined, non-kicked keys; `Post` and channel posting require membership; `members` feeds `groupkeys::rotation_need`/`encrypt`. **Signed off for B4 (b4-plan.md) on these conditions:** an `InviteRevoke` voids every `Join` on that invite that isn't its ancestor (otherwise a backdated Join ordered before the revoke gets in); expiry by `join.ts` is soft (see accepted risks); T1 sends an automatic `InviteRevoke` at expiry | B4 (records/authority) |
| GAP-4 | **P0** | Sync answers any peer for any log; topic names reveal servers | Bind libp2p peer ID to identity at connect (identity key signs `mvs/peer-binding/v1 ‖ peer_id ‖ expiry`, exchanged in a small request-response protocol); answer pulls and accept gossip for a server's logs only from bound identities that are members (bootstrap exception: a joiner presenting a valid invite gets the control log only). Topic = `BLAKE3(mvs/topic/v1 ‖ server secret from the invite ‖ scope)` instead of the raw log name | dev-sync |
| GAP-5 | **P0** | Encryption isn't wired: `Message` bodies plaintext, no rotation record kind, nobody decides who may rotate. The UI already tells users kicks re-encrypt (screenshot 26) | Add `Kind::KeyRotation` (channel scope, 16 KiB, shards per groupkeys decision B); `authority::may_rotate` (moderator+, per groupkeys README); integration feeds `apply` and `set_accepted` (allow-list from `authority::State::accepted`; unknown or pending = deny), refuses rotations whose recipients aren't members, encrypts every `Message`, flags `WrapInvalid` in the UI | B4 (kind, `may_rotate`) + dev-crypto (`core/api/src/crypto`, after the T1 skeleton) |
| GAP-6 | **P0** (partial) / P1 | Every user's IP (LAN included) is published in the DHT and enumerable via the relay; IPFS-default DHT protocol | **P0:** clients in `kad::Mode::Client`; filter non-global addresses on relay and clients; custom `/mvs/kad/1.0.0`; gossipsub `MessageAuthenticity::Anonymous` (records are already signed); tell testers in the alpha invite that members can see their IP. **P1:** "relay only" mode (spec): no direct listen addresses published, connect only through circuits, default on for servers in the public index | dev-sync (clients) + devops (relay, T4a) |
| GAP-7 | ✔ PR #17 (once merged) | Relay on the owner's PC logs who talks to whom forever | R5: relay events at `debug`, log rotation; document what's kept. Done in #17: per-connection lines at `debug`, a counts-only `stats` line, json-file rotation 5×10 MB, no Caddy access log | devops (infra) |
| GAP-8 | P1 | No storage quotas: valid records from any author fill disks | Per-log and per-author byte quotas in the store; store only records that pass authorisation (GAP-2/3), park the rest briefly; rotation records rate-limited per author. In memory: `groupkeys` caps unaccepted rotations at 64 per channel (✔ PR #51 once merged). T1 must re-fetch a rotation that authority accepts but `ChannelKeys` evicted (`apply` returns those IDs, possibly including the one just applied), and create `ChannelKeys` only for channels authority knows | sync + `core/api` |
| GAP-9 | **P0** (T4a) / P1 | Relay hardening | **P0, before the closed alpha (T4a):** R7 (lockfile, `--locked`, digests), R3, R6, R1–R2, R10 (✔ relay side in PR #30 once merged; the relay still answers the legacy `/ipfs/kad/1.0.0` too, which puts it on the public IPFS DHT, so drop it with the `core/sync` switch before any public release). P1: R4, R9; add 198.18.0.0/15, 192.0.0.0/24, NAT64, 6to4, Teredo and `fec0::/10` to the relay's `is_global`; a second bootstrap node before the open alpha. R8 is done in PR #17 (read-only root, `cap_drop`, limits) | devops (infra) |
| GAP-10 | P1 | No key-compromise recovery: no identity revocation/successor, no ownership transfer | `Kind::OwnerTransfer` signed by the owner (and by a pre-registered recovery key, spec "admin recovery code"), plus a user-level "key moved" statement signed by old and new keys; servers treat the old key as kicked. Device keys (identity already reserves `device/ed25519/n`) at stage 3 | identity + authority + records |
| GAP-11 | P1 | Stolen device: optional app lock; SQLite store and peer key unencrypted | Encrypt the store with the already-derived `local-storage/key` (SQLCipher-compatible or per-row AEAD); prompt to set the app lock on first server join; peer key created 0600 / owner-only ACL (reuse updater's Windows ACL code, `core/updater` PR #8 finding 15) | identity + sync |
| GAP-12 | P1 | Single update key (closed alpha, accepted); seeder not built; latest root must be embedded per release | 2-of-3 on hardware tokens before open alpha (plan in `core/updater/SECURITY.md`); release checklist item "embed latest root"; code signing before public beta (spec) | updater + devops |
| GAP-13 | P1 | Sybil/spam: identity proof-of-work (spec) and local spam rules not built | PoW stamp bound to the identity key, checked at `Join`; per-author rate limits at accept time (local, per spec); invite-only / approval modes | identity + authority + `core/api` |
| GAP-14 | P2 | Malicious moderator: no rate limits, no moderation log | Moderation log (spec stage 3); local limit on kicks/rotations per moderator per hour with owner alert | authority + UI |
| GAP-15 | P2 | Metadata: control log and rotation recipient lists in clear to anyone with sync access | After GAP-4, residual to members only. Later: encrypt control-log bodies to members, pad messages, blind recipient keys in rotations | authority + groupkeys |
| GAP-16 | P1 (before `--profile voice`) | TURN SSRF into owner's LAN; empty secret fallback | T1, T2 | devops (infra) |
| GAP-17 | **P0** when Tauri glue lands | Core↔UI IPC surface not defined | Minimal Tauri capabilities; no command returns words/keys except the one-time words screen; external links open via the OS with confirmation; keep CSP without `unsafe-eval` | dev-voice-shell (B1) + dev-identity |
| GAP-18 | P2 | UI shows author-claimed `ts` | Order by DAG; mark messages whose `ts` is far from receive time | UI + `core/api` |
| GAP-19 | P1 | Sync resource abuse that the limits in PR #5 don't cover: incoming connection slots squatted with free peer IDs (256 slots, idle timeout 1 h, no per-IP cap); pull amplification (~100 B request → up to 4 MiB page, 50 pulls/s globally) | Per-IP connection cap (e.g. 8) and a shorter idle timeout for peers that share no open log; a global and per-peer **byte** budget for pull responses (e.g. 8 MiB/s total), not just a request count | dev-sync |
| GAP-20 | **P0** before the first `v*` tag | Release workflow (PR #17): the core build jobs hold `contents: write` while running every dependency's build script, and restore a `rust-cache`. A malicious crate or a poisoned cache could swap release assets, which the owner then signs offline, trusting `SHA256SUMS` from the same run | Build jobs `contents: read`, hand artifacts on with `actions/upload-artifact`, and upload from one small job that runs no third-party code; no cache in release builds. ✔ in PR #32 once merged; publish also takes exactly one named file per expected artifact, and `manifest-body.mjs` refuses platforms outside `PLATFORMS` | devops |
| GAP-21 | P2 | A compromised crate or npm package can still poison its own platform's binary (the `ui` bundle is in every archive). The owner's offline signature proves CI built it, not that it's clean | Reproducible builds: a second, independent build (another runner or the owner's PC) whose hashes must match before signing; review lockfile diffs | devops |

### GAP-2 design note: bounding what a kick voids

**Problem.** To stop backdating, a kick voids the target's records that it
doesn't cover (today: control records concurrent with the kick; with GAP-2:
channel messages that aren't ancestors of the kick's channel cut). The kicker
chooses the deps and the cut, so a malicious moderator can choose them **stale**
on purpose and erase days of the target's history. The order-independent rules
can't tell an honest kick that missed a few messages from an abusive one.
Timestamps can't decide either, because they're forgeable.

**Proposed bound: witnessed records survive.** A target's record R is voided by
kick K only if both:

1. R is not an ancestor of K's deps (control) or of K's cut (channel), as today; **and**
2. no **witness** exists: an accepted record by another author (not the
   target) that has R as an ancestor through its deps.

If another member's message builds on R, R existed before that member wrote,
and the rest of the channel has seen it. A stale cut can't erase it. In an
active channel nearly everything gets witnessed within seconds, so an abusive
kick can only erase the target's last unwitnessed messages. Those are also
the ones an honest kick legitimately races with.

Details:

- **Witnesses are found by a fixpoint**, the same as authority's voiding loop. A
  witness that is itself voided (say its author was kicked concurrently) stops
  counting, so repeat until nothing changes. The result still depends only on
  the record set.
- **Scope: non-privileged records only** (posts and channel messages). For the
  target's privileged records (grants, revokes, kicks, rotations), keep
  today's rule. Otherwise a colluding ordinary member could witness a
  backdated kick and keep it alive.
- **Residual risk.** A colluding non-kicked member can witness a kicked user's
  backdated messages and keep them visible. That member could already
  repost the content under their own key, so nothing new is lost. Moderators
  can delete individual messages (spec MVP item).
- **Transparency.** Each kick shows how many of the target's records it
  voided. The moderation log (GAP-14) records the cut, so the owner can see
  and reverse an abusive kick by revoking the moderator.
- **Cost.** The incremental resolver keeps an "ancestor-of-another-author"
  flag for each target record. That's a reverse-reachability walk only when a
  kick arrives, and it's bounded by the target's unwitnessed tail.

The cut stays ≤16 channel heads, as in the GAP-2 row. The format change is
only that the kick body carries the cut. Witnessing needs no format change,
so the bound can be tuned later without breaking records. **Owner decision
needed:** accept this rule, or the simpler option the Tech Lead suggested,
where a kick voids only privileged records and older messages are hidden
from view rather than voided.

### Accepted risks (documented, not gaps)

- Kicked members keep what they already saw (spec).
- Any member can leak plaintext or keys.
- The owner's home IP is public via the bootstrap domain (spec decision 24).
  Mitigation path: a rented bootstrap/relay (any cheap VPS) when that matters.
- One update key in the closed alpha.
- A window between a kick and its arrival during which messages are still
  readable by the kicked member (groupkeys README).
- No forward secrecy within an epoch; a stolen identity reads every epoch it was
  in.
- Invite expiry is judged by the author-claimed `join.ts`, so someone holding an
  expired link can backdate a Join. The hard controls are `max_uses` and
  `InviteRevoke` (which voids concurrent Joins), and T1 revokes automatically
  at expiry. The window lasts until a moderator or the inviter comes online.
  Dropping Joins at ingest by the local clock was rejected because it breaks
  convergence.
- `seq` in records is unused: there's no equivocation detection (an author
  signing two different records with the same `seq`). Harmless today, since
  nothing relies on `seq`. Revisit with GAP-14 if per-author ordering or
  rate limiting starts to use it.

## How to keep this current

- Every PR touching `core/` or `infra/` goes through
  [`PR-CHECKLIST.md`](PR-CHECKLIST.md). If the answer to a question there is
  "no", the PR either fixes it or adds/updates a GAP here.
- When a GAP is fixed, mark it ✔ in the table with the PR number. Don't delete
  rows.
- Re-review this document when a new crate, record kind, network protocol or
  helper service is added.

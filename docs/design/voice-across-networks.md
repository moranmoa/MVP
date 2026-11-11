# Design note: voice calls across networks (G-81)

Status: Security Lead approved (option A, with conditions); Tech Lead review pending. Author: dev-voice-shell (voice engine). No code yet. `infra/**` is devops'; this note only says what it needs.

## What fails today
Voice v1 officially works only on one network. Two people on different networks usually can't connect.

- **No STUN in release builds.**
  - The path exists end to end: `mvs-config.json` `stun` → `desktop/src/config.js` `stunFor` → `core.status.stunUrl` → `sanitize` → `useVoiceEngine` → `iceServersFor`.
  - But the release variable `MVS_STUN_URL` is empty, so builds ship without one.
  - So each side offers only **host** candidates. The shell's `WEBRTC_IP_POLICY` also limits these to the default-route interface. Behind a home NAT, those are private addresses the other side can't reach.
  - It worked "once across networks" most likely because one side was directly reachable (a public IP, or a forwarded/full-cone mapping). The other side then learned a peer-reflexive candidate from the incoming checks. That isn't something to rely on.
- **Our STUN server isn't usable yet.**
  - `infra` has `--profile stun` (coturn `--stun-only`, no logs).
  - Its answers are only right once the host sees real source addresses. That's #220: the relay moves to Proxmox with host networking.
- **No TURN.**
  - STUN alone fails when either side is behind a symmetric NAT or CGNAT, or on a network that blocks UDP. That's common on mobile hotspots, some ISPs, and offices.
  - The `voice` profile (coturn with TURN) exists but is not hardened. GAP-16 covers this: T1 TURN SSRF into the host's LAN, and T2 an empty-secret fallback.
- **Relay-only mode turns voice off** (`relay-only`). That stays the same here; see "Later".

## Plan
### 1. STUN: our own server, nothing third-party
- **Privacy:** no public STUN, Google's or anyone else's. A STUN server sees every caller's public IP and when they call. Ours keeps no logs (GAP-7, `--no-stdout-log --log-file=/dev/null`).
- **Devops, after #220:**
  - run `--profile stun` on the relay host (3478/udp);
  - set `MVS_STUN_URL=stun:<relay-host>:3478` for release builds. The value comes from the release variable and is never committed.
- **Code:** none needed; the path above already carries it.
- **Result:** most pairs behind ordinary home NATs connect with server-reflexive (srflx) candidates.

### 2. TURN: a relay hosted with our relay, after #220
- coturn on the same host as the relay, in the `voice` profile, hardened first (GAP-16).
- **Network hardening:**
  - `--denied-peer-ip` for every private, loopback, link-local and CGNAT range (T1), plus `--no-multicast-peers`;
  - the secret is required (`${TURN_SECRET:?}`) and `change-me` is refused (T2);
  - `--total-quota`, `--user-quota`, `--max-bps` (Opus is about 40 kbit/s per stream, so a cap of a few hundred kbit/s per allocation is plenty);
  - no logs (GAP-7), a read-only root, and `cap_drop: ALL` like the `stun` service.
- **Listeners:**
  - `turn:` on 3478 udp+tcp;
  - `turns:` (TLS) on 5349 for networks that block UDP;
  - 443 with SNI routing later, if 5349 turns out to be blocked too. That's devops' choice, since 443 is already the reverse proxy's.
- **Relay ports:** a small UDP range (today 49160–49200, about 40 concurrent allocations). Devops sizes it.
- **What TURN sees:**
  - both callers' public IPs, and the volume and timing of a call;
  - **not** the audio. Media is SRTP inside DTLS, end to end. The engine's fingerprint check (#231: signed SDP fingerprint = presented certificate) is unchanged, so a TURN server can't impersonate a peer.

#### Credentials: short-lived and gated
coturn's REST-API scheme (`--use-auth-secret`) works like this:
- `username = "<expiry-unix>:<random>"`;
- `credential = base64(HMAC-SHA1(secret, username))`.

The secret lives **only on the relay host** (an env file or Docker secret). It is never in git, client builds or the core.

Who mints them: a small handler on the relay node, reached over the libp2p connection the core already has to it. The new protocol is `/mvs/turn/1`, request-response.

- **The request:** `{}` from a Noise-authenticated peer.
- **The answer:** `{ urls, username, credential, ttl }`, where `urls` is the TURN URLs of *this* host.
- **TTL:** 1 h.
- **Rate limit:** per peer ID, for example 4 answers per 10 min, and refused while the peer has no relay reservation.

Gating: the manager asked for credentials that are member-gated. A server-agnostic relay doesn't know server membership, so there are two options. **This is the decision we need from the Security Lead.**
- **A (recommended for the first release): identity-gated.**
  - Only a Noise-authenticated MVS node gets credentials, rate-limited and expiring.
  - The real gate is the core's signalling. A TURN allocation only carries traffic to the addresses coturn has *permissions* for, and those are peer addresses learned from member-only, signed `call.signal`.
  - What an abuser gets is a capped, short-lived relay that can't reach private ranges. That's bounded by quotas, `max-bps` and `denied-peer-ip`.
- **B (later, if A is not enough): membership tickets.**
  - The requester's core attaches a ticket signed by a call participant: `mvs/turn-ticket/v1 { server, channel, requester, expiry }`.
  - The relay can check the signature, but not that the signer is a member, without the server's control log. That's a GAP-4-sized change, so B is a follow-up.

**Decision (Security Lead, on #333):** option A for the alpha, B later. A is effectively a bounded open relay, so these conditions are required:
1. Credentials go only to peers that hold a relay reservation, rate-limited **per peer ID and per IP**.
2. The username is `<expiry>:<random>` and **never** the peer ID or identity.
3. Hard quotas: `--user-quota`, `--total-quota` and `--max-bps`. Allocation lifetime is no longer than the credential TTL.
4. `denied-peer-ip` also covers `0.0.0.0/8`, `fc00::/7`, `fe80::/10` and **the relay host's own public IPs** (hairpin). N7 tests each one.
5. The T2 secret rules: the secret is required and `change-me` is refused.

**Long calls:** coturn checks the timestamp when an allocation is made and when it is refreshed. A call longer than the TTL needs new credentials. The engine fetches new ones at about 80% of the TTL, then `setConfiguration`, then an ICE restart by the offerer. To verify in step 4: if refreshing an existing allocation with the old username is enough, the restart isn't needed.

### 3. What changes
| Where | Change | Owner |
| --- | --- | --- |
| Relay host (`infra/`) | `stun` profile live after #220; TURN hardened (GAP-16); the `/mvs/turn/1` handler with the secret from env | devops |
| sync (`core/sync`) | the client side of `/mvs/turn/1` to the configured relay | dev-sync |
| api (`core/api`) | `getTurnCredentials(): { urls, username, credential, ttl }`. Errors `relay-only`, `not-implemented`, `rate-limited`. Callable only during a call | dev-identity |
| UI core (`ui/src/core`) | the command, plus `sanitize` (see below) | dev-voice-shell (allow-list: desktop owner) |
| Voice engine (`ui/src/voice`) | see below | dev-voice-shell |
| Desktop config | none for TURN: the URLs come from the relay. `stun` unchanged | – |
| Release (`.github`) | set `MVS_STUN_URL` | devops / owner |

`sanitize` rules for the credentials:
- `urls` must be `turn:` or `turns:` URLs whose host **exactly matches** the configured relay host (taken from the `relay` multiaddr's `/dns4/`). That way a confused or compromised answer can't send media through a third party.
- The port must be on an allow-list (3478, 5349, and 443 once that exists).
- The only query parameter allowed is `transport=udp|tcp`.
- `username` and `credential` are at most 256 chars each.
- `ttl` is between 60 and 86 400 s.
- Anything else drops the TURN entry, and the call goes on with STUN only.

Voice engine changes:
- `iceServersFor(stunUrl, turn)` gives `[{ urls: stun }, { urls: turn.urls, username, credential }]`. Each part is optional, and there is still no default.
- On join, fetch the credentials before the first `RTCPeerConnection`. If that fails, go on without TURN (no wait longer than 2 s).
- Refresh at 80% of the TTL, as described under "Long calls".
- **Logging:** credentials are never logged, and never put in errors or diagnostics.
- **Diagnostics:** report the selected candidate pair's *types* (`host` / `srflx` / `relay`), never the addresses. QA and support can then tell "connected through TURN" from "direct".
- **Failures:** `peerFailed` stays as it is. With TURN in place, a `failed` should be rare, and it means "this network blocks calls".

### 4. Steps
1. **This note:** reviews from the Security Lead (gating, option A or B) and the Tech Lead.
2. **Devops:** #220, then STUN live and `MVS_STUN_URL` set. No client code; the most common case starts working.
3. **Devops:** TURN hardened (GAP-16), not reachable from outside until step 5's tests pass.
4. **dev-sync + dev-identity:** `/mvs/turn/1` and `getTurnCredentials`, each with tests.
5. **Me:** the engine and UI-core changes above, with unit tests (fake ICE servers, refresh, sanitize).
6. **QA (with me):** the two-network e2e below, then a real-world check on two different ISPs (owner).

## Test plan: two Docker networks
This extends QA's `qa/voice-e2e` (#276) with a new compose file.
- **Networks:**
  - `net-a` and `net-b` are the callers' "homes". Each sits behind its own NAT container (iptables `MASQUERADE`).
  - `wan` holds the relay, coturn (STUN and TURN) and the two NAT routers. No container has a route to the real internet.
- **The apps:** a and b run as today: the real core, the desktop shell, and the fake microphone.
- **Checks:** each case checks connected, the presented certificate equals the signed fingerprint, RTP bytes growing both ways, and the selected pair's candidate types.

| # | Setup | Expected |
| --- | --- | --- |
| N1 | Both NATs, no STUN, no TURN | No connection, and both see `peerFailed: failed` (proves the NATs isolate) |
| N2 | Both NATs endpoint-independent, STUN only | Connected over `srflx ↔ srflx`, audio both ways |
| N3 | One NAT symmetric (`MASQUERADE --random-fully`), STUN only | No connection |
| N4 | N3 plus TURN | Connected with `relay` on at least one side, audio both ways, fingerprint check passes |
| N5 | `net-b` blocks UDP out, TURN | Connected over TURN-TCP / `turns:` |
| N6 | Wrong or expired credentials | No relay candidates; falls back to STUN; no crash, no credentials in logs |
| N7 | A client allocates and tries to relay to each denied range: private, loopback, link-local, CGNAT, `0.0.0.0/8`, `fc00::/7`, `fe80::/10`, and the relay host's own public IP (hairpin) | Each one refused (T1) |
| N8 | `/mvs/turn/1` rate limit (5 requests in 10 min), per peer ID and per IP; a peer without a reservation | 5th refused, `rate-limited`; no reservation refused |
| N8b | Credentials | The username is `<expiry>:<random>` with no peer ID; an allocation outlives neither the TTL nor the quotas |
| N9 | Relay-only on | Join refused `relay-only`, no TURN request |
| N10 | Privacy | coturn writes no logs. There are no DNS lookups or connections outside the compose networks. Credentials appear in no app or core log: an automated check greps `mvs.log` and the shell logs for the username and credential |
| N11 | Long call with a short TTL (2 min in the test) | Audio continues across the refresh |

## Later (not G-81)
- **Voice in relay-only mode:** with TURN, `iceTransportPolicy: 'relay'` hides the user's IP from the other callers. It could make voice possible in relay-only mode, but that is its own decision with the Security Lead.
- **A second STUN/TURN host for redundancy.**

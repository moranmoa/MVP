# Direct connections across NAT (DCUtR): design note

**Status: proposal, for the security lead and the tech lead.** It follows
F2 (#183, a direct connection on one LAN). Today two users behind
different NATs talk only through relay circuits. The circuits work, but
every byte goes through the owner's relay (R11 caps it, GAP-7 logs it).

**What it takes.** libp2p's DCUtR ("Direct Connection Upgrade through
Relay"): once two peers are connected through a circuit, they swap the
addresses they were observed at and dial each other at the same moment.
That opens a hole in both NATs, after which the relayed connection is
replaced (our `track_connection` already closes it).
- **Pieces:** the `dcutr` behaviour; identify's *observed* address, plus
  AutoNAT or a confirmation step so that only addresses that really work
  are offered; and a transport that can punch.
- **TCP with port reuse** (`tcp::Config::port_reuse(true)`) keeps one
  transport, and works behind most home NATs. Simultaneous TCP open is
  less reliable.
- **QUIC** punches far more reliably (UDP) and is libp2p's recommended
  path. It adds a transport, and with it a second listener and port. The
  relay would need QUIC too only if we wanted relay connections over it;
  for punching, the peers need it, not the relay.
- **Recommendation:** QUIC for punching, with TCP and wss kept for the
  relay. About 2–4 days, then a real-network run with qa (B5) across two
  ISPs. Symmetric NAT and CGNAT won't punch; those stay relayed.

**Security and privacy.**
- **Who learns my public IP.** Hole punching shows each side's public
  IP and port to the other. Only do it with **bound members**: the same
  rule as F2, a GAP-4 binding on a shared server, and not kicked
  (ReadGate). Never with strangers, or holders of a leaked link who
  haven't bound.
- **The DHT.** Observed or public addresses never go into it (GAP-6).
  DCUtR exchanges addresses only over the existing relayed connection.
- **"Relay only" (GAP-6 P1).** A user setting, default off for now and
  on for servers in the public index, per the spec. With it on, the node
  doesn't take part in DCUtR (neither starts nor answers), doesn't send
  observed or listen addresses to peers, and never upgrades. The IP stays
  hidden from members, at the cost of the relay.
- **Abuse.** DCUtR makes us dial addresses a peer sends: cap them (a
  few per attempt, global addresses only), rate-limit attempts per peer,
  and back off on failure (as for DHT dials, #158).

**Questions for the security lead.**
1. Is bound and not kicked the right bar for revealing our public IP to a
   peer, or should it be the owner's choice per server?
2. "Relay only": a per-user setting, a per-server setting, or both?

## The security lead's checklist (#184)

1. **Who learns whose IP.**
   - **Through the relay** (today): nobody learns a member's IP, only the
     relay does.
   - **On a LAN** (F2, #183): only a peer *bound* on a shared server gets
     dialled at a LAN address, and only after it reported it to us over the
     existing connection.
   - **Across NAT:** the same bar, a bound member of a shared server who
     isn't kicked. DCUtR is never started toward, or answered for, an
     unbound peer, a relay or DHT neighbour, or a leaked-link holder who
     hasn't bound.
2. **"Relay only".** A per-user setting (per-server can come later). It
   turns off DCUtR in both directions, the direct-address upgrade (F2),
   and sending listen or observed addresses. Default for the closed alpha:
   off, as the owner wants direct connections; the alpha invite says so
   (point 6). Cost: every message makes two hops through the relay. That's
   tens of ms more on the same continent, and bounded by R11 (8 circuits,
   32 MiB, 1 h).
3. **mDNS.** Not used, and not planned. F2 learns LAN addresses from the
   peer itself over the relayed connection (identify), not by broadcasting
   on the LAN, so nothing about us is published on the local network.
4. **Address filtering.** Observed addresses (identify or AutoNAT) go
   through the same filters as listen addresses: global only, private ones
   only under `allow_lan` and only toward bound peers. They never go into
   the DHT. DCUtR dials only addresses the peer sent in the DCUtR exchange
   over the authenticated connection, filtered the same way.
5. **Resources.**
   - At most 4 addresses per punch attempt.
   - At most 3 attempts per peer per hour, backed off like DHT dials
     (#158).
   - At most 8 punches in flight in total.
   - A failed punch leaves the relayed connection in place.
6. **The alpha invite** gets a line: "Members of a server you're in can
   see your IP address, unless you turn on Relay only."

## Build plan: member-gated hole punching

**The problem.** libp2p's `dcutr::Behaviour` (0.12, in libp2p 0.54)
creates its handler for every relayed connection the moment it is
established. The side that *accepted* the relayed connection starts the
handshake at once, and sends its observed and external addresses. At
that moment we don't know who the peer is: the GAP-4 binding arrives with
the first pull, and the ReadGate check runs after it.

**Approach A: a gating wrapper (preferred), like `GlobalIdentify` (#190).**
`MemberDcutr` wraps `dcutr::Behaviour` and implements `NetworkBehaviour`.
1. In `handle_established_{inbound,outbound}_connection`, for a relayed
   connection, return
   `Either::Right(dummy::ConnectionHandler)`, so no DCUtR on it yet. Any
   other connection gets `Either::Right` too: DCUtR only runs on relayed
   ones.
2. Once the node marks the peer a member (binding verified and
   `is_member`, the F2 check), and relay-only is off, the wrapper opens a
   **second relayed connection** to that peer, dialling through our relay
   with `PeerCondition::Always`. For that connection it hands out
   `Either::Left(dcutr handler)`. The wrapper remembers which peers are
   "cleared", and only for those does it create the real handler. DCUtR
   then runs on the new relayed connection, between two members.
3. Success gives a direct connection: `track_connection` closes the
   relayed ones and emits `NodeEvent::Link { direct: true }` (#187).
   Failure leaves the existing relayed connection in place.
4. **Caps** (#184): one cleared attempt per peer at a time, at most 3 per
   peer per hour with `DialBackoff`, and at most 8 in flight overall. The
   addresses DCUtR dials go through `handle_pending_outbound_connection` in
   the wrapper, which keeps at most 4 that are `reachable(.., false)`.
   Private ones never leave through DCUtR.
5. **Observed addresses.** DCUtR sends the swarm's external addresses.
   Those come from AutoNAT through the relay only (#184 amendment 2), or
   from identify's observed address once confirmed. They are filtered the
   same way: global only, and never into the DHT.

The cost is one extra circuit per member pair, short-lived. Hooks used:
the `NetworkBehaviour` connection-establishment methods,
`handle_pending_outbound_connection`, and `Either` with
`dummy::ConnectionHandler`. No fork of libp2p.

**Approach B: our own coordination (fallback, if A misbehaves).** Drop
`dcutr`, and punch over the members-only path we already have.
1. Pull answers to a member already carry `direct_addrs` (#190). Add our
   confirmed *observed* global addresses to them.
2. A small request-response protocol, `/mvs/punch/1.0.0`, on the existing
   relayed connection, members only. A sends `Connect { addrs }`. B replies
   with its addrs and A measures the round trip. Then A dials B's addrs
   after RTT/2 while B dials A's at once: DCUtR's synchronisation, done by
   hand. Both sides dial with `PeerCondition::Always`, and for TCP from
   the listening port (`port_reuse`).
3. The same caps and filters as A. More code, roughly a day more than A,
   but full control over who gets what and when.

**Transport.** QUIC (`libp2p::quic`, already in our libp2p version) next
to TCP, for punching; it is far more reliable through home NATs. Relay
and wss stay on TCP. If QUIC is blocked (some corporate networks), TCP
with `port_reuse(true)` is the fallback.

**Tests.**
- In-process: the wrapper never creates a DCUtR handler before membership,
  and creates one after. A non-member, or relay-only, gets none.
- Caps: 3 attempts per hour, 4 addresses each.
- Real network with qa (B5): home network plus a mobile hotspot. Measure
  how often punching succeeds, and the time to direct.

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

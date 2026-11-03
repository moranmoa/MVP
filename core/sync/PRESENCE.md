# Presence (M2 gate): design note

**Status: proposal, for the tech lead's OK before building.** Requirements:
B18-p2p-states §5 and §8, test 8. A peer shows as "online" within 5 s once
it is active, and as "offline" within 30 s after it closes the app.
`members.changed` fires on every change, and the server's `reachability`
(`connecting` / `online` / `nobody-online`) and `lastSeenAt` follow it.

**Who is online comes from the GAP-4 bindings, nothing new on the wire.**
- Every node already pulls each open log from each connected peer every
  5 s, and on connect at once. On a server opened with its secret, every
  pull carries the requester's binding: an identity signature over its
  peer ID and the server's control handle, checked against the Noise
  remote.
- Pull answers will carry the answerer's binding on those servers too (not
  only when they carry acks), so both sides learn who the other is. That
  is about 150 bytes per answer.
- Sync keeps, per server, the identities seen through a verified binding
  in the last `PRESENCE_TTL` = 15 s, each with its connected peer IDs and
  when it was last seen.
- An identity goes **online** with its first verified binding, at most 5 s
  after connecting; usually at once, since a pull runs on connect.
- It goes **offline** when its last connection closes, or when nothing has
  refreshed it for 15 s. That covers an app killed without closing its TCP
  connection: pulls stop, and it goes offline within 15–20 s, under the
  30 s target.
- Our own identity is never listed. A peer bound to two servers counts in
  each one separately.

**What api gets, and the privacy line.**
- A new `NodeEvent::Presence { server, identity, online }`, emitted on
  changes only.
- `Node::presence(server) -> Presence { online: Vec<PublicKey>, last_seen:
  Option<SystemTime>, reachability }`. Reachability is `Connecting` until
  the first relay reservation or bootstrap connection (or 10 s), then
  `Online` while anyone is online, else `NobodyOnline`. `last_seen` is the
  last time anyone was.
- Sync reports *bound identities*, which includes holders of the secret
  who aren't members. api filters them through the same authority state as
  its `ReadGate` (a member, not kicked) before `members.changed`, so a
  stranger holding a leaked link never shows in the member list.
- Presence is only ever learnt from, and shown to, peers that know the
  server secret: a binding goes only to a peer that answered on the handle.
  The relay and DHT neighbours learn nothing new.
- Presence is never stored or synced, and is not part of authority.
- Servers opened by plain name have no bindings, so no presence; api is
  switching to `open_server` anyway (the GAP-4 follow-up).

**Tests.**
- Unit: TTL expiry, multiple connections per identity, events only on
  change, our own identity excluded.
- End to end, 3 nodes:
  - B shows online on A within 5 s of connecting;
  - after B shuts down, offline within 30 s;
  - after B drops all traffic without closing (a killed process, simulated
    by blackholing its pulls), offline within 30 s;
  - a bound non-member is reported, so the filtering is api's, and that is
    documented.
- A harness check, if cheap: presence settles on all 4 peers.

**Out of scope:** typing indicators and per-device presence.

## Additions (tech lead, #141)

1. **The answerer's binding only goes to members.** A pull answer carries
   our binding only if the requester's bound identity passes our
   `ReadGate` (a member, not kicked). That check runs on the answering
   thread, like a channel pull's. So a holder of a leaked link, or a kicked
   member, can still pull the control log, but can't learn who is online.
   Tested: a bound non-member gets answers with no binding, and never sees
   the answerer online.
2. **`NobodyOnline` means "nobody reachable from here".** Others may be
   online but out of reach from this device (NAT, relay down). The UI copy
   (`rail.serverNobodyOnline`) should not promise more.
3. **Bounded.** At most `MAX_PRESENCE` = 256 identities per server. A new
   identity past the cap is not tracked until an entry expires; the bound
   is tested.

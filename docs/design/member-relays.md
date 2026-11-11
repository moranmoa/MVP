# Member relays, relay fallback, and the email sign-in second holder

- **Status:** proposal, Security Lead (2026-11-11), for the Tech Lead and the owner.
- **Owner decision:** members can relay for each other. It's opt-in, but on by default for server owners and admins. Email sign-in should use it if possible. If our relay is down, traffic must find other ways.

## 1. Member relays

**What it is:** libp2p circuit relay v2 running in a member's MVS node, used only by co-members. Traffic stays Noise end-to-end, so the relaying member never sees content. DCUtR still upgrades a relayed link to a direct one where it can.

**Who relays:**
- **Owners and admins:** on by default; they can switch it off in Settings.
- **Everyone else:** off; opt in from Settings.

**Who never relays, whatever their role:**
- users under 18 (from the age claim);
- relay-only users;
- a device on a metered connection, when the OS reports one.

**Rules (all required):**
1. **Members only.** A reservation or circuit is granted only to a peer that is a *bound current member* of a server we share (the same `member_identity` gate as DCUtR and voice), and that check runs **before** the reservation is accepted. A stranger gets a plain refusal.
2. **Never advertised publicly.** Member relays aren't put in the public DHT and aren't announced to non-members. Members learn of them through the member-only sync path (as F2 LAN addresses are today).
3. **Quotas.** At most 8 reservations and 16 circuits, about 256 kbit/s per circuit and 2 Mbit/s in total, with a 2-minute circuit lifetime (renewable) and a daily data cap (for example 2 GB). When a limit is hit, refuse; never queue.
4. **Global off switch.** A signed flag in the update manifest turns member relaying off everywhere, in case of abuse.
5. **No metadata kept.** Only counts are logged, never peer IDs or IPs.

**What a relaying member learns:** the IP addresses and peer IDs of the members it carries, plus when and how much they talk. It never sees content. It can drop or delay traffic, so other paths stay preferred (§2).

**The extra risk of on-by-default for owners and admins.** Their home IP becomes reachable by every member of their servers. Their connection carries other members' traffic and can be loaded up to the quotas, and they see their members' IPs. An admin who is also a minor never relays (the age rule wins).

**The disclosure they see** (once, when they become an owner or admin; also in Settings):
> "Relay for your members is ON. Members of your servers can connect through this computer when they can't reach each other directly. They'll be able to see this computer's IP address, and you'll see theirs. Nothing they send is readable by you. This uses up to [2 GB] a day. You can turn it off in Settings → Privacy."

**GAP-4b and T23.** Only bound members can use a member relay, and they already know each other through the binding, so this adds no membership leak. The member gate runs before any reservation.

## 2. Fallback when our relay is down

**Order of paths to a co-member** (the first one that works wins, and is re-checked as described below):
1. **A direct link** (an existing DCUtR or LAN connection).
2. **Our relay** (the bootstrap), if healthy.
3. **A second bootstrap** (after #220 and GAP-9), if configured.
4. **A member relay** of a server we share: owner or admin relays first, then opted-in members; the one with the lowest RTT first.

**Health checks:**
- Ping every relay we hold a reservation on every 30 s. Three misses mark it down for 2 minutes with backoff, and we move to the next path.
- Re-probe the higher-priority paths every 2 minutes, and move back when one is healthy.
- With every path down, the node keeps retrying and shows "offline".
- Relay-only users only ever use paths 2–4, never 1.

## 3. Email sign-in: why member relays can't hold the OPRF or recovery shares

**Why not:**
- The OPRF threshold and the recovery split only protect anyone if the holders are **independent operators** that stay **always on**.
- A member relay is a member of the same servers and comes and goes. A few colluding (or compromised) owners and admins would hold enough shares to guess any password offline, or to take over accounts with recovery on.
- It also isn't online when someone needs to sign in.

**Options the owner can choose from, closest first:**
- **A. A second operator-run bootstrap (recommended).** After #220, a second server **run by a different person or organisation**, holding the second OPRF share and a recovery share. This is what email sign-in actually needs. Until it exists, the feature stays behind its flag, as already approved.
- **B. The user's own devices as extra holders** of their *own* sealed envelopes and backup (already in place via #339). This helps availability but doesn't replace the OPRF shares.
- **C. Owner and admin relays as extra mailbox holders** for the **encrypted** password envelope and backup blobs, never as OPRF or recovery share holders. They add copies, so sign-in still works if our relay's mailbox is down, without adding trust. The same caps as the backup store apply (64 KiB, rate limits, exact-tag lookup, a constant not-found).

**Recommendation:** C now, together with relaying, for availability. A for email sign-in itself. Email sign-in can't go live with only one operator, whatever else is added.

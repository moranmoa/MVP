//! Who is online, per server (M2, design in `PRESENCE.md`): identities seen
//! through a verified GAP-4 binding in the last [`TTL`], with the peers
//! they're connected on. Never stored or synced; not used for authority.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use records::{PublicKey, ServerId};

/// Without a fresh binding (pulls run every 5 s) an identity goes offline
/// after this long: covers an app killed without closing its connections.
pub(crate) const TTL: Duration = Duration::from_secs(15);
/// Identities tracked per server.
pub(crate) const MAX_PER_SERVER: usize = 256;

struct Seen {
    peers: HashSet<PeerId>,
    last: Instant,
}

#[derive(Default)]
pub(crate) struct Presence {
    servers: HashMap<ServerId, HashMap<PublicKey, Seen>>,
}

impl Presence {
    /// `identity` proved itself on `peer` for `server` just now. True if it
    /// just came online.
    pub fn seen(&mut self, server: ServerId, identity: PublicKey, peer: PeerId, now: Instant) -> bool {
        let online = self.servers.entry(server).or_default();
        if let Some(seen) = online.get_mut(&identity) {
            seen.peers.insert(peer);
            seen.last = now;
            return false;
        }
        if online.len() >= MAX_PER_SERVER {
            return false;
        }
        online.insert(identity, Seen { peers: HashSet::from([peer]), last: now });
        true
    }

    /// `peer` has no connection left: identities seen only on it go offline.
    pub fn disconnected(&mut self, peer: &PeerId) -> Vec<(ServerId, PublicKey)> {
        self.remove_where(|seen| {
            seen.peers.remove(peer);
            seen.peers.is_empty()
        })
    }

    /// Identities not refreshed for [`TTL`] go offline.
    pub fn expire(&mut self, now: Instant) -> Vec<(ServerId, PublicKey)> {
        self.remove_where(|seen| now.duration_since(seen.last) >= TTL)
    }

    /// The online identities, each with the peers it was seen on.
    pub fn peers(&self, server: &ServerId) -> Vec<(PublicKey, Vec<PeerId>)> {
        let Some(m) = self.servers.get(server) else { return Vec::new() };
        m.iter().map(|(k, s)| (*k, s.peers.iter().copied().collect())).collect()
    }

    pub fn online(&self, server: &ServerId) -> Vec<PublicKey> {
        self.servers.get(server).map(|m| m.keys().copied().collect()).unwrap_or_default()
    }

    fn remove_where(&mut self, mut gone: impl FnMut(&mut Seen) -> bool) -> Vec<(ServerId, PublicKey)> {
        let mut out = Vec::new();
        for (server, online) in &mut self.servers {
            online.retain(|identity, seen| {
                let keep = !gone(seen);
                if !keep {
                    out.push((*server, *identity));
                }
                keep
            });
        }
        self.servers.retain(|_, m| !m.is_empty());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> PublicKey {
        PublicKey::from_bytes([n; 32])
    }

    #[test]
    fn online_on_first_binding_offline_on_last_disconnect_or_silence() {
        let (s, t) = (ServerId::from_bytes([1; 32]), ServerId::from_bytes([2; 32]));
        let (p, q) = (PeerId::random(), PeerId::random());
        let now = Instant::now();
        let mut presence = Presence::default();
        assert!(presence.seen(s, key(1), p, now));
        assert!(!presence.seen(s, key(1), q, now)); // a second device: no new event
        assert!(presence.seen(t, key(1), p, now)); // per server
        assert_eq!(presence.disconnected(&p), vec![(t, key(1))]);
        assert_eq!(presence.online(&s), vec![key(1)]); // still on q
        assert_eq!(presence.disconnected(&q), vec![(s, key(1))]);
        assert!(presence.online(&s).is_empty());

        // Silent: no refresh for TTL.
        presence.seen(s, key(2), p, now);
        assert!(presence.expire(now + TTL - Duration::from_secs(1)).is_empty());
        presence.seen(s, key(2), p, now + Duration::from_secs(10));
        assert!(presence.expire(now + TTL).is_empty()); // refreshed
        assert_eq!(presence.expire(now + Duration::from_secs(10) + TTL), vec![(s, key(2))]);
    }

    #[test]
    fn presence_is_capped_per_server() {
        let s = ServerId::from_bytes([1; 32]);
        let now = Instant::now();
        let mut presence = Presence::default();
        for i in 0..MAX_PER_SERVER + 10 {
            let mut k = [0; 32];
            k[..8].copy_from_slice(&(i as u64).to_le_bytes());
            presence.seen(s, PublicKey::from_bytes(k), PeerId::random(), now);
        }
        assert_eq!(presence.online(&s).len(), MAX_PER_SERVER);
    }
}

//! Bounded in-memory state that remote peers can make grow: pushed records
//! waiting for their deps, and request rates.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::{Record, RecordId};

/// Pushed records waiting for a dep, keyed by that dep. Capped by count and
/// by bytes, and entries expire: anything dropped here is fetched by the next
/// pull anyway, so this is only a shortcut.
pub(crate) struct Pending {
    by_dep: HashMap<RecordId, Vec<Waiting>>,
    /// Ids of every parked record: copies of one record are kept once.
    ids: HashSet<RecordId>,
    count: usize,
    bytes: usize,
    max_count: usize,
    max_bytes: usize,
}

pub(crate) struct Waiting {
    pub record: Record,
    /// Encoded size plus bookkeeping, counted against `max_bytes`.
    cost: usize,
    since: Instant,
}

impl Pending {
    pub fn new(max_count: usize, max_bytes: usize) -> Self {
        Self { by_dep: HashMap::new(), ids: HashSet::new(), count: 0, bytes: 0, max_count, max_bytes }
    }

    /// Keeps `record` until `missing` arrives. Returns false (and drops it)
    /// when full.
    pub fn insert(&mut self, missing: RecordId, record: Record, since: Instant) -> bool {
        let cost = record.encode().len() + 128;
        self.park(missing, Waiting { record, cost, since })
    }

    fn park(&mut self, missing: RecordId, waiting: Waiting) -> bool {
        if self.ids.contains(&waiting.record.id()) {
            return true;
        }
        if self.count >= self.max_count || self.bytes + waiting.cost > self.max_bytes {
            return false;
        }
        self.count += 1;
        self.bytes += waiting.cost;
        self.ids.insert(waiting.record.id());
        self.by_dep.entry(missing).or_default().push(waiting);
        true
    }

    /// Removes and returns the records that were waiting for `dep`.
    pub fn take(&mut self, dep: &RecordId) -> Vec<Waiting> {
        let waiting = self.by_dep.remove(dep).unwrap_or_default();
        for w in &waiting {
            self.count -= 1;
            self.bytes -= w.cost;
            self.ids.remove(&w.record.id());
        }
        waiting
    }

    /// Puts back a record from [`Pending::take`] that still lacks a dep.
    /// `since` is kept from the first time it was parked, so a record that
    /// keeps finding another missing dep still expires.
    pub fn reinsert(&mut self, missing: RecordId, waiting: Waiting) -> bool {
        self.park(missing, waiting)
    }

    /// Drops everything parked before `now - ttl`.
    pub fn expire(&mut self, now: Instant, ttl: Duration) {
        let (mut count, mut bytes, mut ids) = (0, 0, HashSet::new());
        self.by_dep.retain(|_, waiting| {
            waiting.retain(|w| now.saturating_duration_since(w.since) < ttl);
            for w in waiting.iter() {
                count += 1;
                bytes += w.cost;
                ids.insert(w.record.id());
            }
            !waiting.is_empty()
        });
        self.count = count;
        self.bytes = bytes;
        self.ids = ids;
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.count
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}


/// Channel records this node refused, remembered so a pull can list them as
/// "have" and peers stop sending them every round. A record is refused only
/// once all its deps are stored (or are refused themselves), so claiming it
/// never hides an ancestor we still need. Bounded: the oldest are forgotten
/// first, and forgetting one only costs a re-send.
pub(crate) struct Refused {
    order: VecDeque<(String, RecordId)>,
    ids: HashSet<RecordId>,
    max: usize,
}

impl Refused {
    pub fn new(max: usize) -> Self {
        Self { order: VecDeque::new(), ids: HashSet::new(), max }
    }

    pub fn insert(&mut self, log: String, id: RecordId) {
        if !self.ids.insert(id) {
            return;
        }
        self.order.push_back((log, id));
        if self.order.len() > self.max {
            if let Some((_, old)) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }

    /// Forgets everything refused in logs starting with `prefix` (a
    /// server's logs): a new control record may have made them acceptable,
    /// so they should be pulled and judged again.
    pub fn forget_prefix(&mut self, prefix: &str) {
        let ids = &mut self.ids;
        self.order.retain(|(log, id)| {
            let keep = !log.starts_with(prefix);
            if !keep {
                ids.remove(id);
            }
            keep
        });
    }

    pub fn contains(&self, id: &RecordId) -> bool {
        self.ids.contains(id)
    }

    /// Whether every id in `missing` was refused: a record that only waits
    /// for refused records can never be stored either.
    pub fn covers(&self, missing: &[RecordId]) -> bool {
        !missing.is_empty() && missing.iter().all(|id| self.ids.contains(id))
    }

    /// The newest `n` refused ids of `log`, to send as `have` markers.
    pub fn markers(&self, log: &str, n: usize) -> Vec<RecordId> {
        self.order.iter().rev().filter(|(l, _)| l == log).map(|(_, id)| *id).take(n).collect()
    }
}

/// Direct incoming connections per source address. IPv6 is counted per /64
/// (one ordinary customer prefix holds 2^64 addresses), IPv4 per address;
/// an IPv4-mapped IPv6 address counts as the IPv4 one.
pub(crate) struct PerIp {
    counts: HashMap<std::net::IpAddr, usize>,
    by_connection: HashMap<libp2p::swarm::ConnectionId, std::net::IpAddr>,
    max: usize,
}

/// The address a connection from `ip` is counted under.
pub(crate) fn ip_bucket(ip: std::net::IpAddr) -> std::net::IpAddr {
    use std::net::{IpAddr, Ipv6Addr};
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

impl PerIp {
    pub fn new(max: usize) -> Self {
        Self { counts: HashMap::new(), by_connection: HashMap::new(), max }
    }

    /// Counts a new connection from `ip`; false (and not counted) if its
    /// bucket is already full.
    pub fn admit(&mut self, id: libp2p::swarm::ConnectionId, ip: std::net::IpAddr) -> bool {
        let bucket = ip_bucket(ip);
        let n = self.counts.entry(bucket).or_default();
        if *n >= self.max {
            return false;
        }
        *n += 1;
        self.by_connection.insert(id, bucket);
        true
    }

    pub fn release(&mut self, id: libp2p::swarm::ConnectionId) {
        let Some(bucket) = self.by_connection.remove(&id) else { return };
        if let Some(n) = self.counts.get_mut(&bucket) {
            *n -= 1;
            if *n == 0 {
                self.counts.remove(&bucket);
            }
        }
    }
}

/// Token bucket per key: `burst` requests at once, refilled at `per_sec`.
///
/// A key's bucket is kept after it disconnects, so reconnecting doesn't buy a
/// fresh burst. At most `max_keys` buckets are kept: when full, buckets that
/// have refilled completely are dropped first (forgetting them changes
/// nothing), then the least recently used.
pub(crate) struct RateLimiter<K> {
    buckets: HashMap<K, (f64, Instant)>,
    burst: f64,
    per_sec: f64,
    max_keys: usize,
}

impl<K: Hash + Eq + Clone> RateLimiter<K> {
    pub fn new(burst: u32, per_sec: u32, max_keys: usize) -> Self {
        Self { buckets: HashMap::new(), burst: burst as f64, per_sec: per_sec as f64, max_keys }
    }

    fn refilled(&self, tokens: f64, last: Instant, now: Instant) -> f64 {
        (tokens + now.saturating_duration_since(last).as_secs_f64() * self.per_sec).min(self.burst)
    }

    pub fn allow(&mut self, key: K, now: Instant) -> bool {
        if !self.buckets.contains_key(&key) && self.buckets.len() >= self.max_keys {
            self.make_room(now);
        }
        let (burst, per_sec) = (self.burst, self.per_sec);
        let (tokens, last) = self.buckets.entry(key).or_insert((burst, now));
        *tokens = (*tokens + now.saturating_duration_since(*last).as_secs_f64() * per_sec).min(burst);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Tokens `key` has now, without spending any. For byte budgets, where
    /// the cost is only known afterwards (see [`RateLimiter::charge`]).
    pub fn available(&mut self, key: K, now: Instant) -> usize {
        if !self.buckets.contains_key(&key) && self.buckets.len() >= self.max_keys {
            self.make_room(now);
        }
        let (burst, per_sec) = (self.burst, self.per_sec);
        let (tokens, last) = self.buckets.entry(key).or_insert((burst, now));
        *tokens = (*tokens + now.saturating_duration_since(*last).as_secs_f64() * per_sec).min(burst);
        *last = now;
        tokens.max(0.0) as usize
    }

    /// Spends `cost` tokens. The balance may go negative (a page is never
    /// cut below one record); [`RateLimiter::available`] then reports 0
    /// until it has refilled.
    pub fn charge(&mut self, key: K, now: Instant, cost: usize) {
        self.available(key.clone(), now);
        if let Some((tokens, _)) = self.buckets.get_mut(&key) {
            *tokens -= cost as f64;
        }
    }

    /// Gives back tokens reserved with [`RateLimiter::charge`] but not used
    /// (never above the burst).
    pub fn refund(&mut self, key: K, now: Instant, amount: usize) {
        self.available(key.clone(), now);
        let burst = self.burst;
        if let Some((tokens, _)) = self.buckets.get_mut(&key) {
            *tokens = (*tokens + amount as f64).min(burst);
        }
    }

    fn make_room(&mut self, now: Instant) {
        let full: Vec<K> = self
            .buckets
            .iter()
            .filter(|(_, (tokens, last))| self.refilled(*tokens, *last, now) >= self.burst)
            .map(|(k, _)| k.clone())
            .collect();
        for k in full {
            self.buckets.remove(&k);
        }
        if self.buckets.len() >= self.max_keys {
            if let Some(oldest) = self.buckets.iter().min_by_key(|(_, (_, last))| *last).map(|(k, _)| k.clone()) {
                self.buckets.remove(&oldest);
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.buckets.len()
    }
}

/// Receipt acks waiting for their author (B6): ids of records we stored,
/// per (server, author), until that author pulls from us. Bounded per author
/// and in authors, and entries expire; a lost ack only leaves the author's
/// tick at "sent".
pub(crate) struct Acks<K> {
    by_author: HashMap<K, VecDeque<(RecordId, Instant)>>,
    per_author: usize,
    max_authors: usize,
    ttl: Duration,
}

impl<K: Hash + Eq + Clone> Acks<K> {
    pub fn new(per_author: usize, max_authors: usize, ttl: Duration) -> Self {
        Acks { by_author: HashMap::new(), per_author, max_authors, ttl }
    }

    pub fn insert(&mut self, author: K, id: RecordId, now: Instant) {
        if !self.by_author.contains_key(&author) && self.by_author.len() >= self.max_authors {
            self.by_author.retain(|_, q| q.back().is_some_and(|(_, t)| now.duration_since(*t) < self.ttl));
            if self.by_author.len() >= self.max_authors {
                return;
            }
        }
        let queue = self.by_author.entry(author).or_default();
        if queue.iter().any(|(known, _)| *known == id) {
            return;
        }
        if queue.len() >= self.per_author {
            queue.pop_front();
        }
        queue.push_back((id, now));
    }

    /// Up to `max` unexpired ids for `author`, oldest first; they are removed.
    pub fn take(&mut self, author: &K, max: usize, now: Instant) -> Vec<RecordId> {
        let Some(queue) = self.by_author.get_mut(author) else { return Vec::new() };
        queue.retain(|(_, t)| now.duration_since(*t) < self.ttl);
        let out = queue.drain(..max.min(queue.len())).map(|(id, _)| id).collect();
        if queue.is_empty() {
            self.by_author.remove(author);
        }
        out
    }
}

/// When a peer found through the DHT may be dialled again after failed
/// dials: the wait doubles from `min` up to `max`, and a connection resets
/// it. A peer whose relay reservation ended stays in DHT answers for a
/// while; without this every node re-dialled its circuit every discovery
/// round, and the relay denied each one.
pub(crate) struct DialBackoff<K> {
    peers: HashMap<K, (Duration, Instant)>,
    min: Duration,
    max: Duration,
    max_peers: usize,
}

impl<K: Hash + Eq + Clone> DialBackoff<K> {
    pub fn new(min: Duration, max: Duration, max_peers: usize) -> Self {
        DialBackoff { peers: HashMap::new(), min, max, max_peers }
    }

    pub fn may_dial(&self, peer: &K, now: Instant) -> bool {
        self.peers.get(peer).is_none_or(|(_, until)| now >= *until)
    }

    /// A dial failed: wait `jitter(wait)` before the next one, where the
    /// wait doubles each time.
    pub fn failed(&mut self, peer: K, now: Instant, jitter: impl FnOnce(Duration) -> Duration) {
        if !self.peers.contains_key(&peer) && self.peers.len() >= self.max_peers {
            self.peers.retain(|_, (_, until)| *until > now);
            if self.peers.len() >= self.max_peers {
                return;
            }
        }
        let wait = match self.peers.get(&peer) {
            Some((last, _)) => (*last * 2).min(self.max),
            None => self.min,
        };
        self.peers.insert(peer, (wait, now + jitter(wait)));
    }

    pub fn connected(&mut self, peer: &K) {
        self.peers.remove(peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_dials_back_off_per_peer_until_connected() {
        let now = Instant::now();
        let (min, max) = (Duration::from_secs(5), Duration::from_secs(300));
        let mut backoff = DialBackoff::new(min, max, 2);
        let same = |d: Duration| d;
        assert!(backoff.may_dial(&"a", now));
        backoff.failed("a", now, same);
        assert!(!backoff.may_dial(&"a", now + Duration::from_secs(4)));
        assert!(backoff.may_dial(&"a", now + min));
        assert!(backoff.may_dial(&"b", now)); // per peer
        // Doubling, capped.
        let mut t = now;
        let mut waits = Vec::new();
        for _ in 0..8 {
            backoff.failed("a", t, same);
            let until = backoff.peers[&"a"].1;
            waits.push((until - t).as_secs());
            t = until;
        }
        assert_eq!(waits, [10, 20, 40, 80, 160, 300, 300, 300]);
        // Discovery every 3 s now dials a dead circuit about once per 5 min.
        backoff.connected(&"a");
        assert!(backoff.may_dial(&"a", t));
        // Bounded.
        backoff.failed("b", now, same);
        backoff.failed("c", now, same);
        backoff.failed("d", now, same);
        assert!(backoff.peers.len() <= 2);
    }

    #[test]
    fn acks_are_capped_per_answer_per_author_and_in_time() {
        let now = Instant::now();
        let mut acks = Acks::new(300, 2, Duration::from_secs(600));
        for i in 0..400u32 {
            let mut id = [0; 32];
            id[..4].copy_from_slice(&i.to_le_bytes());
            acks.insert("alice", RecordId::from_bytes(id), now);
        }
        acks.insert("alice", dep(1), now);
        acks.insert("alice", dep(1), now); // once
        // 300 kept per author (the oldest dropped); 256 per answer, the rest next time.
        assert_eq!(acks.take(&"alice", 256, now).len(), 256);
        assert_eq!(acks.take(&"alice", 256, now).len(), 300 - 256);
        assert!(acks.take(&"alice", 256, now).is_empty());
        // Only the author's own acks, and only while fresh.
        acks.insert("bob", dep(2), now);
        assert!(acks.take(&"carol", 256, now).is_empty());
        assert!(acks.take(&"bob", 256, now + Duration::from_secs(601)).is_empty());
        // A bounded number of authors.
        acks.insert("a", dep(3), now);
        acks.insert("b", dep(4), now);
        acks.insert("c", dep(5), now);
        assert!(acks.take(&"c", 256, now).is_empty());
    }

    fn dep(n: u8) -> RecordId {
        RecordId::from_bytes([n; 32])
    }

    fn blob(i: u32, size: usize) -> Record {
        crate::testutil::message(i as u64, &[], size)
    }

    #[test]
    fn pending_is_capped_by_count() {
        let now = Instant::now();
        let mut pending = Pending::new(3, usize::MAX);
        for i in 0..3 {
            assert!(pending.insert(dep(1), blob(i, 0), now));
        }
        assert!(!pending.insert(dep(2), blob(9, 0), now));
        assert_eq!(pending.len(), 3);
    }

    #[test]
    fn copies_of_one_blob_are_parked_once() {
        let now = Instant::now();
        let mut pending = Pending::new(2, usize::MAX);
        for _ in 0..5 {
            assert!(pending.insert(dep(1), blob(0, 0), now));
        }
        assert_eq!(pending.len(), 1);
        // So a flood of copies doesn't crowd out a different blob.
        assert!(pending.insert(dep(1), blob(1, 0), now));
        // Once taken, the same blob can be parked again.
        assert_eq!(pending.take(&dep(1)).len(), 2);
        assert!(pending.insert(dep(2), blob(0, 0), now));
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn refused_ids_are_bounded_and_listed_per_log() {
        let mut refused = Refused::new(3);
        for n in 1..=4 {
            refused.insert(if n % 2 == 0 { "b".into() } else { "a".into() }, dep(n));
        }
        // The oldest (1) was forgotten.
        assert!(!refused.contains(&dep(1)) && refused.contains(&dep(4)));
        assert_eq!(refused.markers("a", 10), vec![dep(3)]);
        assert_eq!(refused.markers("b", 1), vec![dep(4)]);
        assert!(refused.covers(&[dep(2), dep(3)]));
        assert!(!refused.covers(&[dep(2), dep(9)]));
        assert!(!refused.covers(&[]));
        // A new control record for server "b" makes its refusals stale.
        refused.forget_prefix("b");
        assert!(!refused.contains(&dep(2)) && !refused.contains(&dep(4)) && refused.contains(&dep(3)));
        assert!(refused.markers("b", 10).is_empty());
    }

    #[test]
    fn pending_is_capped_by_bytes() {
        let now = Instant::now();
        let mut pending = Pending::new(usize::MAX, 10_000);
        assert!(pending.insert(dep(1), blob(0, 6_000), now));
        assert!(!pending.insert(dep(1), blob(1, 6_000), now));
        assert!(pending.bytes() <= 10_000);
    }

    #[test]
    fn reinserting_respects_the_cap() {
        // The bug this guards against: blobs released by one dep and parked
        // again under another dep used to bypass the cap.
        let now = Instant::now();
        let mut pending = Pending::new(2, usize::MAX);
        assert!(pending.insert(dep(1), blob(0, 0), now));
        assert!(pending.insert(dep(1), blob(1, 0), now));
        let waiting = pending.take(&dep(1));
        assert_eq!(pending.len(), 0);
        // Something else fills the space before they are parked again.
        assert!(pending.insert(dep(3), blob(2, 0), now));
        let results: Vec<bool> = waiting.into_iter().map(|w| pending.reinsert(dep(2), w)).collect();
        assert_eq!(results, vec![true, false]);
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn pending_expires() {
        let start = Instant::now();
        let mut pending = Pending::new(10, usize::MAX);
        pending.insert(dep(1), blob(0, 100), start);
        pending.insert(dep(2), blob(1, 100), start + Duration::from_secs(50));
        pending.expire(start + Duration::from_secs(70), Duration::from_secs(60));
        assert_eq!(pending.len(), 1);
        assert!(pending.take(&dep(1)).is_empty());
        assert_eq!(pending.take(&dep(2)).len(), 1);
        assert_eq!((pending.len(), pending.bytes()), (0, 0));
    }

    #[test]
    fn rate_limiter_allows_a_burst_then_the_rate() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(5, 10, 100);
        let allowed = (0..20).filter(|_| limiter.allow("peer", start)).count();
        assert_eq!(allowed, 5);
        // 10/s refills one token every 100 ms.
        assert!(limiter.allow("peer", start + Duration::from_millis(100)));
        assert!(!limiter.allow("peer", start + Duration::from_millis(100)));
        // Other peers have their own bucket.
        assert!(limiter.allow("other", start));
    }

    #[test]
    fn reconnecting_does_not_refill_the_bucket() {
        // There is no `forget` on disconnect any more: an empty bucket stays
        // empty until it has really refilled.
        let start = Instant::now();
        let mut limiter = RateLimiter::new(5, 10, 100);
        while limiter.allow("peer", start) {}
        assert!(!limiter.allow("peer", start + Duration::from_millis(50)));
    }

    #[test]
    fn tracked_keys_are_bounded_without_freeing_drained_ones() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(2, 1, 10);
        // A drained peer...
        while limiter.allow(0u32, start) {}
        // ...then a flood of new keys, each leaving a nearly full bucket.
        for k in 1..1000u32 {
            limiter.allow(k, start + Duration::from_millis(k as u64));
        }
        assert!(limiter.len() <= 10);

        // Refilled buckets are dropped before a drained one.
        let mut limiter = RateLimiter::new(2, 1, 3);
        while limiter.allow("drained", start) {} // 0 tokens left
        limiter.allow("a", start); // 1 left
        limiter.allow("b", start); // 1 left
        let later = start + Duration::from_millis(1500); // a, b full again; drained at 1.5
        limiter.allow("c", later);
        assert_eq!(limiter.len(), 2, "a and b were dropped, drained and c kept");
        assert!(limiter.allow("drained", later));
        assert!(!limiter.allow("drained", later), "a dropped bucket would have had 2 tokens");
    }

    #[test]
    fn ipv6_is_capped_per_64_not_per_address() {
        use libp2p::swarm::ConnectionId;
        let mut per_ip = PerIp::new(8);
        // Nine addresses in one /64: the ninth is refused.
        let admitted: Vec<bool> = (1..=9u16)
            .map(|i| per_ip.admit(ConnectionId::new_unchecked(i as usize), format!("2001:db8:1:2::{i:x}").parse().unwrap()))
            .collect();
        assert_eq!(admitted, [vec![true; 8], vec![false]].concat());
        // Another /64 has its own count.
        assert!(per_ip.admit(ConnectionId::new_unchecked(100), "2001:db8:1:3::1".parse().unwrap()));
        // Closing one frees a slot in that /64.
        per_ip.release(ConnectionId::new_unchecked(1));
        assert!(per_ip.admit(ConnectionId::new_unchecked(101), "2001:db8:1:2:ffff::9".parse().unwrap()));
        // IPv4 counts per address, and an IPv4-mapped IPv6 address counts as that IPv4 one.
        let mut per_ip = PerIp::new(1);
        assert!(per_ip.admit(ConnectionId::new_unchecked(1), "203.0.113.7".parse().unwrap()));
        assert!(!per_ip.admit(ConnectionId::new_unchecked(2), "::ffff:203.0.113.7".parse().unwrap()));
        assert!(per_ip.admit(ConnectionId::new_unchecked(3), "203.0.113.8".parse().unwrap()));
    }

    #[test]
    fn byte_budget_is_spent_after_the_fact_and_refills() {
        let start = Instant::now();
        let mut bytes = RateLimiter::new(1000, 100, 10);
        assert_eq!(bytes.available("peer", start), 1000);
        // A page bigger than what was left: the balance goes negative...
        bytes.charge("peer", start, 1500);
        assert_eq!(bytes.available("peer", start), 0);
        // ...and is back above zero only after it has refilled: 500 / 100 per s.
        assert_eq!(bytes.available("peer", start + Duration::from_secs(4)), 0);
        assert_eq!(bytes.available("peer", start + Duration::from_secs(7)), 200);

        // Reserve a page up front, then refund what the page didn't use.
        let mut bytes = RateLimiter::new(1000, 100, 10);
        bytes.charge("peer", start, 800);
        assert_eq!(bytes.available("peer", start), 200, "a second answer can't spend the reserved part");
        bytes.refund("peer", start, 600);
        assert_eq!(bytes.available("peer", start), 800);
        bytes.refund("peer", start, 10_000);
        assert_eq!(bytes.available("peer", start), 1000, "never above the burst");
    }

    #[test]
    fn global_bucket_caps_many_peers() {
        // Sybil peers each get their own bucket, but share the global one.
        let start = Instant::now();
        let mut per_peer = RateLimiter::new(30, 10, 4096);
        let mut global = RateLimiter::new(100, 50, 1);
        let allowed = (0..256u32)
            .flat_map(|peer| (0..30).map(move |_| peer))
            .filter(|&peer| per_peer.allow(peer, start) && global.allow((), start))
            .count();
        assert_eq!(allowed, 100);
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

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

//! Bounded in-memory state that remote peers can make grow: pushed blobs
//! waiting for their deps, and per-peer request rates.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::{Blob, Id};

/// Pushed blobs waiting for a dep, keyed by that dep. Capped by count and by
/// bytes, and entries expire: anything dropped here is fetched by the next
/// pull anyway, so this is only a shortcut.
pub(crate) struct Pending {
    by_dep: HashMap<Id, Vec<Waiting>>,
    /// Ids of every parked blob: copies of one blob are kept once.
    ids: HashSet<Id>,
    count: usize,
    bytes: usize,
    max_count: usize,
    max_bytes: usize,
}

pub(crate) struct Waiting {
    pub log: String,
    pub blob: Blob,
    since: Instant,
}

fn cost(log: &str, blob: &Blob) -> usize {
    // Data, deps, log name, plus the bookkeeping around them.
    blob.data.len() + 32 * blob.deps.len() + log.len() + 128
}

impl Pending {
    pub fn new(max_count: usize, max_bytes: usize) -> Self {
        Self { by_dep: HashMap::new(), ids: HashSet::new(), count: 0, bytes: 0, max_count, max_bytes }
    }

    /// Keeps `blob` until `missing` arrives. Returns false (and drops it) when
    /// full. `since` is kept from the first time the blob was parked, so a
    /// blob that keeps finding another missing dep still expires.
    pub fn insert(&mut self, missing: Id, log: String, blob: Blob, since: Instant) -> bool {
        if self.ids.contains(&blob.id) {
            return true;
        }
        let cost = cost(&log, &blob);
        if self.count >= self.max_count || self.bytes + cost > self.max_bytes {
            return false;
        }
        self.count += 1;
        self.bytes += cost;
        self.ids.insert(blob.id);
        self.by_dep.entry(missing).or_default().push(Waiting { log, blob, since });
        true
    }

    /// Removes and returns the blobs that were waiting for `dep`.
    pub fn take(&mut self, dep: &Id) -> Vec<Waiting> {
        let waiting = self.by_dep.remove(dep).unwrap_or_default();
        for w in &waiting {
            self.count -= 1;
            self.bytes -= cost(&w.log, &w.blob);
            self.ids.remove(&w.blob.id);
        }
        waiting
    }

    /// Puts back a blob from [`Pending::take`] that still lacks a dep.
    pub fn reinsert(&mut self, missing: Id, waiting: Waiting) -> bool {
        self.insert(missing, waiting.log, waiting.blob, waiting.since)
    }

    /// Drops everything parked before `now - ttl`.
    pub fn expire(&mut self, now: Instant, ttl: Duration) {
        let (mut count, mut bytes, mut ids) = (0, 0, HashSet::new());
        self.by_dep.retain(|_, waiting| {
            waiting.retain(|w| now.duration_since(w.since) < ttl);
            for w in waiting.iter() {
                count += 1;
                bytes += cost(&w.log, &w.blob);
                ids.insert(w.blob.id);
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

    fn blob(i: u32, size: usize) -> Blob {
        Blob::new("l", vec![], [i.to_le_bytes().to_vec(), vec![0; size]].concat())
    }

    #[test]
    fn pending_is_capped_by_count() {
        let now = Instant::now();
        let mut pending = Pending::new(3, usize::MAX);
        for i in 0..3 {
            assert!(pending.insert([1; 32], "l".into(), blob(i, 0), now));
        }
        assert!(!pending.insert([2; 32], "l".into(), blob(9, 0), now));
        assert_eq!(pending.len(), 3);
    }

    #[test]
    fn copies_of_one_blob_are_parked_once() {
        let now = Instant::now();
        let mut pending = Pending::new(2, usize::MAX);
        for _ in 0..5 {
            assert!(pending.insert([1; 32], "l".into(), blob(0, 0), now));
        }
        assert_eq!(pending.len(), 1);
        // So a flood of copies doesn't crowd out a different blob.
        assert!(pending.insert([1; 32], "l".into(), blob(1, 0), now));
        // Once taken, the same blob can be parked again.
        assert_eq!(pending.take(&[1; 32]).len(), 2);
        assert!(pending.insert([2; 32], "l".into(), blob(0, 0), now));
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn pending_is_capped_by_bytes() {
        let now = Instant::now();
        let mut pending = Pending::new(usize::MAX, 10_000);
        assert!(pending.insert([1; 32], "l".into(), blob(0, 6_000), now));
        assert!(!pending.insert([1; 32], "l".into(), blob(1, 6_000), now));
        assert!(pending.bytes() <= 10_000);
    }

    #[test]
    fn reinserting_respects_the_cap() {
        // The bug this guards against: blobs released by one dep and parked
        // again under another dep used to bypass the cap.
        let now = Instant::now();
        let mut pending = Pending::new(2, usize::MAX);
        assert!(pending.insert([1; 32], "l".into(), blob(0, 0), now));
        assert!(pending.insert([1; 32], "l".into(), blob(1, 0), now));
        let waiting = pending.take(&[1; 32]);
        assert_eq!(pending.len(), 0);
        // Something else fills the space before they are parked again.
        assert!(pending.insert([3; 32], "l".into(), blob(2, 0), now));
        let results: Vec<bool> = waiting.into_iter().map(|w| pending.reinsert([2; 32], w)).collect();
        assert_eq!(results, vec![true, false]);
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn pending_expires() {
        let start = Instant::now();
        let mut pending = Pending::new(10, usize::MAX);
        pending.insert([1; 32], "l".into(), blob(0, 100), start);
        pending.insert([2; 32], "l".into(), blob(1, 100), start + Duration::from_secs(50));
        pending.expire(start + Duration::from_secs(70), Duration::from_secs(60));
        assert_eq!(pending.len(), 1);
        assert!(pending.take(&[1; 32]).is_empty());
        assert_eq!(pending.take(&[2; 32]).len(), 1);
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

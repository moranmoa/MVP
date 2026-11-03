//! Bounded in-memory state that remote peers can make grow: pushed blobs
//! waiting for their deps, and per-peer request rates.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::{Blob, Id};

/// Pushed blobs waiting for a dep, keyed by that dep. Capped by count and by
/// bytes, and entries expire: anything dropped here is fetched by the next
/// pull anyway, so this is only a shortcut.
pub(crate) struct Pending {
    by_dep: HashMap<Id, Vec<Waiting>>,
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
        Self { by_dep: HashMap::new(), count: 0, bytes: 0, max_count, max_bytes }
    }

    /// Keeps `blob` until `missing` arrives. Returns false (and drops it) when
    /// full. `since` is kept from the first time the blob was parked, so a
    /// blob that keeps finding another missing dep still expires.
    pub fn insert(&mut self, missing: Id, log: String, blob: Blob, since: Instant) -> bool {
        let cost = cost(&log, &blob);
        if self.count >= self.max_count || self.bytes + cost > self.max_bytes {
            return false;
        }
        self.count += 1;
        self.bytes += cost;
        self.by_dep.entry(missing).or_default().push(Waiting { log, blob, since });
        true
    }

    /// Removes and returns the blobs that were waiting for `dep`.
    pub fn take(&mut self, dep: &Id) -> Vec<Waiting> {
        let waiting = self.by_dep.remove(dep).unwrap_or_default();
        for w in &waiting {
            self.count -= 1;
            self.bytes -= cost(&w.log, &w.blob);
        }
        waiting
    }

    /// Puts back a blob from [`Pending::take`] that still lacks a dep.
    pub fn reinsert(&mut self, missing: Id, waiting: Waiting) -> bool {
        self.insert(missing, waiting.log, waiting.blob, waiting.since)
    }

    /// Drops everything parked before `now - ttl`.
    pub fn expire(&mut self, now: Instant, ttl: Duration) {
        let (mut count, mut bytes) = (0, 0);
        self.by_dep.retain(|_, waiting| {
            waiting.retain(|w| now.duration_since(w.since) < ttl);
            for w in waiting.iter() {
                count += 1;
                bytes += cost(&w.log, &w.blob);
            }
            !waiting.is_empty()
        });
        self.count = count;
        self.bytes = bytes;
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
pub(crate) struct RateLimiter<K> {
    buckets: HashMap<K, (f64, Instant)>,
    burst: f64,
    per_sec: f64,
}

impl<K: Hash + Eq> RateLimiter<K> {
    pub fn new(burst: u32, per_sec: u32) -> Self {
        Self { buckets: HashMap::new(), burst: burst as f64, per_sec: per_sec as f64 }
    }

    pub fn allow(&mut self, key: K, now: Instant) -> bool {
        let (tokens, last) = self.buckets.entry(key).or_insert((self.burst, now));
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * self.per_sec).min(self.burst);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Call when the peer disconnects, so the map only holds connected peers.
    pub fn forget(&mut self, key: &K) {
        self.buckets.remove(key);
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
        let mut limiter = RateLimiter::new(5, 10);
        let allowed = (0..20).filter(|_| limiter.allow("peer", start)).count();
        assert_eq!(allowed, 5);
        // 10/s refills one token every 100 ms.
        assert!(limiter.allow("peer", start + Duration::from_millis(100)));
        assert!(!limiter.allow("peer", start + Duration::from_millis(100)));
        // Other peers have their own bucket.
        assert!(limiter.allow("other", start));
        limiter.forget(&"peer");
        assert_eq!(limiter.len(), 1);
    }
}

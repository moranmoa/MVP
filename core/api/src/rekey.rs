//! How often a join may re-key a server (GAP-8, the security lead's token
//! bucket, 2026-11-10).
//!
//! - Each server has [`JOIN_REKEY_BURST`] join re-keys up front, and gets
//!   one back every [`JOIN_REKEY_REFILL_MS`].
//! - A join re-keys at once while a token is left.
//! - With the bucket empty, every join that comes in meanwhile shares one
//!   re-key, fired exactly when the next token is due ([`JoinBucket::due`]).
//! - Kicks, leaves and revokes never wait and spend no token: that path
//!   doesn't come here.
//!
//! So 100 joins over 5 minutes give at most 3 + 300/15 = 23 rotations.

/// Join re-keys a server may make at once.
pub const JOIN_REKEY_BURST: u32 = 3;
/// One join re-key comes back this often.
pub const JOIN_REKEY_REFILL_MS: u64 = 15_000;

/// One server's join re-key tokens.
#[derive(Clone, Copy, Debug)]
pub(crate) struct JoinBucket {
    tokens: u32,
    /// When the tokens were last counted up to: a whole number of refills
    /// since then are owed (or now, when full).
    since: u64,
}

impl JoinBucket {
    /// Full.
    pub fn new(now: u64) -> JoinBucket {
        JoinBucket { tokens: JOIN_REKEY_BURST, since: now }
    }

    fn refill(&mut self, now: u64) {
        if self.tokens >= JOIN_REKEY_BURST {
            self.since = now;
            return;
        }
        let owed = now.saturating_sub(self.since) / JOIN_REKEY_REFILL_MS;
        if owed == 0 {
            return;
        }
        let owed = u32::try_from(owed).unwrap_or(u32::MAX);
        self.tokens = self.tokens.saturating_add(owed).min(JOIN_REKEY_BURST);
        self.since = if self.tokens == JOIN_REKEY_BURST {
            now
        } else {
            self.since + u64::from(owed) * JOIN_REKEY_REFILL_MS
        };
    }

    /// Spends a token if one is left.
    pub fn take(&mut self, now: u64) -> bool {
        self.refill(now);
        if self.tokens == 0 {
            return false;
        }
        if self.tokens == JOIN_REKEY_BURST {
            // The refill clock starts with the first token spent.
            self.since = now;
        }
        self.tokens -= 1;
        true
    }

    /// When the next token is due (already, when one is left).
    pub fn due(&self) -> u64 {
        if self.tokens > 0 {
            self.since
        } else {
            self.since + JOIN_REKEY_REFILL_MS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn a_burst_of_three_re_keys_at_once_and_a_fourth_waits_for_the_token() {
        let mut bucket = JoinBucket::new(NOW);
        assert!(bucket.take(NOW));
        assert!(bucket.take(NOW + 100));
        assert!(bucket.take(NOW + 200));
        // The 4th: due exactly one refill after the first was spent.
        assert!(!bucket.take(NOW + 300));
        assert_eq!(bucket.due(), NOW + JOIN_REKEY_REFILL_MS);
        assert!(!bucket.take(NOW + JOIN_REKEY_REFILL_MS - 1));
        assert!(bucket.take(NOW + JOIN_REKEY_REFILL_MS));
        assert!(!bucket.take(NOW + JOIN_REKEY_REFILL_MS + 1));
        assert_eq!(bucket.due(), NOW + 2 * JOIN_REKEY_REFILL_MS);
    }

    #[test]
    fn a_hundred_joins_in_five_minutes_make_at_most_twenty_three_rotations() {
        // A join every 3 s; the core retries every 500 ms (a busy loop);
        // joins waiting on an empty bucket share the next rotation.
        let mut bucket = JoinBucket::new(NOW);
        let (mut rotations, mut waiting, mut joined) = (0, false, 0);
        for step in 0..=600u64 {
            let now = NOW + step * 500;
            if step % 6 == 0 && joined < 100 {
                joined += 1;
                waiting = true;
            }
            if waiting && bucket.take(now) {
                rotations += 1;
                waiting = false;
            }
        }
        assert_eq!(joined, 100);
        assert!(rotations <= 23, "{rotations} rotations");
        // And it isn't starved: everyone but the last few was keyed.
        assert!(rotations >= 20, "{rotations} rotations");
    }

    #[test]
    fn a_quiet_server_refills_to_three_and_no_more() {
        let mut bucket = JoinBucket::new(NOW);
        for i in 0..3 {
            assert!(bucket.take(NOW + i));
        }
        let later = NOW + 10 * JOIN_REKEY_REFILL_MS;
        for i in 0..3 {
            assert!(bucket.take(later + i), "token {i} after a long quiet");
        }
        assert!(!bucket.take(later + 3));
    }
}

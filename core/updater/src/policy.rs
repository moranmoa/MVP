//! Client-side rules for which verified manifests to act on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Error, Version, VerifiedManifest};

/// What the client remembers between manifests. Persist it after every
/// accepted manifest, and load it with [`ClientPolicy::restore`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientPolicy {
    channel: String,
    /// `(version, issued_at)` of the newest manifest accepted. Anything
    /// older is a replay.
    newest: Option<(Version, u64)>,
    /// Highest signed `issued_at` accepted. Real time is at least this, so
    /// an attacker who winds the local clock back (e.g. spoofed NTP) cannot
    /// revive an expired manifest.
    time_floor: u64,
    /// Latest `expires_at` accepted, for [`ClientPolicy::freshness`].
    valid_until: u64,
    /// Every version any accepted manifest revoked. Only grows: a later
    /// manifest cannot un-revoke a version.
    revoked: BTreeSet<Version>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    UpToDate,
    Install(Version),
}

/// Whether the client currently holds an unexpired manifest. A network
/// attacker can always withhold new manifests; `Stale` is how the app
/// notices, warns the user and tries other sources (project seeder, HTTPS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale { since: u64 },
    Never,
}

/// What a release binary knows about itself at build time. Used to rebuild
/// a [`ClientPolicy`] when the state file is missing or corrupt, so losing
/// the file never takes the client below what its own binary knows.
#[derive(Clone, Debug, Default)]
pub struct Baseline {
    /// Unix seconds when this release was built.
    pub build_time: u64,
    /// Revocations known when this release was built.
    pub revoked: Vec<Version>,
}

impl ClientPolicy {
    pub fn new(channel: &str) -> Self {
        ClientPolicy {
            channel: channel.to_string(),
            newest: None,
            time_floor: 0,
            valid_until: 0,
            revoked: BTreeSet::new(),
        }
    }

    /// Loads persisted state, falling back to the binary's baseline when
    /// the file is missing, unparsable, or for another channel. Whatever was
    /// saved, the result never knows less than the baseline: the newest
    /// version is at least `installed`, the time floor at least the build
    /// time, and the baked-in revocations are always present.
    /// The flag is true when the saved state was not usable.
    pub fn restore(saved: Option<&[u8]>, channel: &str, installed: Version, base: &Baseline) -> (Self, bool) {
        let parsed = saved.and_then(|b| serde_json::from_slice::<ClientPolicy>(b).ok());
        let (mut p, reset) = match parsed {
            Some(p) if p.channel == channel => (p, false),
            Some(other) => {
                let mut p = ClientPolicy::new(channel);
                p.time_floor = other.time_floor;
                p.revoked = other.revoked;
                (p, true)
            }
            None => (ClientPolicy::new(channel), true),
        };
        if p.newest.map_or(true, |(v, _)| v < installed) {
            p.newest = Some((installed, 0));
        }
        p.time_floor = p.time_floor.max(base.build_time);
        p.revoked.extend(base.revoked.iter().copied());
        (p, reset)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("policy always serialises")
    }

    /// Applies the expiry-floor, no-replay, no-downgrade and no-revoked rules
    /// to a manifest that already passed signature checks. State changes
    /// only when the manifest is accepted.
    pub fn accept(&mut self, m: &VerifiedManifest, installed: Version) -> Result<Decision, Error> {
        let body = m.body();
        let offered = body.version;
        if body.channel != self.channel {
            return Err(Error::WrongChannel { expected: self.channel.clone(), found: body.channel.clone() });
        }
        if body.expires_at <= self.time_floor {
            return Err(Error::Expired { expires_at: body.expires_at, now: self.time_floor });
        }
        if let Some((seen, seen_issued)) = self.newest {
            if (offered, body.issued_at) < (seen, seen_issued) {
                return Err(Error::StaleManifest { seen, offered });
            }
        }
        if self.revoked.contains(&offered) {
            return Err(Error::Revoked(offered));
        }
        if offered < installed {
            return Err(Error::Downgrade { installed, offered });
        }

        self.newest = Some((offered, body.issued_at));
        self.time_floor = self.time_floor.max(body.issued_at);
        self.valid_until = self.valid_until.max(body.expires_at);
        self.revoked.extend(body.revoked.iter().copied());
        Ok(if offered == installed { Decision::UpToDate } else { Decision::Install(offered) })
    }

    /// True when the running version was revoked; the app should treat the
    /// next update as urgent (it still cannot downgrade).
    pub fn is_revoked(&self, v: Version) -> bool {
        self.revoked.contains(&v)
    }

    pub fn revoked(&self) -> &BTreeSet<Version> {
        &self.revoked
    }

    pub fn freshness(&self, now: u64) -> Freshness {
        let now = now.max(self.time_floor);
        match self.valid_until {
            0 => Freshness::Never,
            t if now < t => Freshness::Fresh,
            t => Freshness::Stale { since: t },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{body, key, signed_by, NOW};
    use crate::{ManifestBody, TrustRoot};

    fn verify_at(b: &ManifestBody, now: u64) -> VerifiedManifest {
        let k = key();
        signed_by(b, &[&k]).verify(&TrustRoot::single(k.verifying_key()).unwrap(), now).unwrap()
    }

    fn verified(version: &str, revoked: &[&str]) -> VerifiedManifest {
        let mut b = body(version);
        b.revoked = revoked.iter().map(|v| v.parse().unwrap()).collect();
        verify_at(&b, NOW)
    }

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn newer_version_is_installed() {
        let mut p = ClientPolicy::new("stable");
        assert_eq!(p.accept(&verified("1.1.0", &[]), v("1.0.0")), Ok(Decision::Install(v("1.1.0"))));
        assert_eq!(p.newest.map(|(v, _)| v), Some(v("1.1.0")));
    }

    #[test]
    fn same_version_is_up_to_date() {
        let mut p = ClientPolicy::new("stable");
        assert_eq!(p.accept(&verified("1.0.0", &[]), v("1.0.0")), Ok(Decision::UpToDate));
    }

    #[test]
    fn lower_version_is_a_refused_downgrade() {
        let mut p = ClientPolicy::new("stable");
        assert_eq!(
            p.accept(&verified("1.0.0", &[]), v("1.1.0")),
            Err(Error::Downgrade { installed: v("1.1.0"), offered: v("1.0.0") })
        );
        assert_eq!(p, ClientPolicy::new("stable"), "a refused manifest must not change state");
    }

    #[test]
    fn older_manifest_than_already_seen_is_stale() {
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &[]), v("1.0.0")).unwrap();
        // Still above what is installed, but older than what we already know exists.
        assert_eq!(
            p.accept(&verified("1.1.0", &[]), v("1.0.0")),
            Err(Error::StaleManifest { seen: v("1.2.0"), offered: v("1.1.0") })
        );
    }

    #[test]
    fn refreshed_manifest_of_same_version_is_accepted() {
        // Re-issuing the same release later, with a new expiry or extra revocations.
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &[]), v("1.0.0")).unwrap();
        let mut refresh = body("1.2.0");
        refresh.issued_at += 3600;
        refresh.revoked.insert(v("1.1.0"));
        assert_eq!(p.accept(&verify_at(&refresh, NOW), v("1.0.0")), Ok(Decision::Install(v("1.2.0"))));
        assert!(p.is_revoked(v("1.1.0")));
    }

    #[test]
    fn older_issue_of_the_same_version_is_a_replay() {
        // Attack: replay the earlier 1.2.0 manifest (before a revocation was
        // added) to keep the client on a shorter or older view.
        let mut p = ClientPolicy::new("stable");
        let original = body("1.2.0");
        let mut refresh = original.clone();
        refresh.issued_at += 3600;
        p.accept(&verify_at(&refresh, NOW), v("1.0.0")).unwrap();
        assert!(matches!(p.accept(&verify_at(&original, NOW), v("1.0.0")), Err(Error::StaleManifest { .. })));
    }

    #[test]
    fn rolled_back_clock_cannot_revive_an_expired_manifest() {
        // Attack: spoof NTP so `now` is in the past, then replay a manifest
        // that really expired. Signature check alone passes; the floor does not.
        let mut p = ClientPolicy::new("stable");
        let mut later = body("1.3.0");
        later.issued_at = NOW + 30 * 86_400;
        later.expires_at = later.issued_at + 7 * 86_400;
        p.accept(&verify_at(&later, later.issued_at), v("1.0.0")).unwrap();

        let old = body("1.4.0"); // newer version number but issued/expired long ago
        let fooled = verify_at(&old, NOW); // the "clock" says NOW
        assert_eq!(
            p.accept(&fooled, v("1.0.0")),
            Err(Error::Expired { expires_at: old.expires_at, now: later.issued_at })
        );
    }

    #[test]
    fn revoked_version_is_never_installed_even_if_later_offered() {
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &["1.3.0"]), v("1.0.0")).unwrap();
        assert_eq!(p.accept(&verified("1.3.0", &[]), v("1.0.0")), Err(Error::Revoked(v("1.3.0"))));
    }

    #[test]
    fn revocations_accumulate_and_cannot_be_undone() {
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &["1.1.0"]), v("1.0.0")).unwrap();
        p.accept(&verified("1.3.0", &[]), v("1.0.0")).unwrap();
        assert!(p.is_revoked(v("1.1.0")));
    }

    #[test]
    fn installed_version_can_be_flagged_as_revoked() {
        let mut p = ClientPolicy::new("stable");
        assert_eq!(p.accept(&verified("1.1.0", &["1.0.0"]), v("1.0.0")), Ok(Decision::Install(v("1.1.0"))));
        assert!(p.is_revoked(v("1.0.0")));
    }

    #[test]
    fn other_channel_is_rejected() {
        let mut p = ClientPolicy::new("beta");
        assert!(matches!(p.accept(&verified("1.1.0", &[]), v("1.0.0")), Err(Error::WrongChannel { .. })));
    }

    // --- persisted state deleted or tampered with ---

    fn baseline() -> Baseline {
        Baseline { build_time: NOW - 50, revoked: vec![v("1.0.5")] }
    }

    #[test]
    fn deleted_state_falls_back_to_the_binary_baseline() {
        let (mut p, reset) = ClientPolicy::restore(None, "stable", v("1.1.0"), &baseline());
        assert!(reset);
        // Baked-in revocation survives the deletion.
        assert!(p.is_revoked(v("1.0.5")));
        // Time floor is the build time: a manifest that expired before this
        // binary was built is refused even with a rolled-back clock.
        let mut ancient = body("1.2.0");
        ancient.issued_at = NOW - 10 * 86_400;
        ancient.expires_at = NOW - 60;
        let fooled = verify_at(&ancient, NOW - 9 * 86_400);
        assert!(matches!(p.accept(&fooled, v("1.1.0")), Err(Error::Expired { .. })));
        // Replaying a manifest older than the installed version is still refused.
        assert!(matches!(p.accept(&verified("1.0.9", &[]), v("1.1.0")), Err(Error::StaleManifest { .. })));
    }

    #[test]
    fn corrupt_state_is_reset_not_trusted() {
        let (p, reset) = ClientPolicy::restore(Some(b"{not json"), "stable", v("1.1.0"), &baseline());
        assert!(reset);
        assert_eq!(p.newest, Some((v("1.1.0"), 0)));
    }

    #[test]
    fn tampered_state_cannot_go_below_the_baseline() {
        let mut saved = ClientPolicy::new("stable");
        saved.newest = Some((v("0.1.0"), 0)); // lowered by an attacker
        saved.time_floor = 0;
        let (p, reset) = ClientPolicy::restore(Some(saved.to_json().as_bytes()), "stable", v("1.1.0"), &baseline());
        assert!(!reset);
        assert_eq!(p.newest, Some((v("1.1.0"), 0)));
        assert_eq!(p.time_floor, NOW - 50);
        assert!(p.is_revoked(v("1.0.5")));
    }

    #[test]
    fn saved_state_round_trips() {
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &["1.1.0"]), v("1.0.0")).unwrap();
        let (back, reset) = ClientPolicy::restore(Some(p.to_json().as_bytes()), "stable", v("1.0.0"), &Baseline::default());
        assert!(!reset);
        assert_eq!(back, p);
    }

    #[test]
    fn channel_switch_keeps_revocations_and_time_floor() {
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &["1.1.0"]), v("1.0.0")).unwrap();
        let (beta, reset) = ClientPolicy::restore(Some(p.to_json().as_bytes()), "beta", v("1.0.0"), &Baseline::default());
        assert!(reset);
        assert!(beta.is_revoked(v("1.1.0")));
        assert_eq!(beta.time_floor, p.time_floor);
    }

    #[test]
    fn freshness_reports_withheld_updates() {
        let mut p = ClientPolicy::new("stable");
        assert_eq!(p.freshness(NOW), Freshness::Never);
        let b = body("1.1.0");
        p.accept(&verify_at(&b, NOW), v("1.0.0")).unwrap();
        assert_eq!(p.freshness(NOW), Freshness::Fresh);
        assert_eq!(p.freshness(b.expires_at), Freshness::Stale { since: b.expires_at });
    }
}

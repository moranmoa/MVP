//! Client-side rules for which verified manifests to act on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Error, Version, VerifiedManifest};

/// What the client remembers between manifests. Persist it; losing it only
/// weakens replay protection back to the expiry window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPolicy {
    pub channel: String,
    /// Highest manifest version accepted so far.
    pub highest_seen: Option<Version>,
    /// Every version any accepted manifest revoked. Only grows: a later
    /// manifest cannot un-revoke a version.
    pub revoked: BTreeSet<Version>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    UpToDate,
    Install(Version),
}

impl ClientPolicy {
    pub fn new(channel: &str) -> Self {
        ClientPolicy { channel: channel.to_string(), highest_seen: None, revoked: BTreeSet::new() }
    }

    /// Applies the no-downgrade, no-revoked and no-replay rules to a manifest
    /// that already passed signature and expiry checks. State changes only
    /// when the manifest is accepted.
    pub fn accept(&mut self, m: &VerifiedManifest, installed: Version) -> Result<Decision, Error> {
        let body = m.body();
        let offered = body.version;
        if body.channel != self.channel {
            return Err(Error::WrongChannel { expected: self.channel.clone(), found: body.channel.clone() });
        }
        if let Some(seen) = self.highest_seen {
            if offered < seen {
                return Err(Error::StaleManifest { seen, offered });
            }
        }
        if self.revoked.contains(&offered) {
            return Err(Error::Revoked(offered));
        }
        if offered < installed {
            return Err(Error::Downgrade { installed, offered });
        }

        self.highest_seen = Some(offered);
        self.revoked.extend(body.revoked.iter().copied());
        Ok(if offered == installed { Decision::UpToDate } else { Decision::Install(offered) })
    }

    /// True when the running version was revoked; the app should treat the
    /// next update as urgent (it still cannot downgrade).
    pub fn is_revoked(&self, v: Version) -> bool {
        self.revoked.contains(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{body, key, signed_by, NOW};
    use crate::TrustRoot;

    fn verified(version: &str, revoked: &[&str]) -> VerifiedManifest {
        let k = key();
        let mut b = body(version);
        b.revoked = revoked.iter().map(|v| v.parse().unwrap()).collect();
        signed_by(&b, &[&k]).verify(&TrustRoot::single(k.verifying_key()).unwrap(), NOW).unwrap()
    }

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn newer_version_is_installed() {
        let mut p = ClientPolicy::new("stable");
        assert_eq!(p.accept(&verified("1.1.0", &[]), v("1.0.0")), Ok(Decision::Install(v("1.1.0"))));
        assert_eq!(p.highest_seen, Some(v("1.1.0")));
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
        // Re-issuing the same release with a new expiry or extra revocations.
        let mut p = ClientPolicy::new("stable");
        p.accept(&verified("1.2.0", &[]), v("1.0.0")).unwrap();
        assert_eq!(p.accept(&verified("1.2.0", &["1.1.0"]), v("1.0.0")), Ok(Decision::Install(v("1.2.0"))));
        assert!(p.is_revoked(v("1.1.0")));
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
}

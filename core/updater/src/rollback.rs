//! Install / health-check / rollback as a pure state machine. The real
//! installer persists [`InstallState`] (it is serde-serialisable) and does
//! what [`Boot`] says; it never decides on its own.
//!
//! Lifecycle of an update:
//!
//! 1. A verified artifact is written next to the running version and
//!    [`InstallState::stage`]d. Nothing changes for the running app.
//! 2. On the next start, [`InstallState::begin_boot`] returns
//!    [`Boot::Trial`]: start the new version.
//! 3. If its startup health check passes, the app calls
//!    [`InstallState::report_healthy`] and the new version becomes current.
//!    The old one stays on disk as `previous`.
//! 4. A failed health check, a crash or a hang all look the same: the app
//!    exits without reporting healthy, and the next `begin_boot` counts
//!    another attempt. After `max_attempts` attempts the trial is dropped
//!    and the old version starts again ([`Boot::RolledBack`]).
//!
//! The fallback is always the version that was running before, never an
//! older one, and never a revoked one.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::manifest::is_valid_name;
use crate::{Error, Version, VerifiedArtifact};

/// Upper bound on trial boots. A tampered state file with a huge value
/// would otherwise keep a broken version booting forever.
pub const MAX_ATTEMPTS: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawInstallState")]
pub struct InstallState {
    /// The platform this install runs on; artifacts for others are refused.
    platform: String,
    /// Last version that passed its health check (or came with the installer).
    current: Version,
    /// The version before `current`, kept on disk for manual recovery.
    previous: Option<Version>,
    trial: Option<Trial>,
    /// Versions that failed their trial here. They are never retried; the
    /// fix has to come as a higher version.
    failed: BTreeSet<Version>,
    /// Set when a trial failed and `current` is revoked: nothing is safe to
    /// start. Cleared by staging a new version.
    needs_recovery: bool,
    max_attempts: u32,
}

/// Unchecked mirror of [`InstallState`]; deserialising goes through
/// [`InstallState::validate`] so a corrupt or tampered file is refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInstallState {
    platform: String,
    current: Version,
    previous: Option<Version>,
    trial: Option<Trial>,
    failed: BTreeSet<Version>,
    needs_recovery: bool,
    max_attempts: u32,
}

/// The part of a damaged state file still worth keeping (no unknown-field
/// check, so it reads even when the full struct does not).
#[derive(Deserialize)]
struct FailedOnly {
    platform: String,
    #[serde(default)]
    failed: BTreeSet<Version>,
}

impl TryFrom<RawInstallState> for InstallState {
    type Error = Error;

    fn try_from(r: RawInstallState) -> Result<Self, Error> {
        let s = InstallState {
            platform: r.platform,
            current: r.current,
            previous: r.previous,
            trial: r.trial,
            failed: r.failed,
            needs_recovery: r.needs_recovery,
            max_attempts: r.max_attempts,
        };
        s.validate()?;
        Ok(s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trial {
    pub version: Version,
    /// Boots of this version so far without a healthy report.
    pub attempts: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot {
    /// Start the existing version.
    Normal(Version),
    /// Start the staged version and run its health check.
    Trial(Version),
    /// The staged version used up its attempts and was dropped.
    RolledBack { failed: Version, boot: Version },
    /// The trial failed and the fallback version is revoked. Hand over to
    /// the minimal recovery updater to fetch a newer release.
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Committed {
    pub version: Version,
    /// An older version that is no longer needed and can be deleted.
    pub discard: Option<Version>,
}

impl InstallState {
    /// `installed` is what the installer put on disk; `max_attempts` is how
    /// many trial boots a new version gets (N in the spec), clamped to
    /// `1..=MAX_ATTEMPTS`.
    pub fn new(installed: Version, platform: &str, max_attempts: u32) -> Self {
        InstallState {
            platform: platform.to_string(),
            current: installed,
            previous: None,
            trial: None,
            failed: BTreeSet::new(),
            needs_recovery: false,
            max_attempts: max_attempts.clamp(1, MAX_ATTEMPTS),
        }
    }

    /// Loads persisted state. A missing, corrupt or inconsistent file, one
    /// for another platform, or one whose `current` is below what the
    /// launcher knows is installed, is replaced by a fresh state. The flag
    /// is true when that happened.
    pub fn restore(saved: Option<&[u8]>, installed: Version, platform: &str, max_attempts: u32) -> (Self, bool) {
        match saved.and_then(|b| serde_json::from_slice::<InstallState>(b).ok()) {
            Some(s) if s.platform == platform && s.current >= installed => (s, false),
            _ => {
                let mut fresh = InstallState::new(installed, platform, max_attempts);
                // Keep the versions that already failed here, if that part can
                // still be read: a reset must not give them another trial.
                if let Some(old) = saved.and_then(|b| serde_json::from_slice::<FailedOnly>(b).ok()) {
                    if old.platform == platform {
                        fresh.failed = old.failed.into_iter().filter(|v| *v > installed).collect();
                    }
                }
                (fresh, true)
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("state always serialises")
    }

    fn validate(&self) -> Result<(), Error> {
        let bad = |why: &str| Err(Error::InvalidState(why.to_string()));
        if !is_valid_name(&self.platform) {
            return bad("bad platform name");
        }
        if !(1..=MAX_ATTEMPTS).contains(&self.max_attempts) {
            return bad("max_attempts out of range");
        }
        if self.previous.is_some_and(|p| p >= self.current) {
            return bad("previous is not older than current");
        }
        if self.failed.contains(&self.current) {
            return bad("current version is marked failed");
        }
        if let Some(t) = &self.trial {
            if t.version <= self.current || t.attempts > self.max_attempts || self.failed.contains(&t.version) {
                return bad("inconsistent trial");
            }
            if self.needs_recovery {
                return bad("recovery flag set during a trial");
            }
        }
        Ok(())
    }

    pub fn current(&self) -> Version {
        self.current
    }

    pub fn previous(&self) -> Option<Version> {
        self.previous
    }

    pub fn trial(&self) -> Option<&Trial> {
        self.trial.as_ref()
    }

    pub fn has_failed(&self, v: Version) -> bool {
        self.failed.contains(&v)
    }

    /// Stages a verified artifact for the next start. A newer artifact may
    /// replace a staged one that has not proven itself yet.
    pub fn stage(&mut self, artifact: &VerifiedArtifact, revoked: &BTreeSet<Version>) -> Result<(), Error> {
        let v = artifact.version();
        if artifact.platform() != self.platform {
            return Err(Error::WrongPlatform { expected: self.platform.clone(), found: artifact.platform().into() });
        }
        if revoked.contains(&v) {
            return Err(Error::Revoked(v));
        }
        if self.failed.contains(&v) {
            return Err(Error::PreviouslyFailed(v));
        }
        if v <= self.current {
            return Err(Error::NotNewer { current: self.current, offered: v });
        }
        match &self.trial {
            Some(t) if t.version == v => return Ok(()),
            Some(t) if t.version > v => return Err(Error::NotNewer { current: t.version, offered: v }),
            _ => {}
        }
        self.trial = Some(Trial { version: v, attempts: 0 });
        self.needs_recovery = false;
        Ok(())
    }

    /// Called once at every app start, before anything else. Says which
    /// version to run.
    pub fn begin_boot(&mut self, revoked: &BTreeSet<Version>) -> Boot {
        if self.needs_recovery {
            return Boot::RecoveryRequired;
        }
        if let Some(t) = &mut self.trial {
            if !revoked.contains(&t.version) && t.attempts < self.max_attempts {
                t.attempts += 1;
                return Boot::Trial(t.version);
            }
            let failed = t.version;
            self.trial = None;
            self.failed.insert(failed);
            if revoked.contains(&self.current) {
                self.needs_recovery = true;
                return Boot::RecoveryRequired;
            }
            return Boot::RolledBack { failed, boot: self.current };
        }
        // A revoked `current` with nothing newer still runs: downgrading is
        // never allowed, so the only way out is the next release.
        Boot::Normal(self.current)
    }

    /// The trial version passed its startup health check.
    pub fn report_healthy(&mut self) -> Result<Committed, Error> {
        match &self.trial {
            Some(t) if t.attempts > 0 => {
                let version = t.version;
                let discard = self.previous.replace(self.current);
                self.current = version;
                self.trial = None;
                Ok(Committed { version, discard })
            }
            _ => Err(Error::NoTrial),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    fn art(s: &str) -> VerifiedArtifact {
        VerifiedArtifact::for_tests(v(s), "linux-x86_64")
    }

    fn none() -> BTreeSet<Version> {
        BTreeSet::new()
    }

    fn set(vs: &[&str]) -> BTreeSet<Version> {
        vs.iter().map(|s| v(s)).collect()
    }

    #[test]
    fn no_update_boots_current() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        assert_eq!(s.begin_boot(&none()), Boot::Normal(v("1.0.0")));
        assert_eq!(s.report_healthy(), Err(Error::NoTrial));
    }

    #[test]
    fn healthy_trial_is_committed_and_previous_kept() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        assert_eq!(s.report_healthy(), Ok(Committed { version: v("1.1.0"), discard: None }));
        assert_eq!(s.current(), v("1.1.0"));
        assert_eq!(s.previous(), Some(v("1.0.0")));
        assert_eq!(s.begin_boot(&none()), Boot::Normal(v("1.1.0")));
    }

    #[test]
    fn only_one_previous_version_is_kept() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        for next in ["1.1.0", "1.2.0"] {
            s.stage(&art(next), &none()).unwrap();
            s.begin_boot(&none());
            s.report_healthy().unwrap();
        }
        assert_eq!(s.previous(), Some(v("1.1.0")));
        s.stage(&art("1.3.0"), &none()).unwrap();
        s.begin_boot(&none());
        assert_eq!(s.report_healthy().unwrap().discard, Some(v("1.1.0")));
    }

    #[test]
    fn healthy_on_a_later_attempt_still_commits() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none()); // crashed
        s.begin_boot(&none()); // crashed
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        assert!(s.report_healthy().is_ok());
        assert_eq!(s.current(), v("1.1.0"));
    }

    #[test]
    fn n_failed_boots_roll_back_to_previous() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        for _ in 0..3 {
            assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        }
        assert_eq!(s.begin_boot(&none()), Boot::RolledBack { failed: v("1.1.0"), boot: v("1.0.0") });
        assert_eq!(s.current(), v("1.0.0"));
        assert!(s.has_failed(v("1.1.0")));
        assert_eq!(s.begin_boot(&none()), Boot::Normal(v("1.0.0")));
        assert_eq!(s.report_healthy(), Err(Error::NoTrial), "late healthy report must not commit a dropped trial");
    }

    #[test]
    fn single_attempt_configuration() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        assert!(matches!(s.begin_boot(&none()), Boot::RolledBack { .. }));
    }

    #[test]
    fn failed_version_is_not_retried_but_a_fix_is_accepted() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.begin_boot(&none());
        assert_eq!(s.stage(&art("1.1.0"), &none()), Err(Error::PreviouslyFailed(v("1.1.0"))));
        s.stage(&art("1.1.1"), &none()).unwrap();
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.1")));
    }

    #[test]
    fn staging_rejects_old_same_and_revoked_versions() {
        let mut s = InstallState::new(v("1.1.0"), "linux-x86_64", 3);
        assert!(matches!(s.stage(&art("1.0.0"), &none()), Err(Error::NotNewer { .. })));
        assert!(matches!(s.stage(&art("1.1.0"), &none()), Err(Error::NotNewer { .. })));
        assert_eq!(s.stage(&art("1.2.0"), &set(&["1.2.0"])), Err(Error::Revoked(v("1.2.0"))));
        assert_eq!(s.trial(), None);
    }

    #[test]
    fn newer_artifact_replaces_unproven_trial() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.stage(&art("1.2.0"), &none()).unwrap();
        assert_eq!(s.trial(), Some(&Trial { version: v("1.2.0"), attempts: 0 }));
        assert!(matches!(s.stage(&art("1.1.5"), &none()), Err(Error::NotNewer { .. })));
    }

    #[test]
    fn restaging_same_version_keeps_attempt_count() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 2);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.trial().unwrap().attempts, 1, "re-downloading must not reset the failure count");
    }

    #[test]
    fn trial_revoked_after_staging_is_dropped() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.begin_boot(&set(&["1.1.0"])), Boot::RolledBack { failed: v("1.1.0"), boot: v("1.0.0") });
    }

    #[test]
    fn never_roll_back_to_a_revoked_version() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 1);
        let revoked = set(&["1.0.0"]);
        s.stage(&art("1.1.0"), &revoked).unwrap();
        assert_eq!(s.begin_boot(&revoked), Boot::Trial(v("1.1.0")));
        assert_eq!(s.begin_boot(&revoked), Boot::RecoveryRequired);
        assert_eq!(s.begin_boot(&revoked), Boot::RecoveryRequired, "recovery is sticky");
        // The recovery updater fetches a fix; staging it clears recovery.
        s.stage(&art("1.1.1"), &revoked).unwrap();
        assert_eq!(s.begin_boot(&revoked), Boot::Trial(v("1.1.1")));
    }

    #[test]
    fn rollback_target_is_current_never_an_older_previous() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.report_healthy().unwrap();
        s.stage(&art("1.2.0"), &none()).unwrap();
        s.begin_boot(&none());
        assert_eq!(s.begin_boot(&none()), Boot::RolledBack { failed: v("1.2.0"), boot: v("1.1.0") });
    }

    #[test]
    fn state_survives_serialisation_mid_trial() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 2);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        let mut back: InstallState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        back.begin_boot(&none());
        assert!(matches!(back.begin_boot(&none()), Boot::RolledBack { .. }));
    }

    #[test]
    fn artifact_for_another_platform_cannot_be_staged() {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        let other = VerifiedArtifact::for_tests(v("1.1.0"), "windows-x86_64");
        assert!(matches!(s.stage(&other, &none()), Err(Error::WrongPlatform { .. })));
    }

    #[test]
    fn attempts_are_clamped() {
        let s = InstallState::new(v("1.0.0"), "linux-x86_64", 1_000_000);
        assert_eq!(s.max_attempts, MAX_ATTEMPTS);
        assert_eq!(InstallState::new(v("1.0.0"), "linux-x86_64", 0).max_attempts, 1);
    }

    fn tampered(edit: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        let mut json = serde_json::to_value(&s).unwrap();
        edit(&mut json);
        json.to_string()
    }

    #[test]
    fn tampered_state_is_refused_on_load() {
        let cases: Vec<(&str, String)> = vec![
            // never roll back: a broken trial would boot forever
            ("huge max_attempts", tampered(|j| j["max_attempts"] = 4_000_000_000u32.into())),
            ("zero max_attempts", tampered(|j| j["max_attempts"] = 0.into())),
            ("attempts above max", tampered(|j| j["trial"]["attempts"] = 99.into())),
            // a "trial" that is really a downgrade
            ("trial below current", tampered(|j| j["trial"]["version"] = "0.9.0".into())),
            ("failed trial retried", tampered(|j| j["failed"] = serde_json::json!(["1.1.0"]))),
            ("previous newer than current", tampered(|j| j["previous"] = "2.0.0".into())),
            ("path in platform", tampered(|j| j["platform"] = "../x".into())),
            ("unknown field", tampered(|j| j["surprise"] = true.into())),
        ];
        for (what, json) in cases {
            assert!(serde_json::from_str::<InstallState>(&json).is_err(), "{what} should be refused");
            let (s, reset) = InstallState::restore(Some(json.as_bytes()), v("1.0.0"), "linux-x86_64", 3);
            assert!(reset, "{what}");
            assert_eq!((s.current(), s.trial()), (v("1.0.0"), None), "{what}");
        }
    }

    #[test]
    fn restore_keeps_valid_state_and_resets_missing_or_foreign_state() {
        let good = tampered(|_| {});
        let (s, reset) = InstallState::restore(Some(good.as_bytes()), v("1.0.0"), "linux-x86_64", 3);
        assert!(!reset);
        assert_eq!(s.trial().unwrap().attempts, 1, "attempt count must survive a restart");

        assert!(InstallState::restore(None, v("1.0.0"), "linux-x86_64", 3).1);
        assert!(InstallState::restore(Some(good.as_bytes()), v("1.0.0"), "windows-x86_64", 3).1);
        // State claiming an older current than the launcher knows is installed.
        assert!(InstallState::restore(Some(good.as_bytes()), v("1.0.5"), "linux-x86_64", 3).1);
    }

    #[test]
    fn reset_keeps_versions_that_already_failed() {
        // Review of #8: a corrupt file must not hand a failed version another trial.
        let mut s = InstallState::new(v("1.0.0"), "linux-x86_64", 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.begin_boot(&none()); // rolled back, 1.1.0 failed
        let mut json = serde_json::to_value(&s).unwrap();
        json["max_attempts"] = 0.into(); // now invalid
        let (mut r, reset) = InstallState::restore(Some(json.to_string().as_bytes()), v("1.0.0"), "linux-x86_64", 3);
        assert!(reset);
        assert!(r.has_failed(v("1.1.0")));
        assert_eq!(r.stage(&art("1.1.0"), &none()), Err(Error::PreviouslyFailed(v("1.1.0"))));
    }
}

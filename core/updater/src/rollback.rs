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

use crate::{Error, Version, VerifiedArtifact};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallState {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// many trial boots a new version gets (N in the spec).
    pub fn new(installed: Version, max_attempts: u32) -> Self {
        InstallState {
            current: installed,
            previous: None,
            trial: None,
            failed: BTreeSet::new(),
            needs_recovery: false,
            max_attempts: max_attempts.max(1),
        }
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
        let v = artifact.version;
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
        VerifiedArtifact { version: v(s), platform: "linux-x86_64".into(), sha256: "00".repeat(32), size: 1 }
    }

    fn none() -> BTreeSet<Version> {
        BTreeSet::new()
    }

    fn set(vs: &[&str]) -> BTreeSet<Version> {
        vs.iter().map(|s| v(s)).collect()
    }

    #[test]
    fn no_update_boots_current() {
        let mut s = InstallState::new(v("1.0.0"), 3);
        assert_eq!(s.begin_boot(&none()), Boot::Normal(v("1.0.0")));
        assert_eq!(s.report_healthy(), Err(Error::NoTrial));
    }

    #[test]
    fn healthy_trial_is_committed_and_previous_kept() {
        let mut s = InstallState::new(v("1.0.0"), 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        assert_eq!(s.report_healthy(), Ok(Committed { version: v("1.1.0"), discard: None }));
        assert_eq!(s.current(), v("1.1.0"));
        assert_eq!(s.previous(), Some(v("1.0.0")));
        assert_eq!(s.begin_boot(&none()), Boot::Normal(v("1.1.0")));
    }

    #[test]
    fn only_one_previous_version_is_kept() {
        let mut s = InstallState::new(v("1.0.0"), 3);
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
        let mut s = InstallState::new(v("1.0.0"), 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none()); // crashed
        s.begin_boot(&none()); // crashed
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        assert!(s.report_healthy().is_ok());
        assert_eq!(s.current(), v("1.1.0"));
    }

    #[test]
    fn n_failed_boots_roll_back_to_previous() {
        let mut s = InstallState::new(v("1.0.0"), 3);
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
        let mut s = InstallState::new(v("1.0.0"), 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.0")));
        assert!(matches!(s.begin_boot(&none()), Boot::RolledBack { .. }));
    }

    #[test]
    fn failed_version_is_not_retried_but_a_fix_is_accepted() {
        let mut s = InstallState::new(v("1.0.0"), 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.begin_boot(&none());
        assert_eq!(s.stage(&art("1.1.0"), &none()), Err(Error::PreviouslyFailed(v("1.1.0"))));
        s.stage(&art("1.1.1"), &none()).unwrap();
        assert_eq!(s.begin_boot(&none()), Boot::Trial(v("1.1.1")));
    }

    #[test]
    fn staging_rejects_old_same_and_revoked_versions() {
        let mut s = InstallState::new(v("1.1.0"), 3);
        assert!(matches!(s.stage(&art("1.0.0"), &none()), Err(Error::NotNewer { .. })));
        assert!(matches!(s.stage(&art("1.1.0"), &none()), Err(Error::NotNewer { .. })));
        assert_eq!(s.stage(&art("1.2.0"), &set(&["1.2.0"])), Err(Error::Revoked(v("1.2.0"))));
        assert_eq!(s.trial(), None);
    }

    #[test]
    fn newer_artifact_replaces_unproven_trial() {
        let mut s = InstallState::new(v("1.0.0"), 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.stage(&art("1.2.0"), &none()).unwrap();
        assert_eq!(s.trial(), Some(&Trial { version: v("1.2.0"), attempts: 0 }));
        assert!(matches!(s.stage(&art("1.1.5"), &none()), Err(Error::NotNewer { .. })));
    }

    #[test]
    fn restaging_same_version_keeps_attempt_count() {
        let mut s = InstallState::new(v("1.0.0"), 2);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.trial().unwrap().attempts, 1, "re-downloading must not reset the failure count");
    }

    #[test]
    fn trial_revoked_after_staging_is_dropped() {
        let mut s = InstallState::new(v("1.0.0"), 3);
        s.stage(&art("1.1.0"), &none()).unwrap();
        assert_eq!(s.begin_boot(&set(&["1.1.0"])), Boot::RolledBack { failed: v("1.1.0"), boot: v("1.0.0") });
    }

    #[test]
    fn never_roll_back_to_a_revoked_version() {
        let mut s = InstallState::new(v("1.0.0"), 1);
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
        let mut s = InstallState::new(v("1.0.0"), 1);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        s.report_healthy().unwrap();
        s.stage(&art("1.2.0"), &none()).unwrap();
        s.begin_boot(&none());
        assert_eq!(s.begin_boot(&none()), Boot::RolledBack { failed: v("1.2.0"), boot: v("1.1.0") });
    }

    #[test]
    fn state_survives_serialisation_mid_trial() {
        let mut s = InstallState::new(v("1.0.0"), 2);
        s.stage(&art("1.1.0"), &none()).unwrap();
        s.begin_boot(&none());
        let mut back: InstallState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        back.begin_boot(&none());
        assert!(matches!(back.begin_boot(&none()), Boot::RolledBack { .. }));
    }
}

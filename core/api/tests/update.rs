//! In-app update through the api (B8-5), with an in-memory `UpdateSource`:
//! the commands and events of CONFORMANCE.md "Proposed: in-app update".
//! The verification itself is `updater`'s; these check it is wired in.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use api::update::{manifest_channel, next_interval, UpdateSetup, Updates, CHECK_EVERY, CHECK_JITTER};
use api::*;
use common::{Peer, World, NOW_MS};
use serde_json::{json, Value};
use updater::source::{SourceError, UpdateSource};
use updater::{
    hash_reader, Artifact, Baseline, ManifestBody, SignedManifest, SigningKey, TrustRoot, UpdateDirs, Version, APP_EXE,
    FORMAT,
};

const PLATFORM: &str = "windows-x86_64";
const NOW: u64 = NOW_MS / 1000;

fn v(s: &str) -> Version {
    s.parse().unwrap()
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// A zip with the app exe, as release.yml packs the unpacked app folder.
fn payload() -> (Vec<u8>, u64) {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file(APP_EXE, zip::write::SimpleFileOptions::default()).unwrap();
    w.write_all(b"new app").unwrap();
    (w.finish().unwrap().into_inner(), 7)
}

/// One published release: a signed manifest and its payload.
#[derive(Clone)]
struct Release {
    manifest: Vec<u8>,
    payload: Vec<u8>,
}

fn release(channel: &str, version: &str, signer: &SigningKey) -> Release {
    let (payload, unpacked) = payload();
    let (sha256, size) = hash_reader(Cursor::new(&payload)).unwrap();
    let mut artifacts = BTreeMap::new();
    artifacts.insert(PLATFORM.to_string(), Artifact { sha256, size, unpacked_bytes: Some(unpacked) });
    let body = ManifestBody {
        format: FORMAT,
        channel: channel.into(),
        version: v(version),
        issued_at: NOW - 60,
        expires_at: NOW + 86_400,
        artifacts,
        revoked: Default::default(),
        notes: BTreeMap::from([("en".to_string(), "Fixes".to_string()), ("he".to_string(), "תיקונים".to_string())]),
    };
    let mut m = SignedManifest::new(&body).unwrap();
    m.sign(signer);
    Release { manifest: m.to_json().into_bytes(), payload }
}

/// The update host: one release per manifest channel, and a log of which
/// channel each source was built for.
#[derive(Clone, Default)]
struct Host {
    releases: Arc<Mutex<BTreeMap<String, Release>>>,
    built_for: Arc<Mutex<Vec<String>>>,
}

impl Host {
    fn publish(&self, r: Release, channel: &str) {
        self.releases.lock().unwrap().insert(channel.into(), r);
    }
}

/// `HttpsSource::release(channel)`'s stand-in: bound to one channel.
struct Fake {
    host: Host,
    channel: String,
}

impl UpdateSource for Fake {
    fn manifest(&self, channel: &str) -> Result<Vec<u8>, SourceError> {
        assert_eq!(channel, self.channel, "a source only serves its own channel");
        let releases = self.host.releases.lock().unwrap();
        releases.get(channel).map(|r| r.manifest.clone()).ok_or(SourceError::NotFound)
    }
    fn root_updates(&self) -> Result<Vec<Vec<u8>>, SourceError> {
        Ok(vec![])
    }
    fn artifact(&self, _v: Version, _p: &str, max_len: u64, out: &mut dyn Write) -> Result<(), SourceError> {
        let releases = self.host.releases.lock().unwrap();
        let r = releases.get(&self.channel).ok_or(SourceError::NotFound)?;
        if r.payload.len() as u64 > max_len {
            return Err(SourceError::TooLarge);
        }
        out.write_all(&r.payload).map_err(|e| SourceError::Offline(e.to_string()))
    }
}

struct Install {
    dir: PathBuf,
    host: Host,
    peer: Peer,
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A core running `installed`, with updates on and the shipped root being
/// `key(1)`.
fn install(tag: &str, installed: &str) -> Install {
    let dir = std::env::temp_dir().join(format!("mvs-api-update-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("install")).unwrap();
    fs::write(dir.join("install").join(APP_EXE), b"base").unwrap();
    fs::create_dir_all(dir.join("data")).unwrap();
    let host = Host::default();
    let peer = core_at(&dir, &host, installed);
    Install { dir, host, peer }
}

/// A core running `installed` in the install at `dir`.
fn core_at(dir: &std::path::Path, host: &Host, installed: &str) -> Peer {
    let dirs = UpdateDirs::new(dir.join("install"), dir.join("state"));
    let h = host.clone();
    let setup = UpdateSetup {
        dirs,
        core_data: dir.join("data"),
        platform: PLATFORM.into(),
        installed: v(installed),
        baseline: Baseline::default(),
        root: Some(TrustRoot::single(key(1).verifying_key()).unwrap()),
        source: Arc::new(move |channel| {
            h.built_for.lock().unwrap().push(channel.to_string());
            Ok(Box::new(Fake { host: h.clone(), channel: channel.into() }) as Box<dyn UpdateSource>)
        }),
    };
    let mut peer = World::new().peer();
    peer.core = peer.core.with_updates(Updates::new(setup));
    peer
}

fn call(peer: &Peer, cmd: &str) -> Result<Value, ErrorCode> {
    peer.core.handle_value(cmd, Value::Null).map_err(|e| e.code)
}

fn ready_events(peer: &Peer) -> Vec<UpdateInfo> {
    peer.events
        .take()
        .into_iter()
        .filter_map(|e| match e {
            Event::UpdateReady(info) => Some(info),
            _ => None,
        })
        .collect()
}

#[test]
fn a_newer_signed_version_is_offered_downloaded_and_staged() {
    let i = install("offered", "1.0.0");
    i.host.publish(release("alpha", "1.1.0", &key(1)), "alpha");

    assert_eq!(
        call(&i.peer, "checkForUpdate"),
        Ok(json!({ "status": "available", "current": "1.0.0", "available": "1.1.0", "freshness": "fresh" }))
    );
    assert_eq!(call(&i.peer, "applyUpdate"), Err(ErrorCode::NothingStaged));

    assert_eq!(call(&i.peer, "downloadUpdate"), Ok(json!({ "version": "1.1.0" })));
    assert_eq!(ready_events(&i.peer), vec![UpdateInfo { version: "1.1.0".into(), waiting_for_call: false }]);
    assert_eq!(fs::read(i.dir.join("install/versions/1.1.0").join(APP_EXE)).unwrap(), b"new app");
    assert_eq!(call(&i.peer, "checkForUpdate").unwrap()["status"], "staged");

    // "Restart to update": main relaunches through the launcher on ok.
    assert_eq!(call(&i.peer, "applyUpdate"), Ok(json!({ "version": "1.1.0" })));
    // Still the old version running, and it was never a trial: nothing to commit.
    assert_eq!(call(&i.peer, "markHealthy"), Ok(json!({ "committed": null })));
}

#[test]
fn the_background_loop_downloads_once_the_ui_is_up() {
    let i = install("background", "1.0.0");
    i.host.publish(release("alpha", "1.1.0", &key(1)), "alpha");
    let core = &i.peer.core;
    std::thread::scope(|s| {
        let looping = s.spawn(|| core.run_update_loop());
        // The first markHealthy (UI loaded) starts the first check at once,
        // not after FIRST_CHECK_AFTER.
        assert_eq!(call(&i.peer, "markHealthy"), Ok(json!({ "committed": null })));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut seen = vec![];
        while seen.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            seen = ready_events(&i.peer);
        }
        assert_eq!(seen, vec![UpdateInfo { version: "1.1.0".into(), waiting_for_call: false }]);
        core.stop_updates();
        looping.join().unwrap();
    });
}

#[test]
fn the_trial_version_commits_and_announces_its_notes() {
    let i = install("commit", "1.0.0");
    i.host.publish(release("alpha", "1.1.0", &key(1)), "alpha");
    call(&i.peer, "downloadUpdate").unwrap();
    // Relaunch: the launcher picks the staged version as a trial.
    let dirs = UpdateDirs::new(i.dir.join("install"), i.dir.join("state"));
    let plan = updater::launcher::plan(&dirs, &i.dir.join("data"), v("1.0.0"), PLATFORM).unwrap();
    assert_eq!(plan.boot, updater::Boot::Trial(v("1.1.0")));

    let new = core_at(&i.dir, &i.host, "1.1.0");
    assert_eq!(call(&new, "markHealthy"), Ok(json!({ "committed": "1.1.0" })));
    // No profile yet: Hebrew notes, as text.
    let installed: Vec<_> = new.events.take().into_iter().filter(|e| matches!(e, Event::UpdateInstalled { .. })).collect();
    assert_eq!(installed, vec![Event::UpdateInstalled { version: "1.1.0".into(), notes: Some("תיקונים".into()) }]);
    // Once only.
    assert_eq!(call(&new, "markHealthy"), Ok(json!({ "committed": null })));
}

#[test]
fn a_bad_signature_is_rejected_and_nothing_is_staged() {
    let i = install("forged", "1.0.0");
    i.host.publish(release("alpha", "1.1.0", &key(2)), "alpha");

    assert_eq!(call(&i.peer, "checkForUpdate"), Err(ErrorCode::InvalidUpdate));
    assert_eq!(call(&i.peer, "downloadUpdate"), Err(ErrorCode::InvalidUpdate));
    assert!(ready_events(&i.peer).is_empty());
    assert!(!i.dir.join("install/versions/1.1.0").exists());
    assert_eq!(call(&i.peer, "applyUpdate"), Err(ErrorCode::NothingStaged));
}

#[test]
fn a_tampered_payload_is_rejected() {
    let i = install("tampered", "1.0.0");
    let mut r = release("alpha", "1.1.0", &key(1));
    let last = r.payload.len() - 1;
    r.payload[last] ^= 1;
    i.host.publish(r, "alpha");

    assert_eq!(call(&i.peer, "downloadUpdate"), Err(ErrorCode::InvalidUpdate));
    assert!(ready_events(&i.peer).is_empty());
    assert_eq!(call(&i.peer, "applyUpdate"), Err(ErrorCode::NothingStaged));
}

#[test]
fn a_manifest_for_another_channel_is_rejected() {
    // The beta manifest served where the stable one should be.
    let i = install("swapped", "1.0.0");
    i.host.publish(release("beta", "1.1.0", &key(1)), "alpha");
    assert_eq!(call(&i.peer, "checkForUpdate"), Err(ErrorCode::InvalidUpdate));
}

#[test]
fn beta_gets_pre_releases_and_stable_does_not() {
    let i = install("channels", "1.0.0");
    // A pre-release on beta only; stable has nothing newer than what runs.
    i.host.publish(release("alpha", "1.0.0", &key(1)), "alpha");
    i.host.publish(release("beta", "1.1.0", &key(1)), "beta");

    assert_eq!(call(&i.peer, "checkForUpdate").unwrap()["status"], "up-to-date");
    assert_eq!(call(&i.peer, "downloadUpdate"), Err(ErrorCode::NothingToDownload));

    i.peer.core.set_update_channel(UpdateChannel::Beta).unwrap();
    assert_eq!(
        call(&i.peer, "checkForUpdate").unwrap(),
        json!({ "status": "available", "current": "1.0.0", "available": "1.1.0", "freshness": "fresh" })
    );
    assert_eq!(call(&i.peer, "downloadUpdate"), Ok(json!({ "version": "1.1.0" })));

    // Each channel has its own fixed-tag release (`update-<channel>`).
    assert_eq!(*i.host.built_for.lock().unwrap(), ["alpha", "beta"]);
}

#[test]
fn switching_back_to_stable_never_downgrades() {
    // A beta user on 0.3.0 switches to stable, which is still at 0.2.5.
    let i = install("no-downgrade", "0.3.0");
    i.host.publish(release("beta", "0.3.0", &key(1)), "beta");
    i.host.publish(release("alpha", "0.2.5", &key(1)), "alpha");
    i.peer.core.set_update_channel(UpdateChannel::Beta).unwrap();
    assert_eq!(call(&i.peer, "checkForUpdate").unwrap()["status"], "up-to-date");

    i.peer.core.set_update_channel(UpdateChannel::Stable).unwrap();
    // The running version is the floor: the older stable is refused (logged
    // only), never offered, downloaded or staged.
    assert_eq!(call(&i.peer, "checkForUpdate"), Err(ErrorCode::InvalidUpdate));
    assert_eq!(call(&i.peer, "downloadUpdate"), Err(ErrorCode::InvalidUpdate));
    assert!(ready_events(&i.peer).is_empty());
    assert!(!i.dir.join("install/versions/0.2.5").exists());
    assert_eq!(call(&i.peer, "applyUpdate"), Err(ErrorCode::NothingStaged));

    // Once stable passes 0.3.0, it updates as usual.
    i.host.publish(release("alpha", "0.4.0", &key(1)), "alpha");
    assert_eq!(call(&i.peer, "checkForUpdate").unwrap()["available"], "0.4.0");
}

#[test]
fn ui_channels_map_to_manifest_channels() {
    // Closed alpha: the UI's "stable" is the `alpha` manifest (§0).
    assert_eq!(manifest_channel(UpdateChannel::Stable), "alpha");
    assert_eq!(manifest_channel(UpdateChannel::Beta), "beta");
}

#[test]
fn nothing_published_is_up_to_date_and_never_checked_is_reported() {
    let i = install("empty", "1.0.0");
    assert_eq!(
        call(&i.peer, "checkForUpdate"),
        Ok(json!({ "status": "up-to-date", "current": "1.0.0", "available": null, "freshness": "never" }))
    );
}

#[test]
fn without_an_update_root_updates_are_off() {
    let peer = World::new().peer();
    for cmd in ["checkForUpdate", "downloadUpdate", "applyUpdate", "markHealthy"] {
        assert_eq!(call(&peer, cmd), Err(ErrorCode::NotImplemented), "{cmd}");
    }
    // No loop either: it returns at once.
    peer.core.run_update_loop();
}

#[test]
fn the_loop_stops_when_asked() {
    let i = install("stop", "1.0.0");
    i.peer.core.stop_updates();
    // Returns at once instead of waiting for the first check.
    i.peer.core.run_update_loop();
}

#[test]
fn checks_are_an_hour_apart_with_five_minutes_of_jitter() {
    // Owner, 2026-11-11: a published update should show within about an hour.
    assert_eq!((CHECK_EVERY.as_secs(), CHECK_JITTER.as_secs()), (3600, 300));
    let lo = CHECK_EVERY - CHECK_JITTER;
    let hi = CHECK_EVERY + CHECK_JITTER;
    let spread: std::collections::BTreeSet<_> = (0..50).map(|_| next_interval()).collect();
    assert!(spread.iter().all(|d| *d >= lo && *d <= hi));
    assert!(spread.len() > 1, "jittered");
}

#[test]
fn update_error_codes_and_events_match_conformance() {
    let codes = [
        (ErrorCode::Offline, "offline"),
        (ErrorCode::InvalidUpdate, "invalid-update"),
        (ErrorCode::CheckClock, "check-clock"),
        (ErrorCode::NothingToDownload, "nothing-to-download"),
        (ErrorCode::NothingStaged, "nothing-staged"),
        (ErrorCode::DiskFull, "disk-full"),
    ];
    for (code, text) in codes {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(text));
    }
    let installed = Event::UpdateInstalled { version: "1.1.0".into(), notes: Some("Fixes".into()) };
    assert_eq!(
        serde_json::to_value(installed).unwrap(),
        json!({ "event": "update.installed", "payload": { "version": "1.1.0", "notes": "Fixes" } })
    );
    let rolled = Event::UpdateRolledBack { failed: "1.1.0".into(), running: "1.0.0".into() };
    assert_eq!(
        serde_json::to_value(rolled).unwrap(),
        json!({ "event": "update.rolledBack", "payload": { "failed": "1.1.0", "running": "1.0.0" } })
    );
}

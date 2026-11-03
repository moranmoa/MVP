//! Two cores exchange records through a real relay, over `SyncNetwork`.
//! Needs a relay (`RELAY`, its multiaddr) and `--ignored`. Run it with
//! `bash core/api/tests/two_peers.sh`, which starts a throwaway relay on its
//! own Docker network (as `infra/test/relay-catchup.sh` does) and runs this
//! test next to it. Never point it at the live relay or its network.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use api::syncnet::SyncNetwork;
use api::*;
use identity::MemoryStore;

struct Peer {
    core: Core,
    _dir: std::path::PathBuf,
}

fn peer(name: &str, relay: &str) -> Peer {
    let dir = std::env::temp_dir().join(format!("mvs-two-peers-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let net =
        SyncNetwork::start(relay.parse().unwrap(), &dir, Arc::new(MemberGate::new())).unwrap();
    let core = Core::open(
        Config { db_path: Some(dir.join("core.db")), app_version: "test".into() },
        Ports {
            secrets: Arc::new(MemoryStore::new()),
            network: Arc::new(net),
            events: Arc::new(CollectingSink::new()),
            clock: Arc::new(SystemClock),
            cipher: Arc::new(PendingGroupKeys),
        },
    )
    .unwrap();
    Peer { core, _dir: dir }
}

fn sign_up(peer: &Peer, name: &str) {
    let input = NewIdentityInput {
        locale: Locale::He,
        country: "IL".into(),
        birth_date: "1990-05-01".into(),
    };
    let words = peer.core.create_identity(input).unwrap().words;
    let positions = peer.core.get_recovery_challenge().unwrap().positions;
    let answers: BTreeMap<usize, String> =
        positions.iter().map(|p| (*p, words[p - 1].clone())).collect();
    assert!(peer.core.check_recovery_words(answers).unwrap().ok);
    let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
    peer.core.update_profile(profile).unwrap();
}

fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn two_users_meet_through_a_relay() {
    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    let owner = peer("owner", &relay);
    let guest = peer("guest", &relay);
    sign_up(&owner, "Owner");
    sign_up(&guest, "Guest");

    let server = owner.core.create_server("Two peers", AgeRestriction::None).unwrap();
    eprintln!("server created; waiting for the guest to find it");
    eventually("the guest to join", || {
        guest.core.join_server(&server.invite_link).unwrap();
        guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
    });
    eprintln!("guest joined; waiting for the owner to see them");
    eventually("the owner to see two members", || {
        owner.core.sync_now().unwrap();
        owner.core.list_members(&server.id).unwrap().len() == 2
    });
    let members = owner.core.list_members(&server.id).unwrap();
    assert!(members.iter().any(|m| m.display_name == "Guest"));
    eprintln!("both sides agree: {} members", members.len());
}

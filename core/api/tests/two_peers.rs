//! Two users chat through a real relay, over `SyncNetwork` and the real
//! group cipher; a third, non-member peer gets the records but can't read.
//! Needs a relay (`RELAY`, its multiaddr) and `--ignored`. Run it with
//! `bash core/api/tests/two_peers.sh`, which starts a throwaway relay on its
//! own Docker network (as `infra/test/relay-catchup.sh` does) and runs this
//! test next to it. Never point it at the live relay or its network.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use api::crypto::GroupKeysCipher;
use api::syncnet::SyncNetwork;
use api::*;
use identity::MemoryStore;
use records::{Kind, Scope};

/// BIP-39 test vectors: fixed identities for the three peers.
const OWNER: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const GUEST: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";
const STRANGER: &str =
    "letter advice cage absurd amount doctor acoustic avoid letter advice cage above";

struct Peer {
    core: Core,
    events: Arc<CollectingSink>,
    net: Arc<SyncNetwork>,
    _cipher: Arc<GroupKeysCipher>,
    _dir: std::path::PathBuf,
}

fn peer(name: &str, words: &str, relay: &str) -> Peer {
    let dir = std::env::temp_dir().join(format!("mvs-two-peers-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let net = Arc::new(
        SyncNetwork::start(relay.parse().unwrap(), &dir, Arc::new(MemberGate::new())).unwrap(),
    );
    let events = Arc::new(CollectingSink::new());
    // Locked: the core unlocks it when the identity is restored.
    let cipher = Arc::new(GroupKeysCipher::locked());
    let core = Core::open(
        Config { db_path: Some(dir.join("core.db")), app_version: "test".into() },
        Ports {
            secrets: Arc::new(MemoryStore::new()),
            network: net.clone(),
            events: events.clone(),
            clock: Arc::new(SystemClock),
            cipher: cipher.clone(),
        },
    )
    .unwrap();
    core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
    let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
    core.update_profile(profile).unwrap();
    Peer { core, events, net, _cipher: cipher, _dir: dir }
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
fn two_users_chat_through_a_relay() {
    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    let owner = peer("Owner", OWNER, &relay);
    let guest = peer("Guest", GUEST, &relay);
    let stranger = peer("Stranger", STRANGER, &relay);

    let server = owner.core.create_server("Two peers", AgeRestriction::None).unwrap();
    eventually("the guest to join", || {
        guest.core.join_server(&server.invite_link).unwrap();
        guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
    });
    eventually("the owner to see two members", || {
        owner.core.sync_now().unwrap();
        owner.core.list_members(&server.id).unwrap().len() == 2
    });
    eprintln!("both are members; the owner writes");

    // The owner's first message rotates the channel key for both members.
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    let sent = owner.core.send_message(&channel, "שלום, מה נשמע?", None).unwrap();
    assert_eq!(sent.body, "שלום, מה נשמע?");
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 1);

    eventually("the guest to read it", || {
        guest.core.sync_now().unwrap();
        guest.core.get_message_count(&channel).unwrap() == 1
    });
    let range = guest.core.get_messages(&channel, 0, 10).unwrap();
    assert_eq!(range.messages[0].body, "שלום, מה נשמע?");
    assert_eq!(range.messages[0].author_id, server.owner_id);
    eprintln!("the guest read it; the guest answers");

    // A plain member writes under the key the owner made.
    guest.core.send_message(&channel, "hi!", None).unwrap();
    eventually("the owner to read the answer", || {
        owner.core.sync_now().unwrap();
        owner.core.get_message_count(&channel).unwrap() == 2
    });
    let bodies: Vec<String> = owner
        .core
        .get_messages(&channel, 0, 10)
        .unwrap()
        .messages
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert_eq!(bodies, ["שלום, מה נשמע?", "hi!"]);

    // B6-p over the relay: the owner edits its message and deletes the
    // guest's; both cores prune, in their own db and in sync's store.
    let server_key =
        records::ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let messages = owner.net.fetch(server_key);
    let mut old: Vec<Vec<u8>> = messages
        .iter()
        .filter(|r| r.kind() == Kind::Message)
        .map(|r| r.body()[..r.body().len().min(64)].to_vec())
        .collect();
    assert_eq!(old.len(), 2);
    owner.core.edit_message(&sent.id, "edited").unwrap();
    let hi = owner.core.get_messages(&channel, 0, 10).unwrap().messages[1].id.clone();
    owner.core.delete_message(&hi).unwrap();
    eventually("the guest to see the edit and the delete", || {
        guest.core.sync_now().unwrap();
        let got = guest.core.get_messages(&channel, 0, 10).unwrap().messages;
        got.len() == 2 && got[0].body == "edited" && got[1].deleted == Some(true)
    });
    owner.core.sync_now().unwrap();
    old.dedup();
    for (name, dir) in [("owner", &owner._dir), ("guest", &guest._dir)] {
        // Right after the prune, with the peers still running: the db files
        // and their WAL and rollback journals.
        for file in [
            "core.db",
            "core.db-wal",
            "core.db-journal",
            "store.db",
            "store.db-wal",
            "store.db-journal",
        ] {
            let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
            for body in &old {
                assert!(
                    !bytes.windows(body.len()).any(|w| w == body.as_slice()),
                    "{name}'s {file} still holds an old ciphertext"
                );
            }
        }
    }
    eprintln!("B6-p: old bodies gone from both dbs");

    // GAP-4: a stranger with only the server ID gets nothing. Even with the
    // secret from the link it gets the control log (a joiner needs it
    // before its join exists) but no channel record: the read gate serves
    // channels to members only.
    let server_id =
        records::ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    std::thread::sleep(Duration::from_secs(3));
    assert!(stranger.net.fetch(server_id).is_empty(), "no secret, nothing");
    let secret: [u8; 32] =
        hex::decode(server.invite_link.rsplit('/').next().unwrap()).unwrap().try_into().unwrap();
    let general = BTreeSet::from([records::ChannelId::from_bytes(
        hex::decode(&channel).unwrap().try_into().unwrap(),
    )]);
    stranger.net.open_server(server_id, &secret, &general);
    eventually("the stranger to get the control log", || {
        stranger.net.fetch(server_id).iter().any(|r| r.kind() == Kind::Genesis)
    });
    std::thread::sleep(Duration::from_secs(5));
    let held = stranger.net.fetch(server_id);
    assert!(
        held.iter().all(|r| r.scope() == Scope::Control),
        "a non-member got channel records: {}",
        held.iter().filter(|r| r.scope() != Scope::Control).count()
    );
    eprintln!("the stranger gets no channel records: done");
}

/// The app's background sync (`Core::run_sync_loop`, as mvs-core runs it):
/// the guest gets the owner's message, with `message.added`, and nobody
/// calls `sync_now`. Over the direct link, messages arrive live (gossip),
/// not on the next 5 s pull (the v0.2.3 regression).
#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn the_background_sync_delivers_without_sync_now() {
    background_delivery("Loop", false);
}

/// The same, when the smaller peer ID can't dial and the larger side's
/// fallback dial makes the direct link (as in devops' v0.2.3 smoke).
#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn the_background_sync_delivers_live_over_the_fallback_link() {
    background_delivery("Fallback", true);
}

/// Live delivery: well under the 5 s pull.
const LIVE: Duration = Duration::from_secs(1);

fn background_delivery(name: &str, fallback: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};

    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    let owner = peer(&format!("{name}Owner"), OWNER, &relay);
    let guest = peer(&format!("{name}Guest"), GUEST, &relay);
    if fallback {
        let smaller = if owner.net.peer_id() < guest.net.peer_id() { &owner } else { &guest };
        smaller.net.test_no_direct_dials();
    }
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for p in [&owner, &guest] {
            let hints = p.net.take_hints().expect("hints");
            let stop = &stop;
            scope.spawn(move || p.core.run_sync_loop(&hints, stop));
        }

        let server = owner.core.create_server("Loop", AgeRestriction::None).unwrap();
        eventually("the guest to join", || {
            guest.core.join_server(&server.invite_link).unwrap();
            guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
        });
        eventually("the owner's loop to see the guest", || {
            owner.core.list_members(&server.id).unwrap().len() == 2
        });
        let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
        // M2: the owner sees the guest online through sync's presence.
        let guest_id = guest.core.get_identity().unwrap().unwrap().id;
        eventually("the owner to see the guest online", || {
            owner
                .core
                .list_members(&server.id)
                .unwrap()
                .iter()
                .any(|m| m.id == guest_id && m.presence == Presence::Online)
        });
        // The regression showed only once the link went direct.
        eventually("a direct link", || {
            owner.net.direct_peers().contains(&guest.net.peer_id())
                && guest.net.direct_peers().contains(&owner.net.peer_id())
        });
        guest.events.take();
        let start = Instant::now();
        let sent = owner.core.send_message(&channel, "no sync_now here", None).unwrap();
        while guest.core.get_message_count(&channel).unwrap_or(0) != 1 {
            assert!(start.elapsed() < Duration::from_secs(90), "the guest's loop never brought the message");
            std::thread::sleep(Duration::from_millis(10));
        }
        let took = start.elapsed();
        eprintln!("{name}: owner to guest in {took:?}");
        assert!(took < LIVE, "owner to guest took {took:?}: not live, only on a pull");
        let added: Vec<_> = guest
            .events
            .take()
            .into_iter()
            .filter_map(|e| match e {
                Event::MessageAdded(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(added.len(), 1, "one message.added: {added:?}");
        assert_eq!(
            (added[0].id.as_str(), added[0].body.as_str()),
            (sent.id.as_str(), "no sync_now here")
        );
        // B6: the guest's node acknowledged it, so the owner sees `received`.
        eventually("the owner's message to be received", || {
            owner.core.get_messages(&channel, 0, 1).unwrap().messages[0].send_state
                == Some(SendState::Received)
        });
        // And back: the guest's post reaches the owner live too.
        let start = Instant::now();
        guest.core.send_message(&channel, "and back", None).unwrap();
        while owner.core.get_message_count(&channel).unwrap_or(0) != 2 {
            assert!(start.elapsed() < Duration::from_secs(90), "the owner's loop never brought the reply");
            std::thread::sleep(Duration::from_millis(10));
        }
        let took = start.elapsed();
        eprintln!("{name}: guest to owner in {took:?}");
        assert!(took < LIVE, "guest to owner took {took:?}: not live, only on a pull");

        stop.store(true, Ordering::SeqCst);
        owner.net.wake();
        guest.net.wake();
    });
    eprintln!("background sync ({name}): done");
}

/// v0.2.4 join flake: the relay's DHT still lists many peers that have gone
/// (app installs, earlier runs), so the peers closest to the joiner's own ID
/// need not include the inviter. The joiner, started before the inviter has
/// its relay reservation, still finds it, through the server's provider key.
#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn a_joiner_finds_the_inviter_past_stale_peers() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).try_init();
    let stale = stale_peers();
    let peers: Vec<_> = (0..stale)
        .map(|i| {
            let dir = std::env::temp_dir().join(format!("mvs-two-peers-{}-stale{i}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            SyncNetwork::start(relay.parse().unwrap(), &dir, Arc::new(MemberGate::new())).unwrap()
        })
        .collect();
    // Long enough to get a reservation and into the relay's DHT; then gone.
    std::thread::sleep(Duration::from_secs(10));
    drop(peers);

    let owner = peer("StaleOwner", OWNER, &relay);
    let server = owner.core.create_server("Stale", AgeRestriction::None).unwrap();
    // The joiner starts right away: the owner may have no reservation yet.
    let guest = peer("StaleGuest", GUEST, &relay);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for p in [&owner, &guest] {
            let hints = p.net.take_hints().expect("hints");
            let stop = &stop;
            scope.spawn(move || p.core.run_sync_loop(&hints, stop));
        }
        let start = Instant::now();
        let joined = || guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id);
        while !joined() && start.elapsed() < JOIN_WITHIN {
            guest.core.join_server(&server.invite_link).unwrap();
        }
        let took = start.elapsed();
        stop.store(true, Ordering::SeqCst);
        owner.net.wake();
        guest.net.wake();
        assert!(joined(), "the guest never found the owner past {stale} stale peers ({took:?})");
        eprintln!("joined past {stale} stale peers in {took:?}");
    });
}

/// Gone peers the relay's DHT still lists: well over its 20 closest
/// (`STALE_PEERS` overrides it).
fn stale_peers() -> usize {
    std::env::var("STALE_PEERS").ok().and_then(|n| n.parse().ok()).unwrap_or(60)
}
/// The app's join screen gives up after 120 s.
const JOIN_WITHIN: Duration = Duration::from_secs(45);

/// v0.2.0 live smoke: B joins A's server by invite and writes first; A never
/// sends. A's background sync sees the join and keys the channel for B, so
/// B's send is allowed and A reads it. Nobody calls `sync_now`.
#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn a_joiner_writes_first_through_a_relay() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    let owner = peer("SmokeA", OWNER, &relay);
    let guest = peer("SmokeB", GUEST, &relay);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for p in [&owner, &guest] {
            let hints = p.net.take_hints().expect("hints");
            let stop = &stop;
            scope.spawn(move || p.core.run_sync_loop(&hints, stop));
        }
        let server = owner.core.create_server("Smoke", AgeRestriction::None).unwrap();
        eventually("B to join", || {
            guest.core.join_server(&server.invite_link).unwrap();
            guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
        });
        let channel = guest.core.list_channels(&server.id).unwrap().remove(0).id;
        eventually("B to be allowed to write", || {
            guest.core.send_message(&channel, "hello from B", None).is_ok()
        });
        eventually("A to read B's message", || {
            owner
                .core
                .get_messages(&channel, 0, 9)
                .unwrap()
                .messages
                .iter()
                .any(|m| m.body == "hello from B")
        });
        stop.store(true, Ordering::SeqCst);
        owner.net.wake();
        guest.net.wake();
    });
    eprintln!("smoke: B wrote first, A read it");
}

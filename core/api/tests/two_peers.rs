//! Two users chat through a real relay, over `SyncNetwork` and the real
//! group cipher; a third, non-member peer gets the records but can't read.
//! Needs a relay (`RELAY`, its multiaddr) and `--ignored`. Run it with
//! `bash core/api/tests/two_peers.sh`, which starts a throwaway relay on its
//! own Docker network (as `infra/test/relay-catchup.sh` does) and runs this
//! test next to it. Never point it at the live relay or its network.
//!
//! The voice test (`two_members_join_a_call_and_signal_over_sync`) runs its
//! own in-process relay instead, so it runs with the other tests.

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

/// T8-02: a short link joins over real sync: the guest, a stranger with no
/// secret, asks for the envelope (`/mvs/invite/1`), gets the owner's
/// publication, opens and checks it, and joins.
#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn a_restore_on_a_fresh_device_brings_the_servers_back() {
    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    // My first device: a server, and its backup (the login round writes it).
    let first = peer("BackupFirst", OWNER, &relay);
    let server = first.core.create_server("Backed up", AgeRestriction::None).unwrap();
    first.core.sync_now().unwrap();
    // A clean machine, the same words: the server comes back from the
    // network (G-91: `/mvs/backup/1`, my first device as the holder).
    let fresh = peer("BackupFresh", OWNER, &relay);
    eventually("the fresh device to get my server back", || {
        let _ = first.core.sync_now();
        let _ = fresh.core.sync_now();
        fresh.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
    });
    eprintln!("restore from the network: done");
}

/// T8-02 (continued): see above.
#[test]
#[ignore = "needs a relay: set RELAY and run with --ignored"]
fn a_short_link_joins_through_a_relay() {
    let Ok(relay) = std::env::var("RELAY") else {
        eprintln!("RELAY not set; skipped");
        return;
    };
    let owner = peer("ShortOwner", OWNER, &relay);
    let guest = peer("ShortGuest", GUEST, &relay);
    let server = owner.core.create_server("Short", AgeRestriction::None).unwrap();
    let made = owner.core.create_invite(&server.id, None, None).unwrap();
    assert!(made.short_link.len() <= "mvs://join/".len() + 51);
    eventually("the guest to join by the short link", || {
        guest.core.join_server(&made.short_link).unwrap();
        guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
    });
    eventually("the owner to see two members", || {
        owner.core.sync_now().unwrap();
        owner.core.list_members(&server.id).unwrap().len() == 2
    });
    eprintln!("short link: done");
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

// ---- voice over sync (voice v1, piece A) -----------------------------------

const SDP: &str = "v=0\r\na=fingerprint:sha-256 AB:CD:EF\r\na=setup:actpass\r\n";

#[derive(libp2p::swarm::NetworkBehaviour)]
struct RelayBehaviour {
    relay: libp2p::relay::Behaviour,
    identify: libp2p::identify::Behaviour,
    ping: libp2p::ping::Behaviour,
    kad: libp2p::kad::Behaviour<libp2p::kad::store::MemoryStore>,
}

/// A relay like `infra/relay` on loopback (as `core/sync/tests/relay.rs`
/// starts), with only circuit addresses in its DHT: peers meet through it.
fn start_relay(runtime: &tokio::runtime::Runtime) -> String {
    use libp2p::futures::StreamExt;
    use libp2p::swarm::SwarmEvent;
    use libp2p::{identify, kad, noise, ping, relay, tcp, yamux, Multiaddr, StreamProtocol, SwarmBuilder};
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let _guard = runtime.enter();
    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)
        .unwrap()
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            let mut kad = kad::Behaviour::with_config(
                peer_id,
                kad::store::MemoryStore::new(peer_id),
                kad::Config::new(StreamProtocol::new("/mvs/kad/1.0.0")),
            );
            kad.set_mode(Some(kad::Mode::Server));
            RelayBehaviour {
                relay: relay::Behaviour::new(peer_id, relay::Config::default()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/mvs/0.1.0".into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
                kad,
            }
        })
        .unwrap()
        .build();
    let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap();
    swarm.listen_on(addr.clone()).unwrap();
    swarm.add_external_address(addr.clone());
    let me = *swarm.local_peer_id();
    runtime.spawn(async move {
        loop {
            if let SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) = swarm.select_next_some().await
            {
                for addr in info.listen_addrs {
                    if addr.iter().any(|p| p == libp2p::multiaddr::Protocol::P2pCircuit) {
                        swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                    }
                }
            }
        }
    });
    format!("{addr}/p2p/{me}")
}

fn call_states(events: &[Event]) -> Vec<&CallState> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CallState(state) => Some(state),
            _ => None,
        })
        .collect()
}

/// Voice A end to end: two members on `SyncNetwork` (the real node, a
/// relay between them) join a call, see each other, and an offer goes from
/// one to the other through sync's member-only `/mvs/voice/1`, verified.
#[test]
fn two_members_join_a_call_and_signal_over_sync() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let relay = start_relay(&runtime);
    let owner = peer("VoiceOwner", OWNER, &relay);
    let guest = peer("VoiceGuest", GUEST, &relay);
    let owner_id = owner.core.get_identity().unwrap().unwrap().id;
    let guest_id = guest.core.get_identity().unwrap().unwrap().id;

    let server = owner.core.create_server("Voice", AgeRestriction::None).unwrap();
    eventually("the guest to join", || {
        let _ = guest.core.join_server(&server.invite_link);
        guest.core.list_servers().unwrap().iter().any(|s| s.id == server.id)
    });
    eventually("the owner to see two members", || {
        owner.core.sync_now().unwrap();
        owner.core.list_members(&server.id).unwrap().len() == 2
    });
    // Calls happen only in the server's voice room.
    let room = owner.core.get_voice_room(&server.id).unwrap().room_id;

    for p in [&owner, &guest] {
        p.core.set_voice(p.net.clone());
    }
    let inboxes = [owner.net.take_voice().expect("signals"), guest.net.take_voice().expect("signals")];
    // What `deliver_voice` does on its own thread in mvs-core, by hand here.
    let deliver = || {
        for (inbox, p) in inboxes.iter().zip([&owner, &guest]) {
            while let Ok((from, blob)) = inbox.try_recv() {
                p.core.voice_received(Some(from), &blob);
            }
        }
    };
    // Each has the other bound as a member: online on the server.
    let server_key =
        records::ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    eventually("both online to each other", || {
        let o = owner.net.online(server_key).unwrap_or_default();
        let g = guest.net.online(server_key).unwrap_or_default();
        !o.is_empty() && !g.is_empty()
    });

    owner.core.join_call(&room).unwrap();
    guest.core.join_call(&room).unwrap();
    let mut seen = Vec::new();
    eventually("the owner to see the guest in the call", || {
        deliver();
        owner.core.voice_tick();
        guest.core.voice_tick();
        seen.extend(owner.events.take());
        call_states(&seen).last().is_some_and(|s| s.participants.len() == 2)
    });
    let last = call_states(&seen).last().copied().unwrap().clone();
    assert!(last.participants.iter().any(|p| p.identity == guest_id), "{last:?}");

    guest.core.send_signal(&owner_id, SignalKind::Offer, SDP.into()).unwrap();
    let mut signals = Vec::new();
    eventually("the offer to reach the owner", || {
        deliver();
        signals.extend(owner.events.take().into_iter().filter(|e| matches!(e, Event::CallSignal { .. })));
        !signals.is_empty()
    });
    assert_eq!(
        signals,
        [Event::CallSignal {
            from: guest_id.clone(),
            kind: SignalKind::Offer,
            payload: SDP.into(),
            fingerprint_verified: true,
        }]
    );
    owner.core.leave_call().unwrap();
    guest.core.leave_call().unwrap();
}

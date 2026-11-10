//! Relay behaviour (M2 criteria), against an in-process relay that can be
//! restarted: a peer that comes online catches up as soon as it has a relay
//! reservation, and a node gets a new reservation after the relay restarts.
//! The Docker harness can't restart the shared infra relay, hence this test.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use libp2p::{
    identify, identity, kad, noise, ping, relay, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux, Multiaddr,
    PeerId, StreamProtocol, SwarmBuilder,
};
use records::{ChannelId, Draft, Kind, RecordId, Scope, ServerId, SigningKey};
use sync::{
    log_name, AcceptAll, Access, ControlLog, Node, NodeConfig, NodeEvent, ReadGate, Record, ServerSecret, VoiceError,
    MAX_VOICE_BLOB,
};
use tokio::sync::mpsc;

#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
}

/// A relay like `infra/relay` (circuits, identify, `/mvs/kad/1.0.0`), on a
/// fixed key and port so a restart looks the same to its clients.
fn start_relay(key: identity::Keypair, port: u16) -> tokio::task::JoinHandle<()> {
    start_relay_counting(key, port, Arc::default(), false)
}

/// Also records the destination of every circuit request (accepted or
/// denied), as the live relay's circuit counters do.
fn start_relay_counting(
    key: identity::Keypair,
    port: u16,
    circuits: Arc<std::sync::Mutex<Vec<(PeerId, bool)>>>,
    circuits_only: bool,
) -> tokio::task::JoinHandle<()> {
    let mut swarm = SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)
        .unwrap()
        // QUIC on the same port number, as infra/relay listens on 4001/udp.
        .with_quic()
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            let mut kad = kad::Behaviour::with_config(
                peer_id,
                kad::store::MemoryStore::new(peer_id),
                kad::Config::new(StreamProtocol::new("/mvs/kad/1.0.0")),
            );
            kad.set_mode(Some(kad::Mode::Server));
            RelayBehaviour {
                relay: relay::Behaviour::new(
                    peer_id,
                    // R11, as on the live relay.
                    relay::Config {
                        max_circuits_per_peer: 8,
                        max_circuit_duration: Duration::from_secs(3600),
                        max_circuit_bytes: 32 << 20,
                        ..relay::Config::default()
                    },
                ),
                identify: identify::Behaviour::new(identify::Config::new("/mvs/0.1.0".into(), key.public())),
                ping: ping::Behaviour::new(ping::Config::new()),
                kad,
            }
        })
        .unwrap()
        .build();
    let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap();
    swarm.listen_on(addr.clone()).unwrap();
    // Reservations carry the relay's external addresses; a client refuses
    // one without (infra/relay sets them from EXTERNAL_ADDRS).
    swarm.add_external_address(addr);
    // Reached over QUIC only as a bootstrap address (the punch test), so
    // reservations keep the TCP address alone.
    swarm.listen_on(format!("/ip4/127.0.0.1/udp/{port}/quic-v1").parse().unwrap()).unwrap();
    tokio::spawn(async move {
        loop {
            match swarm.select_next_some().await {
                SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                    for addr in info.listen_addrs {
                        if circuits_only && !addr.iter().any(|p| p == libp2p::multiaddr::Protocol::P2pCircuit) {
                            continue;
                        }
                        swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                    }
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(
                    relay::Event::CircuitReqAccepted { dst_peer_id, .. },
                )) => circuits.lock().unwrap().push((dst_peer_id, true)),
                SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(relay::Event::CircuitReqDenied { dst_peer_id, .. })) => {
                    circuits.lock().unwrap().push((dst_peer_id, false))
                }
                _ => {}
            }
        }
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn test_server() -> (ServerId, ChannelId) {
    (ServerId::from_bytes([5; 32]), ChannelId::from_bytes([6; 32]))
}

fn messages(n: u64) -> Vec<Record> {
    let (server, channel) = test_server();
    let key = SigningKey::from_bytes(&[7; 32]);
    let mut out: Vec<Record> = Vec::new();
    for seq in 0..n {
        let deps: Vec<RecordId> = out.last().map(|r| vec![r.id()]).unwrap_or_default();
        let draft = Draft {
            server,
            scope: Scope::Channel(channel),
            seq,
            deps,
            auth_deps: vec![],
            ts: seq,
            kind: Kind::Message,
            body: seq.to_le_bytes().to_vec(),
        };
        out.push(draft.sign(&key).unwrap());
    }
    out
}

/// A node that knows only the relay, plus a channel of the times it got a
/// relay reservation.
async fn node(name: &str, relay: &Multiaddr) -> (Node, mpsc::UnboundedReceiver<Instant>) {
    node_lan(name, relay, true).await
}

/// Like [`node`]; with `allow_lan` off, peers on this host only meet through
/// relay circuits, as two users behind NAT do.
async fn node_lan(name: &str, relay: &Multiaddr, allow_lan: bool) -> (Node, mpsc::UnboundedReceiver<Instant>) {
    let (server, channel) = test_server();
    let dir = std::env::temp_dir().join(format!("sync-relay-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = NodeConfig {
        relay: relay.clone(),
        data_dir: dir,
        logs: vec![log_name(server, Scope::Channel(channel))],
        authorizer: Arc::new(AcceptAll),
        listen: "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
        bootstrap: vec![],
        allow_lan,
        relay_only: false,
    };
    let (node, mut events) = Node::start(config).await.unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if matches!(event, NodeEvent::RelayReserved) {
                let _ = tx.send(Instant::now());
            }
        }
    });
    (node, rx)
}

async fn reserved_within(rx: &mut mpsc::UnboundedReceiver<Instant>, limit: Duration) -> Instant {
    tokio::time::timeout(limit, rx.recv()).await.expect("no relay reservation in time").unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_coming_online_catches_up_within_30_s_of_its_reservation() {
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let _relay = start_relay(relay_key, port);

    let (a, mut a_reserved) = node("a", &relay).await;
    reserved_within(&mut a_reserved, Duration::from_secs(20)).await;
    for record in messages(50) {
        a.append(record).await.unwrap();
    }

    // B was offline while A wrote; it comes online knowing only the relay.
    let (b, mut b_reserved) = node("b", &relay).await;
    let reserved = reserved_within(&mut b_reserved, Duration::from_secs(20)).await;
    let (server, channel) = test_server();
    let log = log_name(server, Scope::Channel(channel));
    while b.state(&log).await.unwrap().count < 50 {
        assert!(reserved.elapsed() < Duration::from_secs(30), "B didn't catch up within 30 s of its reservation");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!("caught up 50 messages {:?} after the reservation", reserved.elapsed());
    assert_eq!(b.state(&log).await.unwrap().hash, a.state(&log).await.unwrap().hash);
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_reserves_again_within_a_minute_of_a_relay_restart() {
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let running = start_relay(relay_key.clone(), port);

    let (b, mut reserved) = node("restart", &relay).await;
    reserved_within(&mut reserved, Duration::from_secs(20)).await;

    // The relay goes away for 5 s (connections drop), then comes back with
    // the same key and port.
    running.abort();
    let _ = running.await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let back = Instant::now();
    let _relay = start_relay(relay_key, port);
    let again = reserved_within(&mut reserved, Duration::from_secs(60)).await;
    eprintln!("reserved again {:?} after the relay came back", again - back);
    b.shutdown().await.unwrap();
}

/// The live relay saw circuits_denied climb: clients kept asking for
/// circuits to peers whose reservation had ended. A peer that goes offline
/// stays in the relay's DHT table, so every discovery round (Kademlia's own
/// lookups included) asked for a circuit to it. Count those requests.
#[tokio::test(flavor = "multi_thread")]
async fn an_offline_peer_is_not_asked_for_circuits_again_and_again() {
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _relay = start_relay_counting(relay_key, port, circuits.clone(), false);

    // C reserves, so the relay's table has its circuit address; then it goes.
    let (c, mut c_reserved) = node("gone", &relay).await;
    reserved_within(&mut c_reserved, Duration::from_secs(20)).await;
    tokio::time::sleep(Duration::from_secs(2)).await; // identify reaches the relay
    let gone = c.peer_id();
    c.shutdown().await.unwrap();

    // A keeps discovering for 30 s.
    let (a, mut a_reserved) = node("stays", &relay).await;
    reserved_within(&mut a_reserved, Duration::from_secs(20)).await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    let asked = circuits.lock().unwrap().iter().filter(|(dst, _)| *dst == gone).count();
    eprintln!("circuit requests to the offline peer in 30 s: {asked}");
    // A few tries are fine; one every discovery round (3 s) is not.
    // Every dial backs off, Kademlia's too (DialGuard). One every 3 s would be 10+.
    assert!(asked <= 3, "{asked} circuit requests to a peer that went offline");
    a.shutdown().await.unwrap();
}

/// v0.2.0's live smoke: two users behind NAT couldn't chat, and the relay
/// denied circuits by the thousand while accepting only a few. Two peers
/// that can only meet through relay circuits must sync, and keep using the
/// circuits they have: a handful of circuit requests in total, none denied,
/// over many discovery and pull rounds.
#[tokio::test(flavor = "multi_thread")]
async fn two_peers_behind_nat_reuse_their_circuits() {
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Hands out only circuit addresses, as for users behind NAT (their own
    // addresses are private and unreachable).
    let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);

    let (a, mut a_reserved) = node_lan("nat-a", &relay, false).await;
    let (b, mut b_reserved) = node_lan("nat-b", &relay, false).await;
    reserved_within(&mut a_reserved, Duration::from_secs(20)).await;
    reserved_within(&mut b_reserved, Duration::from_secs(20)).await;
    for record in messages(10) {
        a.append(record).await.unwrap();
    }
    let (server, channel) = test_server();
    let log = log_name(server, Scope::Channel(channel));
    let deadline = Instant::now() + Duration::from_secs(30);
    while b.state(&log).await.unwrap().count < 10 {
        assert!(Instant::now() < deadline, "no sync through the relay");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 90 s: 30 discovery rounds and 18 pull rounds on each side.
    tokio::time::sleep(Duration::from_secs(90)).await;
    let all = circuits.lock().unwrap().clone();
    let accepted = all.iter().filter(|(_, ok)| *ok).count();
    let denied = all.len() - accepted;
    eprintln!("circuits accepted {accepted}, denied {denied}");
    assert_eq!(denied, 0, "the relay denied {denied} circuits");
    assert!(accepted <= 4, "{accepted} circuits opened between two peers");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// A node of a server opened with its secret (so peers bind), meeting
/// others only through relay circuits, with "LAN allowed" (F2) as given.
async fn bound_node(name: &str, relay: &Multiaddr, key: u8, allow_lan: bool, gate: Arc<dyn ReadGate>) -> Node {
    bound_node_events(name, relay, key, allow_lan, gate).await.0
}

/// [`bound_node`] with its events.
async fn bound_node_events(name: &str, relay: &Multiaddr, key: u8, allow_lan: bool, gate: Arc<dyn ReadGate>) -> (Node, mpsc::Receiver<NodeEvent>) {
    bound_node_on_events(name, relay, key, allow_lan, gate, "/ip4/127.0.0.1/tcp/0".parse().unwrap(), vec![], false).await
}

/// [`bound_node`] listening on `listen`, dialling `bootstrap` too.
async fn bound_node_on(
    name: &str,
    relay: &Multiaddr,
    key: u8,
    allow_lan: bool,
    gate: Arc<dyn ReadGate>,
    listen: Multiaddr,
    bootstrap: Vec<Multiaddr>,
) -> Node {
    bound_node_on_events(name, relay, key, allow_lan, gate, listen, bootstrap, false).await.0
}

/// [`bound_node_on`] with its events, and `relay_only` from the start.
#[allow(clippy::too_many_arguments)]
async fn bound_node_on_events(
    name: &str,
    relay: &Multiaddr,
    key: u8,
    allow_lan: bool,
    gate: Arc<dyn ReadGate>,
    listen: Multiaddr,
    bootstrap: Vec<Multiaddr>,
    relay_only: bool,
) -> (Node, mpsc::Receiver<NodeEvent>) {
    let (server, channel) = test_server();
    let dir = std::env::temp_dir().join(format!("sync-relay-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = NodeConfig {
        relay: relay.clone(),
        data_dir: dir,
        logs: vec![],
        authorizer: Arc::new(AcceptAll),
        listen,
        bootstrap,
        allow_lan,
        relay_only,
    };
    let access = Access {
        binder: Some(Arc::new(SigningKey::from_bytes(&[key; 32]))),
        read_gate: Some(gate),
        ..Access::default()
    };
    let (node, events) = Node::start_with(config, access).await.unwrap();
    node.open_server(server, ServerSecret::from_bytes([42; 32])).unwrap();
    node.open_channel(server, channel).await.unwrap();
    (node, events)
}

/// F2: two bound peers on one LAN, which first meet through the relay
/// (the DHT never carries LAN addresses), switch to a direct connection:
/// both see the other as direct while both keep running. Without
/// "LAN allowed" they stay relayed.
#[tokio::test(flavor = "multi_thread")]
async fn bound_peers_on_one_lan_switch_from_relayed_to_direct() {
    // Shown only if the test fails: what was offered, tried and refused.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("sync=debug,libp2p_swarm=debug,libp2p_tcp=debug")
        .with_test_writer()
        .try_init();
    for allow_lan in [true, false] {
        let relay_key = identity::Keypair::generate_ed25519();
        let port = free_port();
        let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
        let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
        // Circuit addresses only in the relay's DHT, like for peers behind NAT.
        let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);
        let a = bound_node(&format!("lan-a-{allow_lan}"), &relay, 1, allow_lan, Arc::new(AcceptAll)).await;
        let b = bound_node(&format!("lan-b-{allow_lan}"), &relay, 2, allow_lan, Arc::new(AcceptAll)).await;
        let b_identity = records::PublicKey::from_bytes(SigningKey::from_bytes(&[2; 32]).verifying_key().to_bytes());
        let (ids, start) = ([a.peer_id(), b.peer_id()], Instant::now());
        let opened = || circuits.lock().unwrap().iter().filter(|(p, ok)| *ok && ids.contains(p)).count();
        // They meet through the relay first.
        while opened() == 0 {
            assert!(start.elapsed() < Duration::from_secs(60), "never met through the relay");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if allow_lan {
            // Driven by A's own state (B shown as direct), not by the relay
            // noticing the circuit close; generous for a loaded CI runner. The
            // LAN address is loopback, which every runner has.
            let deadline = Instant::now() + Duration::from_secs(60);
            while !a.presence(test_server().0).await.unwrap().direct.contains(&b_identity) {
                if Instant::now() >= deadline {
                    let (la, lb) = (a.listen_addrs().await.unwrap(), b.listen_addrs().await.unwrap());
                    panic!("stayed relayed on one LAN; A listens on {la:?}, B on {lb:?}, A has {:?}", a.peers().await.unwrap());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "the direct connection is up");
        } else {
            tokio::time::sleep(Duration::from_secs(10)).await;
            assert!(a.direct_peers().await.unwrap().is_empty(), "went direct without LAN allowed");
            // The UI indicator: B is online on A, through the relay.
            let seen = a.presence(test_server().0).await.unwrap();
            assert!(seen.online.contains(&b_identity) && seen.direct.is_empty(), "{seen:?}");
        }
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
    // "LAN allowed" but not bound (no shared server secret): stay relayed.
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);
    let (a, _) = node_lan("lan-unbound-a", &relay, true).await;
    let (b, _) = node_lan("lan-unbound-b", &relay, true).await;
    let ids = [a.peer_id(), b.peer_id()];
    let start = Instant::now();
    while !circuits.lock().unwrap().iter().any(|(p, ok)| *ok && ids.contains(p)) {
        assert!(start.elapsed() < Duration::from_secs(30), "unbound peers never met through the relay");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(a.direct_peers().await.unwrap().is_empty() && b.direct_peers().await.unwrap().is_empty(), "an unbound peer was dialled at its LAN address");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();

    // Bound, but neither side's read gate counts the other as a member (a
    // non-member with the secret, or a kicked member): no LAN dial, relayed.
    struct Nobody;
    impl ReadGate for Nobody {
        fn may_read(&self, _: &records::PublicKey, _: &ControlLog<'_>) -> bool {
            false
        }
    }
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);
    let a = bound_node("lan-nonmember-a", &relay, 3, true, Arc::new(Nobody)).await;
    let b = bound_node("lan-nonmember-b", &relay, 4, true, Arc::new(Nobody)).await;
    let ids = [a.peer_id(), b.peer_id()];
    let start = Instant::now();
    while !circuits.lock().unwrap().iter().any(|(p, ok)| *ok && ids.contains(p)) {
        assert!(start.elapsed() < Duration::from_secs(30), "non-members never met through the relay");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(a.direct_peers().await.unwrap().is_empty() && b.direct_peers().await.unwrap().is_empty(), "a bound non-member was dialled at its LAN address");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();


    // "Relay only" (GAP-6) on one side: even two members on one LAN, with
    // LAN allowed, stay relayed, and that side listens on its circuit only.
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);
    let a = bound_node("relay-only-a", &relay, 5, true, Arc::new(AcceptAll)).await;
    a.set_relay_only(true).unwrap();
    let b = bound_node("relay-only-b", &relay, 6, true, Arc::new(AcceptAll)).await;
    let ids = [a.peer_id(), b.peer_id()];
    let start = Instant::now();
    while !circuits.lock().unwrap().iter().any(|(p, ok)| *ok && ids.contains(p)) {
        assert!(start.elapsed() < Duration::from_secs(30), "relay-only peers never met through the relay");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(a.direct_peers().await.unwrap().is_empty() && b.direct_peers().await.unwrap().is_empty(), "a relay-only peer went direct");
    let listening = a.listen_addrs().await.unwrap();
    assert!(
        listening.iter().all(|addr| addr.iter().any(|p| p == libp2p::multiaddr::Protocol::P2pCircuit)),
        "relay only, yet listening directly: {listening:?}"
    );
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();

    // The other side's turn: the smaller peer ID dials first, but here it
    // can't (as if a firewall blocked it). The larger one dials once after
    // waiting its turn, and the connection still goes direct.
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);
    let a = bound_node("turn-a", &relay, 9, true, Arc::new(AcceptAll)).await;
    let b = bound_node("turn-b", &relay, 10, true, Arc::new(AcceptAll)).await;
    let id = |n: u8| records::PublicKey::from_bytes(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes());
    let ((smaller, _), (larger, smaller_id)) =
        if a.peer_id() < b.peer_id() { ((&a, id(10)), (&b, id(9))) } else { ((&b, id(9)), (&a, id(10))) };
    // F2 on its own: a hole punch would go direct before the larger turn.
    a.test_no_hole_punch().unwrap();
    b.test_no_hole_punch().unwrap();
    smaller.test_no_direct_dials().unwrap();
    let start = Instant::now();
    // The larger side shows the smaller one as direct: its own dial worked.
    while !larger.presence(test_server().0).await.unwrap().direct.contains(&smaller_id) {
        assert!(start.elapsed() < Duration::from_secs(60), "the larger side never took its turn");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(start.elapsed() >= Duration::from_secs(5), "went direct before the larger side's turn");
    assert_eq!(larger.peers().await.unwrap(), vec![smaller.peer_id()]);
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// DCUtR (#184): two members who meet only through the relay, neither
/// making the F2 direct dial, hole-punch to a direct connection, and both
/// show the other as direct (the UI indicator). Here the observed address
/// the relay reports is loopback, so this needs "LAN allowed"; without it
/// there is nothing to punch to and the pair falls back cleanly: still
/// connected and online through the relay.
#[tokio::test(flavor = "multi_thread")]
async fn members_hole_punch_from_relayed_to_direct_or_stay_relayed() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("sync=debug,libp2p_dcutr=debug")
        .with_test_writer()
        .try_init();
    let id = |n: u8| records::PublicKey::from_bytes(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes());
    for allow_lan in [true, false] {
        let relay_key = identity::Keypair::generate_ed25519();
        let port = free_port();
        let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
        let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let _relay = start_relay_counting(relay_key, port, circuits.clone(), true);
        let a = bound_node(&format!("punch-a-{allow_lan}"), &relay, 11, allow_lan, Arc::new(AcceptAll)).await;
        let b = bound_node(&format!("punch-b-{allow_lan}"), &relay, 12, allow_lan, Arc::new(AcceptAll)).await;
        // No F2 dial from either side: only a hole punch can go direct.
        a.test_no_direct_dials().unwrap();
        b.test_no_direct_dials().unwrap();
        let start = Instant::now();
        if allow_lan {
            // Windows never completes the TCP simultaneous open that a DCUtR
            // punch is (see `try_direct`), so there the punch may fail. That is
            // still a pass, as long as it fails cleanly: the pair stays
            // connected and online through the relay. Elsewhere it must punch.
            let punched = loop {
                let (sa, sb) = (a.presence(test_server().0).await.unwrap(), b.presence(test_server().0).await.unwrap());
                if sa.direct.contains(&id(12)) && sb.direct.contains(&id(11)) {
                    break true;
                }
                if start.elapsed() >= Duration::from_secs(60) {
                    assert!(cfg!(windows), "never punched: A sees {sa:?}, B sees {sb:?}");
                    assert!(sa.online.contains(&id(12)) && sb.online.contains(&id(11)), "a failed punch dropped the link: A sees {sa:?}, B sees {sb:?}");
                    break false;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            };
            if punched {
                assert_eq!(a.direct_peers().await.unwrap(), vec![b.peer_id()]);
            } else {
                assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "a failed punch closed the relayed connection");
            }
        } else {
            let ids = [a.peer_id(), b.peer_id()];
            while !circuits.lock().unwrap().iter().any(|(p, ok)| *ok && ids.contains(p)) {
                assert!(start.elapsed() < Duration::from_secs(30), "never met through the relay");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
            assert!(a.direct_peers().await.unwrap().is_empty(), "went direct with nothing to punch to");
            let seen = a.presence(test_server().0).await.unwrap();
            assert!(seen.online.contains(&id(12)) && seen.direct.is_empty(), "not online through the relay: {seen:?}");
            assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "the relayed connection went");
        }
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
}

// ---- voice signalling (voice v1, piece A) ----------------------------------

fn voice_id(n: u8) -> records::PublicKey {
    records::PublicKey::from_bytes(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes())
}

/// A read gate whose refused identities can change (a member kicked, or a
/// non-member admitted), as the control log would make them.
#[derive(Clone, Default)]
struct Refusing(Arc<std::sync::Mutex<std::collections::HashSet<records::PublicKey>>>);

impl Refusing {
    fn set(&self, who: records::PublicKey, refused: bool) {
        let mut set = self.0.lock().unwrap();
        if refused {
            set.insert(who);
        } else {
            set.remove(&who);
        }
    }
}

impl ReadGate for Refusing {
    fn may_read(&self, who: &records::PublicKey, _: &ControlLog<'_>) -> bool {
        !self.0.lock().unwrap().contains(who)
    }
}

/// A fresh in-process relay that only knows circuit addresses.
fn voice_relay() -> (Multiaddr, tokio::task::JoinHandle<()>) {
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let handle = start_relay_counting(relay_key, port, Arc::default(), true);
    (relay, handle)
}

/// Until each side has the other bound (online), and direct when asked.
async fn voice_meet(a: &Node, b: &Node, ida: records::PublicKey, idb: records::PublicKey, direct: bool) {
    let start = Instant::now();
    loop {
        let (sa, sb) = (a.presence(test_server().0).await.unwrap(), b.presence(test_server().0).await.unwrap());
        let online = sa.online.contains(&idb) && sb.online.contains(&ida);
        if online && (!direct || (sa.direct.contains(&idb) && sb.direct.contains(&ida))) {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "never met: A sees {sa:?}, B sees {sb:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The voice signals that arrive within `wait`.
async fn voice_signals(events: &mut mpsc::Receiver<NodeEvent>, wait: Duration) -> Vec<(records::PublicKey, Vec<u8>)> {
    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
        if let NodeEvent::VoiceSignal { from, bytes } = event {
            got.push((from, bytes));
        }
    }
    got
}

/// The next voice signal, within 20 s.
async fn next_voice_signal(events: &mut mpsc::Receiver<NodeEvent>) -> (records::PublicKey, Vec<u8>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(NodeEvent::VoiceSignal { from, bytes })) => return (from, bytes),
            Ok(Some(_)) => {}
            other => panic!("no voice signal: {other:?}"),
        }
    }
}

/// Two members exchange blobs through the relay and over a direct
/// connection; the sender is the identity the peer is bound to. A blob over
/// 16 KiB, or one to an identity nobody connected is bound to, isn't sent.
#[tokio::test(flavor = "multi_thread")]
async fn members_exchange_voice_signals_relayed_and_direct() {
    for direct in [false, true] {
        let (relay, _relay) = voice_relay();
        let (a, mut ea) = bound_node_events(&format!("voice-a-{direct}"), &relay, 21, direct, Arc::new(AcceptAll)).await;
        let (b, mut eb) = bound_node_events(&format!("voice-b-{direct}"), &relay, 22, direct, Arc::new(AcceptAll)).await;
        voice_meet(&a, &b, voice_id(21), voice_id(22), direct).await;

        let offer = vec![1u8; MAX_VOICE_BLOB];
        a.voice_send(voice_id(22), offer.clone()).await.unwrap();
        b.voice_send(voice_id(21), b"answer".to_vec()).await.unwrap();
        assert_eq!(next_voice_signal(&mut eb).await, (voice_id(21), offer));
        assert_eq!(next_voice_signal(&mut ea).await, (voice_id(22), b"answer".to_vec()));
        if direct {
            assert_eq!(a.direct_peers().await.unwrap(), vec![b.peer_id()]);
        } else {
            assert!(a.direct_peers().await.unwrap().is_empty(), "the relayed run went direct");
        }

        assert_eq!(a.voice_send(voice_id(22), vec![0; MAX_VOICE_BLOB + 1]).await, Err(VoiceError::TooLarge));
        assert_eq!(a.voice_send(voice_id(99), b"x".to_vec()).await, Err(VoiceError::NotMember));
        assert!(voice_signals(&mut eb, Duration::from_secs(2)).await.is_empty(), "something else arrived");
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
}

/// A peer bound to a non-member, then to a kicked member, is refused both
/// ways: its signals are dropped unanswered, and none go to it.
#[tokio::test(flavor = "multi_thread")]
async fn voice_signals_go_between_current_members_only() {
    let (relay, _relay) = voice_relay();
    let gate = Refusing::default();
    // B doesn't count A as a member: A has the secret, but no membership.
    gate.set(voice_id(23), true);
    let (a, _ea) = bound_node_events("voice-gate-a", &relay, 23, false, Arc::new(AcceptAll)).await;
    let (b, mut eb) = bound_node_events("voice-gate-b", &relay, 24, false, Arc::new(gate.clone())).await;
    voice_meet(&a, &b, voice_id(23), voice_id(24), false).await;

    // A thinks B a member and sends; B drops it.
    a.voice_send(voice_id(24), b"from a non-member".to_vec()).await.unwrap();
    assert!(voice_signals(&mut eb, Duration::from_secs(3)).await.is_empty(), "a non-member's signal arrived");
    assert_eq!(b.voice_send(voice_id(23), b"to a non-member".to_vec()).await, Err(VoiceError::NotMember));

    // Admitted: it goes through.
    gate.set(voice_id(23), false);
    a.voice_send(voice_id(24), b"member".to_vec()).await.unwrap();
    assert_eq!(next_voice_signal(&mut eb).await, (voice_id(23), b"member".to_vec()));
    b.voice_send(voice_id(23), b"to a member".to_vec()).await.unwrap();

    // Kicked: dropped again, and nothing goes to it.
    gate.set(voice_id(23), true);
    a.voice_send(voice_id(24), b"from a kicked member".to_vec()).await.unwrap();
    assert!(voice_signals(&mut eb, Duration::from_secs(3)).await.is_empty(), "a kicked member's signal arrived");
    assert_eq!(b.voice_send(voice_id(23), b"to a kicked member".to_vec()).await, Err(VoiceError::NotMember));
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// A member's flood: at most a burst of 20, then 5 a second, reaches the
/// app; the rest is dropped.
#[tokio::test(flavor = "multi_thread")]
async fn the_voice_rate_cap_holds() {
    let (relay, _relay) = voice_relay();
    let (a, _ea) = bound_node_events("voice-rate-a", &relay, 25, false, Arc::new(AcceptAll)).await;
    let (b, mut eb) = bound_node_events("voice-rate-b", &relay, 26, false, Arc::new(AcceptAll)).await;
    voice_meet(&a, &b, voice_id(25), voice_id(26), false).await;
    let start = Instant::now();
    for n in 0..100u32 {
        a.voice_send(voice_id(26), n.to_le_bytes().to_vec()).await.unwrap();
    }
    let got = voice_signals(&mut eb, Duration::from_secs(3)).await;
    let allowed = 20 + 5 * start.elapsed().as_secs_f64().ceil() as usize;
    assert!(got.len() >= 20, "only {} arrived", got.len());
    assert!(got.len() <= allowed, "{} arrived, at most {allowed} allowed", got.len());
    assert!(got.iter().all(|(from, _)| *from == voice_id(25)));
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// T8-02 piece 3: a member's invite publication reaches the other members
/// (`/mvs/invite-pub/1`), also one that binds later; anyone, a stranger
/// with no server secret included, can ask for its envelope handle
/// (`/mvs/invite/1`) and gets it, or the same empty answer for a handle
/// nobody serves or one that was dropped.
#[tokio::test(flavor = "multi_thread")]
async fn invite_publications_reach_members_and_answer_anyone() {
    let relay_key = identity::Keypair::generate_ed25519();
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
    let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _relay = start_relay_counting(relay_key, port, circuits, true);
    let server = test_server().0;
    let id = |n: u8| records::PublicKey::from_bytes(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes());

    let (a, _a_events) = bound_node_events("invite-a", &relay, 21, false, Arc::new(AcceptAll)).await;
    let (b, mut b_events) = bound_node_events("invite-b", &relay, 22, false, Arc::new(AcceptAll)).await;
    let start = Instant::now();
    while !a.presence(server).await.unwrap().online.contains(&id(22)) {
        assert!(start.elapsed() < Duration::from_secs(60), "the members never bound");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Served and announced: B, a member, gets it as an event.
    let (handle, publication) = ([0xab; 32], vec![0x5a; 300]);
    assert!(a.serve_invite(server, handle, publication.clone(), true).await.unwrap());
    let got = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(NodeEvent::InvitePublication { server: s, from, bytes }) = b_events.recv().await {
                return (s, from, bytes);
            }
        }
    })
    .await
    .expect("B never got the publication");
    assert_eq!(got, (server, id(21), publication.clone()));
    // First wins; over the size cap, empty, or a server we don't hold: refused.
    assert!(!a.serve_invite(server, handle, vec![1; 10], false).await.unwrap());
    assert!(!a.serve_invite(server, [1; 32], vec![1; sync::MAX_INVITE_PUBLICATION + 1], false).await.unwrap());
    assert!(!a.serve_invite(server, [1; 32], vec![], false).await.unwrap());
    assert!(!a.serve_invite(ServerId::from_bytes([3; 32]), [1; 32], vec![1; 10], false).await.unwrap());

    // A member that binds later catches up, unannounced.
    let (c, mut c_events) = bound_node_events("invite-c", &relay, 23, false, Arc::new(AcceptAll)).await;
    let got = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(NodeEvent::InvitePublication { bytes, .. }) = c_events.recv().await {
                return bytes;
            }
        }
    })
    .await
    .expect("a late member never caught up");
    assert_eq!(got, publication);

    // A stranger (no secret, not bound) asks by handle.
    let (stranger, _) = node_lan("invite-stranger", &relay, false).await;
    let start = Instant::now();
    loop {
        let answers = stranger.fetch_invite(handle).await.unwrap();
        if answers.contains(&publication) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "the stranger never got the envelope");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(stranger.fetch_invite([0xcd; 32]).await.unwrap().is_empty(), "nobody serves that handle");
    a.drop_invite(handle).unwrap();
    b.drop_invite(handle).unwrap();
    c.drop_invite(handle).unwrap();
    assert!(!stranger.fetch_invite(handle).await.unwrap().contains(&publication), "served after it was dropped");

    for node in [a, b, c, stranger] {
        node.shutdown().await.unwrap();
    }
}

/// The QUIC punch: as above, but both peers listen on QUIC only and also
/// reach the relay over QUIC (a bootstrap address, as devops' config lists
/// it), so identify observes their QUIC address. A TCP candidate has nobody
/// listening behind it, so going direct means a QUIC punch. QUIC dials leave
/// from the listening socket, which is what makes this work on Windows too;
/// a clean relayed fallback is still accepted there (CI runners' loopback
/// firewall), as for TCP.
#[tokio::test(flavor = "multi_thread")]
async fn members_hole_punch_over_quic_or_stay_relayed() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("sync=debug,libp2p_dcutr=debug")
        .with_test_writer()
        .try_init();
    let id = |n: u8| records::PublicKey::from_bytes(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes());
    let relay_key = identity::Keypair::generate_ed25519();
    let relay_peer = PeerId::from(relay_key.public());
    let port = free_port();
    let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{relay_peer}").parse().unwrap();
    let relay_quic: Multiaddr = format!("/ip4/127.0.0.1/udp/{port}/quic-v1/p2p/{relay_peer}").parse().unwrap();
    let _relay = start_relay_counting(relay_key, port, Arc::default(), true);
    let quic = || "/ip4/127.0.0.1/udp/0/quic-v1".parse::<Multiaddr>().unwrap();
    let a = bound_node_on("quic-punch-a", &relay, 13, true, Arc::new(AcceptAll), quic(), vec![relay_quic.clone()]).await;
    let b = bound_node_on("quic-punch-b", &relay, 14, true, Arc::new(AcceptAll), quic(), vec![relay_quic]).await;
    // No F2 dial from either side: only a hole punch can go direct.
    a.test_no_direct_dials().unwrap();
    b.test_no_direct_dials().unwrap();
    let start = Instant::now();
    let punched = loop {
        let (sa, sb) = (a.presence(test_server().0).await.unwrap(), b.presence(test_server().0).await.unwrap());
        if sa.direct.contains(&id(14)) && sb.direct.contains(&id(13)) {
            break true;
        }
        if start.elapsed() >= Duration::from_secs(60) {
            assert!(cfg!(windows), "never punched over QUIC: A sees {sa:?}, B sees {sb:?}");
            assert!(
                sa.online.contains(&id(14)) && sb.online.contains(&id(13)),
                "a failed punch dropped the link: A sees {sa:?}, B sees {sb:?}"
            );
            break false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    if punched {
        assert_eq!(a.direct_peers().await.unwrap(), vec![b.peer_id()]);
        eprintln!("punched over QUIC in {:?}", start.elapsed());
    } else {
        assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "a failed punch closed the relayed connection");
    }
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// QUIC listens on the TCP port (#200: one rule, one NAT mapping) and goes
/// with TCP while relay-only; a non-member or a relay-only peer never goes
/// direct over QUIC, even with the relay's QUIC address to learn its own.
#[tokio::test(flavor = "multi_thread")]
async fn quic_follows_tcp_and_punches_only_members_off_relay_only() {
    use libp2p::multiaddr::Protocol;
    let _ = tracing_subscriber::fmt().with_env_filter("sync=debug").with_test_writer().try_init();
    struct Nobody;
    impl ReadGate for Nobody {
        fn may_read(&self, _: &records::PublicKey, _: &ControlLog<'_>) -> bool {
            false
        }
    }
    let start_relay_here = || {
        let relay_key = identity::Keypair::generate_ed25519();
        let peer = PeerId::from(relay_key.public());
        let port = free_port();
        let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = start_relay_counting(relay_key, port, circuits.clone(), true);
        let tcp: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{peer}").parse().unwrap();
        let quic: Multiaddr = format!("/ip4/127.0.0.1/udp/{port}/quic-v1/p2p/{peer}").parse().unwrap();
        (handle, circuits, tcp, quic)
    };
    let ports = |addrs: &[Multiaddr]| {
        let (mut tcp, mut udp) = (None, None);
        for a in addrs.iter().filter(|a| !a.iter().any(|p| p == Protocol::P2pCircuit)) {
            for p in a.iter() {
                match p {
                    Protocol::Tcp(port) => tcp = Some(port),
                    Protocol::Udp(port) if a.iter().any(|p| p == Protocol::QuicV1) => udp = Some(port),
                    _ => {}
                }
            }
        }
        (tcp, udp)
    };
    let direct_listening = |addrs: &[Multiaddr]| addrs.iter().any(|a| !a.iter().any(|p| p == Protocol::P2pCircuit));

    // TCP config: QUIC comes up beside it on the same port; relay-only drops
    // both, and turning it off brings both back.
    let (_relay, _, relay, _) = start_relay_here();
    let a = bound_node("quic-port-a", &relay, 15, true, Arc::new(AcceptAll)).await;
    let (tcp, udp) = wait_ports(&a, ports).await;
    assert_eq!(tcp, udp, "QUIC on another port than TCP");
    a.set_relay_only(true).unwrap();
    let start = Instant::now();
    while direct_listening(&a.listen_addrs().await.unwrap()) {
        assert!(start.elapsed() < Duration::from_secs(10), "relay only, yet listening directly");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    a.set_relay_only(false).unwrap();
    let (tcp2, udp2) = wait_ports(&a, ports).await;
    // (On a first run TCP may take a new random port here; QUIC follows it.)
    assert_eq!(tcp2, udp2, "QUIC on another port than TCP after relay-only");
    a.shutdown().await.unwrap();

    // QUIC-only peers that learn their QUIC address from the relay: neither
    // non-members nor a relay-only member go direct.
    let quic = || "/ip4/127.0.0.1/udp/0/quic-v1".parse::<Multiaddr>().unwrap();
    for case in ["non-member", "relay-only"] {
        let (_relay, circuits, relay, relay_quic) = start_relay_here();
        let gate = || -> Arc<dyn ReadGate> { if case == "non-member" { Arc::new(Nobody) } else { Arc::new(AcceptAll) } };
        let a = bound_node_on(&format!("quic-{case}-a"), &relay, 16, true, gate(), quic(), vec![relay_quic.clone()]).await;
        if case == "relay-only" {
            a.set_relay_only(true).unwrap();
        }
        let b = bound_node_on(&format!("quic-{case}-b"), &relay, 17, true, gate(), quic(), vec![relay_quic]).await;
        // Only a punch could go direct.
        a.test_no_direct_dials().unwrap();
        b.test_no_direct_dials().unwrap();
        let ids = [a.peer_id(), b.peer_id()];
        let start = Instant::now();
        while !circuits.lock().unwrap().iter().any(|(p, ok)| *ok && ids.contains(p)) {
            assert!(start.elapsed() < Duration::from_secs(30), "{case}: never met through the relay");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            a.direct_peers().await.unwrap().is_empty() && b.direct_peers().await.unwrap().is_empty(),
            "{case}: punched over QUIC"
        );
        assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "{case}: the relayed connection went");
        if case == "relay-only" {
            let listening = a.listen_addrs().await.unwrap();
            assert!(!direct_listening(&listening), "relay only, yet listening directly: {listening:?}");
        }
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
}

/// The TCP and QUIC ports `node` listens on directly, once it has both.
async fn wait_ports(node: &Node, ports: impl Fn(&[Multiaddr]) -> (Option<u16>, Option<u16>)) -> (u16, u16) {
    let start = Instant::now();
    loop {
        let addrs = node.listen_addrs().await.unwrap();
        if let (Some(tcp), Some(udp)) = ports(&addrs) {
            return (tcp, udp);
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no QUIC listener beside TCP: {addrs:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// GAP-6r: relay-only from the start (`NodeConfig::relay_only`) never listens
/// directly, never dials its bootstrap addresses and never goes direct; and
/// turning relay-only on closes direct connections already open, the peers
/// staying connected through the relay.
#[tokio::test(flavor = "multi_thread")]
async fn relay_only_from_start_never_goes_direct_and_turning_it_on_closes_direct_links() {
    use libp2p::multiaddr::Protocol;
    let _ = tracing_subscriber::fmt().with_env_filter("sync=debug").with_test_writer().try_init();
    let id = |n: u8| records::PublicKey::from_bytes(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes());
    let only_circuits = |addrs: &[Multiaddr]| addrs.iter().all(|a| a.iter().any(|p| p == Protocol::P2pCircuit));
    let relay_here = || {
        let relay_key = identity::Keypair::generate_ed25519();
        let port = free_port();
        let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", PeerId::from(relay_key.public())).parse().unwrap();
        let circuits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = start_relay_counting(relay_key, port, circuits.clone(), true);
        (handle, circuits, relay)
    };

    // From the start: a bootstrap address that counts every connection to it.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    probe.set_nonblocking(true).unwrap();
    let bootstrap: Multiaddr =
        format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", probe.local_addr().unwrap().port(), PeerId::random()).parse().unwrap();
    let (_relay, circuits, relay) = relay_here();
    let tcp = || "/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>().unwrap();
    let (a, _) = bound_node_on_events("ro-start-a", &relay, 30, true, Arc::new(AcceptAll), tcp(), vec![bootstrap], true).await;
    let listening = a.listen_addrs().await.unwrap();
    assert!(only_circuits(&listening), "relay only from the start, yet listening directly: {listening:?}");
    let b = bound_node("ro-start-b", &relay, 31, true, Arc::new(AcceptAll)).await;
    let ids = [a.peer_id(), b.peer_id()];
    let start = Instant::now();
    while !circuits.lock().unwrap().iter().any(|(p, ok)| *ok && ids.contains(p)) {
        assert!(start.elapsed() < Duration::from_secs(30), "relay-only peer never met through the relay");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(a.direct_peers().await.unwrap().is_empty() && b.direct_peers().await.unwrap().is_empty(), "went direct");
    assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "not connected through the relay");
    let seen = a.presence(test_server().0).await.unwrap();
    assert!(seen.online.contains(&id(31)) && seen.direct.is_empty(), "{seen:?}");
    let listening = a.listen_addrs().await.unwrap();
    assert!(only_circuits(&listening), "relay only, yet listening directly: {listening:?}");
    let mut dialled = 0;
    while probe.accept().is_ok() {
        dialled += 1;
    }
    assert_eq!(dialled, 0, "a relay-only node dialled its bootstrap address");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();

    // Turned on later: two members on one LAN go direct, then relay-only on
    // one side closes that link and both stay connected through the relay.
    let (_relay, _, relay) = relay_here();
    let a = bound_node("ro-switch-a", &relay, 32, true, Arc::new(AcceptAll)).await;
    let b = bound_node("ro-switch-b", &relay, 33, true, Arc::new(AcceptAll)).await;
    let start = Instant::now();
    while a.direct_peers().await.unwrap() != vec![b.peer_id()] {
        assert!(start.elapsed() < Duration::from_secs(60), "never went direct on one LAN");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    a.set_relay_only(true).unwrap();
    let start = Instant::now();
    loop {
        let (da, db) = (a.direct_peers().await.unwrap(), b.direct_peers().await.unwrap());
        let seen = a.presence(test_server().0).await.unwrap();
        if da.is_empty() && db.is_empty() && a.peers().await.unwrap() == vec![b.peer_id()] && seen.online.contains(&id(33)) && seen.direct.is_empty() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "still direct or not relayed: A direct {da:?}, B direct {db:?}, A sees {seen:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // And it stays that way: B's F2 dial finds no listener, a late direct
    // connection is closed on arrival.
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(a.direct_peers().await.unwrap().is_empty() && b.direct_peers().await.unwrap().is_empty(), "went direct again");
    assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "the relayed link went");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

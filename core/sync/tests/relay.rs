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
use sync::{log_name, AcceptAll, Access, ControlLog, Node, NodeConfig, NodeEvent, ReadGate, Record, ServerSecret};

/// Destinations of circuits the test relays saw close (all tests in this
/// binary; filter by peer).
static CIRCUITS_CLOSED: std::sync::Mutex<Vec<PeerId>> = std::sync::Mutex::new(Vec::new());
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
                SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(relay::Event::CircuitClosed { dst_peer_id, .. })) => {
                    CIRCUITS_CLOSED.lock().unwrap().push(dst_peer_id)
                }
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
    let (server, channel) = test_server();
    let dir = std::env::temp_dir().join(format!("sync-relay-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = NodeConfig {
        relay: relay.clone(),
        data_dir: dir,
        logs: vec![],
        authorizer: Arc::new(AcceptAll),
        listen: "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
        bootstrap: vec![],
        allow_lan,
    };
    let access = Access {
        binder: Some(Arc::new(SigningKey::from_bytes(&[key; 32]))),
        read_gate: Some(gate),
        ..Access::default()
    };
    let (node, _events) = Node::start_with(config, access).await.unwrap();
    node.open_server(server, ServerSecret::from_bytes([42; 32])).unwrap();
    node.open_channel(server, channel).await.unwrap();
    node
}

/// F2: two bound peers on one LAN, which first meet through the relay
/// (the DHT never carries LAN addresses), switch to a direct connection:
/// the relay sees their circuit close while both keep running. Without
/// "LAN allowed" they stay relayed.
#[tokio::test(flavor = "multi_thread")]
async fn bound_peers_on_one_lan_switch_from_relayed_to_direct() {
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
        let closed = || CIRCUITS_CLOSED.lock().unwrap().iter().filter(|p| ids.contains(p)).count();
        let opened = || circuits.lock().unwrap().iter().filter(|(p, ok)| *ok && ids.contains(p)).count();
        // They meet through the relay first.
        while opened() == 0 {
            assert!(start.elapsed() < Duration::from_secs(30), "never met through the relay");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if allow_lan {
            while closed() == 0 {
                assert!(start.elapsed() < Duration::from_secs(30), "stayed relayed on one LAN");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(a.peers().await.unwrap(), vec![b.peer_id()], "the direct connection is up");
            // The UI indicator: B is online on A, and directly.
            let deadline = Instant::now() + Duration::from_secs(15);
            while !a.presence(test_server().0).await.unwrap().direct.contains(&b_identity) {
                assert!(Instant::now() < deadline, "B never shown as direct");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } else {
            tokio::time::sleep(Duration::from_secs(10)).await;
            assert_eq!(closed(), 0, "went direct without LAN allowed");
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
    let closed = CIRCUITS_CLOSED.lock().unwrap().iter().filter(|p| ids.contains(p)).count();
    assert_eq!(closed, 0, "an unbound peer was dialled at its LAN address");
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
    let closed = CIRCUITS_CLOSED.lock().unwrap().iter().filter(|p| ids.contains(p)).count();
    assert_eq!(closed, 0, "a bound non-member was dialled at its LAN address");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

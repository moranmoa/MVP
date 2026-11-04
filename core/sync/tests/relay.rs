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
use sync::{log_name, AcceptAll, Node, NodeConfig, NodeEvent, Record};
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
    start_relay_counting(key, port, Arc::default())
}

/// Also records the destination of every circuit request (accepted or
/// denied), as the live relay's circuit counters do.
fn start_relay_counting(
    key: identity::Keypair,
    port: u16,
    circuits: Arc<std::sync::Mutex<Vec<PeerId>>>,
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
                relay: relay::Behaviour::new(peer_id, relay::Config::default()),
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
                        swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                    }
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(
                    relay::Event::CircuitReqAccepted { dst_peer_id, .. } | relay::Event::CircuitReqDenied { dst_peer_id, .. },
                )) => circuits.lock().unwrap().push(dst_peer_id),
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
        allow_lan: true,
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
    let _relay = start_relay_counting(relay_key, port, circuits.clone());

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
    let asked = circuits.lock().unwrap().iter().filter(|dst| **dst == gone).count();
    eprintln!("circuit requests to the offline peer in 30 s: {asked}");
    // A few tries are fine; one every discovery round (3 s) is not.
    assert!(asked <= 3, "{asked} circuit requests to a peer that went offline");
    a.shutdown().await.unwrap();
}

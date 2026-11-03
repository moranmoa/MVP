//! SPIKE COPY of infra/relay with three changes for browser peers (see
//! spikes/web/RESULTS.md, "Relay changes for devops", and relay.patch).
//! The harness builds this copy; infra/relay itself is owned by devops.
//!
//! Bootstrap + circuit-relay node for the P2P network.
//!
//! It never stores or reads messages: it only helps peers find each other
//! (Kademlia DHT, server mode) and relays encrypted traffic when two peers
//! can't connect directly (libp2p circuit relay v2).
//!
//! `relay-node healthcheck` exits 0 if the local TCP listener accepts a
//! connection; the Docker healthcheck runs it (the image has no curl/nc).

use std::{
    env, fs,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use futures::StreamExt;
use libp2p::{
    core::{upgrade, Transport as _},
    identify, identity, kad, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, websocket, yamux, Multiaddr, SwarmBuilder,
};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const PROTOCOL_VERSION: &str = "/descord-replacement/0.1.0";

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

/// Counters for the periodic `stats` log line. Counts only - no addresses or
/// peer IDs - so the log can be shared when asking for help.
#[derive(Default)]
struct Stats {
    reservations_accepted: u64,
    reservations_denied: u64,
    circuits_accepted: u64,
    circuits_denied: u64,
    circuits_open: u64,
}

/// The node keeps one identity across restarts so its peer ID (and therefore
/// the bootstrap address baked into clients) stays stable.
fn load_or_create_key(path: &Path) -> Result<identity::Keypair> {
    if path.exists() {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return identity::Keypair::from_protobuf_encoding(&bytes).context("decoding node key");
    }
    let key = identity::Keypair::generate_ed25519();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, key.to_protobuf_encoding()?)
        .with_context(|| format!("writing {}", path.display()))?;
    info!(path = %path.display(), "generated new node key");
    Ok(key)
}

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match env::var(name) {
        Ok(value) => value.parse().ok().with_context(|| format!("{name} must be a number")),
        Err(_) => Ok(default),
    }
}

fn env_opt(name: &str) -> Result<Option<u64>> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => {
            Ok(Some(value.parse().with_context(|| format!("{name} must be a number"))?))
        }
        _ => Ok(None),
    }
}

/// Circuit limits. Unset variables keep libp2p's defaults: 16 circuits in
/// total, 4 per peer, each closed after 2 minutes or 128 KiB. Browsers that
/// can only reach a peer through the relay need more (spikes/web/RESULTS.md).
fn relay_config() -> Result<relay::Config> {
    let mut config = relay::Config::default();
    if let Some(n) = env_opt("MAX_CIRCUITS")? {
        config.max_circuits = n as usize;
    }
    if let Some(n) = env_opt("MAX_CIRCUITS_PER_PEER")? {
        config.max_circuits_per_peer = n as usize;
    }
    if let Some(secs) = env_opt("MAX_CIRCUIT_SECS")? {
        config.max_circuit_duration = Duration::from_secs(secs);
    }
    if let Some(bytes) = env_opt("MAX_CIRCUIT_BYTES")? {
        config.max_circuit_bytes = bytes;
    }
    Ok(config)
}

fn healthcheck() -> ExitCode {
    let port = match env_num("TCP_PORT", 4001u16) {
        Ok(port) => port,
        Err(err) => {
            eprintln!("{err:#}");
            return ExitCode::FAILURE;
        }
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("relay not accepting on {addr}: {err}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> Result<ExitCode> {
    if env::args().nth(1).as_deref() == Some("healthcheck") {
        return Ok(healthcheck());
    }
    run()?;
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let key_path = env::var("KEY_PATH").unwrap_or_else(|_| "/data/node.key".into());
    let key = load_or_create_key(Path::new(&key_path))?;
    let tcp_port = env_num("TCP_PORT", 4001u16)?;
    let quic_port = env_num("QUIC_PORT", 4001u16)?;
    let ws_port = env_num("WS_PORT", 4002u16)?;
    let stats_every = Duration::from_secs(env_num("STATS_INTERVAL_SECS", 300u64)?.max(10));
    let relay_config = relay_config()?;

    let mut swarm = SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        // Nagle off: with it, small relayed messages wait for delayed ACKs
        // (~40 ms per hop, see spikes/web/RESULTS.md).
        .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)?
        .with_quic()
        // What `with_websocket` builds, but on a TCP transport with Nagle off
        // (the builder offers no TCP config for WebSocket).
        .with_other_transport(|key| {
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                websocket::WsConfig::new(tcp::tokio::Transport::new(tcp::Config::default().nodelay(true)))
                    .upgrade(upgrade::Version::V1Lazy)
                    .authenticate(noise::Config::new(key)?)
                    .multiplex(yamux::Config::default()),
            )
        })?
        .with_dns()?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            let mut kademlia = kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id));
            kademlia.set_mode(Some(kad::Mode::Server));
            Behaviour {
                relay: relay::Behaviour::new(peer_id, relay_config),
                identify: identify::Behaviour::new(identify::Config::new(
                    PROTOCOL_VERSION.into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
                kademlia,
            }
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{tcp_port}").parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{quic_port}/quic-v1").parse()?)?;
    // Plain WebSocket: TLS is terminated by the reverse proxy in front of it.
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{ws_port}/ws").parse()?)?;

    // Public addresses (domain-based) that peers should dial, e.g.
    // /dns4/chat.example.com/tcp/4001,/dns4/chat.example.com/tcp/443/wss
    for addr in env::var("EXTERNAL_ADDRS").unwrap_or_default().split(',') {
        let addr = addr.trim();
        if addr.is_empty() {
            continue;
        }
        match addr.parse::<Multiaddr>() {
            Ok(addr) => swarm.add_external_address(addr),
            Err(err) => warn!(%addr, %err, "ignoring invalid EXTERNAL_ADDRS entry"),
        }
    }

    info!(peer_id = %swarm.local_peer_id(), "relay node started");

    let started = Instant::now();
    let mut stats = Stats::default();
    let mut stats_timer = tokio::time::interval(stats_every);

    loop {
        let event = tokio::select! {
            event = swarm.select_next_some() => event,
            _ = stats_timer.tick() => {
                let net = swarm.network_info();
                info!(
                    uptime_secs = started.elapsed().as_secs(),
                    peers = net.num_peers(),
                    connections = net.connection_counters().num_established(),
                    circuits_open = stats.circuits_open,
                    circuits_accepted = stats.circuits_accepted,
                    circuits_denied = stats.circuits_denied,
                    reservations_accepted = stats.reservations_accepted,
                    reservations_denied = stats.reservations_denied,
                    "stats"
                );
                continue;
            }
        };
        match event {
            SwarmEvent::NewListenAddr { address, .. } => info!(%address, "listening"),
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                for addr in info.listen_addrs {
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Relay(event)) => {
                match &event {
                    relay::Event::ReservationReqAccepted { renewed: false, .. } => stats.reservations_accepted += 1,
                    relay::Event::ReservationReqDenied { .. } => stats.reservations_denied += 1,
                    relay::Event::CircuitReqAccepted { .. } => {
                        stats.circuits_accepted += 1;
                        stats.circuits_open += 1;
                    }
                    relay::Event::CircuitReqDenied { .. } => stats.circuits_denied += 1,
                    relay::Event::CircuitClosed { .. } => stats.circuits_open = stats.circuits_open.saturating_sub(1),
                    _ => {}
                }
                debug!(?event, "relay");
            }
            // Per-connection lines are debug-only so the log doesn't grow with
            // every peer; the stats line carries the totals.
            SwarmEvent::ConnectionEstablished { peer_id, .. } => debug!(%peer_id, "connected"),
            SwarmEvent::ConnectionClosed { peer_id, .. } => debug!(%peer_id, "disconnected"),
            _ => {}
        }
    }
}

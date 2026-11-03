//! Bootstrap + circuit-relay node for the P2P network.
//!
//! It never stores or reads messages: it only helps peers find each other
//! (Kademlia DHT, server mode) and relays encrypted traffic when two peers
//! can't connect directly (libp2p circuit relay v2).

use std::{env, fs, path::Path, time::Duration};

use anyhow::{Context, Result};
use futures::StreamExt;
use libp2p::{
    core::{upgrade, Transport as _},
    identify, identity, kad, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, websocket, yamux, Multiaddr, SwarmBuilder,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const PROTOCOL_VERSION: &str = "/descord-replacement/0.1.0";

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
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

fn env_port(name: &str, default: u16) -> Result<u16> {
    match env::var(name) {
        Ok(value) => value.parse().with_context(|| format!("{name} must be a port number")),
        Err(_) => Ok(default),
    }
}

fn env_num(name: &str) -> Result<Option<u64>> {
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
    if let Some(n) = env_num("MAX_CIRCUITS")? {
        config.max_circuits = n as usize;
    }
    if let Some(n) = env_num("MAX_CIRCUITS_PER_PEER")? {
        config.max_circuits_per_peer = n as usize;
    }
    if let Some(secs) = env_num("MAX_CIRCUIT_SECS")? {
        config.max_circuit_duration = Duration::from_secs(secs);
    }
    if let Some(bytes) = env_num("MAX_CIRCUIT_BYTES")? {
        config.max_circuit_bytes = bytes;
    }
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let key_path = env::var("KEY_PATH").unwrap_or_else(|_| "/data/node.key".into());
    let key = load_or_create_key(Path::new(&key_path))?;
    let tcp_port = env_port("TCP_PORT", 4001)?;
    let quic_port = env_port("QUIC_PORT", 4001)?;
    let ws_port = env_port("WS_PORT", 4002)?;
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

    loop {
        match swarm.select_next_some().await {
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
            SwarmEvent::Behaviour(BehaviourEvent::Relay(event)) => info!(?event, "relay"),
            SwarmEvent::ConnectionEstablished { peer_id, .. } => info!(%peer_id, "connected"),
            SwarmEvent::ConnectionClosed { peer_id, .. } => info!(%peer_id, "disconnected"),
            _ => {}
        }
    }
}

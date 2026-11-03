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
    io::Write,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use libp2p::{
    connection_limits::{self, ConnectionLimits},
    identify, identity, kad, memory_connection_limits,
    multiaddr::Protocol,
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, StreamProtocol, SwarmBuilder,
};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const PROTOCOL_VERSION: &str = "/mvs/0.1.0";

/// Our own DHT protocol, so it can never merge with the public IPFS DHT
/// (`/ipfs/kad/1.0.0`). Clients (core/sync) must use the same name.
const KAD_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/kad/1.0.0");

#[derive(NetworkBehaviour)]
struct Behaviour {
    // Limits first: they deny a connection before any other behaviour
    // spends anything on it.
    limits: connection_limits::Behaviour,
    memory: memory_connection_limits::Behaviour,
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
        check_key_mode(path)?;
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return identity::Keypair::from_protobuf_encoding(&bytes).context("decoding node key");
    }
    let key = identity::Keypair::generate_ed25519();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let encoded = key.to_protobuf_encoding()?;
    opts.open(path)
        .and_then(|mut f| f.write_all(&encoded))
        .with_context(|| format!("writing {}", path.display()))?;
    info!(path = %path.display(), "generated new node key");
    Ok(key)
}

/// Anyone who can read the key can impersonate the relay, so refuse to run
/// with a key that group or others can read.
#[cfg(unix)]
fn check_key_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "{} has mode {mode:o}; it must be readable by its owner only. Fix: chmod 600 {}",
            path.display(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_key_mode(_: &Path) -> Result<()> {
    Ok(())
}

/// Whether an address is worth handing to other peers via the DHT: public
/// IPs and DNS names only. Private, loopback, link-local and CGNAT addresses
/// would leak a client's LAN layout and are useless to anyone outside it.
fn is_global(addr: &Multiaddr) -> bool {
    match addr.iter().next() {
        Some(Protocol::Ip4(ip)) => is_global_v4(ip),
        Some(Protocol::Ip6(ip)) => is_global_v6(ip),
        Some(Protocol::Dns(name) | Protocol::Dns4(name) | Protocol::Dns6(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name != "localhost" && !name.ends_with(".localhost") && !name.ends_with(".local")
        }
        _ => false,
    }
}

fn is_global_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // CGNAT 100.64.0.0/10
        || a >= 240)
}

fn is_global_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_global_v4(v4);
    }
    let first = ip.segments()[0];
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (first & 0xffc0) == 0xfe80 // link-local fe80::/10
        || first == 0x2001 && ip.segments()[1] == 0x0db8) // documentation
}

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match env::var(name) {
        Ok(value) => value.parse().ok().with_context(|| format!("{name} must be a number")),
        Err(_) => Ok(default),
    }
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
    let limits = ConnectionLimits::default()
        .with_max_established(Some(env_num("MAX_CONNECTIONS", 512u32)?))
        .with_max_established_per_peer(Some(8))
        .with_max_pending_incoming(Some(64));
    // Absolute bytes, not a percentage: the percentage is of the host's RAM,
    // which ignores the container's memory limit (256 MB in compose).
    let max_memory = env_num("MAX_MEMORY_MB", 200usize)? * 1024 * 1024;

    let mut swarm = SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_dns()?
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            let mut kademlia = kad::Behaviour::with_config(
                peer_id,
                kad::store::MemoryStore::new(peer_id),
                kad::Config::new(KAD_PROTOCOL),
            );
            kademlia.set_mode(Some(kad::Mode::Server));
            Behaviour {
                limits: connection_limits::Behaviour::new(limits),
                memory: memory_connection_limits::Behaviour::with_max_bytes(max_memory),
                relay: relay::Behaviour::new(peer_id, relay::Config::default()),
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
                for addr in info.listen_addrs.into_iter().filter(is_global) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn global(addr: &str) -> bool {
        is_global(&addr.parse().unwrap())
    }

    #[test]
    fn keeps_public_addresses() {
        assert!(global("/ip4/8.8.8.8/tcp/4001"));
        assert!(global("/ip4/100.128.0.1/udp/4001/quic-v1"));
        assert!(global("/ip6/2a00:1450::1/tcp/4001"));
        assert!(global("/dns4/relay.example.com/tcp/443/wss"));
        assert!(global("/ip4/8.8.8.8/tcp/4001/p2p/12D3KooWBVzkbuxE3TA4LU7zEeMBM87G4Ns1KQoPdAc4MR8R3GTc/p2p-circuit"));
    }

    #[test]
    fn drops_local_addresses() {
        for addr in [
            "/ip4/127.0.0.1/tcp/4001",
            "/ip4/10.1.2.3/tcp/4001",
            "/ip4/172.16.0.1/tcp/4001",
            "/ip4/192.168.1.10/udp/4001/quic-v1",
            "/ip4/169.254.1.1/tcp/4001",
            "/ip4/100.64.0.1/tcp/4001",
            "/ip4/0.0.0.0/tcp/4001",
            "/ip4/203.0.113.10/tcp/4001",
            "/ip6/::1/tcp/4001",
            "/ip6/fe80::1/tcp/4001",
            "/ip6/fd00::1/tcp/4001",
            "/ip6/::ffff:192.168.1.1/tcp/4001",
            "/dns4/localhost/tcp/4001",
            "/dns4/printer.local/tcp/4001",
            "/p2p-circuit",
        ] {
            assert!(!global(addr), "{addr} should be dropped");
        }
    }

    #[cfg(unix)]
    #[test]
    fn key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!("relay-key-test-{}", std::process::id()));
        let path = dir.join("node.key");
        let _ = fs::remove_dir_all(&dir);
        let first = load_or_create_key(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(load_or_create_key(&path).unwrap().public(), first.public());

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_or_create_key(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}

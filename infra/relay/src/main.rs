//! Bootstrap + circuit-relay node for the P2P network.
//!
//! It never stores or reads messages: it only helps peers find each other
//! (Kademlia DHT, server mode) and relays encrypted traffic when two peers
//! can't connect directly (libp2p circuit relay v2).
//!
//! It also keeps the backup mailbox (`/mvs/backup/1`, backup.rs): opaque,
//! signed server-list backups, under quotas, for restoring on a new device.
//!
//! `relay-node healthcheck` exits 0 if the local TCP listener accepts a
//! connection; the Docker healthcheck runs it (the image has no curl/nc).

mod backup;

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    num::NonZeroU32,
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
    noise, ping, relay, request_response,
    swarm::{NetworkBehaviour, Swarm, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, StreamProtocol, SwarmBuilder,
};
use libp2p::kad::store::RecordStore as _;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const PROTOCOL_VERSION: &str = "/mvs/0.1.0";

/// Our own DHT protocol, so it can never merge with the public IPFS DHT
/// (`/ipfs/kad/1.0.0`). Clients (core/sync) must use the same name.
const KAD_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/kad/1.0.0");

/// TODO(R2): drop once core/sync speaks KAD_PROTOCOL. Until then the relay
/// also answers on the IPFS name so today's clients can still use its DHT.
const LEGACY_KAD_PROTOCOL: StreamProtocol = StreamProtocol::new("/ipfs/kad/1.0.0");

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
    backup: backup::BackupBehaviour,
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

/// Unset or empty (compose passes `${VAR:-}`) means the default.
fn env_num<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => value.parse().ok().with_context(|| format!("{name} must be a number")),
        _ => Ok(default),
    }
}

/// Circuit limits; each variable left unset keeps libp2p's default.
fn relay_config() -> Result<relay::Config> {
    let mut config = relay::Config::default();
    config.max_circuits = env_num("MAX_CIRCUITS", config.max_circuits)?;
    config.max_circuits_per_peer = env_num("MAX_CIRCUITS_PER_PEER", config.max_circuits_per_peer)?;
    config.max_circuit_duration =
        Duration::from_secs(env_num("MAX_CIRCUIT_SECS", config.max_circuit_duration.as_secs())?);
    config.max_circuit_bytes = env_num("MAX_CIRCUIT_BYTES", config.max_circuit_bytes)?;

    // libp2p's default rate limits, rebuilt so the per-IP ones can skip the
    // reverse proxy: every wss client arrives from the proxy's address, and one
    // shared per-IP bucket (60, then one a minute) would starve them all. The
    // per-peer limits and the direct 4001 listeners keep their full protection.
    let proxies = proxy_ips()?;
    let per_peer = Duration::from_secs(120);
    let per_ip = Duration::from_secs(60);
    let mut limits = relay::Config::default();
    limits.reservation_rate_limiters.clear();
    limits.circuit_src_rate_limiters.clear();
    let mut limits = limits
        .reservation_rate_per_peer(NonZeroU32::new(30).unwrap(), per_peer)
        .reservation_rate_per_ip(NonZeroU32::new(60).unwrap(), per_ip)
        .circuit_src_per_peer(NonZeroU32::new(30).unwrap(), per_peer)
        .circuit_src_per_ip(NonZeroU32::new(60).unwrap(), per_ip);
    let ip_reservations = limits.reservation_rate_limiters.pop().expect("per-IP limiter");
    let ip_circuits = limits.circuit_src_rate_limiters.pop().expect("per-IP limiter");
    config.reservation_rate_limiters = vec![
        limits.reservation_rate_limiters.pop().expect("per-peer limiter"),
        Box::new(SkipProxies { inner: ip_reservations, proxies: proxies.clone() }),
    ];
    config.circuit_src_rate_limiters = vec![
        limits.circuit_src_rate_limiters.pop().expect("per-peer limiter"),
        Box::new(SkipProxies { inner: ip_circuits, proxies }),
    ];
    Ok(config)
}

/// PROXY_IPS: comma-separated addresses of the reverse-proxy hop the relay
/// sees (from infra/.env, never committed). Empty = no exemption.
fn proxy_ips() -> Result<Vec<IpAddr>> {
    let raw = env::var("PROXY_IPS").unwrap_or_default();
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().with_context(|| format!("PROXY_IPS: {s:?} is not an IP address")))
        .collect()
}

/// A per-IP rate limiter that lets the reverse proxy's address through.
struct SkipProxies {
    inner: Box<dyn relay::RateLimiter>,
    proxies: Vec<IpAddr>,
}

impl relay::RateLimiter for SkipProxies {
    fn try_next(&mut self, peer: PeerId, addr: &Multiaddr, now: Instant) -> bool {
        if source_ip(addr).is_some_and(|ip| self.proxies.contains(&ip)) {
            return true;
        }
        self.inner.try_next(peer, addr, now)
    }
}

fn source_ip(addr: &Multiaddr) -> Option<IpAddr> {
    addr.iter().find_map(|p| match p {
        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
        Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
        _ => None,
    })
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
    // Test harnesses and LAN setups have only private addresses; never set
    // this on the public relay (it would hand out clients' LAN addresses).
    let relay_config = relay_config()?;
    info!(
        max_circuits = relay_config.max_circuits,
        max_circuits_per_peer = relay_config.max_circuits_per_peer,
        max_circuit_secs = relay_config.max_circuit_duration.as_secs(),
        max_circuit_bytes = relay_config.max_circuit_bytes,
        "circuit limits"
    );
    let allow_private_addrs = env::var("ALLOW_PRIVATE_ADDRS").is_ok_and(|v| v == "1");
    if allow_private_addrs {
        warn!("ALLOW_PRIVATE_ADDRS=1: private addresses go into the DHT; for test/LAN use only");
    }

    let mut swarm = SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_dns()?
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            let mut kad_config = kad::Config::new(KAD_PROTOCOL);
            // Ours first: it is preferred whenever both sides support it.
            // Deprecated in favour of Config::new, which takes one name only.
            #[allow(deprecated)]
            kad_config.set_protocol_names(vec![KAD_PROTOCOL, LEGACY_KAD_PROTOCOL]);
            // The mailbox's provider records live in this (the only) DHT
            // server's own store: no 12-hourly republish queries for up to
            // MAX_TAGS keys. Revisit with a second bootstrap (GAP-9).
            kad_config.set_provider_publication_interval(None);
            let store_config = kad::store::MemoryStoreConfig {
                max_provided_keys: backup::MAX_TAGS + 1024,
                ..Default::default()
            };
            let mut kademlia = kad::Behaviour::with_config(
                peer_id,
                kad::store::MemoryStore::with_config(peer_id, store_config),
                kad_config,
            );
            kademlia.set_mode(Some(kad::Mode::Server));
            Behaviour {
                limits: connection_limits::Behaviour::new(limits),
                memory: memory_connection_limits::Behaviour::with_max_bytes(max_memory),
                relay: relay::Behaviour::new(peer_id, relay_config),
                identify: identify::Behaviour::new(identify::Config::new(
                    PROTOCOL_VERSION.into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
                kademlia,
                backup: backup::behaviour(),
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
    if swarm.external_addresses().next().is_none() {
        // Reservations then carry no address and clients can't listen on a circuit.
        warn!("EXTERNAL_ADDRS is empty: relayed connections will not work");
    }

    info!(peer_id = %swarm.local_peer_id(), "relay node started");

    // The backup mailbox, on the data volume (the only writable path).
    let backup_dir = env::var("BACKUP_DIR").unwrap_or_else(|_| "/data/backup".into());
    let mut mailbox = backup::Mailbox::open(Path::new(&backup_dir))
        .with_context(|| format!("opening the backup mailbox at {backup_dir}"))?;
    mailbox.sweep(backup::unix_now());
    let local_id = *swarm.local_peer_id();
    for tag in mailbox.tags().copied().collect::<Vec<_>>() {
        provide(&mut swarm, local_id, &tag);
    }
    info!(stored = mailbox.counts.stored, "backup mailbox");
    let proxies = proxy_ips()?;
    // Each connected peer's source IP: request-response events carry the
    // peer only. Never logged.
    let mut peer_ips: HashMap<PeerId, IpAddr> = HashMap::new();

    let started = Instant::now();
    let mut stats = Stats::default();
    // Peers whose circuit addresses we put into the DHT.
    let mut reserved = HashSet::new();
    let mut stats_timer = tokio::time::interval(stats_every);
    let mut sweep_timer = tokio::time::interval(backup::SWEEP_EVERY);

    loop {
        let event = tokio::select! {
            event = swarm.select_next_some() => event,
            _ = sweep_timer.tick() => {
                for tag in mailbox.sweep(backup::unix_now()) {
                    swarm.behaviour_mut().kademlia.store_mut().remove_provider(&kad::RecordKey::new(&tag), &local_id);
                }
                continue;
            }
            _ = stats_timer.tick() => {
                let net = swarm.network_info();
                let b = mailbox.counts;
                info!(
                    uptime_secs = started.elapsed().as_secs(),
                    peers = net.num_peers(),
                    connections = net.connection_counters().num_established(),
                    circuits_open = stats.circuits_open,
                    circuits_accepted = stats.circuits_accepted,
                    circuits_denied = stats.circuits_denied,
                    reservations_accepted = stats.reservations_accepted,
                    reservations_denied = stats.reservations_denied,
                    backups_stored = b.stored,
                    backup_writes_ok = b.writes_ok,
                    backup_refused_invalid = b.refused_invalid,
                    backup_refused_not_newer = b.refused_not_newer,
                    backup_refused_rate = b.refused_rate,
                    backup_refused_full = b.refused_full,
                    backup_refused_disk = b.refused_disk,
                    backup_fetch_hit = b.fetch_hit,
                    backup_fetch_miss = b.fetch_miss,
                    backups_expired = b.expired,
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
                for addr in info.listen_addrs.into_iter().filter(|a| allow_private_addrs || is_global(a)) {
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Relay(event)) => {
                match &event {
                    // The peer is now reachable through us. Identify ran before
                    // the reservation existed, so without this the DHT would only
                    // hold its direct addresses and a relay-only peer could not
                    // be found.
                    relay::Event::ReservationReqAccepted { src_peer_id, .. } => {
                        reserved.insert(*src_peer_id);
                        let relay_id = *swarm.local_peer_id();
                        for addr in circuit_addrs(swarm.external_addresses(), relay_id) {
                            swarm.behaviour_mut().kademlia.add_address(src_peer_id, addr);
                        }
                    }
                    // Kademlia's routing table has no TTL, so the circuit
                    // addresses are withdrawn when the reservation ends.
                    relay::Event::ReservationTimedOut { src_peer_id } => {
                        forget_circuit(&mut swarm, &mut reserved, src_peer_id)
                    }
                    _ => {}
                }
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
            SwarmEvent::Behaviour(BehaviourEvent::Backup(request_response::Event::Message {
                peer,
                message: request_response::Message::Request { request, channel, .. },
            })) => {
                let source = match peer_ips.get(&peer) {
                    Some(ip) if !proxies.contains(ip) => backup::Source::Ip(*ip),
                    _ => backup::Source::Proxied(peer),
                };
                let now = backup::unix_now();
                let answer = match request {
                    backup::Request::Put(blob) => match mailbox.put(source, &blob, now) {
                        Ok(tag) => {
                            provide(&mut swarm, local_id, &tag);
                            vec![1]
                        }
                        Err(_) => vec![0],
                    },
                    backup::Request::Get(tag) => mailbox.get(source, &tag, now).unwrap_or_default(),
                };
                // The peer may be gone already; nothing to do then.
                let _ = swarm.behaviour_mut().backup.send_response(channel, answer);
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                if let Some(ip) = source_ip(endpoint.get_remote_address()) {
                    peer_ips.insert(peer_id, ip);
                }
                debug!(%peer_id, "connected")
            }
            SwarmEvent::ConnectionClosed { peer_id, num_established, .. } => {
                debug!(%peer_id, "disconnected");
                // The relay drops a reservation silently with its connection.
                if num_established == 0 {
                    forget_circuit(&mut swarm, &mut reserved, &peer_id);
                    peer_ips.remove(&peer_id);
                }
            }
            _ => {}
        }
    }
}

/// Announces the relay as a holder of `tag` in its own DHT store (design §2:
/// provider records point restoring devices at the mailbox).
fn provide(swarm: &mut Swarm<Behaviour>, local_id: PeerId, tag: &backup::Tag) {
    let record = kad::ProviderRecord::new(kad::RecordKey::new(tag), local_id, Vec::new());
    let _ = swarm.behaviour_mut().kademlia.store_mut().add_provider(record);
}

fn forget_circuit(swarm: &mut Swarm<Behaviour>, reserved: &mut HashSet<PeerId>, peer: &PeerId) {
    if reserved.remove(peer) {
        let relay_id = *swarm.local_peer_id();
        for addr in circuit_addrs(swarm.external_addresses(), relay_id) {
            swarm.behaviour_mut().kademlia.remove_address(peer, &addr);
        }
    }
}

/// `<external>/p2p/<relay>/p2p-circuit` for each of our external addresses:
/// where a peer holding a reservation with us can be dialed.
fn circuit_addrs<'a>(external: impl Iterator<Item = &'a Multiaddr>, relay_id: PeerId) -> Vec<Multiaddr> {
    external
        .map(|addr| {
            let addr = match addr.iter().last() {
                Some(Protocol::P2p(_)) => addr.clone(),
                _ => addr.clone().with(Protocol::P2p(relay_id)),
            };
            addr.with(Protocol::P2pCircuit)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_proxy_skips_the_per_ip_limit() {
        use libp2p::relay::RateLimiter as _;
        let refuse_all: Box<dyn relay::RateLimiter> = Box::new(|_: PeerId, _: &Multiaddr, _: Instant| false);
        let mut limiter = SkipProxies { inner: refuse_all, proxies: vec!["10.0.0.5".parse().unwrap()] };
        let peer = PeerId::random();
        let now = Instant::now();
        assert!(limiter.try_next(peer, &"/ip4/10.0.0.5/tcp/51000".parse().unwrap(), now));
        assert!(!limiter.try_next(peer, &"/ip4/10.0.0.6/tcp/51000".parse().unwrap(), now));
        assert!(!limiter.try_next(peer, &"/ip6/::1/tcp/51000".parse().unwrap(), now));
    }

    #[test]
    fn circuit_addresses_go_through_the_relay() {
        let relay_id = PeerId::random();
        let external: Vec<Multiaddr> = vec![
            "/dns4/relay.example.org/tcp/4001".parse().unwrap(),
            format!("/dns4/relay.example.org/tcp/443/wss/p2p/{relay_id}").parse().unwrap(),
        ];
        let addrs = circuit_addrs(external.iter(), relay_id);
        assert_eq!(addrs[0].to_string(), format!("/dns4/relay.example.org/tcp/4001/p2p/{relay_id}/p2p-circuit"));
        assert_eq!(addrs[1].to_string(), format!("/dns4/relay.example.org/tcp/443/wss/p2p/{relay_id}/p2p-circuit"));
    }

    fn global(addr: &str) -> bool {
        is_global(&addr.parse().unwrap())
    }

    #[test]
    fn keeps_public_addresses() {
        assert!(global("/ip4/8.8.8.8/tcp/4001"));
        assert!(global("/ip4/100.128.0.1/udp/4001/quic-v1"));
        assert!(global("/ip6/2a00:1450::1/tcp/4001"));
        assert!(global("/dns4/relay.example.org/tcp/443/wss"));
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

//! Spike experiment 1: can two computers behind home/mobile NATs connect
//! directly, or do they need the relay?
//!
//! Person A runs `spike-peer listen <relay-addr>` and sends the printed
//! command to person B, who runs `spike-peer dial <relay-addr> <peer-id>`.
//! B first connects to A through the relay, then both try to hole-punch a
//! direct connection (DCUtR). B prints one RESULT line and exits.
//!
//! Started with no arguments (double-click on Windows), it asks for the role
//! and addresses instead, and keeps the window open until Enter is pressed.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use libp2p::{
    core::{multiaddr::Protocol, ConnectedPoint},
    dcutr, identify, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const PROTOCOL_VERSION: &str = "/mvs/0.1.0";
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    dcutr: dcutr::Behaviour,
}

enum Mode {
    Listen,
    Dial(PeerId),
}

const USAGE: &str = "usage:\n  spike-peer listen <relay-addr>\n  spike-peer dial <relay-addr> <peer-id>\n\
                     <relay-addr> looks like /ip4/1.2.3.4/tcp/4001/p2p/12D3KooW...";

fn prompt(question: &str) -> Result<String> {
    print!("{question}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    // Some shells prefix piped input with a byte-order mark.
    Ok(line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}').to_string())
}

/// Asks for the same values the command line would take.
fn ask_for_args() -> Result<Vec<String>> {
    println!("NAT test - choose a role:");
    println!("  1 = wait for a friend to connect (listen)");
    println!("  2 = connect to a friend who is waiting (dial)");
    let role = prompt("Your choice (1 or 2): ")?;
    let relay = prompt("Relay address (paste what Moran sent you): ")?;
    match role.as_str() {
        "1" => Ok(vec!["listen".into(), relay]),
        "2" => {
            let peer = prompt("Your friend's ID (starts with 12D3KooW): ")?;
            Ok(vec!["dial".into(), relay, peer])
        }
        _ => bail!("please type 1 or 2"),
    }
}

fn parse_args(args: &[String]) -> Result<(Mode, Multiaddr, PeerId)> {
    let (mode, relay) = match args {
        [cmd, relay] if cmd == "listen" => (Mode::Listen, relay),
        [cmd, relay, peer] if cmd == "dial" => {
            (Mode::Dial(peer.parse().context("invalid peer ID")?), relay)
        }
        _ => bail!(USAGE),
    };
    let relay_addr: Multiaddr = relay.parse().context("invalid relay address")?;
    let relay_peer = match relay_addr.iter().last() {
        Some(Protocol::P2p(peer)) => peer,
        _ => bail!("the relay address must end with /p2p/<relay peer ID>"),
    };
    Ok((mode, relay_addr, relay_peer))
}

fn is_relayed(endpoint: &ConnectedPoint) -> bool {
    let has_circuit = |addr: &Multiaddr| addr.iter().any(|p| p == Protocol::P2pCircuit);
    match endpoint {
        ConnectedPoint::Dialer { address, .. } => has_circuit(address),
        // An incoming relayed connection carries the circuit in the local
        // (listening) address, not in the remote one.
        ConnectedPoint::Listener { local_addr, send_back_addr } => {
            has_circuit(local_addr) || has_circuit(send_back_addr)
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let interactive = args.is_empty();

    let result = run(args, interactive).await;
    if let Err(err) = &result {
        eprintln!("Error: {err:#}");
    }
    if interactive {
        let _ = prompt("\nPress Enter to close...");
    }
    if result.is_err() {
        std::process::exit(1);
    }
}

async fn run(args: Vec<String>, interactive: bool) -> Result<()> {
    let args = if interactive { ask_for_args()? } else { args };
    let (mode, relay_addr, relay_peer) = parse_args(&args)?;

    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| Behaviour {
            relay_client,
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(2))),
            identify: identify::Behaviour::new(identify::Config::new(
                PROTOCOL_VERSION.into(),
                key.public(),
            )),
            dcutr: dcutr::Behaviour::new(key.public().to_peer_id()),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Connect to the relay and learn our public address from it. That
    // address is what the other side will try to punch through to. A wrong
    // or unreachable relay must end the run, not hang it.
    swarm.dial(relay_addr.clone())?;
    let relay_deadline = tokio::time::sleep(DIAL_TIMEOUT);
    tokio::pin!(relay_deadline);
    loop {
        tokio::select! {
            _ = &mut relay_deadline => {
                return relay_failed(&relay_addr, &format!("no answer within {} s", DIAL_TIMEOUT.as_secs()));
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }))
                    if peer_id == relay_peer =>
                {
                    info!(address = %info.observed_addr, "relay sees us at");
                    swarm.add_external_address(info.observed_addr);
                    break;
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. }
                    if peer_id.is_none() || peer_id == Some(relay_peer) =>
                {
                    return relay_failed(&relay_addr, &error.to_string());
                }
                _ => {}
            },
        }
    }

    let circuit = relay_addr.clone().with(Protocol::P2pCircuit);
    let started = Instant::now();
    match &mode {
        Mode::Listen => {
            swarm.listen_on(circuit.clone())?;
            let me = *swarm.local_peer_id();
            println!("\nWaiting for a friend. Keep this window open and send them your ID:\n");
            println!("  {me}\n");
            println!("(Or the full command: spike-peer dial {relay_addr} {me})\n");
        }
        Mode::Dial(peer) => swarm.dial(circuit.with(Protocol::P2p(*peer)))?,
    }

    let mut relayed_at = None;
    let mut direct_at = None;
    let mut direct_rtt = None;
    let mut punch_error = None;
    let deadline = tokio::time::sleep(DIAL_TIMEOUT);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline, if matches!(mode, Mode::Dial(_)) => break,
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(BehaviourEvent::RelayClient(
                    relay::client::Event::ReservationReqAccepted { .. },
                )) => info!("relay reservation accepted"),
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } if peer_id != relay_peer => {
                    let relayed = is_relayed(&endpoint);
                    info!(%peer_id, relayed, address = %endpoint.get_remote_address(), "connected");
                    if relayed {
                        relayed_at.get_or_insert(started.elapsed());
                    } else {
                        direct_at.get_or_insert(started.elapsed());
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(dcutr::Event { remote_peer_id, result })) => {
                    match result {
                        Ok(_) => info!(%remote_peer_id, "hole punch succeeded"),
                        Err(err) => {
                            warn!(%remote_peer_id, %err, "hole punch failed");
                            punch_error = Some(err.to_string());
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Ping(ping::Event { peer, connection, result: Ok(rtt) }))
                    if peer != relay_peer =>
                {
                    info!(%peer, ?connection, rtt_ms = rtt.as_millis(), "ping");
                    if direct_at.is_some() {
                        direct_rtt = Some(rtt);
                        if matches!(mode, Mode::Dial(_)) {
                            break;
                        }
                    }
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    warn!(?peer_id, %error, "connection failed");
                }
                _ => {}
            },
        }
    }

    let ms = |d: Option<Duration>| d.map_or("-".to_string(), |d| d.as_millis().to_string());
    let path = match (direct_at, relayed_at) {
        (Some(_), _) => "direct",
        (None, Some(_)) => "relayed",
        (None, None) => "failed",
    };
    println!(
        "RESULT path={path} relayed_ms={} direct_ms={} rtt_ms={} punch_error={}",
        ms(relayed_at),
        ms(direct_at),
        ms(direct_rtt),
        punch_error.as_deref().unwrap_or("-"),
    );
    Ok(())
}

/// Ends the run when the relay can't be reached: the same RESULT line as a
/// failed test (so collected results stay uniform), then a plain error.
fn relay_failed(relay_addr: &Multiaddr, reason: &str) -> Result<()> {
    println!("RESULT path=failed relayed_ms=- direct_ms=- rtt_ms=- punch_error=relay_unreachable");
    bail!(
        "could not connect to the relay at {relay_addr}: {reason}\n\
         Check that the relay address was copied exactly (it must end with the relay's /p2p/12D3KooW... ID) \
         and that this computer is online."
    )
}

//! Harness peer: a sync node driven over a tiny control port.
//!
//! `sync-peer <name>` listens on TCP 7000 for one JSON command per line and
//! answers with one JSON line. Commands: start, stop, write, state,
//! latencies, reset. The node itself only runs between `start` and `stop`,
//! so a stop/start pair exercises the same path as a process restart:
//! open the SQLite store, dial the relay, discover peers, pull what's missing.
//!
//! Environment: RELAY (bootstrap/relay multiaddr), DATA_DIR (default /data).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use rand_core::OsRng;
use records::{Draft, Kind, PublicKey, SigningKey, GENESIS_NONCE_LEN, MAX_DEPS};
use sync::{log_name, AcceptAll, ChannelId, Node, NodeConfig, NodeEvent, Record, Scope, ServerId, Via};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing_subscriber::EnvFilter;

/// The harness server: its owner key is a fixed-seed test key (not secret),
/// so every peer derives the same server id without exchanging anything.
fn harness_channel() -> (ServerId, ChannelId) {
    let owner = SigningKey::from_bytes(&[0x42; 32]);
    let server = ServerId::derive(&PublicKey::from_bytes(owner.verifying_key().to_bytes()), &[0; GENESIS_NONCE_LEN]);
    (server, ChannelId::from_bytes([1; 32]))
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Command {
    Start,
    Stop,
    Write { n: u64, interval_ms: u64 },
    State,
    Latencies,
    Reset,
}

#[derive(Default)]
struct Stats {
    started: Option<Instant>,
    first_peer_ms: Option<u128>,
    first_remote_ms: Option<u128>,
    last_remote_ms: Option<u128>,
    /// Write time → stored here, for blobs written by other peers.
    latencies_ms: Vec<f64>,
    via_push: u64,
    via_pull: u64,
    writes_done: u64,
    writing: bool,
}

struct Peer {
    name: String,
    relay: String,
    data_dir: String,
    node: tokio::sync::Mutex<Option<Node>>,
    stats: Arc<Mutex<Stats>>,
    seq: Mutex<u64>,
    /// This peer's author key (random per process).
    key: SigningKey,
    server: ServerId,
    channel: ChannelId,
    log: String,
}

fn now_micros() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
}

/// Message body: `<author>|<seq>|<written at, unix micros>|<text>`.
fn written_at(data: &[u8]) -> Option<u64> {
    std::str::from_utf8(data).ok()?.split('|').nth(2)?.parse().ok()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let name = std::env::args().nth(1).context("usage: sync-peer <name>")?;
    let (server, channel) = harness_channel();
    let peer = Arc::new(Peer {
        name,
        relay: std::env::var("RELAY").context("RELAY is not set")?,
        data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| "/data".into()),
        node: tokio::sync::Mutex::new(None),
        stats: Arc::default(),
        seq: Mutex::new(0),
        key: SigningKey::generate(&mut OsRng),
        server,
        channel,
        log: log_name(server, Scope::Channel(channel)),
    });
    let listener = TcpListener::bind("0.0.0.0:7000").await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let peer = peer.clone();
        tokio::spawn(async move {
            if let Err(err) = serve(peer, stream).await {
                tracing::warn!(%err, "control connection failed");
            }
        });
    }
}

async fn serve(peer: Arc<Peer>, stream: TcpStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let reply = match serde_json::from_str::<Command>(&line) {
            Ok(command) => handle(&peer, command).await.unwrap_or_else(|err| json!({ "error": format!("{err:#}") })),
            Err(err) => json!({ "error": err.to_string() }),
        };
        write.write_all(format!("{reply}\n").as_bytes()).await?;
    }
    Ok(())
}

async fn handle(peer: &Arc<Peer>, command: Command) -> Result<Value> {
    match command {
        Command::Start => {
            let mut node = peer.node.lock().await;
            anyhow::ensure!(node.is_none(), "already running");
            {
                let mut stats = peer.stats.lock().unwrap();
                stats.started = Some(Instant::now());
                stats.first_peer_ms = None;
                stats.first_remote_ms = None;
                stats.last_remote_ms = None;
            }
            let (started, events) = Node::start(NodeConfig {
                relay: peer.relay.parse()?,
                data_dir: peer.data_dir.clone().into(),
                logs: vec![peer.log.clone()],
                // The harness measures sync, not permissions.
                authorizer: Arc::new(AcceptAll),
            })
            .await?;
            let peer_id = started.peer_id();
            *node = Some(started);
            tokio::spawn(record_events(peer.stats.clone(), events));
            Ok(json!({ "peer_id": peer_id.to_string() }))
        }
        Command::Stop => {
            let node = peer.node.lock().await.take().context("not running")?;
            node.shutdown().await?;
            Ok(json!({ "stopped": true }))
        }
        Command::Write { n, interval_ms } => {
            peer.stats.lock().unwrap().writing = true;
            let peer = peer.clone();
            tokio::spawn(async move {
                for _ in 0..n {
                    let seq = {
                        let mut seq = peer.seq.lock().unwrap();
                        *seq += 1;
                        *seq
                    };
                    let body = format!("{}|{seq}|{}|הודעה מספר {seq}", peer.name, now_micros());
                    let result = match peer.node.lock().await.as_ref() {
                        Some(node) => write(&peer, node, seq, body.into_bytes()).await,
                        None => Err(anyhow::anyhow!("not running")),
                    };
                    if let Err(err) = result {
                        tracing::warn!(%err, "write failed");
                        break;
                    }
                    peer.stats.lock().unwrap().writes_done += 1;
                    if interval_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                    }
                }
                peer.stats.lock().unwrap().writing = false;
            });
            Ok(json!({ "writing": n }))
        }
        Command::State => {
            let node = peer.node.lock().await;
            let mut reply = {
                let stats = peer.stats.lock().unwrap();
                json!({
                    "running": node.is_some(),
                    "writes_done": stats.writes_done,
                    "writing": stats.writing,
                    "first_peer_ms": stats.first_peer_ms,
                    "first_remote_ms": stats.first_remote_ms,
                    "last_remote_ms": stats.last_remote_ms,
                    "since_start_ms": stats.started.map(|s| s.elapsed().as_millis()),
                })
            };
            if let Some(node) = node.as_ref() {
                let state = node.state(&peer.log).await?;
                reply["count"] = json!(state.count);
                reply["hash"] = json!(state.hash);
                reply["heads"] = json!(state.heads);
                reply["peers"] = json!(node.peers().await?.len());
            }
            Ok(reply)
        }
        Command::Latencies => {
            let stats = peer.stats.lock().unwrap();
            Ok(json!({
                "latencies_ms": stats.latencies_ms,
                "via_push": stats.via_push,
                "via_pull": stats.via_pull,
            }))
        }
        Command::Reset => {
            let mut stats = peer.stats.lock().unwrap();
            stats.latencies_ms.clear();
            stats.via_push = 0;
            stats.via_pull = 0;
            Ok(json!({ "reset": true }))
        }
    }
}

async fn record_events(stats: Arc<Mutex<Stats>>, mut events: tokio::sync::mpsc::Receiver<NodeEvent>) {
    while let Some(event) = events.recv().await {
        let mut stats = stats.lock().unwrap();
        let since_start = stats.started.map(|s| s.elapsed().as_millis());
        match event {
            NodeEvent::PeerConnected(_) => {
                if stats.first_peer_ms.is_none() {
                    stats.first_peer_ms = since_start;
                }
            }
            NodeEvent::Stored { record, via, at, .. } if via != Via::Local => {
                if stats.first_remote_ms.is_none() {
                    stats.first_remote_ms = since_start;
                }
                stats.last_remote_ms = since_start;
                match via {
                    Via::Push => stats.via_push += 1,
                    _ => stats.via_pull += 1,
                }
                if let Some(written) = written_at(record.body()) {
                    let at = at.duration_since(UNIX_EPOCH).unwrap().as_micros() as u64;
                    stats.latencies_ms.push(at.saturating_sub(written) as f64 / 1000.0);
                }
            }
            _ => {}
        }
    }
}

/// Signs a message on top of the log's current heads and appends it.
async fn write(peer: &Peer, node: &Node, seq: u64, body: Vec<u8>) -> Result<()> {
    let mut deps = node.heads(&peer.log).await?;
    // More heads than a record may point at: merge some now, the rest later.
    deps.truncate(MAX_DEPS);
    let record: Record = Draft {
        server: peer.server,
        scope: Scope::Channel(peer.channel),
        seq,
        deps,
        auth_deps: Vec::new(),
        ts: now_micros() / 1000,
        kind: Kind::Message,
        body,
    }
    .sign(&peer.key)?;
    node.append(record).await
}

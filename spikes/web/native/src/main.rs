//! Native peer for spike 5: everything a core/sync node does on the wire,
//! plus a WebRTC (webrtc-direct) listener so a browser can connect to it
//! without the relay.
//!
//! - Store: `sync::Store` (core/sync's SQLite store), one log.
//! - Protocols: `/descord-replacement/sync/0.1.0` pull (CBOR request-response)
//!   and gossipsub push on `log/<name>`, exactly as core/sync.
//! - Transports: TCP, webrtc-direct (UDP), relay circuit (listens on a
//!   circuit through the relay so browsers can reach it relayed too).
//!
//! Driven like core/sync's `sync-peer`: one JSON command per line on TCP 7000.
//! Commands: start, write {n, interval_ms}, state, latencies, reset.
//! Environment: RELAY, DATA_DIR (default /data), WEBRTC_PORT (default 9090).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use libp2p::{
    core::{multiaddr::Protocol, muxing::StreamMuxerBox, Transport},
    gossipsub, identify, identity, noise, ping, relay,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder,
};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use serde_json::{json, Value};
use sync::{Append, Blob, Store};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use web_wire::{SyncRequest, SyncResponse, IDENTIFY_VERSION, PAGE, SYNC_PROTOCOL};

const LOG: &str = "general";

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    gossipsub: gossipsub::Behaviour,
    sync: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Command {
    Start,
    Write { n: u64, interval_ms: u64 },
    State,
    Latencies,
    Reset,
}

enum NodeCmd {
    Append { data: Vec<u8>, reply: oneshot::Sender<Result<()>> },
    State { reply: oneshot::Sender<Value> },
}

#[derive(Default)]
struct Stats {
    latencies_ms: Vec<f64>,
    via_push: u64,
    via_pull: u64,
    writes_done: u64,
    writing: bool,
}

struct Peer {
    name: String,
    node: tokio::sync::Mutex<Option<(PeerId, mpsc::UnboundedSender<NodeCmd>)>>,
    stats: Arc<Mutex<Stats>>,
    seq: Mutex<u64>,
}

fn now_micros() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
}

/// Message body: `<author>|<seq>|<written at, unix micros>|<text>` (as in sync-peer).
fn written_at(data: &[u8]) -> Option<u64> {
    std::str::from_utf8(data).ok()?.split('|').nth(2)?.parse().ok()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let name = std::env::args().nth(1).unwrap_or_else(|| "native".into());
    let peer = Arc::new(Peer {
        name,
        node: tokio::sync::Mutex::new(None),
        stats: Arc::default(),
        seq: Mutex::new(0),
    });
    let listener = TcpListener::bind("0.0.0.0:7000").await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let peer = peer.clone();
        tokio::spawn(async move {
            if let Err(err) = serve(peer, stream).await {
                warn!(%err, "control connection failed");
            }
        });
    }
}

async fn serve(peer: Arc<Peer>, stream: TcpStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let reply = match serde_json::from_str::<Command>(&line) {
            Ok(command) => handle(&peer, command)
                .await
                .unwrap_or_else(|err| json!({ "error": format!("{err:#}") })),
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
            let relay: Multiaddr = std::env::var("RELAY").context("RELAY is not set")?.parse()?;
            let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "/data".into());
            let webrtc_port: u16 =
                std::env::var("WEBRTC_PORT").ok().map(|p| p.parse()).transpose()?.unwrap_or(9090);
            let (peer_id, commands) =
                start(relay, data_dir.into(), webrtc_port, peer.stats.clone()).await?;
            *node = Some((peer_id, commands));
            Ok(json!({ "peer_id": peer_id.to_string() }))
        }
        Command::Write { n, interval_ms } => {
            let commands = peer.node.lock().await.as_ref().context("not running")?.1.clone();
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
                    let (reply, rx) = oneshot::channel();
                    if commands.send(NodeCmd::Append { data: body.into_bytes(), reply }).is_err() {
                        break;
                    }
                    if let Ok(Err(err)) | Err(err) = rx.await.map_err(anyhow::Error::from) {
                        warn!(%err, "write failed");
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
            let commands = peer.node.lock().await.as_ref().context("not running")?.1.clone();
            let (reply, rx) = oneshot::channel();
            commands.send(NodeCmd::State { reply }).ok().context("node stopped")?;
            let mut state = rx.await?;
            let stats = peer.stats.lock().unwrap();
            state["writes_done"] = json!(stats.writes_done);
            state["writing"] = json!(stats.writing);
            Ok(state)
        }
        Command::Latencies => {
            let stats = peer.stats.lock().unwrap();
            Ok(
                json!({ "latencies_ms": stats.latencies_ms, "via_push": stats.via_push, "via_pull": stats.via_pull }),
            )
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

fn load_or_create_key(path: &std::path::Path) -> Result<identity::Keypair> {
    if let Ok(bytes) = std::fs::read(path) {
        return Ok(identity::Keypair::from_protobuf_encoding(&bytes)?);
    }
    let key = identity::Keypair::generate_ed25519();
    std::fs::write(path, key.to_protobuf_encoding()?)?;
    Ok(key)
}

async fn start(
    relay: Multiaddr,
    data_dir: std::path::PathBuf,
    webrtc_port: u16,
    stats: Arc<Mutex<Stats>>,
) -> Result<(PeerId, mpsc::UnboundedSender<NodeCmd>)> {
    std::fs::create_dir_all(&data_dir)?;
    let key = load_or_create_key(&data_dir.join("node.key"))?;
    let store = Store::open(data_dir.join("store.db"))?;
    let Some(Protocol::P2p(relay_peer)) = relay.iter().last() else {
        bail!("the relay address must end with /p2p/<relay peer ID>");
    };
    // A fresh self-signed certificate per start. Its hash goes into the
    // advertised address (/certhash/...), so a real node would persist it.
    let cert = libp2p_webrtc::tokio::Certificate::generate(&mut rand::thread_rng())?;

    let mut swarm = SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)?
        .with_other_transport(|key| {
            libp2p_webrtc::tokio::Transport::new(key.clone(), cert)
                .map(|(peer_id, conn), _| (peer_id, StreamMuxerBox::new(conn)))
        })?
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| {
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_millis(500))
                    .build()
                    .map_err(|e| anyhow::anyhow!(e))?,
            )
            .map_err(|e| anyhow::anyhow!(e))?;
            Ok(Behaviour {
                relay_client,
                identify: identify::Behaviour::new(identify::Config::new(
                    IDENTIFY_VERSION.into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
                gossipsub,
                sync: request_response::cbor::Behaviour::new(
                    [(StreamProtocol::new(SYNC_PROTOCOL), ProtocolSupport::Full)],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(30)),
                ),
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{webrtc_port}/webrtc-direct").parse()?)?;
    swarm.behaviour_mut().gossipsub.subscribe(&gossipsub::IdentTopic::new(web_wire::topic(LOG)))?;
    swarm.dial(relay.clone())?;

    let peer_id = *swarm.local_peer_id();
    let (commands, rx) = mpsc::unbounded_channel();
    let task = Task {
        swarm,
        store,
        relay,
        relay_peer,
        circuit_listening: false,
        connected: HashMap::new(),
        listen_addrs: Vec::new(),
        inflight: HashSet::new(),
        stats,
    };
    tokio::spawn(task.run(rx));
    Ok((peer_id, commands))
}

struct Task {
    swarm: Swarm<Behaviour>,
    store: Store,
    relay: Multiaddr,
    relay_peer: PeerId,
    circuit_listening: bool,
    /// Peer → remote address of each open connection.
    connected: HashMap<PeerId, Vec<String>>,
    listen_addrs: Vec<String>,
    inflight: HashSet<PeerId>,
    stats: Arc<Mutex<Stats>>,
}

impl Task {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<NodeCmd>) {
        let mut anti_entropy = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.on_command(command),
                    None => return,
                },
                event = self.swarm.select_next_some() => self.on_swarm_event(event),
                _ = anti_entropy.tick() => {
                    for peer in self.connected.keys().copied().collect::<Vec<_>>() {
                        self.pull(peer);
                    }
                }
            }
        }
    }

    fn on_command(&mut self, command: NodeCmd) {
        match command {
            NodeCmd::Append { data, reply } => {
                let result = (|| {
                    let blob = Blob::new(self.store.heads(LOG)?, data);
                    self.store.append(LOG, &blob)?;
                    let topic = gossipsub::IdentTopic::new(web_wire::topic(LOG));
                    let _ = self.swarm.behaviour_mut().gossipsub.publish(topic, blob.encode());
                    Ok(())
                })();
                let _ = reply.send(result);
            }
            NodeCmd::State { reply } => {
                let state = json!({
                    "running": true,
                    "count": self.store.count(LOG).unwrap_or(0),
                    "hash": self.store.state_hash(LOG).unwrap_or_default(),
                    "peers": self.connected.len(),
                    "connections": self.connected.iter().map(|(p, a)| (p.to_string(), a)).collect::<HashMap<_, _>>(),
                    "listen_addrs": self.listen_addrs,
                });
                let _ = reply.send(state);
            }
        }
    }

    fn pull(&mut self, peer: PeerId) {
        if !self.inflight.insert(peer) {
            return;
        }
        let have = self.store.have_markers(LOG).unwrap_or_default();
        let have = have.into_iter().map(|id| ByteBuf::from(id.to_vec())).collect();
        self.swarm.behaviour_mut().sync.send_request(&peer, SyncRequest { log: LOG.into(), have });
    }

    fn stored(&self, blob: &Blob, push: bool) {
        let mut stats = self.stats.lock().unwrap();
        if push {
            stats.via_push += 1;
        } else {
            stats.via_pull += 1;
        }
        if let Some(written) = written_at(&blob.data) {
            stats.latencies_ms.push(now_micros().saturating_sub(written) as f64 / 1000.0);
        }
    }

    fn on_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!(%address, "listening");
                self.listen_addrs.push(address.to_string());
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                let addr = endpoint.get_remote_address().to_string();
                info!(%peer_id, %addr, "connected");
                if peer_id != self.relay_peer {
                    self.connected.entry(peer_id).or_default().push(addr);
                    self.pull(peer_id);
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, endpoint, num_established, .. } => {
                info!(%peer_id, num_established, "connection closed");
                if num_established == 0 {
                    self.connected.remove(&peer_id);
                    self.inflight.remove(&peer_id);
                } else if let Some(addrs) = self.connected.get_mut(&peer_id) {
                    let addr = endpoint.get_remote_address().to_string();
                    if let Some(i) = addrs.iter().position(|a| *a == addr) {
                        addrs.remove(i);
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                ..
            })) if peer_id == self.relay_peer && !self.circuit_listening => {
                self.circuit_listening = true;
                if let Err(err) =
                    self.swarm.listen_on(self.relay.clone().with(Protocol::P2pCircuit))
                {
                    warn!(%err, "listening on relay circuit failed");
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            })) => {
                let Some(blob) = Blob::decode(&message.data) else { return };
                match self.store.append(LOG, &blob) {
                    Ok(Append::Added) => self.stored(&blob, true),
                    Ok(Append::MissingDeps(_)) => self.pull(propagation_source),
                    Ok(Append::Known) => {}
                    Err(err) => warn!(%err, "storing pushed blob failed"),
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message {
                peer,
                message,
            })) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let have: Vec<_> =
                        request.have.iter().filter_map(|b| b.as_slice().try_into().ok()).collect();
                    let (blobs, more) =
                        self.store.missing_since(LOG, &have, PAGE).unwrap_or_default();
                    let blobs = blobs.iter().map(|b| ByteBuf::from(b.encode())).collect();
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .sync
                        .send_response(channel, SyncResponse { blobs, more });
                }
                request_response::Message::Response { response, .. } => {
                    self.inflight.remove(&peer);
                    debug!(%peer, blobs = response.blobs.len(), "pulled");
                    let blobs: Vec<Blob> =
                        response.blobs.iter().filter_map(|b| Blob::decode(b)).collect();
                    match self.store.append_all(LOG, &blobs) {
                        Ok((added, _)) => {
                            let added: HashSet<_> = added.into_iter().collect();
                            for blob in blobs.iter().filter(|b| added.contains(&b.id)) {
                                self.stored(blob, false);
                            }
                        }
                        Err(err) => warn!(%err, "storing pulled blobs failed"),
                    }
                    if response.more {
                        self.pull(peer);
                    }
                }
            },
            SwarmEvent::Behaviour(BehaviourEvent::Sync(
                request_response::Event::OutboundFailure { peer, error, .. },
            )) => {
                warn!(%peer, %error, "pull failed");
                self.inflight.remove(&peer);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    /// The browser's wire crate encodes blobs byte-for-byte like core/sync.
    #[test]
    fn wire_matches_core_sync() {
        let a = sync::Blob::new(vec![], b"a".to_vec());
        let b = sync::Blob::new(vec![a.id, [7; 32]], "שלום".as_bytes().to_vec());
        let wb = web_wire::Blob::new(b.deps.clone(), b.data.clone());
        assert_eq!(wb.id, b.id);
        assert_eq!(wb.encode(), b.encode());
        assert_eq!(web_wire::Blob::decode(&b.encode()), Some(wb));

        let mut store = sync::Store::in_memory().unwrap();
        let mut mem = web_wire::MemStore::default();
        let mut prev = vec![];
        for i in 0..40 {
            let blob = sync::Blob::new(prev, format!("{i}").into_bytes());
            prev = vec![blob.id];
            store.append("l", &blob).unwrap();
            mem.append(web_wire::Blob::decode(&blob.encode()).unwrap());
        }
        assert_eq!(store.state_hash("l").unwrap(), mem.state_hash());
        assert_eq!(store.have_markers("l").unwrap(), mem.have_markers());
    }
}

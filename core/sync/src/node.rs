//! A libp2p node that keeps a set of logs in sync with its peers.
//!
//! - Discovery: dial the bootstrap/relay node, then ask its Kademlia table
//!   for other peers and dial them (directly, or through the relay circuit).
//! - Pull: on connect and every few seconds, send `have` markers for each
//!   open log; the peer answers with what we're missing, in pages.
//! - Push: every new blob is published on the log's gossipsub topic. A pushed
//!   blob whose deps we lack waits in memory and triggers a pull.
//!
//! Every input from a peer is bounded: blob size and dep count
//! ([`Blob::decode`]), ids recomputed from content, `have` list length,
//! response pages (count and bytes), inbound pull rate per peer, gossip
//! message size and topics, connections, and the pending-blob buffer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, ensure, Context, Result};
use futures::StreamExt;
use libp2p::{
    connection_limits,
    core::multiaddr::Protocol,
    gossipsub::{self, MessageAcceptance, MaxCountSubscriptionFilter, TopicHash, TopicSubscriptionFilter},
    identify, identity, kad, noise, ping, relay,
    request_response::{self, ProtocolSupport},
    swarm::{
        dial_opts::{DialOpts, PeerCondition},
        NetworkBehaviour, SwarmEvent,
    },
    tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder,
};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::limits::{Pending, RateLimiter};
use crate::store::MAX_HAVE;
use crate::{valid_log_name, Append, Blob, Id, Store, MAX_DATA_BYTES, MAX_DEPS, MAX_ENCODED_BYTES};

const SYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/descord-replacement/sync/0.1.0");
const IDENTIFY_VERSION: &str = "/descord-replacement/0.1.0";
/// Blobs per sync response page.
const PAGE: usize = 2000;
/// Blob bytes (data + deps) per page. The CBOR codec drops responses over
/// 10 MiB, so a page must stay well below that.
const PAGE_BYTES: usize = 4 * 1024 * 1024;
const ANTI_ENTROPY: Duration = Duration::from_secs(5);
const DISCOVERY: Duration = Duration::from_secs(3);
/// Pushed blobs waiting for their deps (the pull fetches them anyway).
const MAX_PENDING: usize = 10_000;
const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
const PENDING_TTL: Duration = Duration::from_secs(60);
/// Inbound pulls per peer: a burst, then a steady rate. A 100,000-message
/// catch-up is 50 pages, so this costs an honest peer about 2 s at most.
const PULL_BURST: u32 = 30;
const PULL_RATE: u32 = 10;
/// Largest gossipsub frame: one encoded blob plus signature, key and topic.
const MAX_GOSSIP_BYTES: usize = MAX_ENCODED_BYTES + 4 * 1024;
/// Topics one remote peer may subscribe to, in total and per message.
const MAX_PEER_TOPICS: usize = 1024;
const MAX_TOPICS_PER_MESSAGE: usize = 128;
const MAX_INCOMING_CONNECTIONS: u32 = 256;
const MAX_PENDING_INCOMING: u32 = 64;
/// A direct and a relayed connection to the same peer can briefly coexist.
const MAX_CONNECTIONS_PER_PEER: u32 = 2;

pub struct NodeConfig {
    /// Bootstrap/relay node, ending in /p2p/<peer id>.
    pub relay: Multiaddr,
    /// Holds `store.db` and `node.key`.
    pub data_dir: PathBuf,
    /// Logs to sync from the start; more can be opened later.
    pub logs: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Via {
    Local,
    Push,
    Pull,
}

#[derive(Debug)]
pub enum NodeEvent {
    Stored { log: String, blob: Blob, via: Via, at: SystemTime },
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
}

#[derive(Debug, Clone, Serialize)]
pub struct LogState {
    pub count: u64,
    pub hash: String,
    pub heads: usize,
}

enum Command {
    Append { log: String, data: Vec<u8>, reply: oneshot::Sender<Result<Id>> },
    Open { log: String },
    State { log: String, reply: oneshot::Sender<Result<LogState>> },
    Peers { reply: oneshot::Sender<Vec<PeerId>> },
    Shutdown { reply: oneshot::Sender<()> },
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncRequest {
    log: String,
    have: Vec<ByteBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncResponse {
    /// Encoded blobs, in an order they can be appended in.
    blobs: Vec<ByteBuf>,
    more: bool,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour<gossipsub::IdentityTransform, MaxCountSubscriptionFilter<LogTopics>>,
    sync: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
    limits: connection_limits::Behaviour,
}

/// Remote peers' subscriptions are only tracked for well-formed log topics.
struct LogTopics;

impl TopicSubscriptionFilter for LogTopics {
    fn can_subscribe(&mut self, topic: &TopicHash) -> bool {
        log_of_topic(topic).is_some()
    }
}

fn log_of_topic(topic: &TopicHash) -> Option<&str> {
    topic.as_str().strip_prefix("log/").filter(|log| valid_log_name(log))
}

/// Handle to a running node. Dropping it does not stop the node; call
/// [`Node::shutdown`].
pub struct Node {
    peer_id: PeerId,
    commands: mpsc::UnboundedSender<Command>,
}

impl Node {
    /// Opens the store, starts networking and returns the handle plus a
    /// stream of events (blobs stored, peers connected).
    pub async fn start(config: NodeConfig) -> Result<(Node, mpsc::UnboundedReceiver<NodeEvent>)> {
        for log in &config.logs {
            ensure!(valid_log_name(log), "invalid log name {log:?}");
        }
        std::fs::create_dir_all(&config.data_dir)?;
        let key = load_or_create_key(&config.data_dir.join("node.key"))?;
        let store = Store::open(config.data_dir.join("store.db"))?;
        let relay_peer = match config.relay.iter().last() {
            Some(Protocol::P2p(peer)) => peer,
            _ => bail!("the relay address must end with /p2p/<relay peer ID>"),
        };

        let mut swarm = build_swarm(key)?;
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        swarm.behaviour_mut().kad.add_address(&relay_peer, config.relay.clone());
        swarm.dial(config.relay.clone())?;

        let peer_id = *swarm.local_peer_id();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, event_rx) = mpsc::unbounded_channel();
        let mut task = Task {
            swarm,
            store,
            relay_addr: config.relay,
            relay_peer,
            logs: HashSet::new(),
            connected: HashSet::new(),
            inflight: HashMap::new(),
            again: HashSet::new(),
            pending: Pending::new(MAX_PENDING, MAX_PENDING_BYTES),
            inbound_pulls: RateLimiter::new(PULL_BURST, PULL_RATE),
            events,
            circuit_listening: false,
        };
        for log in config.logs {
            task.open(log);
        }
        tokio::spawn(task.run(command_rx));
        Ok((Node { peer_id, commands }, event_rx))
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Writes a new blob on top of the log's current heads.
    pub async fn append(&self, log: &str, data: Vec<u8>) -> Result<Id> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Append { log: log.into(), data, reply })?;
        rx.await?
    }

    /// Starts syncing a log (channel logs are only synced once opened).
    pub fn open(&self, log: &str) -> Result<()> {
        ensure!(valid_log_name(log), "invalid log name {log:?}");
        self.send(Command::Open { log: log.into() })
    }

    pub async fn state(&self, log: &str) -> Result<LogState> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::State { log: log.into(), reply })?;
        rx.await?
    }

    pub async fn peers(&self) -> Result<Vec<PeerId>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Peers { reply })?;
        Ok(rx.await?)
    }

    /// Closes all connections and the store.
    pub async fn shutdown(self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Shutdown { reply })?;
        Ok(rx.await?)
    }

    fn send(&self, command: Command) -> Result<()> {
        self.commands.send(command).ok().context("node has stopped")
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

fn build_swarm(key: identity::Keypair) -> Result<Swarm<Behaviour>> {
    Ok(SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)?
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| {
            let peer_id = key.public().to_peer_id();
            let mut kad = kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id));
            // Server mode so peers can find each other through any peer, not
            // only through the bootstrap node.
            kad.set_mode(Some(kad::Mode::Server));
            let gossip_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_millis(500))
                .max_transmit_size(MAX_GOSSIP_BYTES)
                // Forward a message only after we checked it (see `on_gossip`),
                // so invalid blobs don't travel further through us.
                .validate_messages()
                .build()
                .map_err(|e| anyhow::anyhow!(e))?;
            let gossipsub = gossipsub::Behaviour::new_with_subscription_filter(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossip_config,
                None,
                MaxCountSubscriptionFilter {
                    filter: LogTopics,
                    max_subscribed_topics: MAX_PEER_TOPICS,
                    max_subscriptions_per_request: MAX_TOPICS_PER_MESSAGE,
                },
            )
            .map_err(|e| anyhow::anyhow!(e))?;
            let limits = connection_limits::ConnectionLimits::default()
                .with_max_established_incoming(Some(MAX_INCOMING_CONNECTIONS))
                .with_max_pending_incoming(Some(MAX_PENDING_INCOMING))
                .with_max_established_per_peer(Some(MAX_CONNECTIONS_PER_PEER));
            Ok(Behaviour {
                relay_client,
                identify: identify::Behaviour::new(identify::Config::new(
                    IDENTIFY_VERSION.into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
                kad,
                gossipsub,
                sync: request_response::cbor::Behaviour::new(
                    [(SYNC_PROTOCOL, ProtocolSupport::Full)],
                    request_response::Config::default().with_request_timeout(Duration::from_secs(30)),
                ),
                limits: connection_limits::Behaviour::new(limits),
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build())
}

fn topic(log: &str) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("log/{log}"))
}

struct Task {
    swarm: Swarm<Behaviour>,
    store: Store,
    relay_addr: Multiaddr,
    relay_peer: PeerId,
    logs: HashSet<String>,
    connected: HashSet<PeerId>,
    /// Pulls waiting for a response.
    inflight: HashMap<request_response::OutboundRequestId, (PeerId, String)>,
    /// Pulls asked for while one was in flight: run again after it.
    again: HashSet<(PeerId, String)>,
    /// Pushed blobs waiting for a dep.
    pending: Pending,
    inbound_pulls: RateLimiter<PeerId>,
    events: mpsc::UnboundedSender<NodeEvent>,
    circuit_listening: bool,
}

impl Task {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut anti_entropy = tokio::time::interval(ANTI_ENTROPY);
        // The first discovery runs as soon as the relay connection is up;
        // after that, keep looking for peers that joined later.
        let mut discovery = tokio::time::interval(DISCOVERY);
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Shutdown { reply }) => {
                        drop(self.swarm);
                        drop(self.store);
                        let _ = reply.send(());
                        return;
                    }
                    Some(command) => self.on_command(command),
                    None => return,
                },
                event = self.swarm.select_next_some() => self.on_swarm_event(event),
                _ = anti_entropy.tick() => {
                    self.pending.expire(Instant::now(), PENDING_TTL);
                    self.pull_all();
                }
                _ = discovery.tick() => self.discover(),
            }
        }
    }

    fn on_command(&mut self, command: Command) {
        match command {
            Command::Append { log, data, reply } => {
                let _ = reply.send(self.append_local(&log, data));
            }
            Command::Open { log } => self.open(log),
            Command::State { log, reply } => {
                let state = (|| {
                    Ok(LogState {
                        count: self.store.count(&log)?,
                        hash: self.store.state_hash(&log)?,
                        heads: self.store.heads(&log)?.len(),
                    })
                })();
                let _ = reply.send(state);
            }
            Command::Peers { reply } => {
                let _ = reply.send(self.connected.iter().copied().collect());
            }
            Command::Shutdown { .. } => unreachable!(),
        }
    }

    fn open(&mut self, log: String) {
        if !self.logs.insert(log.clone()) {
            return;
        }
        if let Err(err) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic(&log)) {
            warn!(%err, "subscribe failed");
        }
        for peer in self.connected.clone() {
            self.pull(peer, &log);
        }
    }

    fn append_local(&mut self, log: &str, data: Vec<u8>) -> Result<Id> {
        if !self.logs.contains(log) {
            bail!("log {log} is not open");
        }
        ensure!(data.len() <= MAX_DATA_BYTES, "message is {} bytes, limit is {MAX_DATA_BYTES}", data.len());
        // With more heads than a blob may point at, merge some now; the rest
        // stay heads and get merged by the next writes.
        let mut deps = self.store.heads(log)?;
        deps.truncate(MAX_DEPS);
        let blob = Blob::new(log, deps, data);
        match self.store.append(log, &blob)? {
            Append::Added => {}
            other => bail!("storing own blob failed: {other:?}"),
        }
        // No subscribed peers yet is fine: they'll pull it.
        let _ = self.swarm.behaviour_mut().gossipsub.publish(topic(log), blob.encode());
        let id = blob.id;
        self.emit_stored(log, blob, Via::Local);
        Ok(id)
    }

    fn emit_stored(&self, log: &str, blob: Blob, via: Via) {
        let _ = self.events.send(NodeEvent::Stored { log: log.into(), blob, via, at: SystemTime::now() });
    }

    fn discover(&mut self) {
        if self.swarm.is_connected(&self.relay_peer) {
            let me = *self.swarm.local_peer_id();
            self.swarm.behaviour_mut().kad.get_closest_peers(me);
        } else {
            let _ = self.swarm.dial(self.relay_addr.clone());
        }
    }

    fn pull_all(&mut self) {
        for peer in self.connected.clone() {
            for log in self.logs.clone() {
                self.pull(peer, &log);
            }
        }
    }

    fn pull(&mut self, peer: PeerId, log: &str) {
        let key = (peer, log.to_string());
        if self.inflight.values().any(|k| *k == key) {
            self.again.insert(key);
            return;
        }
        let have = match self.store.have_markers(log) {
            Ok(have) => have.into_iter().map(|id| ByteBuf::from(id.to_vec())).collect(),
            Err(err) => return warn!(%err, "reading markers failed"),
        };
        let request = self.swarm.behaviour_mut().sync.send_request(&peer, SyncRequest { log: log.into(), have });
        self.inflight.insert(request, key);
    }

    fn on_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                if peer_id == self.relay_peer {
                    let me = *self.swarm.local_peer_id();
                    self.swarm.behaviour_mut().kad.get_closest_peers(me);
                } else if self.connected.insert(peer_id) {
                    info!(%peer_id, address = %endpoint.get_remote_address(), "peer connected");
                    let _ = self.events.send(NodeEvent::PeerConnected(peer_id));
                    for log in self.logs.clone() {
                        self.pull(peer_id, &log);
                    }
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                if self.connected.remove(&peer_id) {
                    info!(%peer_id, "peer disconnected");
                    let _ = self.events.send(NodeEvent::PeerDisconnected(peer_id));
                }
                self.inflight.retain(|_, (p, _)| *p != peer_id);
                self.again.retain(|(p, _)| *p != peer_id);
                self.inbound_pulls.forget(&peer_id);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                if peer_id == self.relay_peer {
                    // Relay circuit as a fallback for peers that can't reach us
                    // directly (NAT). On a LAN the direct address wins.
                    if !self.circuit_listening {
                        self.circuit_listening = true;
                        let circuit = self.relay_addr.clone().with(Protocol::P2pCircuit);
                        if let Err(err) = self.swarm.listen_on(circuit) {
                            warn!(%err, "listening on relay circuit failed");
                        }
                    }
                } else {
                    for addr in info.listen_addrs {
                        self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Kad(kad::Event::OutboundQueryProgressed {
                result: kad::QueryResult::GetClosestPeers(Ok(ok)),
                ..
            })) => {
                for peer in ok.peers {
                    if peer.peer_id == *self.swarm.local_peer_id() || self.connected.contains(&peer.peer_id) {
                        continue;
                    }
                    debug!(peer = %peer.peer_id, "discovered");
                    let opts = DialOpts::peer_id(peer.peer_id)
                        .addresses(peer.addrs)
                        .condition(PeerCondition::DisconnectedAndNotDialing)
                        .build();
                    let _ = self.swarm.dial(opts);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            })) => {
                let acceptance = self.on_gossip(propagation_source, &message);
                let _ = self.swarm.behaviour_mut().gossipsub.report_message_validation_result(
                    &message_id,
                    &propagation_source,
                    acceptance,
                );
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message { peer, message })) => {
                match message {
                    request_response::Message::Request { request, channel, .. } => {
                        if !self.inbound_pulls.allow(peer, Instant::now()) {
                            // Dropping the channel fails the request on their side.
                            debug!(%peer, "pull rate limit exceeded");
                            return;
                        }
                        let started = Instant::now();
                        let response = answer(&self.store, &self.logs, request);
                        debug!(n = response.blobs.len(), ms = started.elapsed().as_millis(), "answered pull");
                        let _ = self.swarm.behaviour_mut().sync.send_response(channel, response);
                    }
                    request_response::Message::Response { request_id, response } => {
                        if let Some(key) = self.inflight.remove(&request_id) {
                            self.on_response(key, response);
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::OutboundFailure {
                peer, request_id, error,
            })) => {
                debug!(%peer, %error, "pull failed");
                // The anti-entropy timer retries.
                if let Some(key) = self.inflight.remove(&request_id) {
                    self.again.remove(&key);
                }
            }
            _ => {}
        }
    }

    /// Checks a pushed message: Accept forwards it to our mesh peers, Reject
    /// stops it here (and counts against the sender once peer scoring is on).
    fn on_gossip(&mut self, from: PeerId, message: &gossipsub::Message) -> MessageAcceptance {
        let Some(log) = log_of_topic(&message.topic) else { return MessageAcceptance::Reject };
        if !self.logs.contains(log) {
            return MessageAcceptance::Ignore;
        }
        let log = log.to_string();
        match Blob::decode(&log, &message.data) {
            Ok(blob) => {
                self.on_pushed(from, log, blob);
                MessageAcceptance::Accept
            }
            Err(err) => {
                warn!(%from, %err, "rejected pushed blob");
                MessageAcceptance::Reject
            }
        }
    }

    fn on_response(&mut self, key: (PeerId, String), response: SyncResponse) {
        let (peer, log) = key.clone();
        let mut blobs = Vec::new();
        for bytes in &response.blobs {
            match Blob::decode(&log, bytes) {
                Ok(blob) => blobs.push(blob),
                Err(err) => debug!(%peer, %err, "dropped pulled blob"),
            }
        }
        let mut added_any = false;
        if !blobs.is_empty() {
            match self.store.append_all(&log, &blobs) {
                Ok((added, _skipped)) => {
                    added_any = !added.is_empty();
                    let added: HashSet<Id> = added.into_iter().collect();
                    let now = SystemTime::now();
                    for blob in blobs.iter().filter(|b| added.contains(&b.id)) {
                        let _ = self.events.send(NodeEvent::Stored {
                            log: log.clone(),
                            blob: blob.clone(),
                            via: Via::Pull,
                            at: now,
                        });
                    }
                    for id in added {
                        self.release_pending(id);
                    }
                }
                Err(err) => warn!(%err, "storing pulled blobs failed"),
            }
        }
        if follow_up(response.more, added_any) || self.again.remove(&key) {
            self.pull(peer, &log);
        }
    }

    fn on_pushed(&mut self, from: PeerId, log: String, blob: Blob) {
        match self.store.append(&log, &blob) {
            Ok(Append::Added) => {
                let id = blob.id;
                self.emit_stored(&log, blob, Via::Push);
                self.release_pending(id);
            }
            Ok(Append::Known) => {}
            Ok(Append::MissingDeps(missing)) => {
                self.pending.insert(missing[0], log.clone(), blob, Instant::now());
                self.pull(from, &log);
            }
            // Decoding already checked it; this is the store's own guard.
            Ok(Append::Invalid(err)) => warn!(%from, %err, "rejected pushed blob"),
            Err(err) => warn!(%err, "storing pushed blob failed"),
        }
    }

    /// Retries pushed blobs that were waiting for `id`.
    fn release_pending(&mut self, id: Id) {
        let mut ready = vec![id];
        while let Some(id) = ready.pop() {
            for waiting in self.pending.take(&id) {
                match self.store.append(&waiting.log, &waiting.blob) {
                    Ok(Append::Added) => {
                        ready.push(waiting.blob.id);
                        self.emit_stored(&waiting.log, waiting.blob, Via::Push);
                    }
                    Ok(Append::MissingDeps(missing)) => {
                        // Subject to the same caps as a new arrival.
                        self.pending.reinsert(missing[0], waiting);
                    }
                    Ok(Append::Known | Append::Invalid(_)) => {}
                    Err(err) => warn!(%err, "storing pending blob failed"),
                }
            }
        }
    }
}

/// Answers a pull: what the requester is missing from `request.log`, one
/// page. Unknown or unopened logs get an empty answer; at most [`MAX_HAVE`]
/// `have` ids are looked at, and malformed ones are skipped.
fn answer(store: &Store, logs: &HashSet<String>, request: SyncRequest) -> SyncResponse {
    let empty = SyncResponse { blobs: Vec::new(), more: false };
    if !logs.contains(&request.log) {
        return empty;
    }
    let have: Vec<Id> =
        request.have.iter().take(MAX_HAVE).filter_map(|b| b.as_slice().try_into().ok()).collect();
    match store.missing_since(&request.log, &have, PAGE, PAGE_BYTES) {
        Ok((blobs, more)) => {
            SyncResponse { blobs: blobs.iter().map(|b| ByteBuf::from(b.encode())).collect(), more }
        }
        Err(err) => {
            warn!(%err, "missing_since failed");
            empty
        }
    }
}

/// Whether to ask for the next page right away. A peer that keeps saying
/// "more" without sending anything new is not followed; the anti-entropy
/// timer tries again later.
fn follow_up(more: bool, added_any: bool) -> bool {
    more && added_any
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(log: &str, have: Vec<ByteBuf>) -> SyncRequest {
        SyncRequest { log: log.into(), have }
    }

    fn store_with(log: &str, n: usize, size: usize) -> (Store, Vec<Id>) {
        let mut store = Store::in_memory().unwrap();
        let mut deps = vec![];
        let mut ids = vec![];
        for i in 0..n {
            let blob = Blob::new(log, deps, [(i as u32).to_le_bytes().to_vec(), vec![0; size]].concat());
            store.append(log, &blob).unwrap();
            deps = vec![blob.id];
            ids.push(blob.id);
        }
        (store, ids)
    }

    #[test]
    fn answers_only_open_logs() {
        let (store, _) = store_with("open", 3, 0);
        let logs = HashSet::from(["open".to_string()]);
        assert_eq!(answer(&store, &logs, request("open", vec![])).blobs.len(), 3);
        assert!(answer(&store, &logs, request("closed", vec![])).blobs.is_empty());
        assert!(answer(&store, &logs, request("x'; DROP TABLE blobs; --", vec![])).blobs.is_empty());
        assert_eq!(store.count("open").unwrap(), 3);
    }

    #[test]
    fn malformed_and_excess_have_ids_are_ignored() {
        let (store, ids) = store_with("l", 5, 0);
        let logs = HashSet::from(["l".to_string()]);
        let have = vec![ByteBuf::from(vec![1, 2, 3]), ByteBuf::from(ids[4].to_vec())];
        assert!(answer(&store, &logs, request("l", have)).blobs.is_empty());
        // 100,000 have ids cost the same as MAX_HAVE.
        let flood = vec![ByteBuf::from(vec![0xee; 32]); 100_000];
        assert_eq!(answer(&store, &logs, request("l", flood)).blobs.len(), 5);
    }

    #[test]
    fn response_pages_fit_the_codec_limit() {
        // 400 blobs of 30 KB is 12 MB, more than the 10 MiB response cap.
        let (store, _) = store_with("l", 400, 30_000);
        let logs = HashSet::from(["l".to_string()]);
        let response = answer(&store, &logs, request("l", vec![]));
        assert!(response.more);
        let size: usize = response.blobs.iter().map(|b| b.len()).sum();
        assert!(size <= PAGE_BYTES + MAX_ENCODED_BYTES, "page is {size} bytes");
        // Every blob in it decodes back to a valid blob of the log.
        for bytes in &response.blobs {
            Blob::decode("l", bytes).unwrap();
        }
    }

    #[test]
    fn empty_more_responses_are_not_followed() {
        assert!(follow_up(true, true));
        assert!(!follow_up(true, false));
        assert!(!follow_up(false, true));
    }

    #[test]
    fn topic_filter_accepts_only_log_topics() {
        let mut filter = LogTopics;
        assert!(filter.can_subscribe(&TopicHash::from_raw("log/general")));
        assert!(!filter.can_subscribe(&TopicHash::from_raw("log/")));
        assert!(!filter.can_subscribe(&TopicHash::from_raw("something-else")));
        assert!(!filter.can_subscribe(&TopicHash::from_raw(format!("log/{}", "x".repeat(10_000)))));
    }
}

//! A libp2p node that keeps a set of logs in sync with its peers.
//!
//! - Discovery: dial the bootstrap/relay node, then ask its Kademlia table
//!   for other peers and dial them (directly, or through the relay circuit).
//! - Pull: on connect and every few seconds, send `have` markers for each
//!   open log; the peer answers with what we're missing, in pages. Answers
//!   are computed on a blocking thread with a read-only store connection, so
//!   a large log never stalls the network task.
//! - Push: every new record is published on the log's gossipsub topic. A
//!   pushed record whose deps we lack waits in memory and triggers a pull.
//!
//! Every record from a peer goes through the ingest checks in the crate docs
//! (decode, derived log, deps in place, [`Authorizer`]) before it is stored,
//! forwarded on gossip or served to anyone. Every input is bounded: record
//! size, `have` list length, response pages (count and bytes), inbound pull
//! rate per peer and in total, concurrent answers, gossip message size and
//! topics, connections, and the pending-record buffer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, ensure, Context, Result};
use futures::StreamExt;
use libp2p::{
    connection_limits,
    core::multiaddr::Protocol,
    gossipsub::{self, MaxCountSubscriptionFilter, MessageAcceptance, TopicHash, TopicSubscriptionFilter},
    identify, identity, kad, noise, ping, relay,
    request_response::{self, ProtocolSupport, ResponseChannel},
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
use crate::{log_name, log_of, valid_log_name, Append, Authorizer, Record, RecordId, Scope, Store};

const SYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/mvs/sync/0.2.0");
const IDENTIFY_VERSION: &str = "/mvs/0.1.0";
/// Records per sync response page.
const PAGE: usize = 2000;
/// Record bytes per page. The CBOR codec drops responses over 10 MiB, so a
/// page must stay well below that.
const PAGE_BYTES: usize = 4 * 1024 * 1024;
const ANTI_ENTROPY: Duration = Duration::from_secs(5);
const DISCOVERY: Duration = Duration::from_secs(3);
/// Pushed records waiting for their deps (the pull fetches them anyway).
const MAX_PENDING: usize = 10_000;
const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
const PENDING_TTL: Duration = Duration::from_secs(60);
/// Inbound pulls per peer: a burst, then a steady rate. A 100,000-message
/// catch-up is 50 pages, so this costs an honest peer about 2 s at most.
const PULL_BURST: u32 = 30;
const PULL_RATE: u32 = 10;
/// Inbound pulls from all peers together. PeerIds are free, so the per-peer
/// limit alone doesn't bound the work: each pull walks the log's ancestry.
const GLOBAL_PULL_BURST: u32 = 100;
const GLOBAL_PULL_RATE: u32 = 50;
/// Pulls being answered on blocking threads at once; more are dropped.
const MAX_ANSWERING: usize = 8;
/// Per-peer pull buckets remembered (also after disconnecting).
const MAX_TRACKED_PEERS: usize = 4096;
/// Node events waiting for the app; beyond this, events are dropped and a
/// [`NodeEvent::Lagged`] follows.
const EVENT_BUFFER: usize = 65_536;
/// Largest gossipsub frame: one record plus signature, key and topic.
const MAX_GOSSIP_BYTES: usize = records::MAX_RECORD_BYTES + 4 * 1024;
/// Topics one remote peer may subscribe to, in total and per message.
const MAX_PEER_TOPICS: usize = 1024;
const MAX_TOPICS_PER_MESSAGE: usize = 128;
const MAX_INCOMING_CONNECTIONS: u32 = 256;
/// Peers hand us addresses to dial (Kademlia results, identify), so our own
/// dialling is capped as well.
const MAX_OUTGOING_CONNECTIONS: u32 = 128;
const MAX_CONNECTIONS: u32 = 384;
const MAX_PENDING_INCOMING: u32 = 64;
/// A direct and a relayed connection to the same peer can briefly coexist.
const MAX_CONNECTIONS_PER_PEER: u32 = 2;

pub struct NodeConfig {
    /// Bootstrap/relay node, ending in /p2p/<peer id>.
    pub relay: Multiaddr,
    /// Holds `store.db` and `node.key`.
    pub data_dir: PathBuf,
    /// Logs to sync from the start (see [`crate::log_name`]); more can be
    /// opened later.
    pub logs: Vec<String>,
    /// Decides whether a record's author may write it. Called on the node's
    /// task inside a store transaction: it must not call back into the node.
    pub authorizer: Arc<dyn Authorizer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Via {
    Local,
    Push,
    Pull,
}

#[derive(Debug)]
pub enum NodeEvent {
    Stored { log: String, record: Record, via: Via, at: SystemTime },
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    /// The app fell behind and `dropped` events were lost. The records are
    /// all in the store: re-read the open logs from there.
    Lagged { dropped: u64 },
}

#[derive(Debug, Clone, Serialize)]
pub struct LogState {
    pub count: u64,
    pub hash: String,
    pub heads: usize,
}

enum Command {
    Append { record: Record, reply: oneshot::Sender<Result<()>> },
    Heads { log: String, reply: oneshot::Sender<Result<Vec<RecordId>>> },
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
    /// Encoded records, in an order they can be appended in.
    records: Vec<ByteBuf>,
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
    /// stream of events (records stored, peers connected).
    pub async fn start(config: NodeConfig) -> Result<(Node, mpsc::Receiver<NodeEvent>)> {
        for log in &config.logs {
            ensure!(valid_log_name(log), "invalid log name {log:?}");
        }
        std::fs::create_dir_all(&config.data_dir)?;
        let key = load_or_create_key(&config.data_dir.join("node.key"))?;
        let db = config.data_dir.join("store.db");
        let store = Store::open(&db)?;
        let reader = Arc::new(Mutex::new(Store::open_read_only(&db)?));
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
        let (events, event_rx) = mpsc::channel(EVENT_BUFFER);
        let (answers, answer_rx) = mpsc::unbounded_channel();
        let (decoded, decoded_rx) = mpsc::unbounded_channel();
        let mut task = Task {
            swarm,
            store,
            reader,
            authorizer: config.authorizer,
            relay_addr: config.relay,
            relay_peer,
            logs: HashSet::new(),
            connected: HashSet::new(),
            inflight: HashMap::new(),
            decoding: HashSet::new(),
            again: HashSet::new(),
            pending: Pending::new(MAX_PENDING, MAX_PENDING_BYTES),
            inbound_pulls: RateLimiter::new(PULL_BURST, PULL_RATE, MAX_TRACKED_PEERS),
            all_inbound_pulls: RateLimiter::new(GLOBAL_PULL_BURST, GLOBAL_PULL_RATE, 1),
            answers,
            answering: 0,
            decoded,
            events: EventSink { tx: events, dropped: 0 },
            circuit_listening: false,
        };
        for log in config.logs {
            task.open(log);
        }
        tokio::spawn(task.run(command_rx, answer_rx, decoded_rx));
        Ok((Node { peer_id, commands }, event_rx))
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Stores and publishes a record written here. The caller signs it,
    /// usually on top of [`Node::heads`] of its log; it goes through the same
    /// checks as a record from a peer.
    pub async fn append(&self, record: Record) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Append { record, reply })?;
        rx.await?
    }

    /// The current heads of a log: the deps for the next record written to it.
    pub async fn heads(&self, log: &str) -> Result<Vec<RecordId>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Heads { log: log.into(), reply })?;
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
            // only through the bootstrap node. (GAP-6 moves this to client mode.)
            kad.set_mode(Some(kad::Mode::Server));
            let gossip_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_millis(500))
                .max_transmit_size(MAX_GOSSIP_BYTES)
                // Forward a message only after it passed the ingest checks
                // (see `on_gossip`), so junk doesn't travel further through us.
                .validate_messages()
                // Dedup by content, not by (source, seqno): the same record
                // republished by another peer or with a new seqno is one message.
                .message_id_fn(|m| {
                    let mut hasher = blake3::Hasher::new();
                    hasher.update(m.topic.as_str().as_bytes());
                    hasher.update(&[0]);
                    hasher.update(&m.data);
                    gossipsub::MessageId::from(hasher.finalize().as_bytes().to_vec())
                })
                // TODO: enable peer scoring so `MessageAcceptance::Reject`
                // costs the sender something; today it only stops forwarding.
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
                .with_max_established_per_peer(Some(MAX_CONNECTIONS_PER_PEER))
                .with_max_established_outgoing(Some(MAX_OUTGOING_CONNECTIONS))
                .with_max_established(Some(MAX_CONNECTIONS));
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

type Answer = (ResponseChannel<SyncResponse>, SyncResponse);
/// A pulled page after decoding: (peer, log), the records that verified
/// and belong to the log, and whether the peer said there is more.
type Decoded = ((PeerId, String), Vec<Record>, bool);

struct Task {
    swarm: Swarm<Behaviour>,
    store: Store,
    /// Read-only connection used by blocking threads to answer pulls.
    reader: Arc<Mutex<Store>>,
    authorizer: Arc<dyn Authorizer>,
    relay_addr: Multiaddr,
    relay_peer: PeerId,
    logs: HashSet<String>,
    connected: HashSet<PeerId>,
    /// Pulls waiting for a response.
    inflight: HashMap<request_response::OutboundRequestId, (PeerId, String)>,
    /// Pulled pages being decoded; a pull for these waits like one in flight.
    decoding: HashSet<(PeerId, String)>,
    /// Pulls asked for while one was in flight: run again after it.
    again: HashSet<(PeerId, String)>,
    /// Pushed records waiting for a dep.
    pending: Pending,
    inbound_pulls: RateLimiter<PeerId>,
    all_inbound_pulls: RateLimiter<()>,
    /// Finished answers coming back from blocking threads.
    answers: mpsc::UnboundedSender<Answer>,
    answering: usize,
    /// Pulled pages coming back from decoding on blocking threads.
    decoded: mpsc::UnboundedSender<Decoded>,
    events: EventSink,
    circuit_listening: bool,
}

/// What happened to one incoming record.
#[derive(Debug, PartialEq, Eq)]
enum Ingest {
    Added,
    Known,
    /// Parked until its deps arrive; not judged yet.
    Waiting,
    /// Malformed, misfiled, cross-log, or refused by the authorizer.
    Rejected,
}

impl Task {
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut answers: mpsc::UnboundedReceiver<Answer>,
        mut decoded: mpsc::UnboundedReceiver<Decoded>,
    ) {
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
                Some((channel, response)) = answers.recv() => {
                    self.answering = self.answering.saturating_sub(1);
                    let _ = self.swarm.behaviour_mut().sync.send_response(channel, response);
                }
                Some((key, records, more)) = decoded.recv() => self.on_decoded(key, records, more),
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
            Command::Append { record, reply } => {
                let _ = reply.send(self.append_local(record));
            }
            Command::Heads { log, reply } => {
                let _ = reply.send(self.store.heads(&log).map_err(Into::into));
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

    fn append_local(&mut self, record: Record) -> Result<()> {
        let log = log_of(&record);
        if !self.logs.contains(&log) {
            bail!("log {log} is not open");
        }
        match self.store.append(&record, &*self.authorizer)? {
            Append::Added => {}
            other => bail!("storing own record failed: {other:?}"),
        }
        // No subscribed peers yet is fine: they'll pull it.
        let _ = self.swarm.behaviour_mut().gossipsub.publish(topic(&log), record.encode());
        let id = record.id();
        self.emit(NodeEvent::Stored { log, record, via: Via::Local, at: SystemTime::now() });
        self.release_pending(id);
        Ok(())
    }

    fn emit(&mut self, event: NodeEvent) {
        self.events.emit(event);
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
        if self.decoding.contains(&key) || self.inflight.values().any(|k| *k == key) {
            self.again.insert(key);
            return;
        }
        let have = match self.store.have_markers(log) {
            Ok(have) => have.into_iter().map(|id| ByteBuf::from(id.as_bytes().to_vec())).collect(),
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
                    // Peer ID with its address only at debug level (PR checklist).
                    debug!(%peer_id, address = %endpoint.get_remote_address(), "peer connected");
                    info!(%peer_id, "peer connected");
                    self.emit(NodeEvent::PeerConnected(peer_id));
                    for log in self.logs.clone() {
                        self.pull(peer_id, &log);
                    }
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                if self.connected.remove(&peer_id) {
                    info!(%peer_id, "peer disconnected");
                    self.emit(NodeEvent::PeerDisconnected(peer_id));
                }
                self.inflight.retain(|_, (p, _)| *p != peer_id);
                self.decoding.retain(|(p, _)| *p != peer_id);
                self.again.retain(|(p, _)| *p != peer_id);
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
                    request_response::Message::Request { request, channel, .. } => self.on_request(peer, request, channel),
                    request_response::Message::Response { request_id, response } => {
                        if let Some(key) = self.inflight.remove(&request_id) {
                            self.on_response(key, response);
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
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

    /// Rate-limits a pull, then answers it on a blocking thread with the
    /// read-only connection. Dropping the channel fails the request on the
    /// other side.
    fn on_request(&mut self, peer: PeerId, request: SyncRequest, channel: ResponseChannel<SyncResponse>) {
        let now = Instant::now();
        if !self.inbound_pulls.allow(peer, now) || !self.all_inbound_pulls.allow((), now) {
            debug!(%peer, "pull rate limit exceeded");
            return;
        }
        if !self.logs.contains(&request.log) {
            let _ = self.swarm.behaviour_mut().sync.send_response(channel, empty_response());
            return;
        }
        if self.answering >= MAX_ANSWERING {
            debug!(%peer, "too many pulls being answered");
            return;
        }
        self.answering += 1;
        let reader = self.reader.clone();
        let answers = self.answers.clone();
        tokio::task::spawn_blocking(move || {
            let response = match reader.lock() {
                Ok(store) => answer(&store, &request),
                Err(_) => empty_response(),
            };
            let _ = answers.send((channel, response));
        });
    }

    /// Checks a pushed message. Accept forwards it to our mesh peers; Reject
    /// stops it here (and counts against the sender once peer scoring is on);
    /// Ignore neither forwards nor blames (a duplicate, or not judgeable yet).
    fn on_gossip(&mut self, from: PeerId, message: &gossipsub::Message) -> MessageAcceptance {
        let record = match check_pushed(&self.logs, message) {
            Ok(record) => record,
            Err(Refuse::Ignore) => return MessageAcceptance::Ignore,
            Err(Refuse::Reject) => {
                warn!(%from, "rejected pushed record");
                return MessageAcceptance::Reject;
            }
        };
        match self.ingest(from, record, Via::Push) {
            Ingest::Added => MessageAcceptance::Accept,
            Ingest::Known | Ingest::Waiting => MessageAcceptance::Ignore,
            Ingest::Rejected => MessageAcceptance::Reject,
        }
    }

    /// Stores one record from a peer if it passes the checks, or parks it
    /// until its deps arrive.
    fn ingest(&mut self, from: PeerId, record: Record, via: Via) -> Ingest {
        match self.store.append(&record, &*self.authorizer) {
            Ok(Append::Added) => {
                let id = record.id();
                self.emit(NodeEvent::Stored { log: log_of(&record), record, via, at: SystemTime::now() });
                self.release_pending(id);
                Ingest::Added
            }
            Ok(Append::Known) => Ingest::Known,
            Ok(Append::MissingDeps(missing)) => {
                let log = log_of(&record);
                let control = log_name(record.server(), Scope::Control);
                let needs_control = record.auth_deps().iter().any(|d| missing.contains(d));
                self.pending.insert(missing[0], record, Instant::now());
                self.pull(from, &log);
                if needs_control && self.logs.contains(&control) {
                    self.pull(from, &control);
                }
                Ingest::Waiting
            }
            Ok(Append::Invalid(err)) => {
                warn!(%from, %err, "rejected record");
                Ingest::Rejected
            }
            Ok(Append::Unauthorized) => {
                debug!(%from, "record refused by the authorizer");
                Ingest::Rejected
            }
            Err(err) => {
                warn!(%err, "storing record failed");
                Ingest::Known
            }
        }
    }

    /// Decodes a pulled page (up to 2,000 signature checks) on a blocking
    /// thread, so the network task keeps serving gossip meanwhile.
    fn on_response(&mut self, key: (PeerId, String), response: SyncResponse) {
        self.decoding.insert(key.clone());
        let decoded = self.decoded.clone();
        tokio::task::spawn_blocking(move || {
            let records = decode_page(&key.1, &response.records);
            let _ = decoded.send((key, records, response.more));
        });
    }

    fn on_decoded(&mut self, key: (PeerId, String), records: Vec<Record>, more: bool) {
        self.decoding.remove(&key);
        let (peer, log) = key.clone();
        let mut added_any = false;
        if !records.is_empty() {
            match self.store.append_all(&records, &*self.authorizer) {
                Ok(added) => {
                    added_any = !added.is_empty();
                    let added: HashSet<RecordId> = added.into_iter().collect();
                    let now = SystemTime::now();
                    for record in records.into_iter().filter(|r| added.contains(&r.id())) {
                        self.emit(NodeEvent::Stored { log: log.clone(), record, via: Via::Pull, at: now });
                    }
                    for id in added {
                        self.release_pending(id);
                    }
                }
                Err(err) => warn!(%err, "storing pulled records failed"),
            }
        }
        if follow_up(more, added_any) || self.again.remove(&key) {
            self.pull(peer, &log);
        }
    }

    /// Retries pushed records that were waiting for `id`.
    fn release_pending(&mut self, id: RecordId) {
        let mut ready = vec![id];
        while let Some(id) = ready.pop() {
            for waiting in self.pending.take(&id) {
                match self.store.append(&waiting.record, &*self.authorizer) {
                    Ok(Append::Added) => {
                        ready.push(waiting.record.id());
                        let log = log_of(&waiting.record);
                        self.emit(NodeEvent::Stored { log, record: waiting.record, via: Via::Push, at: SystemTime::now() });
                    }
                    Ok(Append::MissingDeps(missing)) => {
                        // Subject to the same caps as a new arrival.
                        self.pending.reinsert(missing[0], waiting);
                    }
                    Ok(Append::Known | Append::Invalid(_) | Append::Unauthorized) => {}
                    Err(err) => warn!(%err, "storing pending record failed"),
                }
            }
        }
    }
}

/// The node's side of the event channel. If the app stops reading, events
/// are dropped rather than piling up, and a [`NodeEvent::Lagged`] with the
/// count goes out as soon as there is room again (before anything newer).
struct EventSink {
    tx: mpsc::Sender<NodeEvent>,
    /// Events dropped since the app last kept up.
    dropped: u64,
}

impl EventSink {
    fn emit(&mut self, event: NodeEvent) {
        if self.dropped > 0 {
            match self.tx.try_send(NodeEvent::Lagged { dropped: self.dropped }) {
                Ok(()) => self.dropped = 0,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    self.dropped += 1;
                    return;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(event) {
            self.dropped += 1;
            debug!("event buffer full, dropping an event");
        }
    }
}

/// Why a pushed message isn't ingested: Ignore (not ours to judge) or Reject
/// (invalid; not forwarded and, once scoring is on, held against the sender).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refuse {
    Ignore,
    Reject,
}

/// The record in a pushed message, if it may be ingested: the topic is a
/// log we have open (else Ignore), the bytes decode and verify, and the
/// record signed the topic's log (else Reject).
fn check_pushed(open: &HashSet<String>, message: &gossipsub::Message) -> Result<Record, Refuse> {
    let topic_log = log_of_topic(&message.topic).ok_or(Refuse::Reject)?;
    if !open.contains(topic_log) {
        return Err(Refuse::Ignore);
    }
    let record = Record::decode(&message.data).map_err(|_| Refuse::Reject)?;
    // Filed under the log it signed, never the topic it came on.
    if log_of(&record) != topic_log {
        return Err(Refuse::Reject);
    }
    Ok(record)
}

/// Decodes (and so verifies) a pulled page for `log`. A page is for one log:
/// records that don't decode, or that signed another log, are dropped.
fn decode_page(log: &str, page: &[ByteBuf]) -> Vec<Record> {
    let records: Vec<Record> =
        page.iter().filter_map(|bytes| Record::decode(bytes).ok()).filter(|r| log_of(r) == log).collect();
    if records.len() != page.len() {
        debug!(dropped = page.len() - records.len(), "dropped invalid or misfiled pulled records");
    }
    records
}

fn empty_response() -> SyncResponse {
    SyncResponse { records: Vec::new(), more: false }
}

/// Answers a pull: what the requester is missing from `request.log`, one
/// page, straight from the stored bytes (they passed the ingest checks when
/// they were stored). At most [`MAX_HAVE`] `have` ids are looked at, and
/// malformed ones are skipped. The caller checks the log is open.
fn answer(store: &Store, request: &SyncRequest) -> SyncResponse {
    let have: Vec<RecordId> = request
        .have
        .iter()
        .take(MAX_HAVE)
        .filter_map(|b| <[u8; 32]>::try_from(b.as_slice()).ok().map(RecordId::from_bytes))
        .collect();
    match store.missing_since(&request.log, &have, PAGE, PAGE_BYTES) {
        Ok((page, more)) => SyncResponse { records: page.into_iter().map(|s| ByteBuf::from(s.bytes)).collect(), more },
        Err(err) => {
            warn!(%err, "missing_since failed");
            empty_response()
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
    use crate::testutil::{channel, message_chain, server};
    use crate::AcceptAll;

    fn chan() -> String {
        log_name(server(), Scope::Channel(channel()))
    }

    fn request(log: &str, have: Vec<ByteBuf>) -> SyncRequest {
        SyncRequest { log: log.into(), have }
    }

    fn store_with(n: usize, size: usize) -> (Store, Vec<Record>) {
        let mut store = Store::in_memory().unwrap();
        let chain = message_chain(n, size);
        store.append_all(&chain, &AcceptAll).unwrap();
        (store, chain)
    }

    #[test]
    fn malformed_and_excess_have_ids_are_ignored() {
        let (store, chain) = store_with(5, 0);
        let have = vec![ByteBuf::from(vec![1, 2, 3]), ByteBuf::from(chain[4].id().as_bytes().to_vec())];
        assert!(answer(&store, &request(&chan(), have)).records.is_empty());
        // 100,000 have ids cost the same as MAX_HAVE.
        let flood = vec![ByteBuf::from(vec![0xee; 32]); 100_000];
        assert_eq!(answer(&store, &request(&chan(), flood)).records.len(), 5);
        // An unknown log name is just empty (SQL parameter, never SQL text).
        assert!(answer(&store, &request("x'; DROP TABLE records; --", vec![])).records.is_empty());
        assert_eq!(store.count(&chan()).unwrap(), 5);
    }

    #[test]
    fn response_pages_fit_the_codec_limit() {
        // 400 records of ~16 KiB is ~6.5 MB; a page stops at PAGE_BYTES.
        let (store, _) = store_with(400, 16 * 1024 - 8);
        let response = answer(&store, &request(&chan(), vec![]));
        assert!(response.more);
        let size: usize = response.records.iter().map(|b| b.len()).sum();
        assert!(size <= PAGE_BYTES + records::MAX_RECORD_BYTES, "page is {size} bytes");
        // Every record in it decodes and belongs to the log.
        for bytes in &response.records {
            assert_eq!(log_of(&Record::decode(bytes).unwrap()), chan());
        }
    }

    fn pushed(topic_log: &str, data: Vec<u8>) -> gossipsub::Message {
        gossipsub::Message {
            source: None,
            data,
            sequence_number: None,
            topic: TopicHash::from_raw(format!("log/{topic_log}")),
        }
    }

    #[test]
    fn pushed_records_are_decoded_and_filed_by_what_they_signed() {
        let control = log_name(server(), Scope::Control);
        let open: HashSet<String> = [chan(), control.clone()].into();
        let m = crate::testutil::message(0, &[], 0);
        assert_eq!(check_pushed(&open, &pushed(&chan(), m.encode())), Ok(m.clone()));

        // A real record on another open log's topic: refused, not refiled.
        assert_eq!(check_pushed(&open, &pushed(&control, m.encode())), Err(Refuse::Reject));
        // Garbage, truncated, or a bad signature.
        assert_eq!(check_pushed(&open, &pushed(&chan(), vec![1, 2, 3])), Err(Refuse::Reject));
        let mut bytes = m.encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(check_pushed(&open, &pushed(&chan(), bytes.clone())), Err(Refuse::Reject));
        assert_eq!(check_pushed(&open, &pushed(&chan(), bytes[..last].to_vec())), Err(Refuse::Reject));
        // A log we don't have open: not ours to judge.
        assert_eq!(check_pushed(&HashSet::new(), &pushed(&chan(), m.encode())), Err(Refuse::Ignore));
        // Not a log topic at all.
        let mut odd = pushed(&chan(), m.encode());
        odd.topic = TopicHash::from_raw("other");
        assert_eq!(check_pushed(&open, &odd), Err(Refuse::Reject));
    }

    #[tokio::test]
    async fn dropped_events_are_reported_as_lagged() {
        let (tx, mut rx) = mpsc::channel(2);
        let mut sink = EventSink { tx, dropped: 0 };
        let peer = PeerId::random();
        for _ in 0..5 {
            sink.emit(NodeEvent::PeerConnected(peer));
        }
        // Two fit, three were dropped.
        assert!(matches!(rx.recv().await, Some(NodeEvent::PeerConnected(_))));
        assert!(matches!(rx.recv().await, Some(NodeEvent::PeerConnected(_))));
        // The next event is preceded by the count of what was lost.
        sink.emit(NodeEvent::PeerDisconnected(peer));
        assert!(matches!(rx.recv().await, Some(NodeEvent::Lagged { dropped: 3 })));
        assert!(matches!(rx.recv().await, Some(NodeEvent::PeerDisconnected(_))));
        sink.emit(NodeEvent::PeerConnected(peer));
        assert!(matches!(rx.recv().await, Some(NodeEvent::PeerConnected(_))));
    }

    #[test]
    fn pulled_pages_keep_only_valid_records_of_the_log() {
        let good = crate::testutil::message(0, &[], 0);
        let other_log = crate::testutil::genesis();
        let mut forged = good.encode();
        forged[40] ^= 1;
        let page = [good.encode(), other_log.encode(), forged, vec![], vec![0; 100_000]].map(ByteBuf::from);
        assert_eq!(decode_page(&chan(), &page), vec![good]);
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
        assert!(filter.can_subscribe(&TopicHash::from_raw(format!("log/{}", chan()))));
        assert!(!filter.can_subscribe(&TopicHash::from_raw("log/")));
        assert!(!filter.can_subscribe(&TopicHash::from_raw("something-else")));
        assert!(!filter.can_subscribe(&TopicHash::from_raw(format!("log/{}", "x".repeat(10_000)))));
    }
}

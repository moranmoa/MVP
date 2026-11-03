//! Browser peer (rust-libp2p → wasm32) for spike 5.
//!
//! Transports: WebSocket (to the relay), relay circuit (through the relay to
//! a native peer) and webrtc-direct (straight to a native peer). Protocols are
//! core/sync's: CBOR pull on `/mvs/sync/0.1.0` and gossipsub
//! push on `log/<name>`. Blobs live in memory ([`web_wire::MemStore`]).
//!
//! JS API (all async calls return a Promise of a JSON string):
//! `new WebPeer(log, eagerFlush)`, `peerId()`, `dial(addr)`, `append(data)`, `rtt(peer, n)`,
//! `state()`, `closeRelayed(peer)`, `stop()`.

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::{select, AsyncRead, AsyncWrite, StreamExt};
use libp2p::{
    core::{multiaddr::Protocol, transport::Transport as _, upgrade::Version},
    gossipsub, identify, identity, noise, ping, relay,
    request_response::{self, OutboundRequestId, ProtocolSupport},
    swarm::{
        dial_opts::{DialOpts, PeerCondition},
        ConnectionId, NetworkBehaviour, SwarmEvent,
    },
    websocket_websys, yamux, Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder,
};
use serde_bytes::ByteBuf;
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;
use web_wire::{
    Append, Blob, MemStore, SyncRequest, SyncResponse, IDENTIFY_VERSION, MAX_GOSSIP_BYTES, PAGE,
    PAGE_BYTES, SYNC_PROTOCOL,
};

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    gossipsub: gossipsub::Behaviour,
    sync: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
}

type Reply = oneshot::Sender<Result<Value, String>>;

enum Cmd {
    Dial { addr: Multiaddr, reply: Reply },
    Append { data: Vec<u8>, reply: Reply },
    Rtt { peer: PeerId, n: u32, reply: Reply },
    State { reply: Reply },
    CloseRelayed { peer: PeerId, reply: Reply },
    Stop,
}

enum Outbound {
    Pull,
    /// A timed pull, for the round-trip measurement.
    Rtt {
        started: f64,
    },
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or_else(js_sys::Date::now)
}

/// Wall clock in µs, comparable with the native peers' timestamps.
fn wall_micros() -> f64 {
    match web_sys::window().and_then(|w| w.performance()) {
        Some(p) => (p.time_origin() + p.now()) * 1000.0,
        None => js_sys::Date::now() * 1000.0,
    }
}

/// Message body: `<author>|<seq>|<written at, unix micros>|<text>`.
fn written_at(data: &[u8]) -> Option<f64> {
    std::str::from_utf8(data).ok()?.split('|').nth(2)?.parse().ok()
}

fn is_relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| p == Protocol::P2pCircuit)
}

#[wasm_bindgen]
pub struct WebPeer {
    peer_id: String,
    commands: mpsc::UnboundedSender<Cmd>,
}

#[wasm_bindgen]
impl WebPeer {
    #[wasm_bindgen(constructor)]
    /// `eager_flush`: work around websocket-websys' 100 ms flush polling (see [`EagerFlush`]).
    pub fn new(log: String, eager_flush: bool) -> Result<WebPeer, JsError> {
        let key = identity::Keypair::generate_ed25519();
        let swarm = build_swarm(key, eager_flush).map_err(|e| JsError::new(&e))?;
        let peer_id = swarm.local_peer_id().to_string();
        let (commands, rx) = mpsc::unbounded();
        let mut task = Task {
            swarm,
            log,
            store: MemStore::default(),
            dials: HashMap::new(),
            outbound: HashMap::new(),
            rtts: HashMap::new(),
            connections: HashMap::new(),
            identified: HashMap::new(),
            ping_ms: HashMap::new(),
            received: Vec::new(),
            via_push: 0,
            via_pull: 0,
            events: Vec::new(),
        };
        let topic = gossipsub::IdentTopic::new(web_wire::topic(&task.log));
        task.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .map_err(|e| JsError::new(&e.to_string()))?;
        wasm_bindgen_futures::spawn_local(task.run(rx));
        Ok(WebPeer { peer_id, commands })
    }

    #[wasm_bindgen(js_name = peerId)]
    pub fn peer_id(&self) -> String {
        self.peer_id.clone()
    }

    /// Resolves once the connection is up: `{ms, peer, addr, relayed}`.
    pub fn dial(&self, addr: String) -> js_sys::Promise {
        self.call(move |reply| {
            Ok(Cmd::Dial { addr: addr.parse().map_err(|e| format!("{e}"))?, reply })
        })
    }

    /// Appends `data` on top of the current heads and pushes it.
    pub fn append(&self, data: String) -> js_sys::Promise {
        self.call(move |reply| Ok(Cmd::Append { data: data.into_bytes(), reply }))
    }

    /// `n` sequential sync requests to `peer`, each timed: `{ms: [..]}`.
    pub fn rtt(&self, peer: String, n: u32) -> js_sys::Promise {
        self.call(move |reply| {
            Ok(Cmd::Rtt { peer: peer.parse().map_err(|e| format!("{e}"))?, n, reply })
        })
    }

    /// Drops the swarm: closes all connections (and their relay circuits).
    pub fn stop(&self) {
        let _ = self.commands.unbounded_send(Cmd::Stop);
    }

    pub fn state(&self) -> js_sys::Promise {
        self.call(|reply| Ok(Cmd::State { reply }))
    }

    /// Closes the relayed connections to `peer` (keeps direct ones).
    #[wasm_bindgen(js_name = closeRelayed)]
    pub fn close_relayed(&self, peer: String) -> js_sys::Promise {
        self.call(move |reply| {
            Ok(Cmd::CloseRelayed { peer: peer.parse().map_err(|e| format!("{e}"))?, reply })
        })
    }

    fn call(&self, make: impl FnOnce(Reply) -> Result<Cmd, String>) -> js_sys::Promise {
        let (reply, rx) = oneshot::channel();
        let sent = make(reply)
            .and_then(|cmd| self.commands.unbounded_send(cmd).map_err(|e| e.to_string()));
        wasm_bindgen_futures::future_to_promise(async move {
            sent.map_err(|e| JsValue::from_str(&e))?;
            match rx.await {
                Ok(Ok(value)) => Ok(JsValue::from_str(&value.to_string())),
                Ok(Err(err)) => Err(JsValue::from_str(&err)),
                Err(_) => Err(JsValue::from_str("peer stopped")),
            }
        })
    }
}

fn build_swarm(key: identity::Keypair, eager_flush: bool) -> Result<Swarm<Behaviour>, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    Ok(SwarmBuilder::with_existing_identity(key)
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                websocket_websys::Transport::default()
                    .map(move |conn, _| EagerFlush { conn, eager: eager_flush })
                    .upgrade(Version::V1)
                    .authenticate(noise::Config::new(key)?)
                    .multiplex(yamux::Config::default()),
            )
        })
        .map_err(|e| err(&e))?
        .with_other_transport(|key| {
            libp2p_webrtc_websys::Transport::new(libp2p_webrtc_websys::Config::new(key))
        })
        .map_err(|e| err(&e))?
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .map_err(|e| err(&e))?
        .with_behaviour(|key, relay_client| {
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_millis(500))
                    // Same as core/sync: size cap and content-based message ids.
                    .max_transmit_size(MAX_GOSSIP_BYTES)
                    .message_id_fn(|m| {
                        gossipsub::MessageId::from(web_wire::gossip_message_id(
                            m.topic.as_str(),
                            &m.data,
                        ))
                    })
                    .build()
                    .map_err(|e| e.to_string())?,
            )?;
            Ok(Behaviour {
                relay_client,
                identify: identify::Behaviour::new(identify::Config::new(
                    IDENTIFY_VERSION.into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(
                    ping::Config::new().with_interval(Duration::from_secs(1)),
                ),
                gossipsub,
                sync: request_response::cbor::Behaviour::new(
                    [(StreamProtocol::new(SYNC_PROTOCOL), ProtocolSupport::Full)],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(30)),
                ),
            })
        })
        .map_err(|e| err(&e))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build())
}

struct Task {
    swarm: Swarm<Behaviour>,
    log: String,
    store: MemStore,
    dials: HashMap<ConnectionId, (f64, Reply)>,
    outbound: HashMap<OutboundRequestId, Outbound>,
    /// Peer → (remaining, timings, reply) of a running round-trip measurement.
    rtts: HashMap<PeerId, (u32, Vec<f64>, Reply)>,
    connections: HashMap<ConnectionId, (PeerId, Multiaddr)>,
    identified: HashMap<String, Vec<String>>,
    /// Ping round trips per kind of connection ("relayed" / "direct").
    ping_ms: HashMap<&'static str, Vec<f64>>,
    /// Write → stored latencies (ms) of blobs from other peers.
    received: Vec<f64>,
    via_push: u64,
    via_pull: u64,
    events: Vec<String>,
}

impl Task {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            select! {
                command = commands.next() => match command {
                    Some(Cmd::Stop) | None => return,
                    Some(command) => self.on_command(command),
                },
                event = self.swarm.select_next_some() => self.on_swarm_event(event),
            }
        }
    }

    fn log_event(&mut self, event: String) {
        web_sys::console::log_1(&event.clone().into());
        self.events.push(format!("{:.0} {event}", now_ms()));
    }

    fn on_command(&mut self, command: Cmd) {
        match command {
            Cmd::Dial { addr, reply } => {
                let opts = match addr.iter().last() {
                    // Dial even when already connected (relayed) to the peer.
                    Some(Protocol::P2p(peer)) => DialOpts::peer_id(peer)
                        .addresses(vec![addr])
                        .condition(PeerCondition::Always)
                        .build(),
                    _ => DialOpts::unknown_peer_id().address(addr).build(),
                };
                let id = opts.connection_id();
                match self.swarm.dial(opts) {
                    Ok(()) => {
                        self.dials.insert(id, (now_ms(), reply));
                    }
                    Err(err) => {
                        let _ = reply.send(Err(err.to_string()));
                    }
                }
            }
            Cmd::Append { data, reply } => {
                let blob = Blob::new(&self.log, self.store.heads(), data);
                let id = blob.id;
                let encoded = blob.encode();
                self.store.append(blob);
                let topic = gossipsub::IdentTopic::new(web_wire::topic(&self.log));
                // No subscribed peer yet is fine: peers pull it.
                let published = self.swarm.behaviour_mut().gossipsub.publish(topic, encoded);
                let _ = reply.send(Ok(json!({
                    "id": id.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    "published": published.is_ok(),
                })));
            }
            Cmd::Rtt { peer, n, reply } => {
                self.rtts.insert(peer, (n, Vec::new(), reply));
                self.timed_pull(peer);
            }
            Cmd::State { reply } => {
                let connections: Vec<Value> = self
                    .connections
                    .values()
                    .map(|(peer, addr)| json!({ "peer": peer.to_string(), "addr": addr.to_string(), "relayed": is_relayed(addr) }))
                    .collect();
                let _ = reply.send(Ok(json!({
                    "peer_id": self.swarm.local_peer_id().to_string(),
                    "count": self.store.len(),
                    "hash": self.store.state_hash(),
                    "connections": connections,
                    "identified": self.identified,
                    "latencies_ms": self.received,
                    "via_push": self.via_push,
                    "via_pull": self.via_pull,
                    "ping_ms": self.ping_ms,
                    "events": self.events,
                })));
            }
            Cmd::Stop => unreachable!(),
            Cmd::CloseRelayed { peer, reply } => {
                let relayed: Vec<ConnectionId> = self
                    .connections
                    .iter()
                    .filter(|(_, (p, addr))| *p == peer && is_relayed(addr))
                    .map(|(id, _)| *id)
                    .collect();
                for id in &relayed {
                    self.swarm.close_connection(*id);
                }
                let _ = reply.send(Ok(json!({ "closed": relayed.len() })));
            }
        }
    }

    fn sync_request(&mut self, peer: PeerId, purpose: Outbound) {
        let have =
            self.store.have_markers().into_iter().map(|id| ByteBuf::from(id.to_vec())).collect();
        let id = self
            .swarm
            .behaviour_mut()
            .sync
            .send_request(&peer, SyncRequest { log: self.log.clone(), have });
        self.outbound.insert(id, purpose);
    }

    fn timed_pull(&mut self, peer: PeerId) {
        self.sync_request(peer, Outbound::Rtt { started: now_ms() });
    }

    fn store_blob(&mut self, blob: Blob, push: bool) -> Append {
        let latency = written_at(&blob.data).map(|w| (wall_micros() - w) / 1000.0);
        let result = self.store.append(blob);
        if result == Append::Added {
            if push {
                self.via_push += 1;
            } else {
                self.via_pull += 1;
            }
            self.received.extend(latency);
        }
        result
    }

    fn on_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                let addr = endpoint.get_remote_address().clone();
                self.log_event(format!("connected {peer_id} via {addr}"));
                self.connections.insert(connection_id, (peer_id, addr.clone()));
                if let Some((started, reply)) = self.dials.remove(&connection_id) {
                    let _ = reply.send(Ok(json!({
                        "ms": now_ms() - started,
                        "peer": peer_id.to_string(),
                        "addr": addr.to_string(),
                        "relayed": is_relayed(&addr),
                    })));
                }
            }
            SwarmEvent::OutgoingConnectionError { connection_id, peer_id, error } => {
                self.log_event(format!("dial to {peer_id:?} failed: {error}"));
                if let Some((_, reply)) = self.dials.remove(&connection_id) {
                    let _ = reply.send(Err(error.to_string()));
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, connection_id, cause, .. } => {
                self.log_event(format!("closed {peer_id}: {cause:?}"));
                self.connections.remove(&connection_id);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                let serves_sync = info.protocols.iter().any(|p| p.as_ref() == SYNC_PROTOCOL);
                self.identified.insert(
                    peer_id.to_string(),
                    info.listen_addrs.iter().map(|a| a.to_string()).collect(),
                );
                // Pull on connect, like core/sync (only from peers that sync).
                if serves_sync {
                    self.sync_request(peer_id, Outbound::Pull);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Ping(ping::Event {
                connection,
                result: Ok(rtt),
                ..
            })) => {
                if let Some((_, addr)) = self.connections.get(&connection) {
                    let kind = if is_relayed(addr) { "relayed" } else { "direct" };
                    self.ping_ms.entry(kind).or_default().push(rtt.as_secs_f64() * 1000.0);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            })) => {
                let Some(blob) = Blob::decode(&self.log, &message.data) else { return };
                if self.store_blob(blob, true) == Append::MissingDeps {
                    self.sync_request(propagation_source, Outbound::Pull);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message {
                peer,
                message,
            })) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let have: Vec<_> =
                        request.have.iter().filter_map(|b| b.as_slice().try_into().ok()).collect();
                    let (blobs, more) = if request.log == self.log {
                        self.store.missing_since(&have, PAGE, PAGE_BYTES)
                    } else {
                        (Vec::new(), false)
                    };
                    let blobs = blobs.iter().map(|b| ByteBuf::from(b.encode())).collect();
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .sync
                        .send_response(channel, SyncResponse { blobs, more });
                }
                request_response::Message::Response { request_id, response } => {
                    let purpose = self.outbound.remove(&request_id);
                    for bytes in &response.blobs {
                        if let Some(blob) = Blob::decode(&self.log, bytes) {
                            self.store_blob(blob, false);
                        }
                    }
                    if let Some(Outbound::Rtt { started }) = purpose {
                        self.on_rtt(peer, now_ms() - started);
                    } else if response.more {
                        self.sync_request(peer, Outbound::Pull);
                    }
                }
            },
            SwarmEvent::Behaviour(BehaviourEvent::Sync(
                request_response::Event::OutboundFailure { peer, request_id, error },
            )) => {
                self.log_event(format!("sync request to {peer} failed: {error}"));
                if let Some(Outbound::Rtt { .. }) = self.outbound.remove(&request_id) {
                    if let Some((_, _, reply)) = self.rtts.remove(&peer) {
                        let _ = reply.send(Err(error.to_string()));
                    }
                }
            }
            _ => {}
        }
    }

    fn on_rtt(&mut self, peer: PeerId, ms: f64) {
        let Some((left, times, _)) = self.rtts.get_mut(&peer) else { return };
        times.push(ms);
        *left -= 1;
        if *left > 0 {
            self.timed_pull(peer);
        } else if let Some((_, times, reply)) = self.rtts.remove(&peer) {
            let _ = reply.send(Ok(json!({ "ms": times })));
        }
    }
}

/// libp2p-websocket-websys 0.4's `poll_flush` stays pending until the
/// browser's send buffer is empty, and only re-checks that on a 100 ms
/// interval timer. Every flush in a handshake (multistream-select, Noise,
/// yamux) waits for that tick, which makes each WebSocket connect take
/// ~400 ms. The browser sends queued data on its own, so with `eager` set we
/// report the flush as done at once; `poll_write` still applies the
/// transport's buffer limit.
struct EagerFlush<C> {
    conn: C,
    eager: bool,
}

impl<C: AsyncRead + Unpin> AsyncRead for EagerFlush<C> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.conn).poll_read(cx, buf)
    }
}

impl<C: AsyncWrite + Unpin> AsyncWrite for EagerFlush<C> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.conn).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.eager {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.conn).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.conn).poll_close(cx)
    }
}

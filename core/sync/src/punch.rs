//! Hole punching (DCUtR) with members only (#184, approach A).
//!
//! libp2p's `dcutr::Behaviour` creates its handler on every relayed
//! connection the moment it is up: the side that accepted the circuit sends
//! its observed addresses at once, and the other side answers with its own.
//! At that moment we don't know who the peer is (the GAP-4 binding comes
//! with the first pull). So every DCUtR handler here sits behind a [`Gate`]:
//! - closed, it is never polled (no outgoing DCUtR stream, so none of our
//!   addresses go out), and an incoming DCUtR stream is held, unanswered;
//! - the node opens it ([`MemberPunch::open`]) once the peer is a verified
//!   member (bound, and our read gate accepts it), and relay-only is off.
//!   A held stream younger than [`HELD_FOR`] is then answered.
//!
//! The relayed connection is never closed here: a direct connection that
//! comes out of a punch is tracked by the node like any other (the relayed
//! one is parked, #206), and a failed punch leaves the relayed one as it is.
//!
//! Punch dials (the addresses the peer sent) are refused unless the peer's
//! gate is open, relay-only is off, and there are at most
//! [`MAX_PUNCH_ADDRS`] addresses, all dialable (global, or LAN when allowed).
//! Our own candidates (identify's observed addresses) are filtered the same
//! way before DCUtR ever sees them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use libp2p::core::{transport::PortUse, Endpoint};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::handler::{ConnectionEvent, FullyNegotiatedInbound};
use libp2p::swarm::{
    ConnectionDenied, ConnectionHandler, ConnectionHandlerEvent, ConnectionId, FromSwarm, NetworkBehaviour,
    NotifyHandler, SubstreamProtocol, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::{dcutr, Multiaddr, PeerId};

use crate::node::reachable;

/// Addresses we dial for one punch (#184 checklist point 5 allows 4; the
/// peer's DCUtR sends every candidate it has, so a little headroom).
pub(crate) const MAX_PUNCH_ADDRS: usize = 8;
/// Gates opened per peer per hour, and for all peers per minute.
const PER_PEER_PER_HOUR: usize = 3;
const ALL_PER_MINUTE: usize = 8;
/// An incoming DCUtR stream held while the gate was closed is answered on
/// opening only if it is younger than this (the peer gives up after 10 s).
const HELD_FOR: Duration = Duration::from_secs(8);

/// Commands to a [`Gate`].
#[derive(Debug)]
pub(crate) enum GateIn<T> {
    Open,
    Close,
    Inner(T),
}

type Held<H> =
    (Instant, FullyNegotiatedInbound<<H as ConnectionHandler>::InboundProtocol, <H as ConnectionHandler>::InboundOpenInfo>);

/// A connection handler that runs `inner` only while open.
pub(crate) struct Gate<H: ConnectionHandler> {
    inner: H,
    open: bool,
    held: Option<Held<H>>,
}

impl<H: ConnectionHandler> Gate<H> {
    pub(crate) fn new(inner: H) -> Self {
        Gate { inner, open: false, held: None }
    }
}

impl<H: ConnectionHandler> ConnectionHandler for Gate<H> {
    type FromBehaviour = GateIn<H::FromBehaviour>;
    type ToBehaviour = H::ToBehaviour;
    type InboundProtocol = H::InboundProtocol;
    type OutboundProtocol = H::OutboundProtocol;
    type InboundOpenInfo = H::InboundOpenInfo;
    type OutboundOpenInfo = H::OutboundOpenInfo;

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        self.inner.listen_protocol()
    }

    fn on_behaviour_event(&mut self, event: Self::FromBehaviour) {
        match event {
            GateIn::Open => {
                self.open = true;
                if let Some((at, stream)) = self.held.take() {
                    if at.elapsed() < HELD_FOR {
                        self.inner.on_connection_event(ConnectionEvent::FullyNegotiatedInbound(stream));
                    }
                }
            }
            GateIn::Close => self.open = false,
            GateIn::Inner(event) => self.inner.on_behaviour_event(event),
        }
    }

    fn connection_keep_alive(&self) -> bool {
        // Closed, DCUtR has no say in keeping the connection.
        self.open && self.inner.connection_keep_alive()
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ConnectionHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::ToBehaviour>> {
        if !self.open {
            // Woken again when a command (Open) arrives.
            return Poll::Pending;
        }
        self.inner.poll(cx)
    }

    fn poll_close(&mut self, cx: &mut Context<'_>) -> Poll<Option<Self::ToBehaviour>> {
        self.inner.poll_close(cx)
    }

    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<Self::InboundProtocol, Self::OutboundProtocol, Self::InboundOpenInfo, Self::OutboundOpenInfo>,
    ) {
        match event {
            // The peer asks to punch before we know it is a member: hold the
            // stream unanswered (dropping an older one).
            ConnectionEvent::FullyNegotiatedInbound(stream) if !self.open => {
                self.held = Some((Instant::now(), stream));
            }
            event => self.inner.on_connection_event(event),
        }
    }
}

/// `dcutr::Behaviour` with every handler behind a [`Gate`].
pub(crate) struct MemberPunch {
    inner: dcutr::Behaviour,
    allow_lan: bool,
    /// Off while relay-only (GAP-6): no gate opens, no punch dial goes out.
    enabled: bool,
    /// Open relayed connections per peer.
    relayed: HashMap<PeerId, HashSet<ConnectionId>>,
    /// Relayed connections whose gate we opened.
    opened: HashSet<ConnectionId>,
    /// Punch dials DCUtR asked for, until they connect or fail.
    dials: HashMap<ConnectionId, PeerId>,
    /// When gates were opened, per peer and for everyone (rate caps).
    per_peer: HashMap<PeerId, VecDeque<Instant>>,
    all: VecDeque<Instant>,
    queued: VecDeque<ToSwarm<dcutr::Event, THandlerInEvent<Self>>>,
}

impl MemberPunch {
    pub(crate) fn new(local: PeerId, allow_lan: bool) -> Self {
        MemberPunch {
            inner: dcutr::Behaviour::new(local),
            allow_lan,
            enabled: true,
            relayed: HashMap::new(),
            opened: HashSet::new(),
            dials: HashMap::new(),
            per_peer: HashMap::new(),
            all: VecDeque::new(),
            queued: VecDeque::new(),
        }
    }

    /// `peer` is a verified member: let DCUtR run on its relayed
    /// connections. False if nothing was opened (relay-only, no relayed
    /// connection, already open, or over the rate caps).
    pub(crate) fn open(&mut self, peer: PeerId) -> bool {
        self.open_at(peer, Instant::now())
    }

    fn open_at(&mut self, peer: PeerId, now: Instant) -> bool {
        if !self.enabled {
            return false;
        }
        let closed: Vec<ConnectionId> =
            self.relayed.get(&peer).into_iter().flatten().filter(|c| !self.opened.contains(c)).copied().collect();
        if closed.is_empty() {
            return false;
        }
        let hour = Duration::from_secs(3600);
        let mine = self.per_peer.entry(peer).or_default();
        while mine.front().is_some_and(|t| now.duration_since(*t) >= hour) {
            mine.pop_front();
        }
        while self.all.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.all.pop_front();
        }
        if mine.len() >= PER_PEER_PER_HOUR || self.all.len() >= ALL_PER_MINUTE {
            tracing::debug!(%peer, "hole punch skipped: rate cap");
            return false;
        }
        mine.push_back(now);
        self.all.push_back(now);
        for id in closed {
            self.opened.insert(id);
            self.queued.push_back(ToSwarm::NotifyHandler { peer_id: peer, handler: NotifyHandler::One(id), event: GateIn::Open });
        }
        true
    }

    /// Relay-only on: close every gate and refuse punch dials. Off: gates
    /// open again only through [`MemberPunch::open`].
    pub(crate) fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            for (peer, conns) in &self.relayed {
                for id in conns {
                    if self.opened.remove(id) {
                        self.queued.push_back(ToSwarm::NotifyHandler {
                            peer_id: *peer,
                            handler: NotifyHandler::One(*id),
                            event: GateIn::Close,
                        });
                    }
                }
            }
        }
    }

    /// Peers with a gate open now.
    pub(crate) fn open_peers(&self) -> Vec<PeerId> {
        self.relayed.keys().filter(|p| self.is_open(p)).copied().collect()
    }

    /// `peer` is no longer a member (kicked, or the read gate changed):
    /// close its gates. DCUtR stops (no retries), and its punch dials are
    /// refused from now on.
    pub(crate) fn close(&mut self, peer: PeerId) {
        for id in self.relayed.get(&peer).into_iter().flatten() {
            if self.opened.remove(id) {
                self.queued.push_back(ToSwarm::NotifyHandler { peer_id: peer, handler: NotifyHandler::One(*id), event: GateIn::Close });
            }
        }
    }

    fn is_open(&self, peer: &PeerId) -> bool {
        self.relayed.get(peer).is_some_and(|c| c.iter().any(|id| self.opened.contains(id)))
    }

    /// Whether DCUtR may dial `addrs` for `peer`.
    fn may_dial(&self, peer: Option<PeerId>, addrs: &[Multiaddr]) -> bool {
        self.enabled
            && peer.is_some_and(|p| self.is_open(&p))
            && !addrs.is_empty()
            && addrs.len() <= MAX_PUNCH_ADDRS
            && addrs.iter().all(|a| punchable(a, self.allow_lan))
    }
}

/// An address DCUtR may send or dial: plain TCP (no circuit, websocket or
/// DNS), global unless LAN is allowed.
pub(crate) fn punchable(addr: &Multiaddr, allow_lan: bool) -> bool {
    let plain = addr.iter().all(|p| matches!(p, Protocol::Ip4(_) | Protocol::Ip6(_) | Protocol::Tcp(_) | Protocol::P2p(_)));
    plain && reachable(addr, allow_lan)
}

fn relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| p == Protocol::P2pCircuit)
}

impl NetworkBehaviour for MemberPunch {
    type ConnectionHandler = Gate<<dcutr::Behaviour as NetworkBehaviour>::ConnectionHandler>;
    type ToSwarm = dcutr::Event;

    fn handle_pending_inbound_connection(
        &mut self,
        id: ConnectionId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner.handle_pending_inbound_connection(id, local, remote)
    }

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        let handler = self.inner.handle_established_inbound_connection(id, peer, local, remote)?;
        if relayed(local) {
            self.relayed.entry(peer).or_default().insert(id);
        }
        Ok(Gate::new(handler))
    }

    fn handle_pending_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: Option<PeerId>,
        addresses: &[Multiaddr],
        role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        if self.dials.contains_key(&id) && !self.may_dial(peer, addresses) {
            tracing::debug!(?peer, ?addresses, "hole punch dial refused");
            return Err(ConnectionDenied::new("hole punch refused: not a member, relay only, or bad addresses"));
        }
        self.inner.handle_pending_outbound_connection(id, peer, addresses, role)
    }

    fn handle_established_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        let handler = self.inner.handle_established_outbound_connection(id, peer, addr, role, port_use)?;
        self.dials.remove(&id);
        if relayed(addr) {
            self.relayed.entry(peer).or_default().insert(id);
        }
        Ok(Gate::new(handler))
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match &event {
            FromSwarm::ConnectionClosed(closed) => {
                self.opened.remove(&closed.connection_id);
                if let Some(conns) = self.relayed.get_mut(&closed.peer_id) {
                    conns.remove(&closed.connection_id);
                    if conns.is_empty() {
                        self.relayed.remove(&closed.peer_id);
                    }
                }
            }
            FromSwarm::DialFailure(failure) => {
                self.dials.remove(&failure.connection_id);
            }
            // Only addresses we would let a member dial become our DCUtR
            // candidates (identify's observed addresses; never circuits,
            // and LAN ones only when allowed).
            FromSwarm::NewExternalAddrCandidate(candidate) if !punchable(candidate.addr, self.allow_lan) => return,
            _ => {}
        }
        self.inner.on_swarm_event(event);
    }

    fn on_connection_handler_event(&mut self, peer: PeerId, id: ConnectionId, event: THandlerOutEvent<Self>) {
        self.inner.on_connection_handler_event(peer, id, event)
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        if let Some(event) = self.queued.pop_front() {
            return Poll::Ready(event);
        }
        match self.inner.poll(cx) {
            Poll::Ready(ToSwarm::Dial { opts }) => {
                if let Some(peer) = opts.get_peer_id() {
                    self.dials.insert(opts.connection_id(), peer);
                }
                Poll::Ready(ToSwarm::Dial { opts })
            }
            Poll::Ready(event) => Poll::Ready(event.map_in(GateIn::Inner)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::swarm::dummy;

    fn conn() -> ConnectionId {
        ConnectionId::new_unchecked(rand_core::RngCore::next_u32(&mut rand_core::OsRng) as usize)
    }

    /// A handler with something to say at once (as DCUtR's initiating side).
    struct Eager(dummy::ConnectionHandler);

    impl ConnectionHandler for Eager {
        type FromBehaviour = ();
        type ToBehaviour = ();
        type InboundProtocol = <dummy::ConnectionHandler as ConnectionHandler>::InboundProtocol;
        type OutboundProtocol = <dummy::ConnectionHandler as ConnectionHandler>::OutboundProtocol;
        type InboundOpenInfo = ();
        type OutboundOpenInfo = ();
        fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, ()> {
            self.0.listen_protocol()
        }
        fn on_behaviour_event(&mut self, _: ()) {}
        fn connection_keep_alive(&self) -> bool {
            true
        }
        fn poll(&mut self, _: &mut Context<'_>) -> Poll<ConnectionHandlerEvent<Self::OutboundProtocol, (), ()>> {
            Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(()))
        }
        fn on_connection_event(&mut self, _: ConnectionEvent<Self::InboundProtocol, Self::OutboundProtocol, (), ()>) {}
    }

    #[test]
    fn a_closed_gate_says_nothing_and_keeps_nothing_alive() {
        let mut gate = Gate::new(Eager(dummy::ConnectionHandler));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(gate.poll(&mut cx).is_pending(), "a closed gate let DCUtR start");
        assert!(!gate.connection_keep_alive());
        gate.on_behaviour_event(GateIn::Open);
        assert!(gate.poll(&mut cx).is_ready());
        assert!(gate.connection_keep_alive());
        gate.on_behaviour_event(GateIn::Close);
        assert!(gate.poll(&mut cx).is_pending());
    }

    /// A non-member's punch is refused: its relayed connection's gate stays
    /// closed, and the dials DCUtR asks for toward it are denied. A member's
    /// go through, with dialable addresses only; relay-only refuses all.
    #[test]
    fn punches_only_with_open_members_and_dialable_addresses() {
        let me = PeerId::random();
        let (member, stranger) = (PeerId::random(), PeerId::random());
        let mut punch = MemberPunch::new(me, false);
        let (cm, cs) = (conn(), conn());
        punch.relayed.entry(member).or_default().insert(cm);
        punch.relayed.entry(stranger).or_default().insert(cs);
        let global: Vec<Multiaddr> = vec!["/ip4/8.8.8.8/tcp/4001".parse().unwrap()];
        let dial = |punch: &mut MemberPunch, peer: PeerId, addrs: &[Multiaddr]| {
            let id = conn();
            punch.dials.insert(id, peer);
            punch.handle_pending_outbound_connection(id, Some(peer), addrs, Endpoint::Dialer).is_ok()
        };
        // Nobody opened yet: nothing goes out.
        assert!(!dial(&mut punch, member, &global));
        assert!(punch.open(member));
        assert!(!punch.open(member), "opened twice");
        assert!(matches!(punch.queued.pop_front(), Some(ToSwarm::NotifyHandler { peer_id, event: GateIn::Open, .. }) if peer_id == member));
        assert!(punch.queued.is_empty(), "the stranger's gate was opened");
        assert!(dial(&mut punch, member, &global));
        assert!(!dial(&mut punch, stranger, &global), "a non-member's punch went out");
        // Not dialable: LAN (not allowed here), circuits, websocket, too many, none.
        for bad in [
            vec!["/ip4/192.168.1.5/tcp/4001".parse().unwrap()],
            vec![format!("/ip4/8.8.4.4/tcp/4001/p2p/{}/p2p-circuit", PeerId::random()).parse().unwrap()],
            vec!["/ip4/8.8.8.8/tcp/443/wss".parse().unwrap()],
            vec![global[0].clone(); MAX_PUNCH_ADDRS + 1],
            vec![],
        ] {
            assert!(!dial(&mut punch, member, &bad), "{bad:?}");
        }
        // Ordinary dials (not DCUtR's) are not this behaviour's business.
        assert!(punch.handle_pending_outbound_connection(conn(), Some(stranger), &[], Endpoint::Dialer).is_ok());
        // Relay only: the open gate closes and nothing goes out.
        punch.set_enabled(false);
        assert!(matches!(punch.queued.pop_front(), Some(ToSwarm::NotifyHandler { event: GateIn::Close, .. })));
        assert!(!dial(&mut punch, member, &global));
        assert!(!punch.open(member));
    }

    #[test]
    fn gates_open_at_most_3_times_per_peer_per_hour() {
        let mut punch = MemberPunch::new(PeerId::random(), false);
        let peer = PeerId::random();
        let start = Instant::now();
        for i in 0..4 {
            punch.relayed.insert(peer, HashSet::from([conn()]));
            assert_eq!(punch.open_at(peer, start + Duration::from_secs(i * 60)), i < 3, "attempt {i}");
        }
        punch.relayed.insert(peer, HashSet::from([conn()]));
        assert!(punch.open_at(peer, start + Duration::from_secs(3601)));
    }

    /// A member kicked after its gate opened: the gate closes again, and
    /// DCUtR's dials toward it are refused from then on.
    #[test]
    fn a_kicked_members_gate_closes() {
        let mut punch = MemberPunch::new(PeerId::random(), false);
        let peer = PeerId::random();
        punch.relayed.insert(peer, HashSet::from([conn()]));
        assert!(punch.open(peer));
        assert_eq!(punch.open_peers(), vec![peer]);
        punch.queued.clear();
        punch.close(peer);
        assert!(matches!(punch.queued.pop_front(), Some(ToSwarm::NotifyHandler { event: GateIn::Close, .. })));
        assert!(punch.open_peers().is_empty());
        let id = conn();
        punch.dials.insert(id, peer);
        let global: Multiaddr = "/ip4/8.8.8.8/tcp/4001".parse().unwrap();
        assert!(punch.handle_pending_outbound_connection(id, Some(peer), &[global], Endpoint::Dialer).is_err());
    }
}

//! What we announce to every connected peer (GAP-6, the security lead's
//! #184 amendment 1). libp2p's identify tells every connected peer all the
//! addresses we listen on, LAN ones included, and the relay passes them on
//! through its DHT. This wraps identify so that it only ever learns about
//! global addresses and circuits through our relay. Direct and LAN
//! addresses go only to members, in pull answers (`for_members_only`).

use std::task::{Context, Poll};

use libp2p::core::{transport::PortUse, Endpoint};
use libp2p::swarm::{
    ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::{identify, Multiaddr, PeerId};

use crate::node::{reachable, via_relay};

/// Whether an address of ours may be announced to anyone: global, or a
/// circuit through our relay.
pub(crate) fn announceable(addr: &Multiaddr, relay: &Multiaddr) -> bool {
    via_relay(addr, relay) || reachable(addr, false)
}

pub(crate) struct GlobalIdentify {
    inner: identify::Behaviour,
    relay: Multiaddr,
}

impl GlobalIdentify {
    pub fn new(inner: identify::Behaviour, relay: Multiaddr) -> Self {
        GlobalIdentify { inner, relay }
    }
}

impl NetworkBehaviour for GlobalIdentify {
    type ConnectionHandler = <identify::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = identify::Event;

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
        self.inner.handle_established_inbound_connection(id, peer, local, remote)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: Option<PeerId>,
        addresses: &[Multiaddr],
        role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
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
        self.inner.handle_established_outbound_connection(id, peer, addr, role, port_use)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        // Identify announces what it hears about here: drop the rest.
        let hidden = match &event {
            FromSwarm::NewListenAddr(e) => !announceable(e.addr, &self.relay),
            FromSwarm::ExpiredListenAddr(e) => !announceable(e.addr, &self.relay),
            FromSwarm::ExternalAddrConfirmed(e) => !announceable(e.addr, &self.relay),
            FromSwarm::ExternalAddrExpired(e) => !announceable(e.addr, &self.relay),
            _ => false,
        };
        if !hidden {
            self.inner.on_swarm_event(event);
        }
    }

    fn on_connection_handler_event(&mut self, peer: PeerId, id: ConnectionId, event: THandlerOutEvent<Self>) {
        self.inner.on_connection_handler_event(peer, id, event)
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        self.inner.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_global_addresses_and_our_relay_circuits_are_announced() {
        let relay: Multiaddr = format!("/ip4/127.0.0.1/tcp/4001/p2p/{}", PeerId::random()).parse().unwrap();
        let yes = ["/ip4/8.8.8.8/tcp/4001".to_string(), format!("{relay}/p2p-circuit")];
        let no = ["/ip4/192.168.1.5/tcp/4001", "/ip4/127.0.0.1/tcp/4001", "/ip4/10.0.0.2/tcp/1", "/ip6/fe80::1/tcp/1"];
        for a in yes {
            assert!(announceable(&a.parse().unwrap(), &relay), "{a}");
        }
        for a in no {
            assert!(!announceable(&a.parse().unwrap(), &relay), "{a}");
        }
    }
}

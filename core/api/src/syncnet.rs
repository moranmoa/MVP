//! [`Network`] over `sync::Node`: records reach other peers through the
//! relay (pull by have-markers, gossipsub push), so two users can exchange
//! them. The core's commands are synchronous, so the adapter owns a tokio
//! runtime and blocks on it.
//!
//! - Reads come from sync's own store, decoded again here (GAP-1): what
//!   `fetch` returns has passed `Record::decode` and sits in the right log.
//! - Node events are hints, never data, and nothing depends on them: the
//!   [`MemberGate`] ties each reject to the control log's count, and the
//!   data is always re-read from the store (so a `Lagged` loses nothing).
//! - TODO(#64): a node that won't start (the `node.key` ACL) must reach the
//!   user as an error or notice; today `SyncNetwork::start` fails and the
//!   sidecar exits with a log line.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use libp2p::Multiaddr;
use records::{ChannelId, Record, RecordId, Scope, ServerId};
use sync::{log_name, log_of, Access, Node, NodeConfig, NodeEvent, ServerSecret, Store};
use tokio::runtime::Runtime;

use crate::access::{KeyBinder, MemberReadGate, MemberStubGate};
use crate::gate::MemberGate;
use crate::ports::Network;
use crate::types::SendState;

/// How long `fetch` waits for a server it has never seen to arrive, so a
/// join right after opening the log can find it.
const FIRST_FETCH_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(200);

/// A `--relay` address: a multiaddr that ends in `/p2p/<peer id>`, so Noise
/// checks it reached that very relay. Without the peer ID it would accept
/// whatever answers at the address (behind Cloudflare, anyone), so it is
/// refused (#114).
pub fn relay_addr(text: &str) -> Result<Multiaddr, String> {
    let addr: Multiaddr = text.parse().map_err(|_| "not a multiaddr".to_owned())?;
    match addr.iter().last() {
        Some(libp2p::multiaddr::Protocol::P2p(_)) => Ok(addr),
        _ => Err("must end in /p2p/<relay peer id>".to_owned()),
    }
}
type SendStates = Arc<Mutex<HashMap<RecordId, SendState>>>;

/// Send states kept; past this the delivered ones are forgotten.
const MAX_SEND_STATES: usize = 65_536;

/// Moves a record's state forward (pending, sent, received), never back.
/// True if it changed.
fn advance(states: &SendStates, id: RecordId, to: SendState) -> bool {
    let rank = |s: SendState| match s {
        SendState::Pending | SendState::Failed => 0,
        SendState::Sent => 1,
        SendState::Received => 2,
    };
    let mut states = states.lock().expect("not poisoned");
    let Some(now) = states.get_mut(&id) else { return false };
    if rank(to) <= rank(*now) {
        return false;
    }
    *now = to;
    true
}

pub struct SyncNetwork {
    runtime: Runtime,
    node: Node,
    store: PathBuf,
    /// Servers opened members-only (GAP-4), with the channels opened.
    opened: Mutex<HashMap<ServerId, BTreeSet<ChannelId>>>,
    /// Servers fetched at least once (the first fetch waits for the pull).
    fetched: Mutex<HashSet<ServerId>>,
    binder: Arc<KeyBinder>,
    /// B6: where each record written here stands.
    send_states: SendStates,
    hints: Mutex<Option<Receiver<()>>>,
    waker: SyncSender<()>,
}

impl SyncNetwork {
    /// Starts a node in `data_dir` (its `store.db` and `node.key`) that
    /// reaches peers through `relay`.
    pub fn start(relay: Multiaddr, data_dir: &Path, gate: Arc<MemberGate>) -> anyhow::Result<Self> {
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let config = NodeConfig {
            relay,
            data_dir: data_dir.to_owned(),
            logs: Vec::new(),
            authorizer: gate,
            listen: "/ip4/0.0.0.0/tcp/0".parse().expect("valid multiaddr"),
            bootstrap: Vec::new(),
            allow_lan: false,
        };
        // GAP-4: our pulls carry bindings signed with the identity key (once
        // unlocked), and channel logs are served to members only.
        let binder = Arc::new(KeyBinder::locked());
        let access = Access {
            binder: Some(binder.clone()),
            read_gate: Some(Arc::new(MemberReadGate::new())),
            // B6-p: stubs only once a held, accepted delete or edit replaced
            // the record.
            stub_gate: Some(Arc::new(MemberStubGate)),
        };
        let (node, mut events) = runtime.block_on(Node::start_with(config, access))?;
        // Events are only hints: a stored record wakes the core's sync loop
        // (`Core::run_sync_loop`), which re-reads the store. Coalesced (one
        // pending hint is enough) and drained, so the node never blocks.
        let send_states: SendStates = Arc::default();
        let states = send_states.clone();
        let (hint, hints) = std::sync::mpsc::sync_channel(1);
        let waker = hint.clone();
        runtime.spawn(async move {
            while let Some(event) = events.recv().await {
                let wake = match event {
                    NodeEvent::Stored { .. } | NodeEvent::Lagged { .. } => true,
                    // B6: my messages' send state moves on; a round re-announces.
                    NodeEvent::Sent { id } => advance(&states, id, SendState::Sent),
                    NodeEvent::Delivered { id } => advance(&states, id, SendState::Received),
                    _ => false,
                };
                if wake {
                    let _ = hint.try_send(());
                }
            }
        });
        Ok(SyncNetwork {
            runtime,
            node,
            store: data_dir.join("store.db"),
            opened: Mutex::new(HashMap::new()),
            fetched: Mutex::new(HashSet::new()),
            binder,
            send_states,
            hints: Mutex::new(Some(hints)),
            waker,
        })
    }

    /// The hints for `Core::run_sync_loop` (once; `None` after).
    pub fn take_hints(&self) -> Option<Receiver<()>> {
        self.hints.lock().expect("not poisoned").take()
    }

    /// Wakes the sync loop now, e.g. so it sees its stop flag.
    pub fn wake(&self) {
        let _ = self.waker.try_send(());
    }

    /// Every record of `log` in sync's store, decoded again.
    fn read(&self, log: &str) -> Vec<Record> {
        let Ok(store) = Store::open_read_only(&self.store) else { return Vec::new() };
        let Ok((stored, _)) = store.missing_since(log, &[], usize::MAX, usize::MAX) else {
            return Vec::new();
        };
        stored
            .into_iter()
            .filter_map(|s| Record::decode(&s.bytes).ok())
            .filter(|r| log_of(r) == log)
            .collect()
    }
}

impl Network for SyncNetwork {
    /// The server must have been opened (`open_server`): logs are never
    /// opened by their plain names, which anyone could ask for (GAP-4).
    fn publish(&self, record: &Record) {
        let mut states = self.send_states.lock().expect("not poisoned");
        if states.len() >= MAX_SEND_STATES {
            states.retain(|_, s| *s != SendState::Received);
        }
        states.entry(record.id()).or_insert(SendState::Pending);
        drop(states);
        if let Err(e) = self.runtime.block_on(self.node.append(record.clone())) {
            eprintln!("mvs-core: sync didn't take a record: {e}");
        }
    }

    fn fetch(&self, server: ServerId) -> Vec<Record> {
        let log = log_name(server, Scope::Control);
        let first = self.fetched.lock().expect("not poisoned").insert(server);
        let mut records = self.read(&log);
        // A server seen for the first time: give the pull a moment.
        let deadline = Instant::now() + FIRST_FETCH_WAIT;
        while first && records.is_empty() && Instant::now() < deadline {
            std::thread::sleep(POLL);
            records = self.read(&log);
        }
        // And every channel opened for it.
        let channels = self.opened.lock().expect("not poisoned").get(&server).cloned();
        for channel in channels.unwrap_or_default() {
            records.extend(self.read(&log_name(server, Scope::Channel(channel))));
        }
        records
    }

    fn unlock(&self, key: &records::SigningKey) {
        self.binder.unlock(key);
    }

    fn lock(&self) {
        self.binder.lock();
    }

    fn send_state(&self, id: RecordId) -> Option<SendState> {
        self.send_states.lock().expect("not poisoned").get(&id).copied()
    }

    fn prune(&self, id: records::RecordId) {
        if let Err(e) = self.runtime.block_on(self.node.prune(id)) {
            eprintln!("mvs-core: sync can't prune a record: {e}");
        }
    }

    fn open_server(&self, server: ServerId, secret: &[u8; 32], channels: &BTreeSet<ChannelId>) {
        let mut opened = self.opened.lock().expect("not poisoned");
        let have = match opened.get_mut(&server) {
            Some(have) => have,
            None => {
                if let Err(e) = self.node.open_server(server, ServerSecret::from_bytes(*secret)) {
                    eprintln!("mvs-core: sync can't open a server: {e}");
                    return;
                }
                opened.entry(server).or_default()
            }
        };
        for channel in channels {
            if have.contains(channel) {
                continue;
            }
            match self.runtime.block_on(self.node.open_channel(server, *channel)) {
                Ok(()) => {
                    have.insert(*channel);
                }
                Err(e) => eprintln!("mvs-core: sync can't open a channel: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use records::SigningKey;

    /// One node, no relay reachable: what it publishes comes back from
    /// `fetch` through sync's store, decoded, in its log.
    #[test]
    fn published_records_come_back_from_the_store() {
        let dir = std::env::temp_dir().join(format!("mvs-syncnet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let relay: Multiaddr =
            "/ip4/127.0.0.1/tcp/9/p2p/12D3KooWBVzkbuxE3TA4LU7zEeMBM87G4Ns1KQoPdAc4MR8R3GTc"
                .parse()
                .unwrap();
        let net = SyncNetwork::start(relay, &dir, Arc::new(MemberGate::new())).unwrap();
        let owner = SigningKey::from_bytes(&[4; 32]);
        let genesis = authority::sign_genesis(&owner, [1; 16], 1, b"{}").unwrap();
        // GAP-4: nothing is synced, or published, for a server not opened.
        net.open_server(genesis.server(), &[7; 32], &BTreeSet::new());
        net.publish(&genesis);
        let other = ServerId::from_bytes([8; 32]);
        assert_eq!(net.read(&log_name(other, Scope::Control)), Vec::<Record>::new());
        let fetched = net.fetch(genesis.server());
        assert_eq!(fetched.iter().map(Record::id).collect::<Vec<_>>(), [genesis.id()]);
        drop(net);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod relay_tests {
    use super::relay_addr;

    #[test]
    fn a_relay_must_name_its_peer_id() {
        let id = "12D3KooWBVzkbuxE3TA4LU7zEeMBM87G4Ns1KQoPdAc4MR8R3GTc";
        assert!(relay_addr(&format!("/dns4/relay.example/tcp/443/p2p/{id}")).is_ok());
        assert!(relay_addr(&format!("/ip4/1.2.3.4/tcp/4001/p2p/{id}")).is_ok());
        for bad in [
            "/dns4/relay.example/tcp/443",
            "/ip4/1.2.3.4/tcp/4001",
            // A peer ID that isn't last doesn't pin the relay either.
            &format!("/p2p/{id}/ip4/1.2.3.4/tcp/4001"),
            "relay.example:443",
            "",
        ] {
            assert!(relay_addr(bad).is_err(), "{bad} should be refused");
        }
    }
}

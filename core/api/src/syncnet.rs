//! [`Network`] over `sync::Node`: records reach other peers through the
//! relay (pull by have-markers, gossipsub push), so two users can exchange
//! them. The core's commands are synchronous, so the adapter owns a tokio
//! runtime and blocks on it.
//!
//! - Reads come from sync's own store, decoded again here (GAP-1): what
//!   `fetch` returns has passed `Record::decode` and sits in the right log.
//! - Node events are hints, never data: a stored control record only tells
//!   the [`MemberGate`] its rejects may be stale, and `Lagged` (events were
//!   dropped) clears every reject. The data is always re-read from the store.
//! - TODO(#64): a node that won't start (the `node.key` ACL) must reach the
//!   user as an error or notice; today `SyncNetwork::start` fails and the
//!   sidecar exits with a log line.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use libp2p::Multiaddr;
use records::{Record, Scope, ServerId};
use sync::{log_name, log_of, Node, NodeConfig, NodeEvent, Store};
use tokio::runtime::Runtime;

use crate::gate::MemberGate;
use crate::ports::Network;

/// How long `fetch` waits for a server it has never seen to arrive, so a
/// join right after opening the log can find it.
const FIRST_FETCH_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(200);

pub struct SyncNetwork {
    runtime: Runtime,
    node: Node,
    store: PathBuf,
    opened: Mutex<HashSet<String>>,
}

impl SyncNetwork {
    /// Starts a node in `data_dir` (its `store.db` and `node.key`) that
    /// reaches peers through `relay`.
    pub fn start(relay: Multiaddr, data_dir: &Path, gate: Arc<MemberGate>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let config = NodeConfig {
            relay,
            data_dir: data_dir.to_owned(),
            logs: Vec::new(),
            authorizer: gate.clone(),
            listen: "/ip4/0.0.0.0/tcp/0".parse().expect("valid multiaddr"),
            bootstrap: Vec::new(),
            allow_lan: false,
        };
        let (node, mut events) = runtime.block_on(Node::start(config))?;
        runtime.spawn(async move {
            while let Some(event) = events.recv().await {
                match event {
                    NodeEvent::Stored { record, .. } if record.scope() == Scope::Control => {
                        gate.control_changed(record.server());
                    }
                    NodeEvent::Lagged { .. } => gate.forget_rejects(),
                    _ => {}
                }
            }
        });
        Ok(SyncNetwork {
            runtime,
            node,
            store: data_dir.join("store.db"),
            opened: Mutex::new(HashSet::new()),
        })
    }

    /// Starts syncing `log` if not already; true the first time.
    fn open(&self, log: &str) -> bool {
        let first = self.opened.lock().expect("not poisoned").insert(log.to_owned());
        if first {
            if let Err(e) = self.node.open(log) {
                eprintln!("mvs-core: sync can't open a log: {e}");
            }
        }
        first
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
    fn publish(&self, record: &Record) {
        self.open(&log_of(record));
        if let Err(e) = self.runtime.block_on(self.node.append(record.clone())) {
            eprintln!("mvs-core: sync didn't take a record: {e}");
        }
    }

    fn fetch(&self, server: ServerId) -> Vec<Record> {
        let log = log_name(server, Scope::Control);
        let first = self.open(&log);
        let mut records = self.read(&log);
        // A server seen for the first time: give the pull a moment.
        let deadline = Instant::now() + FIRST_FETCH_WAIT;
        while first && records.is_empty() && Instant::now() < deadline {
            std::thread::sleep(POLL);
            records = self.read(&log);
        }
        records
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
        net.publish(&genesis);
        let other = ServerId::from_bytes([8; 32]);
        assert_eq!(net.read(&log_name(other, Scope::Control)), Vec::<Record>::new());
        let fetched = net.fetch(genesis.server());
        assert_eq!(fetched.iter().map(Record::id).collect::<Vec<_>>(), [genesis.id()]);
        drop(net);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

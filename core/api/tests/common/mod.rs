#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use api::{
    CollectingSink, Config, Core, FixedClock, GroupCipher, LoopbackNetwork, Network,
    PendingGroupKeys, Ports,
};
use identity::MemoryStore;

/// 2026-11-03T00:00:00Z.
pub const NOW_MS: u64 = 20_760 * 86_400_000;

pub struct Peer {
    pub core: Core,
    pub events: Arc<CollectingSink>,
    pub secrets: Arc<MemoryStore>,
}

pub struct World {
    pub network: Arc<LoopbackNetwork>,
    pub clock: Arc<FixedClock>,
}

impl World {
    pub fn new() -> World {
        World {
            network: Arc::new(LoopbackNetwork::new()),
            clock: Arc::new(FixedClock::new(NOW_MS)),
        }
    }

    pub fn peer(&self) -> Peer {
        self.peer_with(Arc::new(MemoryStore::new()), None, Arc::new(PendingGroupKeys))
    }

    pub fn peer_with(
        &self,
        secrets: Arc<MemoryStore>,
        db_path: Option<PathBuf>,
        cipher: Arc<dyn GroupCipher>,
    ) -> Peer {
        self.peer_on(self.network.clone(), secrets, db_path, cipher)
    }

    /// A peer on its own view of the network (e.g. one that can't reach
    /// anyone yet).
    pub fn peer_on(
        &self,
        network: Arc<dyn Network>,
        secrets: Arc<MemoryStore>,
        db_path: Option<PathBuf>,
        cipher: Arc<dyn GroupCipher>,
    ) -> Peer {
        let events = Arc::new(CollectingSink::new());
        let core = Core::open(
            Config { db_path, app_version: "0.1.0-alpha".into() },
            Ports {
                secrets: secrets.clone(),
                network,
                events: events.clone(),
                clock: self.clock.clone(),
                cipher,
            },
        )
        .expect("core opens");
        Peer { core, events, secrets }
    }
}

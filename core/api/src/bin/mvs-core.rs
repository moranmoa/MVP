//! The core as a sidecar process: JSON lines on stdin and stdout (see
//! `api::sidecar`). Logs go to stderr and never hold words, keys or text.
//!
//! Usage: `mvs-core --data-dir <dir> [--relay <multiaddr>]`. With a relay
//! the core syncs through `sync` (its store and node key in `<dir>/sync`);
//! without one nothing leaves this process.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use api::sidecar::{self, Lines};
use api::syncnet::SyncNetwork;
use api::{Config, Core, LoopbackNetwork, MemberGate, Network, Ports, SystemClock};
use identity::OsKeychain;

struct Args {
    data_dir: PathBuf,
    relay: Option<String>,
}

fn args() -> Option<Args> {
    let mut data_dir = None;
    let mut relay = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next()?;
        match flag.to_str()? {
            "--data-dir" if data_dir.is_none() => data_dir = Some(PathBuf::from(value)),
            "--relay" if relay.is_none() => relay = Some(value.into_string().ok()?),
            _ => return None,
        }
    }
    Some(Args { data_dir: data_dir?, relay })
}

fn main() -> ExitCode {
    let Some(args) = args() else {
        eprintln!("usage: mvs-core --data-dir <dir> [--relay <multiaddr>]");
        return ExitCode::from(2);
    };
    let dir = args.data_dir;
    // Owner-only, or the core doesn't start (fail closed, as sync does).
    if let Err(e) = sidecar::prepare_data_dir(&dir) {
        eprintln!("mvs-core: can't prepare the data dir: {e}");
        return ExitCode::FAILURE;
    }
    let network: Arc<dyn Network> = match args.relay {
        None => Arc::new(LoopbackNetwork::new()),
        Some(relay) => {
            let Ok(relay) = relay.parse() else {
                eprintln!("mvs-core: --relay is not a multiaddr");
                return ExitCode::from(2);
            };
            // TODO(#64): a start failure (the node.key ACL) as a notice
            // the UI shows, not only this log line and an exit.
            match SyncNetwork::start(relay, &dir.join("sync"), Arc::new(MemberGate::new())) {
                Ok(net) => Arc::new(net),
                Err(e) => {
                    eprintln!("mvs-core: sync didn't start: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    let out = Lines::new(io::stdout());
    let ports = Ports {
        secrets: Arc::new(OsKeychain::new()),
        network,
        events: out.clone(),
        clock: Arc::new(SystemClock),
        // Locked until the identity unlocks; service calls unlock/lock.
        cipher: Arc::new(api::crypto::GroupKeysCipher::locked()),
    };
    let config = Config {
        db_path: Some(dir.join("core.db")),
        app_version: env!("CARGO_PKG_VERSION").into(),
    };
    let core = match Core::open(config, ports) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("mvs-core: can't open the core: {:?}", e.code);
            return ExitCode::FAILURE;
        }
    };
    eprintln!("mvs-core: ready");
    match sidecar::serve(&core, io::stdin().lock(), &out) {
        Ok(n) => {
            eprintln!("mvs-core: stdin closed after {n} requests");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mvs-core: i/o error: {}", e.kind());
            ExitCode::FAILURE
        }
    }
}

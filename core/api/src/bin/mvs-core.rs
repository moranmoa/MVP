//! The core as a sidecar process: JSON lines on stdin and stdout (see
//! `api::sidecar`). Logs go to stderr and never hold words, keys or text.
//!
//! Usage: `mvs-core --data-dir <dir>`.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use api::sidecar::{self, Lines};
use api::{Config, Core, LoopbackNetwork, PendingGroupKeys, Ports, SystemClock};
use identity::OsKeychain;

fn data_dir() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    let (flag, dir) = (args.next()?, args.next()?);
    (flag == "--data-dir" && args.next().is_none()).then(|| PathBuf::from(dir))
}

fn main() -> ExitCode {
    let Some(dir) = data_dir() else {
        eprintln!("usage: mvs-core --data-dir <dir>");
        return ExitCode::from(2);
    };
    // Owner-only, or the core doesn't start (fail closed, as sync does).
    if let Err(e) = sidecar::prepare_data_dir(&dir) {
        eprintln!("mvs-core: can't prepare the data dir: {e}");
        return ExitCode::FAILURE;
    }
    let out = Lines::new(io::stdout());
    let ports = Ports {
        secrets: Arc::new(OsKeychain::new()),
        // TODO(sync): the sync adapter. Until then nothing leaves this process.
        network: Arc::new(LoopbackNetwork::new()),
        events: out.clone(),
        clock: Arc::new(SystemClock),
        // TODO(groupkeys): the groupkeys adapter.
        cipher: Arc::new(PendingGroupKeys),
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

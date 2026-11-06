//! The core as a sidecar process: JSON lines on stdin and stdout (see
//! `api::sidecar`). Logs go to stderr and never hold words, keys or text.
//!
//! Usage: `mvs-core --data-dir <dir> [--relay <multiaddr>] [--update-root <dir>]`.
//! With a relay the core syncs through `sync` (its store and node key in
//! `<dir>/sync`); without one nothing leaves this process for sync.
//! `--update-root` (the shell passes it only in a packaged app whose root
//! checks out) turns in-app update on: HTTPS to the channel's GitHub release
//! (B8, `api::update`). Without it the core never contacts the update host.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use api::sidecar::{self, Lines};
use api::syncnet::SyncNetwork;
use api::{Config, Core, LoopbackNetwork, MemberGate, Network, Ports, SystemClock};
use api::update::{UpdateSetup, Updates};
use identity::OsKeychain;
use updater::source::{HttpsSource, UpdateSource};
use updater::{Baseline, UpdateDirs, Version};

/// The app version, set by release.yml (`MVS_APP_VERSION`, as the launcher's
/// `MVS_BASE_VERSION`). A build without it never updates itself.
const APP_VERSION: Option<&str> = option_env!("MVS_APP_VERSION");

/// In-app update for an install at `root`, or why it stays off.
fn updates(root: PathBuf, data_dir: PathBuf) -> Result<Updates, &'static str> {
    let installed: Version = APP_VERSION.and_then(|v| v.parse().ok()).ok_or("this build has no MVS_APP_VERSION")?;
    if !root.is_absolute() {
        return Err("--update-root isn't absolute");
    }
    let state = UpdateDirs::default_state_dir().ok_or("no per-user state folder")?;
    Ok(Updates::new(UpdateSetup {
        dirs: UpdateDirs::new(root, state),
        core_data: data_dir,
        // As the launcher and manifest-body.mjs name it: windows-x86_64.
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        installed,
        baseline: Baseline::default(),
        root: None,
        source: Arc::new(|channel| HttpsSource::release(channel).map(|s| Box::new(s) as Box<dyn UpdateSource>)),
    }))
}

struct Args {
    data_dir: PathBuf,
    relay: Option<String>,
    update_root: Option<PathBuf>,
}

fn args() -> Option<Args> {
    let mut data_dir = None;
    let mut relay = None;
    let mut update_root = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next()?;
        match flag.to_str()? {
            "--data-dir" if data_dir.is_none() => data_dir = Some(PathBuf::from(value)),
            "--relay" if relay.is_none() => relay = Some(value.into_string().ok()?),
            "--update-root" if update_root.is_none() => update_root = Some(PathBuf::from(value)),
            _ => return None,
        }
    }
    Some(Args { data_dir: data_dir?, relay, update_root })
}

/// Logs to stderr only when RUST_LOG is set (the desktop captures and
/// redacts stderr); without it nothing is logged. A bad filter is reported
/// and ignored. Our own lines never hold IPs or IDs below debug.
fn init_logging() {
    let Ok(spec) = std::env::var("RUST_LOG") else { return };
    match tracing_subscriber::EnvFilter::try_new(&spec) {
        Ok(filter) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(io::stderr)
                .with_ansi(false)
                .init();
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "mvs-core: logging on");
        }
        Err(_) => eprintln!("mvs-core: RUST_LOG isn't a valid filter; not logging"),
    }
}

fn main() -> ExitCode {
    init_logging();
    let Some(args) = args() else {
        eprintln!("usage: mvs-core --data-dir <dir> [--relay <multiaddr>] [--update-root <dir>]");
        return ExitCode::from(2);
    };
    let dir = args.data_dir;
    // Owner-only, or the core doesn't start (fail closed, as sync does).
    if let Err(e) = sidecar::prepare_data_dir(&dir) {
        eprintln!("mvs-core: can't prepare the data dir: {e}");
        return ExitCode::FAILURE;
    }
    let sync: Option<Arc<SyncNetwork>> = match args.relay {
        None => None,
        Some(relay) => {
            let relay = match api::syncnet::relay_addr(&relay) {
                Ok(relay) => relay,
                Err(why) => {
                    eprintln!("mvs-core: --relay {why}");
                    return ExitCode::from(2);
                }
            };
            // TODO(#64): a start failure (the node.key ACL) as a notice
            // the UI shows, not only this log line and an exit.
            match SyncNetwork::start(relay, &dir.join("sync"), Arc::new(MemberGate::new())) {
                Ok(net) => Some(Arc::new(net)),
                Err(e) => {
                    eprintln!("mvs-core: sync didn't start: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    let network: Arc<dyn Network> = match &sync {
        Some(net) => net.clone(),
        None => Arc::new(LoopbackNetwork::new()),
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
        app_version: APP_VERSION.unwrap_or(env!("CARGO_PKG_VERSION")).into(),
    };
    let mut core = match Core::open(config, ports) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("mvs-core: can't open the core: {:?}", e.code);
            return ExitCode::FAILURE;
        }
    };
    if let Some(root) = args.update_root {
        match updates(root, dir.clone()) {
            Ok(updates) => core = core.with_updates(updates),
            Err(why) => eprintln!("mvs-core: updates off: {why}"),
        }
    }
    // Lives until the process ends: the update thread below is never
    // joined, so a download in progress can't hold up the exit.
    let core: &'static Core = Box::leak(Box::new(core));
    std::thread::spawn(|| core.run_update_loop());
    eprintln!("mvs-core: ready");
    // With a network, a background sync beside the request loop; it stops
    // and is joined when stdin closes.
    let stop = AtomicBool::new(false);
    let served = std::thread::scope(|scope| {
        if let Some(hints) = sync.as_ref().and_then(|net| net.take_hints()) {
            let stop = &stop;
            scope.spawn(move || core.run_sync_loop(&hints, stop));
        }
        let served = sidecar::serve(core, io::stdin().lock(), &out);
        core.stop_updates();
        stop.store(true, Ordering::SeqCst);
        if let Some(net) = &sync {
            net.wake();
        }
        served
    });
    match served {
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

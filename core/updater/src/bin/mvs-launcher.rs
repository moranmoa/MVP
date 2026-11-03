//! mvs-launcher: picks the app version to start and counts boots (B8-3).
//! See `updater::launcher` and core/updater/docs/B8-in-app-update.md.
//!
//! No console window on Windows, no network, no verification of its own.
//! Build with `MVS_BASE_VERSION=<the installer's app version>`; without it
//! (a development build) it always starts the base copy.
#![windows_subsystem = "windows"]

use std::ffi::OsString;
use std::process::ExitCode;

use updater::launcher::{command, default_core_data, plan, Plan};
use updater::{Boot, UpdateDirs, Version};

fn main() -> ExitCode {
    let forwarded: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(install) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else {
        return ExitCode::FAILURE;
    };
    let state = UpdateDirs::default_state_dir();
    let dirs = UpdateDirs::new(&install, state.clone().unwrap_or_else(|| install.join("update-state-missing")));
    let base: Option<Version> = option_env!("MVS_BASE_VERSION").and_then(|s| s.parse().ok());
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);

    let chosen = match (state, base, default_core_data()) {
        (Some(_), Some(base), Some(core_data)) => plan(&dirs, &core_data, base, &platform).ok(),
        _ => None,
    };
    // Anything unexpected: the installer's copy, never "whatever is there".
    let chosen = chosen.unwrap_or_else(|| Plan {
        exe: dirs.base_exe(),
        boot: Boot::Normal(base.unwrap_or_else(|| "0.0.0".parse().expect("valid"))),
        restored_snapshot: false,
    });
    match command(&chosen, &dirs, &forwarded).spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

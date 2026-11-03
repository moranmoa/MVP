//! Tauri shell for the footprint comparison (experiment 6): the unchanged
//! ui/ build with the mock core, nothing else.
//!
//! SHELL_BENCH_OUT=<file>  inject ../bench.js and append its marks to <file>.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::webview::PageLoadEvent;
use tauri::{WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
fn bench_mark(label: String) {
    let Ok(path) = std::env::var("SHELL_BENCH_OUT") else { return };
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{label} {ms}");
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![bench_mark])
        .setup(|app| {
            let bench = std::env::var_os("SHELL_BENCH_OUT").is_some();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html?demo=1&still=1".into()))
                .title("ShellSpikeTauri")
                .inner_size(1280.0, 800.0)
                .on_page_load(move |w, p| {
                    if bench && p.event() == PageLoadEvent::Finished {
                        let _ = w.eval(include_str!("../../bench.js"));
                    }
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri app");
}

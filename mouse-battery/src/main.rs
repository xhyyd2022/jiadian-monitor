//! Standalone mouse-battery HTTP service.
//!
//! Run this exe; it listens on 127.0.0.1:8767 and serves `/metrics`.
//! The Tauri main app spawns it and proxies the readings.

// Windows: no console window allocated. This is a background service.
#![windows_subsystem = "windows"]

fn main() {
    if let Err(e) = mouse_battery::server::run() {
        // eprintln goes nowhere without a console; that's fine — the service
        // either works or exits silently.
        let _ = e;
        std::process::exit(1);
    }
}

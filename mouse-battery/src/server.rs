//! Standalone HTTP service that exposes peripheral battery readings.
//!
//! Listens on `127.0.0.1:8767` and serves:
//! - `GET /metrics`  → JSON array of all battery readings (mouse1_pct, mouse2_pct, ...)
//! - `GET /devices`  → JSON array of all HID devices on the bus (for UI selection)
//! - `GET /health`   → plain text "ok"

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use crate::Monitor;

pub const PORT: u16 = 8767;

fn handle_stream(mut stream: std::net::TcpStream, monitor: &Monitor) {
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");
    let route = path.split('?').next().unwrap_or("/");

    let (status, ctype, body) = match route {
        "/metrics" => {
            let readings = monitor.snapshot_all();
            let mut arr = Vec::new();
            for (i, r) in readings.iter().enumerate() {
                let n = i + 1;
                let label = if r.charging {
                    format!("{}（充电）", r.device_label)
                } else {
                    r.device_label.clone()
                };
                arr.push(serde_json::json!({
                    "key": format!("mouse{n}_pct"),
                    "group": "外设",
                    "label": label,
                    "value": r.level_pct as f64,
                    "unit": "%",
                    "text": format!("{}%", r.level_pct),
                    "source": format!("HID: {} ({})", r.provider, r.device_label),
                    "kind": "number",
                }));
            }
            ("200 OK", "application/json; charset=utf-8", serde_json::to_string(&arr).unwrap())
        }
        "/devices" => {
            let all = monitor.enumerate_devices();
            // Deduplicate: only return supported devices, one per VID:PID pair.
            // The same physical receiver exposes multiple HID interfaces
            // (mouse, keyboard, vendor page); the dropdown only needs one entry.
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            let mut list = Vec::new();
            for d in all {
                if !d.supported { continue; }
                let key = format!("{:04x}:{:04x}", d.vid, d.pid);
                if seen.insert(key) {
                    list.push(d);
                }
            }
            ("200 OK", "application/json; charset=utf-8", serde_json::to_string(&list).unwrap())
        }
        "/health" => ("200 OK", "text/plain; charset=utf-8", "ok".to_string()),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".to_string()),
    };

    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.as_bytes().len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

pub fn run() -> std::io::Result<()> {
    let monitor = Monitor::new();
    let _handle = monitor.clone().spawn(Duration::from_secs(5));

    let addr = format!("127.0.0.1:{PORT}");
    let listener = TcpListener::bind(&addr)?;

    for stream in listener.incoming() {
        match stream {
            Ok(s) => handle_stream(s, &monitor),
            Err(_) => {}
        }
    }
    Ok(())
}

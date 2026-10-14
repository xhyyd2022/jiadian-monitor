//! 本地指标 API：把「数据拿好之后的样子」（23 项基础指标）以 HTTP 暴露给面板网页。
//!
//! 面板 HTML 不需要被注入数据，自己 `fetch` 即可：
//! ```js
//! const data = await (await fetch("http://127.0.0.1:8766/metrics")).json();
//! for (const m of data) {
//!   const el = document.querySelector(`[data-slot="${m.key}"]`);
//!   if (el) el.textContent = m.text;
//! }
//! ```
//!
//! - `GET /metrics` → `BasicMetric[]`（JSON，UTF-8）
//! - `GET /units`   → `{ mode, units }`（当前单位设置；注入脚本每秒拉一次 → 改完立刻生效）
//! - `GET /health`  → `ok`
//! - `POST /app/show` → 唤起主窗口（第二实例启动时用）
//! - `POST /app/quit` → 归还设备后优雅退出（编译脚本先调它，避免强杀留下租约）
//!
//! 只监听 `127.0.0.1`（不对外），带 `Access-Control-Allow-Origin: *`
//! （面板是 `file://` 页面，origin 为 null）。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

/// 面板网页调用本地 API 的端口。
pub const PORT: u16 = 8766;

pub fn spawn() {
    thread::spawn(|| {
        let addr = format!("127.0.0.1:{PORT}");
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[api] 绑定 {addr} 失败：{e}");
                return;
            }
        };
        eprintln!("[api] 指标 API: http://{addr}/metrics");
        for stream in listener.incoming().flatten() {
            let _ = handle(stream);
        }
    });
}

fn handle(mut s: TcpStream) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = s.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");
    let route = path.split('?').next().unwrap_or("/");

    let (status, ctype, body) = match route {
        "/metrics" => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_string(&crate::metrics::basic()).unwrap_or_else(|_| "[]".into()),
        ),
        "/health" => ("200 OK", "text/plain; charset=utf-8", "ok".to_string()),
        "/units" => (
            "200 OK",
            "application/json; charset=utf-8",
            crate::units::json(),
        ),
        "/app/show" => {
            crate::show_main_window();
            ("200 OK", "text/plain; charset=utf-8", "ok".to_string())
        }
        "/app/quit" => {
            crate::request_quit();
            ("200 OK", "text/plain; charset=utf-8", "quitting".to_string())
        }
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".to_string()),
    };

    let head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ctype}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.as_bytes().len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body.as_bytes())?;
    s.flush()
}

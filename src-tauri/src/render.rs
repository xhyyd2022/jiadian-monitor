//! 离屏渲染：headless Edge/Chrome + CDP `Page.startScreencast` → RGB565 帧。
//!
//! 一个常驻浏览器跑在固定 `W×H` 视口上，页面一变就推帧；`screencastFrameAck`
//! 由调用方在处理完该帧之后再发（天然节流，永远拿最新画面）。

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// 别给子进程新建控制台窗口（GUI 进程启动控制台程序时默认会弹一个黑框）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const CDP_HTTP_TIMEOUT: Duration = Duration::from_secs(2);
const CDP_TIMEOUT: Duration = Duration::from_secs(10);

const BROWSERS: &[&str] = &[
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
];

/// 找可用的 Edge/Chrome。
pub fn find_browser() -> Option<String> {
    BROWSERS
        .iter()
        .find(|p| std::path::Path::new(p).is_file())
        .map(|s| s.to_string())
}

/// 离屏面板 WebView2 的 CDP 端口（与 `main.rs` 设的环境变量一致）。
pub const PANEL_CDP_PORT: u16 = 9333;
/// CLI 模式（外部 Edge）用的 CDP 端口，避开 app 自己的。
pub const CLI_CDP_PORT: u16 = 9335;

/// 找 url 里含 `needle` 的 page target 的 WS URL —— app 的 WebView2 里同时有
/// UI 页（`tauri.localhost`）和离屏面板页，得靠 URL 区分。
pub fn page_ws_url_matching(port: u16, needle: &str) -> Result<String, String> {
    page_ws_url_matching_timeout(port, needle, Duration::from_secs(20))
}

/// 同 [`page_ws_url_matching`]，但允许调用方指定等待上限（重连时先用短超时探测）。
pub fn page_ws_url_matching_timeout(
    port: u16,
    needle: &str,
    timeout: Duration,
) -> Result<String, String> {
    page_ws_url_timeout(port, Some(needle), timeout)
}

fn cdp_http_get(url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    // ureq 3 的 global budget 从 connect 一直覆盖到 body 读完，不能每次 read 重新计时。
    ureq::get(url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
}

fn cdp_http_json(url: &str, timeout: Duration) -> Result<Value, String> {
    let resp = cdp_http_get(url, timeout).map_err(|e| format!("HTTP 请求失败：{e}"))?;
    let text = resp.into_body().read_to_string().map_err(|e| format!("HTTP 读取失败：{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败：{e}"))
}

fn page_ws_url_timeout(port: u16, needle: Option<&str>, timeout: Duration) -> Result<String, String> {
    let list = format!("http://127.0.0.1:{port}/json/list");
    let start = Instant::now();
    let mut last_error = "尚未发起请求".to_string();
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        match cdp_http_json(&list, remaining.min(CDP_HTTP_TIMEOUT)) {
            Ok(v) => {
                if let Some(arr) = v.as_array() {
                    for t in arr {
                        if t.get("type").and_then(|x| x.as_str()) != Some("page") {
                            continue;
                        }
                        let u = t.get("url").and_then(|x| x.as_str()).unwrap_or("");
                        if needle.is_none_or(|needle| u.contains(needle)) {
                            if let Some(ws) = t.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) {
                                return Ok(ws.to_string());
                            }
                        }
                    }
                    last_error = "列表中没有匹配且带 WebSocket URL 的 page target".into();
                } else {
                    last_error = "JSON 响应不是 target 数组".into();
                }
            }
            Err(e) => last_error = e,
        }
        let remaining = timeout.saturating_sub(start.elapsed());
        std::thread::sleep(Duration::from_millis(50).min(remaining));
    }
    let target = needle.map(|s| format!("含「{s}」的页面目标")).unwrap_or_else(|| "page target".into());
    Err(format!("等待 CDP {list} 的 {target} 超时（{timeout:?}）；最近失败：{last_error}"))
}

/// CDP 端口上是否已有浏览器在服务（用于复用，省掉冷启动）。
#[allow(dead_code)] // 端口探活（复用浏览器时用），暂未接入
pub fn cdp_alive(port: u16) -> bool {
    cdp_alive_timeout(port, CDP_HTTP_TIMEOUT)
}

fn cdp_alive_timeout(port: u16, timeout: Duration) -> bool {
    cdp_http_get(&format!("http://127.0.0.1:{port}/json/version"), timeout).is_ok()
}

/// 轮询 CDP `/json/list`，返回 page target 的 WebSocket URL。
pub fn page_ws_url(port: u16) -> Result<String, String> {
    page_ws_url_timeout(port, None, Duration::from_secs(20))
}

/// 让浏览器干净退出（CDP `Browser.close`）。
///
/// 为什么不用 taskkill：`msedge.exe` 只是启动器桩，把窗口交给已有实例后自己就退了，
/// 杀这个 PID 杀不到真身 —— 每轮漏一个 Edge 进程（实测漏了 8 个）。
pub async fn close_browser(port: u16) -> bool {
    close_browser_timeout(port, CDP_HTTP_TIMEOUT).await
}

async fn close_browser_timeout(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let Ok(v) = cdp_http_json(&format!("http://127.0.0.1:{port}/json/version"), timeout) else {
        return false;
    };
    let Some(ws) = v.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) else {
        return false;
    };
    // HTTP、WebSocket 握手与发送共用同一 deadline，不给每个阶段重新分配预算。
    tokio::time::timeout_at(deadline, async {
        let (mut s, _) = tokio_tungstenite::connect_async(ws).await.ok()?;
        s.send(Message::Text(r#"{"id":1,"method":"Browser.close"}"#.into()))
            .await
            .ok()
    })
    .await
    .is_ok_and(|result| result.is_some())
}

/// 常驻 headless 浏览器。
pub struct Browser {
    child: Child,
    pub port: u16,
}

impl Browser {
    pub fn launch(exe: &str, w: u32, h: u32, url: &str, port: u16, profile: &str) -> Result<Browser, String> {
        std::fs::create_dir_all(profile).ok();
        let mut child = Command::new(exe)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-sync",
                "--hide-scrollbars",
                "--force-device-scale-factor=1",
                "--disable-features=Translate,MediaRouter",
                &format!("--window-size={w},{h}"),
                &format!("--remote-debugging-port={port}"),
                &format!("--user-data-dir={profile}"),
                url,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("启动浏览器失败：{e}"))?;

        // 后台抽干 stdout（否则管道写满会让浏览器卡住）
        if let Some(out) = child.stdout.take() {
            std::thread::spawn(move || {
                let r = BufReader::new(out);
                for _ in r.lines().map_while(Result::ok) {}
            });
        }
        Ok(Browser { child, port })
    }

    /// 轮询 CDP 的 `/json/list`，返回 page target 的 WebSocket URL。
    pub fn page_ws_url(&self) -> Result<String, String> {
        page_ws_url(self.port)
    }

    pub fn kill(&mut self) {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &self.child.id().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 一个 CDP WebSocket 会话。
pub struct Cdp {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    next_id: u64,
}

impl Cdp {
    pub async fn connect(ws_url: &str) -> Result<Cdp, String> {
        Self::connect_timeout(ws_url, CDP_TIMEOUT).await
    }

    async fn connect_timeout(ws_url: &str, timeout: Duration) -> Result<Cdp, String> {
        let deadline = tokio::time::Instant::now() + timeout;
        let (ws, _) = tokio::time::timeout_at(deadline, tokio_tungstenite::connect_async(ws_url))
            .await
            .map_err(|_| format!("连接 CDP {ws_url} 超时（{timeout:?}）"))?
            .map_err(|e| format!("连接 CDP {ws_url} 失败：{e}"))?;
        Ok(Cdp { ws, next_id: 0 })
    }

    pub async fn cmd(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.cmd_until(method, params, tokio::time::Instant::now() + CDP_TIMEOUT).await
    }

    async fn cmd_until(
        &mut self,
        method: &str,
        params: Value,
        deadline: tokio::time::Instant,
    ) -> Result<u64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "id": id, "method": method, "params": params });
        tokio::time::timeout_at(deadline, self.ws.send(Message::Text(msg.to_string().into())))
            .await
            .map_err(|_| format!("CDP 发送 {method}（id={id}）超时"))?
            .map_err(|e| format!("CDP 发送 {method}（id={id}）失败：{e}"))?;
        Ok(id)
    }

    /// 发一条命令并等它的响应（忽略事件）。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call_timeout(method, params, CDP_TIMEOUT).await
    }

    async fn call_timeout(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let deadline = tokio::time::Instant::now() + timeout;
        // 发送与整个响应循环共用 deadline；持续收到事件也不能延长命令的总等待时间。
        tokio::time::timeout_at(deadline, async {
            let id = self.cmd_until(method, params, deadline).await?;
            loop {
                let msg = self.recv().await.ok_or_else(|| format!("CDP {method}（id={id}）连接断开"))?;
                if msg.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    if let Some(err) = msg.get("error") {
                        return Err(format!("{method} 失败：{err}"));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
            }
        })
        .await
        .map_err(|_| format!("CDP 调用 {method} 超时（{timeout:?}）"))?
    }

    /// 收一条消息。
    pub async fn recv(&mut self) -> Option<Value> {
        loop {
            let msg = self.ws.next().await?;
            match msg {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        return Some(v);
                    }
                }
                Ok(Message::Binary(b)) => {
                    if let Ok(v) = serde_json::from_slice::<Value>(&b) {
                        return Some(v);
                    }
                }
                Ok(Message::Close(_)) => return None,
                Err(_) => return None,
                _ => {}
            }
        }
    }
}

/// PNG → RGB565 大端（直接截断）。
pub fn png_to_rgb565(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().map_err(|e| format!("PNG 解析失败：{e}"))?;
    let (iw, ih) = {
        let i = reader.info();
        (i.width, i.height)
    };
    let mut buf = vec![0u8; (iw as usize * ih as usize * 4).max(4)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG 解码失败：{e}"))?;
    let (w, h) = (info.width, info.height);
    let data = &buf[..info.buffer_size()];
    // 预分配 + 直接写大端 u16：省掉逐像素 extend_from_slice 的容量检查与调用开销
    let mut out = vec![0u8; w as usize * h as usize * 2];
    match info.color_type {
        png::ColorType::Rgba => {
            for (i, px) in data.chunks_exact(4).enumerate() {
                write565(&mut out, i, px[0], px[1], px[2]);
            }
        }
        png::ColorType::Rgb => {
            for (i, px) in data.chunks_exact(3).enumerate() {
                write565(&mut out, i, px[0], px[1], px[2]);
            }
        }
        _ => return Err(format!("不支持的 PNG 颜色类型：{:?}", info.color_type)),
    }
    Ok((out, w, h))
}

#[inline(always)]
fn write565(out: &mut [u8], i: usize, r: u8, g: u8, b: u8) {
    let v = ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3);
    out[i * 2] = (v >> 8) as u8;
    out[i * 2 + 1] = v as u8;
}

#[allow(dead_code)] // 像素打包工具，暂未接入
#[inline]
fn pack565(r: u8, g: u8, b: u8) -> [u8; 2] {
    let v = ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3);
    v.to_be_bytes()
}

/// screencast 负载的 base64 → **原始 PNG 字节**。
///
/// 单独暴露一步解码：预览要的是原始 PNG（走 `Channel` 二进制推给前端），
/// 面板要的是 RGB565，两边共用这一次 base64 解码，不重复解。
pub fn decode_b64(data_b64: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| format!("base64 解码失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    const TEST_TIMEOUT: Duration = Duration::from_millis(100);
    const STALL: Duration = Duration::from_millis(600);

    // 只绑定系统分配的 loopback 临时端口；模拟端也有超时，不会因测试失败永久挂住。
    fn http_server(serve: impl FnOnce(TcpStream) + Send + 'static) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(start.elapsed() < Duration::from_secs(3), "模拟服务器未收到连接");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("模拟服务器 accept 失败：{e}"),
                }
            };
            // Windows 接受的 socket 可能继承 listener 的非阻塞状态，读超时并不会清除它。
            // 明确恢复阻塞模式，否则请求尚未到达时 read 会偶发 WouldBlock，假装客户端提前失败。
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            serve(stream);
        });
        (port, thread)
    }

    fn json_response(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    async fn accept_ws(listener: tokio::net::TcpListener) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        tokio::time::timeout(Duration::from_secs(2), async {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        }).await.unwrap()
    }

    fn assert_deadline(elapsed: Duration) {
        // Windows 定时器与繁忙 CI 留足容差，但必须明显早于模拟端的 600ms 停顿。
        assert!(elapsed >= Duration::from_millis(50), "过早返回：{elapsed:?}");
        assert!(elapsed < Duration::from_millis(500), "deadline 未生效：{elapsed:?}");
    }

    #[test]
    fn page_poll_bounds_slow_headers_and_body() {
        for slow_body in [false, true] {
            let (port, server) = http_server(move |mut stream| {
                if slow_body {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[").unwrap();
                }
                std::thread::sleep(STALL);
                let _ = stream.write_all(if slow_body { b"]" } else { b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]" });
            });
            let start = Instant::now();
            let err = page_ws_url_matching_timeout(port, "panel", TEST_TIMEOUT).unwrap_err();
            assert_deadline(start.elapsed());
            assert!(err.contains(&port.to_string()) && err.contains("panel"), "{err}");
            assert!(err.contains("最近失败：HTTP"), "{err}");
            server.join().unwrap();
        }
    }

    #[test]
    fn page_poll_global_deadline_survives_body_progress() {
        let (port, server) = http_server(|mut stream| {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n[").unwrap();
            let start = Instant::now();
            while start.elapsed() < STALL {
                if stream.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let start = Instant::now();
        let err = page_ws_url_timeout(port, None, TEST_TIMEOUT).unwrap_err();
        assert_deadline(start.elapsed());
        assert!(err.contains("HTTP 读取失败"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn page_poll_preserves_matching_and_first_page_semantics() {
        let body = json!([
            { "type": "worker", "url": "panel", "webSocketDebuggerUrl": "ws://worker" },
            { "type": "page", "url": "ui", "webSocketDebuggerUrl": "ws://ui" },
            { "type": "page", "url": "panel/no-ws" },
            { "type": "page", "url": "prefix/panel/suffix", "webSocketDebuggerUrl": "ws://panel" }
        ]).to_string();
        for (needle, expected) in [(None, "ws://ui"), (Some("panel"), "ws://panel"), (Some(""), "ws://ui")] {
            let response = json_response(&body);
            let (port, server) = http_server(move |mut stream| stream.write_all(response.as_bytes()).unwrap());
            assert_eq!(page_ws_url_timeout(port, needle, TEST_TIMEOUT).unwrap(), expected);
            server.join().unwrap();
        }
    }

    #[test]
    fn page_poll_retains_last_failure_and_caps_sleep() {
        let response = json_response("invalid json");
        let (port, server) = http_server(move |mut stream| stream.write_all(response.as_bytes()).unwrap());
        let start = Instant::now();
        let err = page_ws_url_timeout(port, None, Duration::from_millis(30)).unwrap_err();
        assert!(start.elapsed() < Duration::from_millis(300));
        assert!(err.contains("JSON 解析失败") && err.contains(&port.to_string()), "{err}");
        server.join().unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let err = page_ws_url_timeout(listener.local_addr().unwrap().port(), None, Duration::ZERO).unwrap_err();
        assert!(err.contains("尚未发起请求"));
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn probe_and_close_bound_slow_http() {
        for close in [false, true] {
            let (port, server) = http_server(|_stream| std::thread::sleep(STALL));
            let start = Instant::now();
            let result = if close {
                tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
                    .block_on(close_browser_timeout(port, TEST_TIMEOUT))
            } else {
                cdp_alive_timeout(port, TEST_TIMEOUT)
            };
            assert!(!result);
            assert_deadline(start.elapsed());
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn connect_bounds_stalled_websocket_handshake() {
        let (port, server) = http_server(|_stream| std::thread::sleep(STALL));
        let url = format!("ws://127.0.0.1:{port}/devtools/page/test");
        let start = Instant::now();
        let err = Cdp::connect_timeout(&url, TEST_TIMEOUT).await.err().unwrap();
        assert_deadline(start.elapsed());
        assert!(err.contains("超时") && err.contains(&url), "{err}");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn close_bounds_stalled_websocket_handshake() {
        let (ws_port, ws_server) = http_server(|_stream| std::thread::sleep(STALL));
        let response = json_response(&json!({
            "webSocketDebuggerUrl": format!("ws://127.0.0.1:{ws_port}/devtools/browser/test")
        }).to_string());
        let (port, server) = http_server(move |mut stream| stream.write_all(response.as_bytes()).unwrap());
        let start = Instant::now();
        assert!(!close_browser_timeout(port, TEST_TIMEOUT).await);
        assert_deadline(start.elapsed());
        server.join().unwrap();
        ws_server.join().unwrap();
    }

    #[tokio::test]
    async fn close_shares_http_and_websocket_deadline() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let ws_port = listener.local_addr().unwrap().port();
        let ws_server = tokio::spawn(async move {
            if let Ok(Ok((stream, _))) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await {
                tokio::time::sleep(Duration::from_millis(70)).await;
                let _ = tokio::time::timeout(Duration::from_secs(2), tokio_tungstenite::accept_async(stream)).await;
            }
        });
        let response = json_response(&json!({
            "webSocketDebuggerUrl": format!("ws://127.0.0.1:{ws_port}/devtools/browser/test")
        }).to_string());
        let (port, server) = http_server(move |mut stream| {
            std::thread::sleep(Duration::from_millis(70));
            stream.write_all(response.as_bytes()).unwrap();
        });
        let start = Instant::now();
        assert!(!close_browser_timeout(port, TEST_TIMEOUT).await);
        assert_deadline(start.elapsed());
        server.join().unwrap();
        ws_server.await.unwrap();
    }

    #[tokio::test]
    async fn cmd_bounds_websocket_backpressure() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("ws://{}/devtools/page/test", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let _ws = accept_ws(listener).await;
            // 握手后不再读，填满 loopback 的 TCP 缓冲区，强制 send 等待可写。
            tokio::time::sleep(STALL).await;
        });
        let mut cdp = Cdp::connect_timeout(&url, Duration::from_secs(2)).await.unwrap();
        let params = json!({ "data": "x".repeat(512 * 1024) });
        let start = Instant::now();
        let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
        let mut error = None;
        for _ in 0..64 {
            if let Err(e) = cdp.cmd_until("Test.backpressure", params.clone(), deadline).await {
                error = Some(e);
                break;
            }
        }
        let err = error.expect("发送缓冲区应在模拟端停读时产生背压");
        assert_deadline(start.elapsed());
        assert!(err.contains("Test.backpressure") && err.contains("超时"), "{err}");
        drop(cdp);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn call_deadline_is_not_reset_by_events() {
        for events in [false, true] {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let url = format!("ws://{}/devtools/page/test", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let mut ws = accept_ws(listener).await;
                assert!(ws.next().await.unwrap().is_ok());
                if events {
                    let start = Instant::now();
                    while start.elapsed() < STALL {
                        if ws.send(Message::Text(r#"{"method":"Page.screencastFrame","params":{}}"#.into())).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                } else {
                    tokio::time::sleep(STALL).await;
                }
            });
            let mut cdp = Cdp::connect_timeout(&url, Duration::from_secs(2)).await.unwrap();
            let start = Instant::now();
            let err = cdp.call_timeout("Runtime.evaluate", json!({}), TEST_TIMEOUT).await.unwrap_err();
            assert_deadline(start.elapsed());
            assert!(err.contains("Runtime.evaluate") && err.contains("超时"), "{err}");
            drop(cdp);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn call_ignores_events_and_other_command_responses() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("ws://{}/devtools/page/test", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept_ws(listener).await;
            let command = ws.next().await.unwrap().unwrap();
            let command: Value = serde_json::from_str(command.to_text().unwrap()).unwrap();
            for message in [
                json!({ "method": "Page.loadEventFired" }),
                json!({ "id": 999, "result": {} }),
                json!({ "id": command["id"], "result": { "value": 42 } }),
            ] {
                ws.send(Message::Text(message.to_string().into())).await.unwrap();
            }
        });
        let mut cdp = Cdp::connect_timeout(&url, Duration::from_secs(2)).await.unwrap();
        let result = cdp.call_timeout("Runtime.evaluate", json!({}), Duration::from_secs(2)).await.unwrap();
        assert_eq!(result, json!({ "value": 42 }));
        server.await.unwrap();
    }
}

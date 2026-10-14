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
    let list = format!("http://127.0.0.1:{port}/json/list");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if let Ok(resp) = ureq::get(&list).call() {
            if let Ok(text) = resp.into_body().read_to_string() {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(arr) = v.as_array() {
                        for t in arr {
                            if t.get("type").and_then(|x| x.as_str()) != Some("page") {
                                continue;
                            }
                            let u = t.get("url").and_then(|x| x.as_str()).unwrap_or("");
                            if u.contains(needle) {
                                if let Some(ws) =
                                    t.get("webSocketDebuggerUrl").and_then(|x| x.as_str())
                                {
                                    return Ok(ws.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("找不到含「{needle}」的页面目标（离屏 webview 未就绪）"))
}

/// CDP 端口上是否已有浏览器在服务（用于复用，省掉冷启动）。
#[allow(dead_code)] // 端口探活（复用浏览器时用），暂未接入
pub fn cdp_alive(port: u16) -> bool {
    ureq::get(&format!("http://127.0.0.1:{port}/json/version"))
        .call()
        .is_ok()
}

/// 轮询 CDP `/json/list`，返回 page target 的 WebSocket URL。
pub fn page_ws_url(port: u16) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if let Ok(resp) = ureq::get(&url).call() {
            if let Ok(text) = resp.into_body().read_to_string() {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(arr) = v.as_array() {
                        for t in arr {
                            if t.get("type").and_then(|x| x.as_str()) == Some("page") {
                                if let Some(ws) = t.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) {
                                    return Ok(ws.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("找不到 page target（浏览器未就绪）".into())
}

/// 让浏览器干净退出（CDP `Browser.close`）。
///
/// 为什么不用 taskkill：`msedge.exe` 只是启动器桩，把窗口交给已有实例后自己就退了，
/// 杀这个 PID 杀不到真身 —— 每轮漏一个 Edge 进程（实测漏了 8 个）。
pub async fn close_browser(port: u16) -> bool {
    let Ok(resp) = ureq::get(&format!("http://127.0.0.1:{port}/json/version")).call() else {
        return false;
    };
    let Ok(text) = resp.into_body().read_to_string() else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(ws) = v.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) else {
        return false;
    };
    let ws = ws.to_string();
    match tokio_tungstenite::connect_async(&ws).await {
        Ok((mut s, _)) => s
            .send(Message::Text(r#"{"id":1,"method":"Browser.close"}"#.into()))
            .await
            .is_ok(),
        Err(_) => false,
    }
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
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| format!("连接 CDP 失败：{e}"))?;
        Ok(Cdp { ws, next_id: 0 })
    }

    pub async fn cmd(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "id": id, "method": method, "params": params });
        self.ws
            .send(Message::Text(msg.to_string().into()))
            .await
            .map_err(|e| format!("CDP 发送失败：{e}"))?;
        Ok(id)
    }

    /// 发一条命令并等它的响应（忽略事件）。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.cmd(method, params).await?;
        loop {
            let msg = self.recv().await.ok_or("CDP 连接断开")?;
            if msg.get("id").and_then(|i| i.as_u64()) == Some(id) {
                if let Some(err) = msg.get("error") {
                    return Err(format!("{method} 失败：{err}"));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
        }
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

/// 解码 screencast 帧的 base64 PNG → RGB565。
pub fn decode_screencast_frame(data_b64: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let png = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| format!("base64 解码失败：{e}"))?;
    png_to_rgb565(&png)
}

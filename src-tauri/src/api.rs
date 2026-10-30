//! 本机数据 API —— **只监听回环地址，不做鉴权**。
//!
//! 目的：把后端已有的数据统一用一套只读 HTTP 接口暴露出来，本机任何程序
//! （面板素材、调试脚本、第三方工具）都能直接取，不必各自去开 USB / 读共享内存 / 解析日志。
//!
//! **为什么可以免鉴权**：只 bind 回环地址（IPv4 `127.0.0.1` + IPv6 `::1`），外网根本
//! 连不上；且接口基本只读，唯二的两个 POST 只作用于本程序自身（唤起窗口 / 退出）。
//!
//! **不写死 `127.0.0.1`**：`localhost` 在双栈机器上可能优先解析成 `::1`，只绑 IPv4 会让
//! `localhost` / `[::1]` 打不开；所以 IPv4 与 IPv6 回环**各绑一个监听**，两种写法都能连。
//!
//! 端点（`GET /` 返回同一份自描述索引）：
//! - `GET  /`          → 接口索引
//! - `GET  /version`   → `{ "version": "x.y.z" }`
//! - `GET  /health`    → `ok`
//! - `GET  /metrics`   → `BasicMetric[]`（23 项基础指标，规范单位）
//! - `GET  /units`     → `{ mode, units }`
//! - `GET  /settings`  → 全部设置项
//! - `GET  /devices`   → 设备列表（本地 WinUSB 枚举 + 上位机富化）
//! - `GET  /link`      → `{ open, ready, serial }`
//! - `GET  /themes`    → 素材主题列表
//! - `POST /app/show`  → 唤起主窗口
//! - `POST /app/quit`  → 归还设备后优雅退出
//!
//! 面板是 `file://` 页面（origin 为 null），所以响应统一带 `Access-Control-Allow-Origin: *`。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

/// 本机数据 API 端口。
pub const PORT: u16 = 8766;

/// IPv4 回环是否绑定成功。
static BOUND_V4: AtomicBool = AtomicBool::new(false);
/// IPv6 回环是否绑定成功。
static BOUND_V6: AtomicBool = AtomicBool::new(false);

/// 回环候选地址：IPv4 `127.0.0.1`、IPv6 `::1`，外加 `localhost` 的解析结果（去重）。
///
/// 绑定、探测（单实例唤醒）都走这一份，避免任何一处写死 `127.0.0.1`：
/// 只写 IPv4 的话，`localhost` 优先解析成 `::1` 的机器会连不上；反之亦然。
pub fn loopback_addrs(port: u16) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = Vec::with_capacity(4);
    push_unique(&mut v, SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    push_unique(&mut v, SocketAddr::from((Ipv6Addr::LOCALHOST, port)));
    // `localhost` 理论上就是上面两个之一，但 hosts 文件可能被改过，解析结果一并纳入。
    if let Ok(it) = ("localhost", port).to_socket_addrs() {
        for a in it {
            push_unique(&mut v, a);
        }
    }
    v
}

fn push_unique(v: &mut Vec<SocketAddr>, a: SocketAddr) {
    if !v.contains(&a) {
        v.push(a);
    }
}

/// 服务基址（`http://127.0.0.1:8766` / `http://[::1]:8766`），取**实际在听的那个**。
///
/// 给注入脚本等自用方用：它们要拼 URL，又不该假设回环一定在 IPv4 上。
pub fn base_url() -> String {
    if BOUND_V4.load(Ordering::SeqCst) {
        format!("http://127.0.0.1:{PORT}")
    } else if BOUND_V6.load(Ordering::SeqCst) {
        format!("http://[::1]:{PORT}")
    } else {
        // 还没绑上（或都失败）：给 IPv4 这个最常见的写法，调用方照旧会失败得可预期。
        format!("http://127.0.0.1:{PORT}")
    }
}

/// 多条监听之间的串行闸门：保持「同一时刻只处理一个请求」的原有语义。
/// `/devices` 这类会碰 USB 的接口最坏要等控制管道超时，不希望两个地址同时打进来。
static DISPATCH: Mutex<()> = Mutex::new(());

const JSON: &str = "application/json; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";

pub fn spawn() {
    thread::spawn(|| {
        // IPv4 与 IPv6 回环是两个不同的 socket（都不是通配地址），互不冲突。
        // - IPv4 失败 → 端口被别的程序占了，或落在系统保留段里；
        // - IPv6 失败 → 系统禁用了 IPv6 栈，降级成「只有 IPv4」，不算错误。
        let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, PORT));

        // 绑定失败不立刻放弃：8766 有可能被 Hyper-V / WSL 的动态端口段临时保留
        // （`netsh int ipv4 show excludedportrange protocol=tcp` 能查到，报 WSAEACCES=10013），
        // 重启 winnat 或重启机器后这一段会变。每 5 秒重试一次，端口一空出来就自动恢复，
        // 用户不用为了「面板没数据」去重启程序。
        let mut warned = false;
        loop {
            let l4 = match TcpListener::bind(v4) {
                Ok(l) => {
                    BOUND_V4.store(true, Ordering::SeqCst);
                    Some(l)
                }
                Err(e) => {
                    if !warned {
                        crate::logging::warn(&format!("本机 API 绑定 {v4} 失败：{e}"));
                    }
                    None
                }
            };
            let l6 = match TcpListener::bind(v6) {
                Ok(l) => {
                    BOUND_V6.store(true, Ordering::SeqCst);
                    Some(l)
                }
                Err(e) => {
                    // 系统禁用 IPv6 时这是常态，且已经有 IPv4 在服务 → 属设计内的降级，记 info。
                    if l4.is_some() {
                        crate::logging::info(&format!(
                            "本机 API 绑定 {v6} 失败（IPv6 可能被禁用，降级为仅 IPv4）：{e}"
                        ));
                    }
                    None
                }
            };

            if l4.is_none() && l6.is_none() {
                if !warned {
                    crate::logging::error(
                        "本机数据 API 未启动：回环地址全部绑定失败。\
                         若错误码是 10013（WSAEACCES），说明端口被系统保留段占用——\
                         常见于装了 Hyper-V / WSL：管理员执行 `net stop winnat && net start winnat` \
                         或重启电脑即可释放；程序每 5 秒会自动重试。",
                    );
                    warned = true;
                }
                thread::sleep(Duration::from_secs(5));
                continue;
            }
            warned = false;

            crate::logging::info(&format!(
                "本机数据 API: {}/  （127.0.0.1 / localhost / [::1] 均可访问）",
                base_url()
            ));
            let mut handles = Vec::with_capacity(2);
            if let Some(l) = l4 {
                handles.push(thread::spawn(move || serve(l)));
            }
            if let Some(l) = l6 {
                handles.push(thread::spawn(move || serve(l)));
            }
            for h in handles {
                let _ = h.join();
            }
            // 服务线程都退出了（正常情况下不会）：等一会儿再重来，别空转。
            thread::sleep(Duration::from_secs(5));
        }
    });
}

/// 单条监听串行 accept。
fn serve(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let _ = handle(stream);
    }
}

fn handle(mut s: TcpStream) -> std::io::Result<()> {
    // 本地空连接不能永久占用串行 API 线程；不改变指标采样或并发模型。
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    s.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = [0u8; 4096];
    let n = s.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut first = req.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("GET").to_string();
    let path = first.next().unwrap_or("/").to_string();
    let route = path.split('?').next().unwrap_or("/");

    let (status, ctype, body) = {
        let _serial = DISPATCH.lock().unwrap_or_else(|e| e.into_inner());
        dispatch(&method, route)
    };

    let head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ctype}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: *\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.as_bytes().len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body.as_bytes())?;
    s.flush()
}

fn dispatch(method: &str, route: &str) -> (&'static str, &'static str, String) {
    // 跨域预检（面板 file:// 直接 fetch 用不到，但第三方网页工具会发）
    if method == "OPTIONS" {
        return ("204 No Content", TEXT, String::new());
    }
    match (method, route) {
        ("GET", "/") => ("200 OK", JSON, index()),
        ("GET", "/version") => (
            "200 OK",
            JSON,
            serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }).to_string(),
        ),
        ("GET", "/health") => ("200 OK", TEXT, "ok".to_string()),
        ("GET", "/metrics") => (
            "200 OK",
            JSON,
            serde_json::to_string(&crate::metrics::basic()).unwrap_or_else(|_| "[]".into()),
        ),
        ("GET", "/units") => ("200 OK", JSON, crate::units::json()),
        ("GET", "/settings") => (
            "200 OK",
            JSON,
            crate::settings::to_json(&crate::settings::current()).to_string(),
        ),
        ("GET", "/devices") => match crate::device_list_blocking() {
            Ok(v) => ("200 OK", JSON, serde_json::Value::Array(v).to_string()),
            Err(e) => (
                "500 Internal Server Error",
                JSON,
                serde_json::json!({ "error": e }).to_string(),
            ),
        },
        ("GET", "/link") => (
            "200 OK",
            JSON,
            serde_json::json!({
                "open": crate::pipeline::link_open(),
                "ready": crate::pipeline::link_ready(),
                "serial": crate::pipeline::link_serial(),
            })
            .to_string(),
        ),
        ("GET", "/themes") => (
            "200 OK",
            JSON,
            serde_json::to_string(&crate::assets::list_themes()).unwrap_or_else(|_| "[]".into()),
        ),
        // 外设电量：品牌 / 型号 / 电量 / 充电状态。数据来自后台轮询线程的快照。
        ("GET", "/peripherals") => ("200 OK", JSON, crate::peripherals::json().to_string()),
        // 触发一次立刻重新读盘并轮询（协议 JSON 改完不用重启程序）。
        ("POST", "/peripherals/refresh") => {
            crate::peripherals::refresh();
            ("200 OK", TEXT, "refreshing".to_string())
        }
        // 兼容旧调用：以前不区分方法，GET 也能唤起 / 退出
        (_, "/app/show") => {
            crate::show_main_window();
            ("200 OK", TEXT, "ok".to_string())
        }
        (_, "/app/quit") => {
            crate::request_quit();
            ("200 OK", TEXT, "quitting".to_string())
        }
        _ => ("404 Not Found", TEXT, "not found".to_string()),
    }
}

/// 自描述索引：让别的程序（或人）一眼看出有哪些数据可取。
fn index() -> String {
    serde_json::json!({
        "name": "jiadian-monitor local data API",
        "version": env!("CARGO_PKG_VERSION"),
        "note": "只监听回环地址（127.0.0.1 / [::1]，localhost 亦可），仅本机可访问，因此不做鉴权",
        "base": base_url(),
        "endpoints": [
            { "method": "GET",  "path": "/",         "desc": "本索引" },
            { "method": "GET",  "path": "/version",  "desc": "版本号" },
            { "method": "GET",  "path": "/health",   "desc": "存活检查（返回 ok）" },
            { "method": "GET",  "path": "/metrics",  "desc": "23 项基础指标（规范单位）" },
            { "method": "GET",  "path": "/units",    "desc": "单位设置 { mode, units }" },
            { "method": "GET",  "path": "/settings", "desc": "全部设置项" },
            { "method": "GET",  "path": "/devices",  "desc": "设备列表（本地枚举 + 上位机富化）" },
            { "method": "GET",  "path": "/link",     "desc": "USB 接管状态 { open, ready, serial }" },
            { "method": "GET",  "path": "/themes",   "desc": "素材主题列表" },
            { "method": "GET",  "path": "/peripherals", "desc": "外设（鼠标/键盘/耳机）显示名、型号、电量、休眠状态" },
            { "method": "POST", "path": "/peripherals/refresh", "desc": "重新读协议 JSON 并立刻轮询一轮" },
            { "method": "POST", "path": "/app/show", "desc": "唤起主窗口" },
            { "method": "POST", "path": "/app/quit", "desc": "归还设备后退出" }
        ]
    })
    .to_string()
}

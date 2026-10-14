//! 本地 HTTP API 客户端：`/usb/status` 与 `/usb/handoff/*`（GUI 在 `127.0.0.1:8765`）。
//!
//! 移交（handoff）让 GUI 暂时让出 USB，本 app 因此可与 GUI 共存。
//! 租约默认 90s，须每 ~20s keepalive；退出前必须 `finish`。

use serde_json::{json, Value};
use std::time::Duration;

const BASE: &str = "http://127.0.0.1:8765";

/// 单次请求超时。**必须设**：ureq 默认不设全局超时，GUI 卡死/无响应时
/// `gui_running()` / `handoff_*` 会一直阻塞 —— 而 `UsbLink::open` 是**持着 OPEN_LOCK** 调它们的，
/// 一旦挂住，设备操作（切设备 / 读信息 / 下发）全部跟着卡死。本地回环 4s 足够。
const TIMEOUT: Duration = Duration::from_secs(4);

fn post(path: &str, payload: Value) -> Result<Value, String> {
    let req = ureq::post(&format!("{BASE}{path}"))
        .config()
        .timeout_global(Some(TIMEOUT))
        .build();
    let resp = req
        .send_json(payload)
        .map_err(|e| format!("{path} 请求失败：{e}"))?;
    let text = resp
        .into_body()
        .read_to_string()
        .map_err(|e| format!("{path} 读响应失败：{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path} 解析失败：{e}（{text}）"))
}

/// GUI 是否在运行（`/usb/status` 返回 `guiRunning=true`）。
pub fn gui_running() -> bool {
    post("/usb/status", json!({}))
        .map(|v| v.get("guiRunning").and_then(|b| b.as_bool()).unwrap_or(false))
        .unwrap_or(false)
}

pub fn status() -> Result<Value, String> {
    post("/usb/status", json!({}))
}

pub fn handoff_start(serial: &str, client_id: &str, ttl_seconds: u32) -> Result<Value, String> {
    post(
        "/usb/handoff/start",
        json!({ "serial": serial, "clientId": client_id, "ttlSeconds": ttl_seconds }),
    )
}

pub fn handoff_keepalive(lease_id: &str, serial: &str) -> Result<Value, String> {
    post(
        "/usb/handoff/keepalive",
        json!({ "leaseId": lease_id, "serial": serial }),
    )
}

pub fn handoff_finish(lease_id: &str, serial: &str) -> Result<Value, String> {
    post(
        "/usb/handoff/finish",
        json!({ "leaseId": lease_id, "serial": serial }),
    )
}

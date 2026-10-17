//! Peripheral config commands: read/write peripherals.json, list detected devices.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralEntry {
    #[serde(with = "hex_vidpid")]
    pub vid: u16,
    #[serde(with = "hex_vidpid")]
    pub pid: u16,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub label: String,
}

fn default_true() -> bool { true }

mod hex_vidpid {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u16, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:04x}", v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
        let s = String::deserialize(d)?;
        u16::from_str_radix(&s, 16).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeripheralConfig {
    #[serde(default)]
    pub devices: Vec<PeripheralEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectedDevice {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub product_string: String,
    pub supported: bool,
    pub provider: Option<String>,
}

fn config_path() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.join("peripherals.json"))
}

#[tauri::command]
pub fn peripherals_get() -> PeripheralConfig {
    let Some(path) = config_path() else { return PeripheralConfig::default() };
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => PeripheralConfig::default(),
    }
}

#[tauri::command]
pub fn peripherals_set(cfg: PeripheralConfig) -> Result<(), String> {
    let Some(path) = config_path() else {
        return Err("无法定位配置文件".into());
    };
    let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn mouse_battery_restart() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no parent")?;
    let mb = dir.join("mouse-battery.exe");
    // Kill existing
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "mouse-battery.exe"])
        .creation_flags(0x08000000)
        .output();
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Start detached, hidden
    std::process::Command::new(&mb)
        .current_dir(dir)
        .creation_flags(0x08000000 | 0x00000200) // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn peripherals_devices() -> Vec<DetectedDevice> {
    // Proxy to mouse-battery.exe /devices using std TcpStream
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;
    let Ok(mut stream) = TcpStream::connect_timeout(
        &"127.0.0.1:8767".parse().unwrap(),
        Duration::from_secs(2),
    ) else { return Vec::new() };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let _ = stream.write_all(b"GET /devices HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).is_err() { return Vec::new() };
    let text = String::from_utf8_lossy(&buf);
    let Some(body) = text.split("\r\n\r\n").nth(1) else { return Vec::new() };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let arr = match val.as_array() { Some(a) => a, None => return Vec::new() };
    arr.iter().filter_map(|d| {
        Some(DetectedDevice {
            vid: d.get("vid")?.as_u64()? as u16,
            pid: d.get("pid")?.as_u64()? as u16,
            usage_page: d.get("usage_page")?.as_u64()? as u16,
            usage: d.get("usage")?.as_u64()? as u16,
            product_string: d.get("product_string")?.as_str().unwrap_or("").to_string(),
            supported: d.get("supported")?.as_bool().unwrap_or(false),
            provider: d.get("provider")?.as_str().map(|s| s.to_string()),
        })
    }).collect()
}

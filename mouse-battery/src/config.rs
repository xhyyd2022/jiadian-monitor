//! Peripheral config: which devices to show, in what order, with custom labels.
//!
//! Reads `peripherals.json` next to the exe. If the file doesn't exist,
//! creates a default one with all currently detected supported devices.
//!
//! Format:
//! ```json
//! {
//!   "devices": [
//!     { "vid": "3554", "pid": "fb2a", "enabled": true, "label": "Akko" },
//!     { "vid": "1915", "pid": "ae8a", "enabled": true, "label": "Sora V2" }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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

/// Custom serde: VID/PID as 4-digit hex string.
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

fn config_path() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.join("peripherals.json"))
}

impl PeripheralConfig {
    /// Load config from disk. If missing, returns default (empty = auto-detect all).
    pub fn load() -> Self {
        let Some(path) = config_path() else { return Self::default() };
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save config to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = config_path() else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "no exe path"));
        };
        let text = serde_json::to_string_pretty(self).unwrap();
        fs::write(&path, text)
    }

    /// Returns true if (vid, pid) is enabled in the config.
    /// If config has no devices (empty), treat all as enabled.
    pub fn is_enabled(&self, vid: u16, pid: u16) -> bool {
        if self.devices.is_empty() {
            return true;
        }
        self.devices.iter().find(|d| d.vid == vid && d.pid == pid).map(|d| d.enabled).unwrap_or(false)
    }

    /// Get the custom label for a device, if set.
    pub fn label_for(&self, vid: u16, pid: u16) -> Option<String> {
        self.devices.iter().find(|d| d.vid == vid && d.pid == pid)
            .map(|d| d.label.clone())
            .filter(|s| !s.is_empty())
    }

    /// Return the ordered list of enabled (vid, pid) pairs.
    /// If config is empty, returns empty (caller should use detection order).
    pub fn ordered_enabled(&self) -> Vec<(u16, u16)> {
        self.devices.iter().filter(|d| d.enabled).map(|d| (d.vid, d.pid)).collect()
    }

    /// Auto-generate a config from detected devices. Writes to disk.
    pub fn auto_generate(detected: &[(u16, u16, &str)]) -> Self {
        let cfg = Self {
            devices: detected.iter().map(|(vid, pid, product)| PeripheralEntry {
                vid: *vid,
                pid: *pid,
                enabled: true,
                label: product.to_string(),
            }).collect(),
        };
        let _ = cfg.save();
        cfg
    }
}

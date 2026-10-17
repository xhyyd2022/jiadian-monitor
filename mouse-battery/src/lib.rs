//! Plugin-based wireless peripheral battery reader.
//!
//! # Architecture
//! Each vendor protocol implements [`MouseBattery`]. The [`Monitor`] enumerates
//! HID devices, asks each provider if it matches, then periodically opens
//! **every** matching device and caches the latest reading per device.
//!
//! # Adding a new protocol
//! 1. Create a new module (e.g. `src/logitech.rs`).
//! 2. Implement [`MouseBattery`] for a unit struct.
//! 3. Register it in [`builtin_providers`].

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::ffi::{CStr, CString};

use hidapi::HidApi;
use serde::Serialize;
use thiserror::Error;

pub mod brands;
pub mod compx;
pub mod config;
pub mod ninjutso;
pub mod server;

use config::PeripheralConfig;

/// Result of one battery read.
#[derive(Debug, Clone, Serialize)]
pub struct BatteryReading {
    /// Provider name that produced this reading (e.g. `"comps"`).
    pub provider: &'static str,
    /// Human-readable device label (e.g. `"Akko 8K Receiver"`).
    pub device_label: String,
    /// Stable device key (VID:PID:usagePage:usage:index).
    pub device_key: String,
    /// Battery percentage 0–100.
    pub level_pct: u8,
    /// True when the peripheral is on the charger.
    pub charging: bool,
}

#[derive(Debug, Error)]
pub enum BatteryError {
    #[error("HID error: {0}")]
    Hid(#[from] hidapi::HidError),
    #[error("no matching provider for device")]
    NoProvider,
    #[error("invalid reply from device")]
    InvalidReply,
    #[error("device did not reply in time")]
    Timeout,
}

/// A vendor-specific battery protocol.
pub trait MouseBattery: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, vid: u16, pid: u16, usage_page: u16, usage: u16) -> bool;
    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError>;
}

/// Build the default set of providers.
pub fn builtin_providers() -> Vec<Box<dyn MouseBattery>> {
    vec![
        Box::new(compx::Compx),
        Box::new(ninjutso::Ninjutso),
        Box::new(brands::Rapoo),
        Box::new(brands::WlMouseCube),
        Box::new(brands::Zynoo),
        Box::new(brands::VgnLd),
        Box::new(brands::ScyroxS),
        Box::new(brands::MchoseV2),
        Box::new(brands::SoraV3),
        Box::new(brands::RogStandard),
        Box::new(brands::SteelSeries),
        Box::new(brands::GWolves),
        Box::new(brands::Pulsar),
        Box::new(brands::WebBrands),
        Box::new(brands::Ajazz),
    ]
}

/// One entry in the multi-device cache.
struct CacheEntry {
    reading: BatteryReading,
    last_ok: Instant,
}

/// Enumerated HID device (for /devices endpoint and UI selection).
#[derive(Debug, Clone, Serialize)]
pub struct HidDeviceInfo {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub product_string: String,
    pub path: String,
    /// Whether any provider claims this device.
    pub supported: bool,
    /// Which provider claims it.
    pub provider: Option<&'static str>,
}

/// Background poller that keeps the latest battery readings for ALL matching
/// peripherals fresh.
pub struct Monitor {
    providers: Vec<Box<dyn MouseBattery>>,
    cache: RwLock<HashMap<String, CacheEntry>>,
    config: PeripheralConfig,
}

impl Monitor {
    pub fn new() -> Arc<Self> {
        Self::with_providers(builtin_providers())
    }

    pub fn with_providers(providers: Vec<Box<dyn MouseBattery>>) -> Arc<Self> {
        let config = PeripheralConfig::load();
        Arc::new(Self {
            providers,
            cache: RwLock::new(HashMap::new()),
            config,
        })
    }

    /// Snapshot all cached readings, ordered by config (never blocks on HID IO).
    pub fn snapshot_all(&self) -> Vec<BatteryReading> {
        let map = match self.cache.read() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        // Build ordered list from config, fall back to cache order.
        let ordered = self.config.ordered_enabled();
        let mut out = Vec::new();
        for (vid, pid) in &ordered {
            let key = format!("{:04x}:{:04x}", vid, pid);
            if let Some(entry) = map.get(&key) {
                out.push(entry.reading.clone());
            }
        }
        // If config is empty (first run), return all cached.
        if out.is_empty() {
            out = map.values().map(|e| e.reading.clone()).collect();
        }
        out
    }

    /// Enumerate all HID devices on the bus (for /devices endpoint).
    pub fn enumerate_devices(&self) -> Vec<HidDeviceInfo> {
        let api = match HidApi::new() {
            Ok(a) => a,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for info in api.device_list() {
            let vid = info.vendor_id();
            let pid = info.product_id();
            let up = info.usage_page();
            let u = info.usage();
            let label = info.product_string().unwrap_or("Unknown").to_string();
            let path = info.path().to_string_lossy().to_string();
            let mut supported = false;
            let mut provider_name: Option<&'static str> = None;
            for p in &self.providers {
                if p.matches(vid, pid, up, u) {
                    supported = true;
                    provider_name = Some(p.name());
                    break;
                }
            }
            out.push(HidDeviceInfo {
                vid, pid, usage_page: up, usage: u,
                product_string: label, path,
                supported, provider: provider_name,
            });
        }
        out
    }

    fn match_provider(&self, vid: u16, pid: u16, usage_page: u16, usage: u16) -> Option<&dyn MouseBattery> {
        self.providers.iter()
            .find(|p| p.matches(vid, pid, usage_page, usage))
            .map(|p| p.as_ref())
    }

    /// One polling cycle: enumerate matching devices, filter by config, read each.
    pub fn poll_once(&self) {
        let api = match HidApi::new() {
            Ok(a) => a,
            Err(_) => return,
        };

        // Collect all matching (vid, pid, path, label, provider) tuples.
        let mut matches: Vec<(u16, u16, CString, String, &dyn MouseBattery)> = Vec::new();
        for info in api.device_list() {
            let vid = info.vendor_id();
            let pid = info.product_id();
            let up = info.usage_page();
            let u = info.usage();
            let label = info.product_string().unwrap_or("Unknown HID device").to_string();
            let path = info.path().to_owned();
            if let Some(p) = self.match_provider(vid, pid, up, u) {
                // Filter by config: skip disabled devices.
                if !self.config.is_enabled(vid, pid) {
                    continue;
                }
                matches.push((vid, pid, path, label, p));
            }
        }

        // Auto-generate config on first run: if config is empty, save detected devices.
        if self.config.devices.is_empty() {
            let detected: Vec<(u16, u16, &str)> = matches.iter()
                .map(|(v, p, _, l, _)| (*v, *p, l.as_str()))
                .collect();
            if !detected.is_empty() {
                let _ = PeripheralConfig::auto_generate(&detected);
            }
        }

        // Read each matching device independently.
        let mut new_cache: HashMap<String, CacheEntry> = HashMap::new();
        for (vid, pid, path, label, provider) in &matches {
            let key = format!("{:04x}:{:04x}", vid, pid);
            // Use custom label from config if set.
            let display_label = self.config.label_for(*vid, *pid).unwrap_or_else(|| label.clone());
            match provider.read(&api, path, &display_label) {
                Ok(mut reading) => {
                    reading.device_key = key.clone();
                    new_cache.insert(key, CacheEntry {
                        reading,
                        last_ok: Instant::now(),
                    });
                }
                Err(_) => {
                    if let Ok(old) = self.cache.read() {
                        if let Some(old_entry) = old.get(&key) {
                            new_cache.insert(key, old_entry.clone());
                        }
                    }
                }
            }
        }

        if let Ok(mut c) = self.cache.write() {
            *c = new_cache;
        }
    }

    pub fn spawn(self: Arc<Self>, interval: Duration) -> MonitorHandle {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = MonitorHandle { stop: stop.clone() };
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                self.poll_once();
                let step = Duration::from_millis(100);
                let mut waited = Duration::ZERO;
                while waited < interval && !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(step);
                    waited += step;
                }
            }
        });
        handle
    }
}

impl Clone for CacheEntry {
    fn clone(&self) -> Self {
        Self {
            reading: self.reading.clone(),
            last_ok: self.last_ok,
        }
    }
}

pub struct MonitorHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl MonitorHandle {
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

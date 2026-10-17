//! Ninjutso Sora V2 battery provider.
//!
//! Reverse-engineered from USB packet analysis (antillon.tech C# reference
//! and local feature-report sweep).
//!
//! Device: VID=0x1915 PID=0xae8a, vendor collection at UP=0xffa0, U=0x0001.
//!
//! The receiver has no OUT endpoint (HID write fails with ERROR_INVALID_FUNCTION)
//! and no interrupt-IN endpoint on the vendor collection (read fails with
//! ERROR_INVALID_USER_BUFFER). All communication goes through HID Feature
//! Reports on report ID 0x05.
//!
//! Protocol:
//! - SetFeature report 0x05, byte[1]=0x15 (battery query), rest zero
//! - GetFeature report 0x05 → byte[9] = battery percentage (0-100)
//! - SetFeature report 0x05, byte[1]=0x16 (charging status query), rest zero
//! - GetFeature report 0x05 → byte[9] = charging flag (1 = charging, 0 = not)

use std::ffi::CStr;
use hidapi::{HidApi, HidDevice};

use crate::{BatteryError, BatteryReading, MouseBattery};

const VID: u16 = 0x1915;
/// All known Ninjutso Sora receiver PIDs (V2, V2 8K, wired variants).
const PIDS: &[u16] = &[0xAE8A, 0xAE8C, 0xAE1C, 0xAE11];
const USAGE_PAGE: u16 = 0xffa0;
const USAGE: u16 = 0x0001;

const REPORT_ID: u8 = 0x05;
const REPORT_LEN: usize = 32;

pub struct Ninjutso;

fn query(dev: &HidDevice, cmd: u8) -> Result<[u8; REPORT_LEN], BatteryError> {
    let mut buf = vec![0u8; REPORT_LEN];
    buf[0] = REPORT_ID;
    buf[1] = cmd;
    dev.send_feature_report(&buf)?;
    std::thread::sleep(std::time::Duration::from_millis(80));
    let mut read = vec![0u8; REPORT_LEN];
    read[0] = REPORT_ID;
    let n = dev.get_feature_report(&mut read)?;
    if n < 13 {
        return Err(BatteryError::InvalidReply);
    }
    let mut out = [0u8; REPORT_LEN];
    out.copy_from_slice(&read[..REPORT_LEN.min(n)]);
    Ok(out)
}

impl MouseBattery for Ninjutso {
    fn name(&self) -> &'static str {
        "ninjutso"
    }

    fn matches(&self, vid: u16, pid: u16, usage_page: u16, usage: u16) -> bool {
        vid == VID && PIDS.contains(&pid) && usage_page == USAGE_PAGE && usage == USAGE
    }

    fn read(
        &self,
        api: &HidApi,
        path: &CStr,
        device_label: &str,
    ) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;

        // Query battery level
        let batt = query(&dev, 0x15)?;
        let level = batt[9];
        if level > 100 {
            return Err(BatteryError::InvalidReply);
        }

        // Query charging status: byte[9] == 0 means charging, 0x01 means discharging.
        let charge = query(&dev, 0x16)?;
        let charging = charge[9] == 0;

        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

//! Additional wireless mouse battery providers reverse-engineered from
//! MouseDock.exe (mousedock_new).
//!
//! Each provider follows the common HID feature-report pattern:
//! send a vendor command → read the reply → parse battery level + charging.
//!
//! Providers implemented here:
//! - Rapoo (0x24AE receivers, VT9S etc.)
//! - WL Mouse Cube (万灵花兽魔方版, report 0x1A06)
//! - ZYNOO GP1 (0x3886, command 0x9A)
//! - VGN Falcon LD (0x391D)
//! - Scyrox V6S/V8S (0x395A)
//! - ROG standard (Angry Miao / ROG Gear Link)

use std::ffi::CStr;
use hidapi::{HidApi, HidDevice};

use crate::{BatteryError, BatteryReading, MouseBattery};

// ─── Rapoo (雷柏) ────────────────────────────────────────────────────────────
// VID 0x24AE, vendor page 0xFF00. Known models: VT9S (0x1453), VT3 series.
// Protocol: send 7-byte output report, read 7-byte input, level at offset 6.

pub struct Rapoo;

const RAPOO_VID: u16 = 0x24AE;
const RAPOO_PIDS: &[u16] = &[0x1453, 0x1412, 0x14A2, 0x14A7, 0x14A5];

impl MouseBattery for Rapoo {
    fn name(&self) -> &'static str { "rapoo" }

    fn matches(&self, vid: u16, pid: u16, usage_page: u16, _usage: u16) -> bool {
        vid == RAPOO_VID && RAPOO_PIDS.contains(&pid) && usage_page == 0xFF00
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Rapoo battery query: report 0x07, command 0x20 at byte[1]
        let mut req = vec![0u8; 7];
        req[0] = 0x07;
        req[1] = 0x20;
        let _ = dev.write(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut buf = vec![0u8; 64];
        let n = dev.read_timeout(&mut buf, 200)?;
        if n < 7 { return Err(BatteryError::InvalidReply); }
        let level = buf[6];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (buf[5] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── WL Mouse Cube (万灵花兽魔方版) ─────────────────────────────────────────
// Report 0x1A06 query, per battery_wlmouse_cube.pyc.

pub struct WlMouseCube;

const WLM_VID: u16 = 0x3554;
const WLM_CUBE_PIDS: &[u16] = &[0xA887, 0xA888];

impl MouseBattery for WlMouseCube {
    fn name(&self) -> &'static str { "wlmouse_cube" }

    fn matches(&self, vid: u16, pid: u16, _up: u16, _u: u16) -> bool {
        vid == WLM_VID && WLM_CUBE_PIDS.contains(&pid)
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Feature report 0x06, sub-command 0x1A at byte[1]
        let mut req = vec![0u8; 64];
        req[0] = 0x06;
        req[1] = 0x1A;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x06;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 8 { return Err(BatteryError::InvalidReply); }
        let level = reply[3];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging: false, // unknown per upstream
        })
    }
}

// ─── ZYNOO GP1 ─────────────────────────────────────────────────────────────
// VID 0x3886, unnumbered 16-byte reports, command 0x9A.

pub struct Zynoo;

const ZYNOO_VID: u16 = 0x3886;

impl MouseBattery for Zynoo {
    fn name(&self) -> &'static str { "zynoo" }

    fn matches(&self, vid: u16, _pid: u16, _up: u16, _u: u16) -> bool {
        vid == ZYNOO_VID
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Output report: [00 9A 00 00 ...]
        let mut req = vec![0u8; 16];
        req[1] = 0x9A;
        let _ = dev.write(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut buf = vec![0u8; 64];
        let n = dev.read_timeout(&mut buf, 200)?;
        if n < 8 { return Err(BatteryError::InvalidReply); }
        let level = buf[2];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (buf[3] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── VGN Falcon LD (猎鹰3 Extreme) ──────────────────────────────────────────
// VID 0x391D, per battery_vgn_ld.pyc bytes 00 / 0225.

pub struct VgnLd;

const VGN_VID: u16 = 0x391D;

impl MouseBattery for VgnLd {
    fn name(&self) -> &'static str { "vgn_ld" }

    fn matches(&self, vid: u16, _pid: u16, _up: u16, _u: u16) -> bool {
        vid == VGN_VID
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Feature report: [00 02 25 00 ...]
        let mut req = vec![0u8; 64];
        req[0] = 0x00;
        req[1] = 0x02;
        req[2] = 0x25;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x00;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 8 { return Err(BatteryError::InvalidReply); }
        let level = reply[3];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging: false,
        })
    }
}

// ─── Scyrox V6S/V8S (塞洛斯) ───────────────────────────────────────────────
// VID 0x395A, per battery_scyrox_s.pyc.

pub struct ScyroxS;

const SCYROX_VID: u16 = 0x395A;

impl MouseBattery for ScyroxS {
    fn name(&self) -> &'static str { "scyrox_s" }

    fn matches(&self, vid: u16, pid: u16, _up: u16, _u: u16) -> bool {
        vid == SCYROX_VID && pid != 0x0006 // 0x0006 is the older scyrox12, handled separately
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Status query: report 0x09
        let mut req = vec![0u8; 64];
        req[0] = 0x09;
        req[1] = 0x2E;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x09;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 10 { return Err(BatteryError::InvalidReply); }
        let level = reply[5];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (reply[6] & 0x40) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── MCHOSE (迈从 M HUB V2) ─────────────────────────────────────────────────
// VID 0x3837, receivers 0x1014 / 0x1018, per battery_mchose_v2.pyc.
// Query: report 0x09, M HUB 0x0900 battery command.

pub struct MchoseV2;

const MCHOSE_VID: u16 = 0x3837;
const MCHOSE_PIDS: &[u16] = &[0x1014, 0x1018];

impl MouseBattery for MchoseV2 {
    fn name(&self) -> &'static str { "mchose_v2" }

    fn matches(&self, vid: u16, pid: u16, _up: u16, _u: u16) -> bool {
        vid == MCHOSE_VID && MCHOSE_PIDS.contains(&pid)
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // M HUB V2 battery query: output report 0x09
        let mut req = vec![0u8; 64];
        req[0] = 0x09;
        req[1] = 0x00;
        req[2] = 0x09;
        let _ = dev.write(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut buf = vec![0u8; 64];
        let n = dev.read_timeout(&mut buf, 300)?;
        if n < 10 { return Err(BatteryError::InvalidReply); }
        // Reply layout: byte[4] = battery level (0-100)
        let level = buf[4];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (buf[5] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── Ninjutso Sora V3 ───────────────────────────────────────────────────────
// VID 0x193A, feature report 6, commands 0x11/0x12/0x13 per battery_sora_v3.pyc.

pub struct SoraV3;

const SORA_V3_VID: u16 = 0x193A;

impl MouseBattery for SoraV3 {
    fn name(&self) -> &'static str { "sora_v3" }
    fn matches(&self, vid: u16, _pid: u16, _up: u16, _u: u16) -> bool { vid == SORA_V3_VID }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Feature 6, command 0x11 = battery query
        let mut req = vec![0u8; 64];
        req[0] = 0x06;
        req[1] = 0x11;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x06;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 6 { return Err(BatteryError::InvalidReply); }
        let level = reply[3];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging: false,
        })
    }
}

// ─── ROG Standard / Angry Miao (Gear Link) ──────────────────────────────────
// Per battery_rog_miao.pyc: bytes 1207 (command), ffaa (marker).

pub struct RogStandard;

const ROG_VIDS: &[u16] = &[0x0B05, 0x3554]; // ASUS + shared receivers

impl MouseBattery for RogStandard {
    fn name(&self) -> &'static str { "rog_standard" }
    fn matches(&self, vid: u16, _pid: u16, _up: u16, _u: u16) -> bool { ROG_VIDS.contains(&vid) }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Output: [12 07 ...]
        let mut req = vec![0u8; 64];
        req[0] = 0x12;
        req[1] = 0x07;
        let _ = dev.write(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut buf = vec![0u8; 64];
        let n = dev.read_timeout(&mut buf, 200)?;
        if n < 8 { return Err(BatteryError::InvalidReply); }
        // Reply contains 0xFFAA marker, battery at offset 5
        let level = buf[5];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (buf[6] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── SteelSeries Aerox ───────────────────────────────────────────────────────
// Per battery_steelseries.pyc: D2 exchange on unnumbered reports.

pub struct SteelSeries;

const STEEL_VID: u16 = 0x1038;

impl MouseBattery for SteelSeries {
    fn name(&self) -> &'static str { "steelseries" }
    fn matches(&self, vid: u16, _pid: u16, _up: u16, _u: u16) -> bool { vid == STEEL_VID }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // D2 battery query
        let mut req = vec![0u8; 64];
        req[0] = 0x00;
        req[1] = 0xD2;
        req[2] = 0x01;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x00;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 6 { return Err(BatteryError::InvalidReply); }
        let level = reply[3];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (reply[4] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── G-Wolves Feature ────────────────────────────────────────────────────────
// Per battery_gwolves_feature.pyc: bytes 0202, 0804, a1028f.

pub struct GWolves;

const GWOLVES_VID: u16 = 0x3554;

impl MouseBattery for GWolves {
    fn name(&self) -> &'static str { "gwolves" }
    fn matches(&self, vid: u16, pid: u16, _up: u16, _u: u16) -> bool {
        vid == GWOLVES_VID && (pid == 0x8A1A || pid == 0x8A1B || pid == 0x8A1C)
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Feature query: a1 02 8f ...
        let mut req = vec![0u8; 64];
        req[0] = 0x83;
        req[1] = 0xA1;
        req[2] = 0x02;
        req[3] = 0x8F;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x83;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 6 { return Err(BatteryError::InvalidReply); }
        let level = reply[4];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging: false,
        })
    }
}

// ─── Pulsar Bibimbap ─────────────────────────────────────────────────────────
// Per battery_pulsar_bibimbap.pyc: cMouse transaction.

pub struct Pulsar;

const PULSAR_VID: u16 = 0x3554;

impl MouseBattery for Pulsar {
    fn name(&self) -> &'static str { "pulsar" }
    fn matches(&self, vid: u16, pid: u16, _up: u16, _u: u16) -> bool {
        vid == PULSAR_VID && (pid == 0x5500 || pid == 0x5501 || pid == 0x6300)
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        let mut req = vec![0u8; 64];
        req[0] = 0x06;
        req[1] = 0x55;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x06;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 6 { return Err(BatteryError::InvalidReply); }
        let level = reply[3];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging: false,
        })
    }
}

// ─── WebBrands Feature83 (AttackShark / IPI / LunaFury / Lamzu / KO-ONE) ────
// Per battery_webbrands.pyc: feature 0x83, sub-command 0xA1.
// Known VIDs: 0x1014, 0x36A7, 0x372E, 0x373E, 0x38EE.

pub struct WebBrands;

impl MouseBattery for WebBrands {
    fn name(&self) -> &'static str { "webbrands" }
    fn matches(&self, vid: u16, _pid: u16, _up: u16, _u: u16) -> bool {
        const VIDS: &[u16] = &[0x1014, 0x36A7, 0x372E, 0x373E, 0x38EE];
        VIDS.contains(&vid)
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        let mut req = vec![0u8; 64];
        req[0] = 0x83;
        req[1] = 0xA1;
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut reply = vec![0u8; 64];
        reply[0] = 0x83;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 6 { return Err(BatteryError::InvalidReply); }
        let level = reply[3];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (reply[4] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

// ─── Ajazz (黑爵 AJ159 / AJ159 P) ──────────────────────────────────────────
// VID 0x249A, PID 0x5C2F. Legacy feature-report path per wireless_ajazz.0.
// Send 33-byte feature report (id=12), read 33-byte reply; battery at [18],
// charging status at [19].

pub struct Ajazz;

const AJAZZ_VID: u16 = 0x249A;
const AJAZZ_PIDS: &[u16] = &[0x5C2F, 0x5C30];

impl MouseBattery for Ajazz {
    fn name(&self) -> &'static str { "ajazz" }

    fn matches(&self, vid: u16, pid: u16, _up: u16, _u: u16) -> bool {
        vid == AJAZZ_VID && AJAZZ_PIDS.contains(&pid)
    }

    fn read(&self, api: &HidApi, path: &CStr, device_label: &str) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain(&dev);
        // Build 33-byte feature report: [0]=report_id(12), payload 32 bytes
        let mut req = vec![0u8; 33];
        req[0] = 12;            // feature report id
        req[1] = 0x04;          // command
        req[9] = 0x12;           // sub-command
        req[13..17].copy_from_slice(b"M620"); // magic
        dev.send_feature_report(&req)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut reply = vec![0u8; 33];
        reply[0] = 12;
        let n = dev.get_feature_report(&mut reply)?;
        if n < 20 { return Err(BatteryError::InvalidReply); }
        // Validate magic
        if &reply[13..17] != b"M620" { return Err(BatteryError::InvalidReply); }
        let level = reply[18];
        if level > 100 { return Err(BatteryError::InvalidReply); }
        let charging = (reply[19] & 0x01) != 0;
        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

/// Drain pending input reports.
fn drain(dev: &HidDevice) {
    let mut buf = [0u8; 128];
    for _ in 0..16 {
        let _ = dev.read_timeout(&mut buf, 1);
    }
}

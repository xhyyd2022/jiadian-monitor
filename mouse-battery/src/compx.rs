//! Compx-protocol wireless mouse battery (Akko Nest, VGN, ATK, Razer-like receivers).
//!
//! Reverse-engineered from MouseDock.exe. The user's "Akko 8K Receiver"
//! enumerates as VID=0x3554 PID=0xFB2A with a vendor collection at
//! usage_page=0xFF02, usage=2.
//!
//! Wire format:
//! - Request (17-byte HID output report): `08 04 49 00*14`
//! - Reply (17 bytes):
//!   - `b[0:2] == 08 04`
//!   - `b[2] != 0xFF`
//!   - `sum(b[0..17]) % 256 == 0x55`
//!   - not equal to the request frame (echo rejection)
//! - Decode:
//!   - `level_pct = b[6]`           (0–100)
//!   - `charging  = (b[7] & 1) != 0`

use std::ffi::CStr;
use hidapi::{HidApi, HidDevice};

use crate::{BatteryError, BatteryReading, MouseBattery};

/// Match any receiver on the Compx vendor collection signature.
///
/// The PID list is intentionally open: any device on known compx VIDs
/// (0x3554 = VGN/Akko, 0x373B = ATK) with usage_page 0xFF02 / usage 2 is
/// treated as Compx, since the protocol is shared.
pub struct Compx;

const COMPS_VIDS: &[u16] = &[0x3554, 0x373B];
const USAGE_PAGE: u16 = 0xFF02;
const USAGE: u16 = 0x02;

/// 17-byte request frame.
fn request_frame() -> [u8; 17] {
    let mut b = [0u8; 17];
    b[0] = 0x08;
    b[1] = 0x04;
    b[2] = 0x49;
    b
}

impl MouseBattery for Compx {
    fn name(&self) -> &'static str {
        "compx"
    }

    fn matches(&self, vid: u16, _pid: u16, usage_page: u16, usage: u16) -> bool {
        COMPS_VIDS.contains(&vid) && usage_page == USAGE_PAGE && usage == USAGE
    }

    fn read(
        &self,
        api: &HidApi,
        path: &CStr,
        device_label: &str,
    ) -> Result<BatteryReading, BatteryError> {
        let dev = api.open_path(path)?;
        drain_input(&dev);

        let req = request_frame();
        let written = dev.write(&req)?;
        if written != req.len() {
            return Err(BatteryError::InvalidReply);
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
        let mut buf = [0u8; 64];
        let reply: [u8; 17] = loop {
            if std::time::Instant::now() > deadline {
                return Err(BatteryError::Timeout);
            }
            let n = dev.read_timeout(&mut buf, 100)?;
            if n < 17 {
                continue;
            }
            let b = &buf[..n];
            if b[0] != 0x08 || b[1] != 0x04 || b[2] == 0xFF {
                continue;
            }
            let sum: u16 = b[..17].iter().map(|&x| x as u16).sum();
            if (sum % 256) as u8 != 0x55 {
                continue;
            }
            if b[..17] == req {
                // Echo of our own request, not a battery reply.
                continue;
            }
            let mut frame = [0u8; 17];
            frame.copy_from_slice(&b[..17]);
            break frame;
        };

        let level = reply[6];
        if level > 100 {
            return Err(BatteryError::InvalidReply);
        }
        let charging = (reply[7] & 1) != 0;

        Ok(BatteryReading {
            provider: self.name(),
            device_label: device_label.to_string(),
            device_key: String::new(),
            level_pct: level,
            charging,
        })
    }
}

/// Discard pending input reports so a stale frame is not mistaken for the
/// battery reply. Mirrors the upstream implementation.
fn drain_input(dev: &HidDevice) {
    let mut buf = [0u8; 128];
    for _ in 0..32 {
        // 1ms timeout; ignore errors (empty queue is the normal case).
        let _ = dev.read_timeout(&mut buf, 1);
    }
}

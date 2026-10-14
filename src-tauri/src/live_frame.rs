//! LIVE 通道（`A5 5A 0B`）帧构造 —— 逐字节对齐 `protocols/live_format_spec.md`。
//!
//! - mode 0 全帧裸图 : `A5 5A 0B | u32LE(W*H*2) | 00 | 全帧 RGB565(大端)`
//! - mode 1 单区域   : `A5 5A 0B | u32LE(W*H*2) | 01 | X,Y,W,H(各u16LE) | 区域 RGB565`
//! - mode 2 批量(≤6) : `A5 5A 0B | u32LE(Σ) | 02 | n | n*8B描述符 | 各段首尾相接`
//! - 退出            : `A5 5A 0B | u32LE 0 | 00`

pub const MAGIC: [u8; 3] = [0xA5, 0x5A, 0x0B];

pub const MODE_FULL: u8 = 0;
pub const MODE_REGION: u8 = 1;
pub const MODE_BATCH: u8 = 2;
#[allow(dead_code)] // 协议保留：LZ4 模式，固件支持但上位机暂未启用
pub const MODE_LZ4: u8 = 3;

/// 设备固件上限：批量一帧最多 6 个区域。
pub const BATCH_MAX_REGIONS: usize = 6;

/// RGB888 → RGB565 **大端**（直接截断，与 live_frame.py 一致）。
pub fn rgb888_to_rgb565_be(rgb: &[u8]) -> Vec<u8> {
    let n = rgb.len() / 3;
    let mut out = Vec::with_capacity(n * 2);
    for px in rgb.chunks_exact(3) {
        let (r, g, b) = (px[0] as u16, px[1] as u16, px[2] as u16);
        let v = ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3);
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

/// mode 0：全帧裸图。`payload` 为 RGB565 大端，长度必须 `W*H*2`。
pub fn frame_full(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&MAGIC);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.push(MODE_FULL);
    v.extend_from_slice(payload);
    v
}

/// mode 1：单区域。
pub fn frame_region(x: u16, y: u16, w: u16, h: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(16 + payload.len());
    v.extend_from_slice(&MAGIC);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.push(MODE_REGION);
    for val in [x, y, w, h] {
        v.extend_from_slice(&val.to_le_bytes());
    }
    v.extend_from_slice(payload);
    v
}

/// mode 2：批量（1..=6 个区域）。
pub fn frame_batch(regions: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
    let n = regions.len();
    debug_assert!((1..=BATCH_MAX_REGIONS).contains(&n));
    let total: usize = regions.iter().map(|r| r.4.len()).sum();
    let mut v = Vec::with_capacity(9 + n * 8 + total);
    v.extend_from_slice(&MAGIC);
    v.extend_from_slice(&(total as u32).to_le_bytes());
    v.push(MODE_BATCH);
    v.push(n as u8);
    for (x, y, w, h, _) in regions {
        for val in [*x, *y, *w, *h] {
            v.extend_from_slice(&val.to_le_bytes());
        }
    }
    for (_, _, _, _, payload) in regions {
        v.extend_from_slice(payload);
    }
    v
}

/// 退出帧：`A5 5A 0B | u32LE 0 | 00`。
pub fn frame_exit() -> Vec<u8> {
    let mut v = Vec::with_capacity(8);
    v.extend_from_slice(&MAGIC);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.push(MODE_FULL);
    v
}

/// 复刻固件 mode 1 的越界判据。
#[allow(dead_code)] // 固件 mode 1 越界判据的参考实现，保留以便对照
pub fn region_ok(w_img: u32, h_img: u32, x: u32, y: u32, w: u32, h: u32) -> bool {
    if w == 0 || h == 0 || x >= w_img || y >= h_img {
        return false;
    }
    w <= w_img - x && h <= h_img - y
}

/// 从整帧（RGB565 大端，行距 `stride`）抽出矩形区域的 RGB565 大端字节。
pub fn extract_region565(frame: &[u8], stride_w: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 2) as usize);
    for yy in y..y + h {
        let off = ((yy * stride_w + x) * 2) as usize;
        out.extend_from_slice(&frame[off..off + (w * 2) as usize]);
    }
    out
}

/// 解析设备回执，返回 (kind, reason)。
pub fn parse_ack(raw: Option<&[u8]>) -> (&'static str, Option<String>) {
    let Some(bytes) = raw else {
        return ("timeout", None);
    };
    let text: String = bytes
        .iter()
        .map(|&b| if b == 0 { ' ' } else { b as char })
        .collect();
    let text = text.trim();
    if text.is_empty() {
        ("timeout", None)
    } else if text.starts_with("LIVE,ok") {
        ("ok", None)
    } else if text.starts_with("LIVE,exit") {
        ("exit", None)
    } else if text.starts_with("LIVE,err") {
        let parts: Vec<&str> = text.split(',').collect();
        ("err", Some(parts.get(2).copied().unwrap_or(text).to_string()))
    } else {
        ("other", Some(text.to_string()))
    }
}

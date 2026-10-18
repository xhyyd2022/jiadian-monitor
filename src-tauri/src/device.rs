//! 设备管理命令（`A5 5A 08 xx`）：亮度 / 旋转 / 跟随息屏 / 状态查询。
//!
//! 这些命令要 USB 使用权，所以走和下发一样的流程：
//! GUI 在跑就先**移交** → 打开设备 → 发命令 → 读 ACK → 归还租约。
//!
//! 帧与 ACK（见 `protocols/protocol_spec.md` §`0x08`）：
//! - 亮度   `A5 5A 08 FF <0..255>` → `DSP,<0..255>`（0 = 息屏）
//! - 旋转   `A5 5A 08 FE <0..3>`   → `ROT,<0..3>`
//! - 跟随息屏 `A5 5A 08 FD <0|1>`  → `VSO,<0|1>`
//! - 状态查询 `A5 5A 08 FC`        → `STA,<亮度>,<息屏>,<旋转>,<跟随息屏>`

use crate::{logging, pipeline, winusb};

/// 拿 USB 使用权执行一段操作。
/// 已经接管（下发管线在跑）就直接复用同一个句柄 —— WinUSB 不允许开第二个。
/// 且**先暂停下发**，命令发完自动继续（设备同一时刻只认一条命令，否则 ACK 会串）。
fn with_device<T>(
    serial: Option<String>,
    f: impl FnOnce(&winusb::Device) -> Result<T, String>,
) -> Result<T, String> {
    let running = pipeline::link_open();
    if running {
        // 多设备：正在接管 A 时对 B 发命令会发错设备 → 直接拒绝，让调用方先停止
        let want = serial.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let (Some(want), Some(cur)) = (want, pipeline::link_serial()) {
            // SN 大小写可能不稳定（`find_by_serial` 内部会大写化），这里用忽略大小写比较，
            // 避免同一台设备因大小写差异被误判成「另一台」而拒绝命令。
            if !want.eq_ignore_ascii_case(cur.trim()) {
                let tail = |s: &str| s.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<String>();
                return Err(format!(
                    "设备 {} 正在接管中，请先「停止」再操作 {}",
                    tail(&cur),
                    tail(want)
                ));
            }
        }
        pipeline::pause();
        // 等在途的那一帧发完（帧间隔 33ms）
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    let r = if running {
        pipeline::with_link(|l| f(l.dev()?))
    } else {
        // 没接管：自己接管一次，做完归还。
        // **只有真由本次新打开（返回 Some(gen)）才关** —— 若 `open_link` 发现已被（管线线程）接管而
        // 复用了同一句柄，关掉会把正在下发的管线链路一起关死。且按**代数**关，避免误关新一轮链路。
        match pipeline::open_link(serial) {
            Ok(opened) => {
                let r = pipeline::with_link(|l| f(l.dev()?));
                if let Some(gen_id) = opened {
                    pipeline::close_link_if_gen(gen_id);
                }
                r
            }
            Err(e) => Err(e),
        }
    };
    if let Err(e) = &r {
        // 设备被拔掉：句柄已经废了。继续留着这条链路，后续每次操作都要等满管道超时，
        // 而且 `with_link` 期间一直占着 LINK 锁 → 界面被拖住。直接释放，让下次操作快速失败。
        if winusb::is_gone(e) {
            logging::warn(&format!("设备已断开，释放链路：{e}"));
            pipeline::close_link();
        }
    }
    if running {
        pipeline::resume();
    }
    r
}

/// 发一条设备命令并读回 ACK 文本。
fn cmd(dev: &winusb::Device, frame: &[u8]) -> Result<String, String> {
    dev.write(frame)?;
    let ack = dev.read(256)?;
    Ok(ack
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default())
}

/// 读显示状态：`STA,<亮度>,<息屏>,<旋转>,<跟随息屏>`。
#[allow(dead_code)] // 读显示状态（STA 命令），暂未接入
pub fn status(serial: Option<String>) -> Result<serde_json::Value, String> {
    with_device(serial, |dev| {
        let ack = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFC])?;
        logging::info(&format!("设备状态：{ack}"));
        if !ack.starts_with("STA,") {
            return Err(format!("状态查询返回异常：{ack}"));
        }
        let p: Vec<i64> = ack[4..]
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        let g = |i: usize| p.get(i).copied().unwrap_or(-1);
        Ok(serde_json::json!({
            "ack": ack,
            "brightness": g(0),
            "screen_off": g(1),
            "rotation": g(2),
            "follow_off": g(3),
        }))
    })
}

/// 设亮度 `0..=255`（0 = 息屏）。
pub fn set_brightness(serial: Option<String>, v: u8) -> Result<String, String> {
    with_device(serial, |dev| {
        let ack = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFF, v])?;
        logging::info(&format!("设亮度 {v} → {ack}"));
        if !ack.starts_with("DSP,") {
            return Err(format!("设亮度返回异常：{ack}"));
        }
        Ok(ack)
    })
}

/// 设旋转 `0..=3`。旋转会让设备清屏回默认背景 → 让管线下一帧整屏重发。
pub fn set_rotation(serial: Option<String>, v: u8) -> Result<String, String> {
    let r = with_device(serial, |dev| {
        let ack = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFE, v])?;
        logging::info(&format!("设旋转 {v} → {ack}"));
        if !ack.starts_with("ROT,") {
            return Err(format!("设旋转返回异常：{ack}"));
        }
        Ok(ack)
    })?;
    pipeline::force_full_frame();
    Ok(r)
}

/// 跟随息屏 `0|1`。
pub fn set_follow_off(serial: Option<String>, on: bool) -> Result<String, String> {
    with_device(serial, |dev| {
        let ack = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFD, u8::from(on)])?;
        logging::info(&format!("设跟随息屏 {on} → {ack}"));
        if !ack.starts_with("VSO,") {
            return Err(format!("设跟随息屏返回异常：{ack}"));
        }
        Ok(ack)
    })
}

/// **完整下发一次配置**：亮度 + 旋转 + 跟随息屏，一次接管里连着发（改任一项都调它）。
///
/// 设备只认"当前这条命令"，单项下发在息屏/旋转之后容易出现状态对不上；
/// 三项一起发能保证设备状态与我们界面上的三个值始终一致。
pub fn apply_config(
    serial: Option<String>,
    brightness: u8,
    rotation: u8,
    follow_off: bool,
) -> Result<serde_json::Value, String> {
    let mut acks: Vec<String> = Vec::new();
    with_device(serial, |dev| {
        let a = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFF, brightness])?;
        if !a.starts_with("DSP,") {
            return Err(format!("设亮度返回异常：{a}"));
        }
        acks.push(a);

        let a = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFE, rotation])?;
        if !a.starts_with("ROT,") {
            return Err(format!("设旋转返回异常：{a}"));
        }
        acks.push(a);

        let a = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFD, u8::from(follow_off)])?;
        if !a.starts_with("VSO,") {
            return Err(format!("设跟随息屏返回异常：{a}"));
        }
        acks.push(a);
        Ok(())
    })?;
    // 旋转会清屏 → 下一帧整屏重发
    pipeline::force_full_frame();
    logging::info(&format!(
        "完整下发配置：亮度 {brightness} / 旋转 {rotation} / 跟随息屏 {follow_off} → {}",
        acks.join(" | ")
    ));
    Ok(serde_json::json!({
        "acks": acks,
        "brightness": brightness,
        "rotation": rotation,
        "follow_off": follow_off,
    }))
}

/// Flash 识别码（JEDEC hex）→ 型号。见 `protocols/features.md` §3.1。
fn flash_model(id: &str) -> Option<&'static str> {
    match id {
        "856017" => Some("Puya P25Q64 (8 MB)"),
        "684019" => Some("博雅 BY25Q256 (32 MB)"),
        _ => None,
    }
}

/// 设备信息：SN + 能力(JED) + 电压(PING) + 显示状态(STA)。
///
/// JED 字段：`JED,<id>,<Mbit>,<动画MB>,<?>,<可存图数>,<W>,<H>,<能力集...>,HW=<变体>`
/// 实测：`JED,684019,256,32,31,218,320,240,GFM2,GFM3,LZ4P,LZ4L,HW=V2`
pub fn info(serial: Option<String>) -> Result<serde_json::Value, String> {
    with_device(serial, |dev| {
        let jed = cmd(dev, &[0xA5, 0x5A, 0x07])?;
        let pong = cmd(dev, &[0xA5, 0x5A, 0x09])?;
        let sta = cmd(dev, &[0xA5, 0x5A, 0x08, 0xFC])?;
        logging::info(&format!("设备信息：JED={jed} PONG={pong} STA={sta}"));

        let f: Vec<&str> = jed.split(',').collect();
        let num = |i: usize| f.get(i).and_then(|x| x.trim().parse::<i64>().ok());
        let caps: Vec<String> = f.iter().skip(8).map(|s| s.trim().to_string()).collect();
        // 型号：优先能力集里的 HW=，没有就按分辨率反推
        let hw = caps
            .iter()
            .find_map(|c| c.strip_prefix("HW=").map(|s| s.to_string()))
            .unwrap_or_else(|| match (num(6), num(7)) {
                (Some(320), Some(170)) => "V1P".into(),
                (Some(320), Some(240)) => "V2".into(),
                _ => "UNKNOWN".into(),
            });
        let flash_id = f.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
        let mv = pong
            .strip_prefix("PONG,")
            .and_then(|s| s.trim().parse::<i64>().ok());
        let sp: Vec<i64> = sta
            .trim_start_matches("STA,")
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();

        Ok(serde_json::json!({
            "serial": dev.serial.clone(),
            "jed": jed,
            "pong": pong,
            "sta": sta,
            "flash_id": flash_id,
            "flash_model": flash_model(&flash_id),
            "flash_mbit": num(2),
            "anim_mb": num(3),
            "frames": num(5),
            "width": num(6),
            "height": num(7),
            "caps": caps,
            "hw": hw,
            "millivolts": mv,
            "brightness": sp.first().copied(),
            "screen_off": sp.get(1).copied(),
            "rotation": sp.get(2).copied(),
            "follow_off": sp.get(3).copied(),
        }))
    })
}

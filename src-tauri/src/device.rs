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
    // 进入时先捕获**此刻**的链路代数：命令失败要释放链路时只能关这一代，
    // 绝不能无条件 `close_link()` —— 否则期间若新一轮管线已接管，会把新链路一起关掉。
    let running_gen = if running { pipeline::link_generation() } else { None };
    // 未接管时自己新开的链路代数（只有真新开才有值）。
    let mut opened_gen: Option<u64> = None;
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
                opened_gen = opened;
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
        // 而且设备命令会排在同一把 I/O 锁后面 → 界面看起来像卡死。直接释放，让下次操作快速失败。
        if winusb::is_gone(e) {
            logging::warn(&format!("设备已断开，释放链路：{e}"));
            // 只关自己捕获到的那一代：期间可能有新一轮管线接管，不能误关。
            if let Some(gen_id) = running_gen.or(opened_gen) {
                pipeline::close_link_if_gen(gen_id);
            }
        }
    }
    if running {
        pipeline::resume();
    }
    r
}

/// 发命令并**按前缀认领回包**，中间收到不认识的行就丢掉继续读。
///
/// 为什么不能像 `cmd()` 那样“发完读下一帧”：设备在上一轮被强杀/崩溃后会残留一条状态行
/// （典型是 `LIVE,exit`），之后每一次读到的都是**上一条命令**的回包 —— 实测日志：
/// `JED=LIVE,exit` / `PONG=JED,684019,…` / `STA=PONG,4490`，界面就显示成
/// 「型号 UNKNOWN、分辨率 null、Flash 255」。整条队列错位一次就永远错位，只能靠认前缀复位。
fn cmd_matching(dev: &winusb::Device, frame: &[u8], prefix: &str) -> Result<String, String> {
    dev.write(frame)?;
    for _ in 0..4 {
        let Some(bytes) = dev.read(256)? else {
            return Err(format!("等待 {prefix} 回包超时"));
        };
        let s = String::from_utf8_lossy(&bytes).trim().to_string();
        if s.starts_with(prefix) {
            return Ok(s);
        }
        // 残留帧：记一笔再继续读，别让它污染后面的命令
        logging::warn(&format!("丢弃非预期回包（在等 {prefix}）：{s}"));
    }
    Err(format!("连续收到非预期回包，未拿到 {prefix}"))
}

/// STA 必须按位置解析；filter_map 会把坏字段删除并让后面的旋转/息屏值向前错位。
fn parse_display_state(ack: &str) -> Result<[i64; 4], String> {
    let payload = ack.strip_prefix("STA,").ok_or_else(|| format!("状态查询返回异常：{ack}"))?;
    let fields: Vec<&str> = payload.split(',').collect();
    if fields.len() != 4 {
        return Err(format!("STA 字段数异常（期望 4，实际 {}）：{ack}", fields.len()));
    }
    let mut values = [0; 4];
    let limits = [(0, 255), (0, 1), (0, 3), (0, 1)];
    for (i, field) in fields.iter().enumerate() {
        let value: i64 = field.trim().parse()
            .map_err(|_| format!("STA 第 {} 个字段不是整数：{ack}", i + 1))?;
        let (min, max) = limits[i];
        if !(min..=max).contains(&value) {
            return Err(format!("STA 第 {} 个字段越界（允许 {min}..={max}）：{ack}", i + 1));
        }
        values[i] = value;
    }
    Ok(values)
}

/// 读显示状态：`STA,<亮度>,<息屏>,<旋转>,<跟随息屏>`。
#[allow(dead_code)] // 读显示状态（STA 命令），暂未接入
pub fn status(serial: Option<String>) -> Result<serde_json::Value, String> {
    with_device(serial, |dev| {
        let ack = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFC], "STA,")?;
        logging::info(&format!("设备状态：{ack}"));
        let state = parse_display_state(&ack)?;
        Ok(serde_json::json!({
            "ack": ack,
            "brightness": state[0],
            "screen_off": state[1],
            "rotation": state[2],
            "follow_off": state[3],
        }))
    })
}

/// 设亮度 `0..=255`（0 = 息屏）。
pub fn set_brightness(serial: Option<String>, v: u8) -> Result<String, String> {
    with_device(serial, |dev| {
        let ack = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFF, v], "DSP,")?;
        logging::info(&format!("设亮度 {v} → {ack}"));
        if !ack.starts_with("DSP,") {
            return Err(format!("设亮度返回异常：{ack}"));
        }
        Ok(ack)
    })
}

/// 设旋转 `0..=3`。旋转会让设备清屏回默认背景 → 让管线下一帧整屏重发。
pub fn set_rotation(serial: Option<String>, v: u8) -> Result<String, String> {
    if v > 3 {
        return Err(format!("旋转值越界：{v}（允许 0..=3）"));
    }
    let result = with_device(serial, |dev| {
        let ack = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFE, v], "ROT,")?;
        logging::info(&format!("设旋转 {v} → {ack}"));
        if !ack.starts_with("ROT,") {
            return Err(format!("设旋转返回异常：{ack}"));
        }
        Ok(ack)
    });
    // 命令可能已执行但 ACK 丢失，失败时也要让下一帧恢复完整画面。
    pipeline::force_full_frame();
    result
}

/// 跟随息屏 `0|1`。
pub fn set_follow_off(serial: Option<String>, on: bool) -> Result<String, String> {
    with_device(serial, |dev| {
        let ack = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFD, u8::from(on)], "VSO,")?;
        logging::info(&format!("设跟随息屏 {on} → {ack}"));
        if !ack.starts_with("VSO,") {
            return Err(format!("设跟随息屏返回异常：{ack}"));
        }
        Ok(ack)
    })
}

/// **完整下发一次配置**：亮度 + 旋转 + 跟随息屏，一次接管里连着发（改任一项都调它）。
///
/// 在一次接管里串行发送，避免三项命令之间被其它操作插入。
/// 设备不支持事务：中途失败可能已部分执行，须保留已确认项数并强制整屏恢复。
pub fn apply_config(
    serial: Option<String>,
    brightness: u8,
    rotation: u8,
    follow_off: bool,
) -> Result<serde_json::Value, String> {
    if rotation > 3 {
        return Err(format!("旋转值越界：{rotation}（允许 0..=3）"));
    }
    let serial_label = serial.as_deref().unwrap_or("自动选择").to_string();
    let mut acks: Vec<String> = Vec::new();
    let result = with_device(serial, |dev| {
        let a = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFF, brightness], "DSP,")?;
        if !a.starts_with("DSP,") {
            return Err(format!("设亮度返回异常：{a}"));
        }
        acks.push(a);

        let a = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFE, rotation], "ROT,")?;
        if !a.starts_with("ROT,") {
            return Err(format!("设旋转返回异常：{a}"));
        }
        acks.push(a);

        let a = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFD, u8::from(follow_off)], "VSO,")?;
        if !a.starts_with("VSO,") {
            return Err(format!("设跟随息屏返回异常：{a}"));
        }
        acks.push(a);
        Ok(())
    });
    // 中途失败也可能已经执行旋转并清屏，不能只在三项全部成功后才重发。
    pipeline::force_full_frame();
    result.map_err(|e| {
        let message = format!(
            "配置下发未完成（serial={serial_label}，已确认 {}/3 项，设备可能已部分更新）：{e}",
            acks.len()
        );
        logging::warn(&message);
        message
    })?;
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
        let jed = cmd_matching(dev, &[0xA5, 0x5A, 0x07], "JED")?;
        let pong = cmd_matching(dev, &[0xA5, 0x5A, 0x09], "PONG,")?;
        let sta = cmd_matching(dev, &[0xA5, 0x5A, 0x08, 0xFC], "STA,")?;
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
        let sp = parse_display_state(&sta)?;

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

#[cfg(test)]
mod tests {
    use super::{apply_config, parse_display_state, set_rotation};

    #[test]
    fn display_state_preserves_positions_and_zero_values() {
        assert_eq!(parse_display_state("STA,200,0,2,1").unwrap(), [200, 0, 2, 1]);
        assert_eq!(parse_display_state("STA,0,0,0,0").unwrap(), [0, 0, 0, 0]);
        assert_eq!(parse_display_state("STA, 255 , 1 , 3 , 1 ").unwrap(), [255, 1, 3, 1]);
    }

    #[test]
    fn malformed_display_state_does_not_shift_fields() {
        for ack in [
            "STA,200,broken,2,1", "STA,200,,2,1", "STA,200,0,2",
            "STA,200,0,2,1,0", "STA,200,0,2,1,", "LIVE,exit", "STA,",
        ] {
            assert!(parse_display_state(ack).is_err(), "不应接受：{ack}");
        }
    }

    #[test]
    fn display_state_rejects_out_of_range_fields() {
        for ack in ["STA,-1,0,0,0", "STA,256,0,0,0", "STA,1,2,0,0", "STA,1,0,4,0", "STA,1,0,0,2"] {
            assert!(parse_display_state(ack).is_err(), "不应接受：{ack}");
        }
    }

    #[test]
    fn invalid_rotation_is_rejected_before_opening_any_device() {
        assert!(set_rotation(None, 4).unwrap_err().contains("旋转值越界"));
        assert!(apply_config(None, 200, 255, false).unwrap_err().contains("旋转值越界"));
    }
}

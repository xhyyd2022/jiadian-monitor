//! 基础指标：数据源（LibreHardwareService）+ Windows API 汇总。
//!
//! 占位符定义见 `app/MONITOR_METRICS.md`：
//! - 单实例不编号：`cpu_load` / `mem_pct` / `sys_uptime`
//! - 多实例从 1 编号：`gpu1_load` / `disk1_pct` / `net1_down` / `fan1`
//! - 磁盘额外给一份合计：`disk_all_pct` / `disk_all_used` / `disk_all_total`
//!
//! 数据源不提供的项用 Windows API 补（磁盘容量 `GetDiskFreeSpaceExW`、网络 `GetIfTable2`、
//! 开机时长 `GetTickCount64`）。读不到就 `value=None`（前端显示 `—`），不报错。
//!
//! 开机时长口径（**用户 2026-10-13 确认保持现状**）：`GetTickCount64` 是「距上次启动的墙上时间」，
//! **含睡眠 / 休眠 / 快速启动关机期**（官方文档明说 includes time in sleep or hibernation）→
//! 天天关机却显示十几天属正常，非 bug；与任务管理器「运行时间」一致。详见 `win32_fill` 里的注释。

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// `GetDriveTypeW` 返回值：固定磁盘。
const DRIVE_FIXED: u32 = 3;
const GB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Serialize, Clone)]
pub struct BasicMetric {
    /// 统一占位符（`cpu_load` / `gpu1_temp` / `cpu_name` …）。
    pub key: String,
    /// 分组（CPU / GPU / 内存 / 磁盘 / 网络 / 风扇 / 系统）。
    pub group: String,
    /// 中文显示名。
    pub label: String,
    /// 原始数值（文本槽或读不到为 None）。
    pub value: Option<f64>,
    pub unit: String,
    /// 格式化好的显示串（`12.3%` / `45.0℃` / `9700X` / `—`）。
    pub text: String,
    /// 数据来源，便于排查（鼠标悬停可见）。
    pub source: String,
    /// `number`（注入 value）或 `text`（注入 text，型号这类固定文本）。
    pub kind: String,
}

fn fmt(v: Option<f64>, unit: &str, dec: usize) -> String {
    match v {
        None => "—".into(),
        Some(x) => {
            let s = format!("{x:.dec$}");
            // % 和 ℃ 不加空格，其余加（`12.3%` / `45.0℃` / `1200 RPM`）
            let sep = if unit == "%" || unit == "℃" { "" } else { " " };
            if unit.is_empty() { s } else { format!("{s}{sep}{unit}") }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push(
    out: &mut Vec<BasicMetric>,
    group: &str,
    key: &str,
    label: &str,
    unit: &str,
    dec: usize,
    v: Option<f64>,
    src: &str,
) {
    out.push(BasicMetric {
        key: key.into(),
        group: group.into(),
        label: label.into(),
        value: v,
        unit: unit.into(),
        text: fmt(v, unit, dec),
        source: src.into(),
        kind: "number".into(),
    });
}

/// 文本槽（型号这类固定文本，注入 `text` 而不是 `value`）。
pub(crate) fn push_text(out: &mut Vec<BasicMetric>, group: &str, key: &str, label: &str, text: &str, src: &str) {
    out.push(BasicMetric {
        key: key.into(),
        group: group.into(),
        label: label.into(),
        value: None,
        unit: String::new(),
        text: text.into(),
        source: src.into(),
        kind: "text".into(),
    });
}

/// 精简 CPU 型号，只留**最终型号**：
/// `AMD Ryzen 7 9700X` → `9700X`；`Intel Core i9-14900K` → `14900K`；`Intel Core i7-14700K` → `14700K`。
///
/// 规则：先按 `CPU [#0]: 型号: 别名` 形式取冒号后的那段，再取最后一个词
/// （AMD 的末段就是 `9700X`），最后剥掉 Intel 的 `i3-/i5-/i7-/i9-` 段首前缀。
pub(crate) fn short_cpu(name: &str) -> String {
    let s = name.split(':').nth(1).unwrap_or(name).trim();
    let last = s.split_whitespace().last().unwrap_or(s);
    for p in ["i3-", "i5-", "i7-", "i9-"] {
        if let Some(rest) = last.strip_prefix(p) {
            return rest.to_string();
        }
    }
    last.to_string()
}

/// 精简显卡型号，只留**最终型号**：
/// `NVIDIA GeForce RTX 5070 Ti` → `5070Ti`；`AMD Radeon RX 9070 GRE` → `9070GRE`；
/// `dGPU [#0]: NVIDIA GeForce RTX 5070 Ti: Palit …` → `5070Ti`。
///
/// 规则：取冒号后的那段 → 去厂商前缀（NVIDIA GeForce / AMD Radeon / Intel）→ 去系列前缀
/// （RTX / GTX / RX / Arc）→ 型号本身是数字开头时把中间空格去掉（`5070 Ti` → `5070Ti`）。
pub(crate) fn short_gpu(name: &str) -> String {
    let s = name.split(':').nth(1).unwrap_or(name).trim();
    let s = s
        .strip_prefix("NVIDIA GeForce ")
        .or_else(|| s.strip_prefix("AMD Radeon "))
        .or_else(|| s.strip_prefix("AMD "))
        .or_else(|| s.strip_prefix("Intel "))
        .unwrap_or(s);
    let s = ["RTX ", "GTX ", "RX ", "Arc "]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.replace(' ', "")
    } else {
        s.to_string()
    }
}

pub(crate) fn uptime_text(secs: Option<f64>) -> String {
    let Some(s) = secs else { return "—".into() };
    let s = s as u64;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}天 {h}小时")
    } else if h > 0 {
        format!("{h}小时 {m}分")
    } else {
        format!("{m}分")
    }
}

pub(crate) struct Drive {
    pub letter: char,
    pub total_gb: f64,
    pub used_gb: f64,
}

/// 所有固定逻辑盘（按盘符 A→Z）。
pub(crate) fn fixed_drives() -> Vec<Drive> {
    let mut out = Vec::new();
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root: Vec<u16> = format!("{letter}:\\")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            if GetDriveTypeW(root.as_ptr()) != DRIVE_FIXED {
                continue;
            }
            let (mut free_avail, mut total, mut total_free) = (0u64, 0u64, 0u64);
            if GetDiskFreeSpaceExW(root.as_ptr(), &mut free_avail, &mut total, &mut total_free) == 0
                || total == 0
            {
                continue;
            }
            let total_gb = total as f64 / GB;
            out.push(Drive {
                letter,
                total_gb,
                used_gb: total_gb - total_free as f64 / GB,
            });
        }
    }
    out
}

/// 固定逻辑盘（Win32；两路数据源共用）。
pub(crate) fn drives() -> Vec<Drive> {
    fixed_drives()
}

/// 逻辑盘所属的**物理盘号**（`\\.\C:` → `IOCTL_STORAGE_GET_DEVICE_NUMBER`）。
///
/// 用途：把「数据源给的**物理盘**传感器」对到「我们按盘符编的 `disk{n}`」上。
/// 光看容量对不齐（一块物理盘可以切好几个分区），盘号才是硬对应。
/// 以 `dwDesiredAccess = 0` 打开卷句柄，**不需要管理员**。
pub(crate) fn disk_number(letter: char) -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D1080;
    #[repr(C)]
    #[derive(Default)]
    struct StorageDeviceNumber {
        device_type: u32,
        device_number: u32,
        partition_number: u32,
    }

    let path: Vec<u16> = format!("\\\\.\\{letter}:")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let h = CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return None;
        }
        let mut sdn = StorageDeviceNumber::default();
        let mut ret = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            std::ptr::null(),
            0,
            &mut sdn as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<StorageDeviceNumber>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        );
        CloseHandle(h);
        (ok != 0).then_some(sdn.device_number)
    }
}

/// 磁盘温度（值由各数据源提供，按盘序一一对应写入 disk{n}_temp）。
pub(crate) fn disk_temps(out: &mut Vec<BasicMetric>, temps: &[Option<f64>], src: &str) {
    let ds = fixed_drives();
    for (i, d) in ds.iter().enumerate() {
        let v = temps.get(i).copied().flatten();
        let n = i + 1;
        push(
            out,
            "磁盘",
            &format!("disk{n}_temp"),
            &format!("磁盘{n} ({}:) 温度", d.letter),
            "℃",
            0,
            v,
            src,
        );
    }
}

// ---------------------------------------------------------------------------
// 网络（Win32 `GetIfTable2`）—— 两路数据源共用，不依赖任何第三方服务
// ---------------------------------------------------------------------------

/// 一块网卡的一次采样。
pub(crate) struct Nic {
    luid: u64,
    /// 连接名（`Alias`）：`以太网` / `WLAN` / `本地连接* 5`…
    alias: String,
    /// 驱动描述（`Description`），已归一化（小写、去末尾 ` #N`）。
    /// 同一块物理网卡会被驱动报出多个实例（Wi-Fi Direct 子接口等），描述只差一个 ` #N`。
    desc_key: String,
    /// 物理地址（最多前 6 字节）。
    mac: [u8; 6],
    mac_len: usize,
    /// 累计收/发字节（开机以来）。
    rx: u64,
    tx: u64,
    /// OperStatus == Up。
    up: bool,
}

/// 采样基线：LUID →（上次收/发字节、时刻、上次算出的速率）。
/// 速率由**我们自己**按两次采样的时间差算，好处是：并发调用（多个面板同时拉 /metrics）
/// 不会互相污染，也不用依赖数据源服务给的「瞬时速率」（那玩意各家口径不一）。
static NET_BASE: OnceLock<Mutex<HashMap<u64, (u64, u64, Instant, Option<(f64, f64)>)>>> = OnceLock::new();

/// 定长 UTF-16 缓冲 → String。
fn wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

/// 虚拟/隧道网卡：别名前缀命中就跳过。
/// 这些不是「多出来的一块网卡」——Hyper-V / WSL 的虚拟交换机流量是**物理网卡的镜像**，
/// Wi-Fi Direct（`本地连接*`）与蓝牙 PAN 是离线占位。收进来只会让 `net1..netN` 变脏。
fn is_virtual_alias(alias: &str) -> bool {
    alias.starts_with("vEthernet (")
        || alias.starts_with("vSwitch (")
        || alias.starts_with("本地连接*")
        || alias.starts_with("蓝牙")
}

/// 驱动描述归一化：小写 + 去掉末尾的 ` #N`。
/// 同一块物理网卡的多个实例（Wi-Fi Direct 子接口）描述就是 `<base>` / `<base> #2` / `<base> #3`…
fn norm_desc(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    if let Some(i) = s.rfind(" #") {
        let tail = &s[i + 2..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return s[..i].trim_end().to_string();
        }
    }
    s
}

/// 两块条目是不是**同一块物理网卡**的不同实例。
///
/// 判据：描述（去 ` #N`）相同 **且** 物理地址「后 5 字节」相同。
/// - Wi-Fi Direct 子接口的 MAC 是把物理网卡 MAC 的**首字节**改几位（`9C:AA:BB:CC:DD:EE` →
///   `9E:AA:BB:CC:DD:EE`）→ 后 5 字节一致 → 判为同一块。
/// - 双口同型号网卡（`… X550-T2` / `… X550-T2 #2`）MAC 只有**最后一位**不同，后 5 字节不同
///   → 判为两块，不会被误并。
/// - 有一边没报 MAC（`Not Present` 的实例）：按描述判为同一块。
fn same_physical(a: &Nic, b: &Nic) -> bool {
    if a.desc_key.is_empty() || a.desc_key != b.desc_key {
        return false;
    }
    match (a.mac6(), b.mac6()) {
        (Some(x), Some(y)) => x[1..] == y[1..],
        _ => true,
    }
}

impl Nic {
    /// 前 6 字节物理地址（没报就 `None`）。
    fn mac6(&self) -> Option<[u8; 6]> {
        (self.mac_len >= 6).then_some(self.mac)
    }
}

/// 枚举**物理**网卡（含当前断开、开机以来没流量的那些 —— 多网卡场景要能都列出来）。
///
/// `MIB_IF_ROW2.InterfaceAndOperStatusFlags` 的 bit0 = HardwareInterface、bit1 = FilterInterface：
/// WFP / QoS Packet Scheduler / Npcap / 火绒 NDIS / Hyper-V 扩展这些**过滤驱动**会各报一块伪网卡，
/// 计数器是同一块物理网卡的副本（实测这台机上能刷出 260+ 条）→ 必须靠硬标志剔除。
fn nics() -> Vec<Nic> {
    let mut out = Vec::new();
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return out; // 读不到就空着（上层显示 —），不报错
        }
        let count = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), count);
        for r in rows {
            let bits = r.InterfaceAndOperStatusFlags._bitfield;
            let hw = bits & 0x01 != 0; // HardwareInterface
            let filter = bits & 0x02 != 0; // FilterInterface
            if filter || !hw {
                continue;
            }
            let alias = wide(&r.Alias);
            if alias.is_empty() || is_virtual_alias(&alias) {
                continue;
            }
            let mut mac = [0u8; 6];
            let n = (r.PhysicalAddressLength as usize).min(6);
            mac[..n].copy_from_slice(&r.PhysicalAddress[..n]);
            out.push(Nic {
                luid: r.InterfaceLuid.Value,
                alias,
                desc_key: norm_desc(&wide(&r.Description)),
                mac,
                mac_len: n,
                rx: r.InOctets,
                tx: r.OutOctets,
                up: r.OperStatus == IfOperStatusUp,
            });
        }
        FreeMibTable(table as *const core::ffi::c_void);
    }
    out
}

/// 诊断用（`--dump-nics`）：把 Win32 看到的物理网卡原样吐出来，方便核对过滤与去重规则。
pub fn dump_nics_json() -> String {
    let all = nics();
    let v: Vec<serde_json::Value> = all
        .iter()
        .map(|n| {
            let mac = n
                .mac6()
                .map(|m| m.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-"))
                .unwrap_or_default();
            serde_json::json!({
                "alias": n.alias,
                "description": n.desc_key,
                "mac": mac,
                "luid": n.luid,
                "up": n.up,
                "in_octets": n.rx,
                "out_octets": n.tx,
            })
        })
        .collect();
    let mut kept: Vec<&Nic> = Vec::new();
    for n in &all {
        match kept.iter_mut().find(|p| same_physical(p, n)) {
            Some(p) => {
                if better(n, p) {
                    *p = n;
                }
            }
            None => kept.push(n),
        }
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "raw": v,
        "kept": kept.iter().map(|n| n.alias.clone()).collect::<Vec<_>>(),
    }))
    .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

/// 同一块网卡的多个实例里，哪个更值得留：在用 > 流量大 > 报了 MAC。
fn better(a: &Nic, b: &Nic) -> bool {
    (a.up, a.rx + a.tx, a.mac_len) > (b.up, b.rx + b.tx, b.mac_len)
}

/// `net1..netN`：**所有物理网卡都出**（多设备兼容是重点，断开的也留着，只是速率为 0），
/// 排序 = 在用 → 开机以来流量大的 → 名字，稳定可复现。
///
/// 去重：同一块物理网卡的多个实例（Wi-Fi Direct 子接口等）只留一个，优先留「在用 / 有流量 / 有 MAC」
/// 的那个（判据见 `same_physical`）。这样「多网卡」是真的多块网卡，而不是一块网卡的副本。
fn net_fill(out: &mut Vec<BasicMetric>) {
    let all = nics();
    let mut picks: Vec<&Nic> = Vec::new();
    for n in &all {
        match picks.iter_mut().find(|p| same_physical(p, n)) {
            Some(p) => {
                if better(n, p) {
                    *p = n;
                }
            }
            None => picks.push(n),
        }
    }
    picks.sort_by(|a, b| {
        b.up.cmp(&a.up)
            .then_with(|| (b.rx + b.tx).cmp(&(a.rx + a.tx)))
            .then_with(|| a.alias.cmp(&b.alias))
    });
    picks.truncate(8);

    let now = Instant::now();
    let base = NET_BASE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut m = base.lock().unwrap();
    // 兜底清理：网卡换了/拔了，别让基线表无限涨（正常机器上就个位数）
    if m.len() > 32 {
        m.retain(|_, v| now.duration_since(v.2).as_secs() < 60);
    }
    for (i, n) in picks.iter().enumerate() {
        let num = i + 1;
        let rate: Option<(f64, f64)>;
        let kb = |c: u64, p: u64, dt: f64| {
            if c >= p {
                (c - p) as f64 / dt / 1024.0
            } else {
                0.0 // 计数器回绕 / 网卡重插
            }
        };
        match m.get(&n.luid).copied() {
            // 首次见到这块网卡：只存基线，下次才有速率
            None => {
                rate = None;
                m.insert(n.luid, (n.rx, n.tx, now, None));
            }
            Some((rx0, tx0, at, last)) => {
                let dt = now.duration_since(at).as_secs_f64();
                if dt < 0.05 {
                    // 两次采样挨太近（多面板并发拉取）：**不推进基线**，沿用上次速率
                    rate = last;
                } else {
                    rate = Some((kb(n.rx, rx0, dt), kb(n.tx, tx0, dt)));
                    m.insert(n.luid, (n.rx, n.tx, now, rate));
                }
            }
        }
        // 标签带上连接名（`网卡1 (以太网) 下行`）：多网卡时才知道哪个是哪个；名字过长就截断
        let tag = if n.alias.is_empty() {
            String::new()
        } else {
            format!(" ({})", truncate_chars(&n.alias, 14))
        };
        push(out, "网络", &format!("net{num}_down"), &format!("网卡{num}{tag} 下行"), "KB/s", 1, rate.map(|r| r.0), &n.alias);
        push(out, "网络", &format!("net{num}_up"), &format!("网卡{num}{tag} 上行"), "KB/s", 1, rate.map(|r| r.1), &n.alias);
        push(out, "网络", &format!("net{num}_total_down"), &format!("网卡{num}{tag} 累计下行"), "GB", 2, Some(n.rx as f64 / GB), &n.alias);
        push(out, "网络", &format!("net{num}_total_up"), &format!("网卡{num}{tag} 累计上行"), "GB", 2, Some(n.tx as f64 / GB), &n.alias);
    }
}

/// 按**字符**（不是字节）截断，超长补 `…`。
fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// 两路数据源**共用**的 Win32 项：磁盘容量/占用/合计 + 网络 + 开机时长。
/// 放在 metrics 里而不是各源里，避免同一件事写两遍。
pub(crate) fn win32_fill(out: &mut Vec<BasicMetric>) {
    let ds = fixed_drives();
    let mut sum_used = 0.0;
    let mut sum_total = 0.0;
    for (i, d) in ds.iter().enumerate() {
        let n = i + 1;
        let has = |out: &Vec<BasicMetric>, k: &str| out.iter().any(|m| m.key == k);
        if !has(out, &format!("disk{n}_pct")) {
            push(
                out,
                "磁盘",
                &format!("disk{n}_pct"),
                &format!("磁盘{n} ({}:) 占用率", d.letter),
                "%",
                1,
                Some(d.used_gb / d.total_gb * 100.0),
                "Win: 逻辑盘",
            );
            push(out, "磁盘", &format!("disk{n}_used"), &format!("磁盘{n} ({}:) 已用", d.letter), "GB", 1, Some(d.used_gb), "Win: 逻辑盘");
            push(out, "磁盘", &format!("disk{n}_total"), &format!("磁盘{n} ({}:) 总量", d.letter), "GB", 1, Some(d.total_gb), "Win: 逻辑盘");
        }
        sum_used += d.used_gb;
        sum_total += d.total_gb;
    }
    if sum_total > 0.0 && !out.iter().any(|m| m.key == "disk_all_total") {
        push(out, "磁盘", "disk_all_pct", "磁盘合计 占用率", "%", 1, Some(sum_used / sum_total * 100.0), "Win: 所有固定盘");
        push(out, "磁盘", "disk_all_used", "磁盘合计 已用", "GB", 1, Some(sum_used), "Win: 所有固定盘");
        push(out, "磁盘", "disk_all_total", "磁盘合计 总量", "GB", 1, Some(sum_total), "Win: 所有固定盘");
    }
    // 网络：Win32 物理网卡（速率自算）。数据源若已给出网络项则沿用、不覆盖。
    if !out.iter().any(|m| m.key == "net1_down") {
        net_fill(out);
    }
    if !out.iter().any(|m| m.key == "sys_uptime") {
        // 规范单位 = **秒**（素材可声明 `data-unit="分/小时/天"`，注入脚本按单位族换算）。
        // `text` 另给一句人类看的「3天 2小时」；数值槽走 `value`（秒）。
        //
        // 【口径，2026-10-13 与用户确认：保持现状】`GetTickCount64` = 距上次启动的**墙上时间**，
        // 官方文档明确「includes time the system spends in sleep or hibernation」→
        // **睡眠 / 休眠 / 快速启动关机期间也照样累加**（快速启动 = 内核会话休眠，下次开机是恢复而非重启，
        // 计数还不归零）。所以「天天关机却显示十几天」是这个口径的正常结果，**不是 bug**；
        // 它和任务管理器的「运行时间」口径一致。
        // 想改成「只算系统处于工作状态」（不含睡眠）→ 换 `QueryUnbiasedInterruptTime()`
        // （Win7+，`Win32_System_WindowsProgramming` feature，返回 100ns，`秒 = ticks / 10_000_000`）；
        // 注意它同样不会因快速启动而归零。`\System\System Up Time` 性能计数器含睡眠，不能用来修。
        let secs = unsafe { GetTickCount64() } as f64 / 1000.0;
        push(out, "系统", "sys_uptime", "开机时长", "s", 0, Some(secs), "Win: GetTickCount64");
        if let Some(m) = out.last_mut() {
            m.text = uptime_text(Some(secs));
        }
    }
    sort_groups(out);
}

/// 按 CPU → GPU → 内存 → 磁盘 → 网络 → 风扇 → 系统 归拢（组内保持原有顺序）。
fn sort_groups(out: &mut Vec<BasicMetric>) {
    const ORDER: [&str; 7] = ["CPU", "GPU", "内存", "磁盘", "网络", "风扇", "系统"];
    out.sort_by_key(|m| ORDER.iter().position(|g| *g == m.group.as_str()).unwrap_or(99));
}

// ---------------------------------------------------------------------------
// 数据源：只用 LibreHardwareService（`source_lhws.rs`）
// ---------------------------------------------------------------------------

/// LibreHardwareService 当前是否启用（前端据此决定要不要提示装它）。
pub fn lhws_enabled() -> bool {
    true
}

/// 当前启用的数据源说明（启动时写一行日志，便于确认）。
pub fn source_hint() -> String {
    "LibreHardwareService".into()
}

/// 基础指标：读数据源，再补 Win32 项。
pub fn basic() -> Vec<BasicMetric> {
    if let Some(v) = crate::source_lhws::read() {
        return v;
    }
    // 没读到：只给 Win32 能算出来的项（其余显示 —）
    let mut out: Vec<BasicMetric> = Vec::new();
    win32_fill(&mut out);
    out
}

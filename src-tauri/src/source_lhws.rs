//! 数据源之二：**LibreHardwareService**（第三方组件，MPL-2.0）—— 读它的共享内存。
//!
//! 独立文件：只用 Win32 映射 + `serde_json`（**不新增依赖**）。服务没装/没跑 → 返回 `None`，上层自动回退。
//!
//! 两个要点：
//! - 它的 **data 块恒定是 JSON**（每个传感器对象后跟一个 0x00 分隔），与 `indexFormat` 配置无关
//!   → 我们**不需要改它的配置、也不需要 MessagePack**。
//! - 它给的是 LibreHardwareMonitor 的**通用名**（`Fan #1`、`CPU Total`…），没有主板厂标签，
//!   所以这里按 `hardwareType + name` 匹配；`fan` 用小值启发式区分 "%（显卡）" 与 "RPM（主板）"。
//!
//! **弃用这一路 = 删本文件 + `metrics.rs::basic()` 里两行。**

use crate::metrics::{BasicMetric, disk_temps, push, push_text, short_cpu, short_gpu, win32_fill};
use serde_json::Value;

const MMF: &str = "Global\\LibreHardwareService/json/sensors/data";
const MUTEX: &str = "Global\\LibreHardwareService/json/sensors/data/MUTEX";
#[allow(dead_code)] // 服务名，供诊断/日志使用
pub const NAME: &str = "LibreHardwareService";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 服务在发布数据吗（共享内存存在）。
#[allow(dead_code)] // 便捷判断；实际走 lhws_install::running()
pub fn available() -> bool {
    crate::lhws_install::running()
}

/// 读一段共享内存：拿互斥体 → 映射 → 只复制 data 段 → 释放。
unsafe fn read_data_block() -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{
        FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
        VirtualQuery,
    };
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{OpenMutexW, ReleaseMutex, WaitForSingleObject};

    let mtx = unsafe { OpenMutexW(SYNCHRONIZE, 0, wide(MUTEX).as_ptr()) };
    let mut locked = false;
    if !mtx.is_null() && unsafe { WaitForSingleObject(mtx, 2000) } == 0 {
        locked = true;
    }
    let result = (|| -> Result<Vec<u8>, String> {
        let map = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide(MMF).as_ptr()) };
        if map.is_null() {
            return Err("服务未运行（共享内存不存在）".into());
        }
        let view = unsafe { MapViewOfFile(map, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            unsafe { CloseHandle(map) };
            return Err("映射共享内存失败".into());
        }
        let out = (|| -> Result<Vec<u8>, String> {
            // 映射区大小（不能越过它读，否则访问违例）
            let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            let got = unsafe {
                VirtualQuery(
                    view.Value as *const core::ffi::c_void,
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            let cap = if got == 0 { 1 << 20 } else { mbi.RegionSize };
            let base = view.Value as *const u8;
            let rd = |off: usize| -> Option<u32> {
                if off + 4 > cap {
                    return None;
                }
                Some(u32::from_ne_bytes(unsafe {
                    *(base.add(off) as *const [u8; 4])
                }))
            };
            let meta_len = rd(0).ok_or("头部不完整")? as usize;
            let hdr = 4 + meta_len;
            let data_len = rd(hdr + 12).ok_or("头部不完整（data-length）")? as usize;
            let data_off = rd(hdr + 16).ok_or("头部不完整（data-offset）")? as usize;
            if data_len == 0 || data_off + data_len > cap {
                return Err(format!("data 段越界（off={data_off} len={data_len} cap={cap}）"));
            }
            Ok(unsafe { std::slice::from_raw_parts(base.add(data_off), data_len) }.to_vec())
        })();
        unsafe {
            UnmapViewOfFile(view);
            CloseHandle(map);
        }
        out
    })();
    unsafe {
        if locked {
            ReleaseMutex(mtx);
        }
        if !mtx.is_null() {
            CloseHandle(mtx);
        }
    }
    result
}

/// 调试用：把共享内存里的**全部**传感器原样吐成 JSON（`--dump-lhws`）。
/// 某个指标对不上时先看这里——有没有这条传感器、原始值是多少、归属哪块硬件。
pub fn dump_raw_json() -> String {
    let d = match unsafe { read_data_block() } {
        Ok(d) => d,
        Err(e) => return format!("{{\"error\": \"{e}\"}}"),
    };
    let arr: Vec<serde_json::Value> = parse(&d)
        .iter()
        .map(|s| {
            serde_json::json!({
                "identifier": s.identifier,
                "name": s.name,
                "sensorType": s.sensor_type,
                "hardwareName": s.hw_name,
                "hardwareType": s.hw_type,
                "value": s.value,
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

struct S {
    identifier: String,
    name: String,
    sensor_type: String,
    hw_name: String,
    hw_type: String,
    value: f64,
}

impl S {
    fn has(&self, kw: &str) -> bool {
        self.name.to_ascii_lowercase().contains(kw)
    }
    fn is(&self, t: &str) -> bool {
        self.sensor_type.eq_ignore_ascii_case(t)
    }
}

/// 标识符末段的通道号：`/lpc/nct6687dr/0/fan/13` → 13（用来给风扇定序）。
fn chan(s: &S) -> u32 {
    s.identifier.rsplit('/').next().and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)
}

/// 把 data 段（对象 + 0x00 交替）拆成传感器列表。
fn parse(data: &[u8]) -> Vec<S> {
    let mut out = Vec::new();
    for chunk in data.split(|b| *b == 0) {
        let t: &[u8] = match chunk.iter().position(|b| !b.is_ascii_whitespace()) {
            Some(i) => &chunk[i..],
            None => continue,
        };
        if !t.starts_with(b"{") {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(t) else { continue };
        let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        out.push(S {
            identifier: g("identifier"),
            name: g("name"),
            sensor_type: g("sensorType"),
            hw_name: g("hardwareName"),
            hw_type: g("hardwareType"),
            value: v.get("value").and_then(|x| x.as_f64()).unwrap_or(0.0),
        });
    }
    out
}

/// 读一遍并映射成 23 项指标；读不到返回 `None`。
pub fn read() -> Option<Vec<BasicMetric>> {
    let data = match unsafe { read_data_block() } {
        Ok(d) => d,
        Err(e) => {
            crate::logging::warn(&format!("LibreHardwareService 不可用：{e}"));
            return None;
        }
    };
    let ss = parse(&data);
    if ss.is_empty() {
        return None;
    }
    let mut out: Vec<BasicMetric> = Vec::new();
    let src = |s: &S| format!("LHS: {}", s.identifier);

    // ---------- CPU ----------
    // 只留**最终型号**（`AMD Ryzen 7 9700X` → `9700X`）：面板上那一格放不下全名，
    // 素材也基本只想要能一眼认出的型号。全名仍留在 `source` 里（悬停可见）。
    let cpu_full = ss
        .iter()
        .find(|s| s.hw_type == "Cpu")
        .map(|s| s.hw_name.trim().to_string())
        .unwrap_or_default();
    let cpu_name = short_cpu(&cpu_full);
    push_text(&mut out, "CPU", "cpu_name", "CPU 型号", &cpu_name, &format!("LHS: hardwareName = {cpu_full}"));
    let cpu = |f: &dyn Fn(&S) -> bool| -> Option<f64> {
        ss.iter()
            .find(|s| s.hw_type == "Cpu" && f(s))
            .map(|s| s.value)
    };
    let load = cpu(&|s| s.is("Load") && (s.name.eq_ignore_ascii_case("CPU Total") || s.has("total")));
    let temp = cpu(&|s| s.is("Temperature") && (s.has("tctl") || s.has("tdie") || s.has("package")))
        .or_else(|| {
            ss.iter()
                .filter(|s| s.hw_type == "Cpu" && s.is("Temperature"))
                .map(|s| s.value)
                .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |x| x.max(v))))
        });
    let power = cpu(&|s| s.is("Power") && s.has("package"));
    let core_temp = ss
        .iter()
        .find(|s| s.hw_type == "Cpu" && s.is("Temperature"))
        .map(|s| src(s));
    push(&mut out, "CPU", "cpu_load", "CPU 占用率", "%", 1, load, "LHS: CPU Total");
    push(&mut out, "CPU", "cpu_temp", "CPU 温度", "℃", 1, temp, core_temp.as_deref().unwrap_or("LHS: CPU 温度"));
    push(&mut out, "CPU", "cpu_power", "CPU 功耗", "W", 1, power, "LHS: CPU Package");

    // ---------- GPU（多实例）----------
    let gpu_types = ["GpuNvidia", "GpuAmd", "GpuIntel"];
    let mut gpus: Vec<&S> = ss
        .iter()
        .filter(|s| gpu_types.contains(&s.hw_type.as_str()))
        .collect();
    gpus.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    gpus.dedup_by(|a, b| a.hw_name == b.hw_name);
    for (i, g) in gpus.iter().enumerate() {
        let n = i + 1;
        let in_gpu = |f: &dyn Fn(&S) -> bool| -> Option<f64> {
            ss.iter()
                .find(|s| s.hw_name == g.hw_name && f(s))
                .map(|s| s.value)
        };
        let gload = in_gpu(&|s| s.is("Load") && s.has("core"));
        let mem_used = in_gpu(&|s| s.has("memory used"));
        let mem_pct = in_gpu(&|s| s.is("Load") && s.has("memory"));
        let mem_total = in_gpu(&|s| s.has("memory total")).or_else(|| match (mem_used, mem_pct) {
            (Some(u), Some(p)) if p > 0.0 => Some(u / p * 100.0),
            _ => None,
        });
        // 显卡风扇：LHM 里 `SensorType.Fan` **就是 RPM**（百分比走 Control 型，不在这）。
        // 之前按「值 > 300 才算 RPM」筛，结果 5070 Ti 空闲零转（0 RPM）被当成没有风扇 → 显示 —。
        // 现在照收：传感器在就显示真实转速，0 就是 0（零转模式），没有这条传感器才显示 —。
        let fan = in_gpu(&|s| s.is("Fan"));
        // 型号同样只留最终型号（`NVIDIA GeForce RTX 5070 Ti` → `5070Ti`），全名进 `source`
        let gpu_name = short_gpu(g.hw_name.trim());
        push_text(&mut out, "GPU", &format!("gpu{n}_name"), &format!("GPU{n} 型号"), &gpu_name, &format!("LHS: hardwareName = {}", g.hw_name.trim()));
        push(&mut out, "GPU", &format!("gpu{n}_load"), &format!("GPU{n} 占用率"), "%", 1, gload, "LHS: GPU Core");
        push(&mut out, "GPU", &format!("gpu{n}_mem_pct"), &format!("GPU{n} 显存占用率"), "%", 1, mem_pct, "LHS: GPU Memory");
        push(&mut out, "GPU", &format!("gpu{n}_mem_used"), &format!("GPU{n} 显存已用"), "MB", 0, mem_used, "LHS: GPU Memory Used");
        push(&mut out, "GPU", &format!("gpu{n}_mem_total"), &format!("GPU{n} 显存总量"), "MB", 0, mem_total, "LHS: GPU Memory Total");
        push(&mut out, "GPU", &format!("gpu{n}_temp"), &format!("GPU{n} 温度"), "℃", 1, in_gpu(&|s| s.is("Temperature") && s.has("core")), "LHS: GPU Core 温度");
        push(&mut out, "GPU", &format!("gpu{n}_fan"), &format!("GPU{n} 风扇"), "RPM", 0, fan, "LHS: GPU Fan");
    }

    // ---------- 内存 ----------
    // LHS 有 **4 个** `Memory` 硬件：两条内存条（只有温度/时序）+ `Total Memory`（物理内存）
    // + `Virtual Memory`（含页面文件的提交量）。按 `hardwareType` 找会撞上前面的（`Virtual Memory`
    // 排在前）→ 之前一直把「提交量 46.6 GB」当成物理内存（真实是 2×16 GB）。
    // 认准 `Memory Used` 且硬件名不含 virtual 的那块。
    let mem_hw: Option<&str> = ss
        .iter()
        .find(|s| {
            s.hw_type == "Memory"
                && s.name.eq_ignore_ascii_case("memory used")
                && !s.hw_name.to_ascii_lowercase().contains("virtual")
        })
        .map(|s| s.hw_name.as_str());
    let memv = |kw: &str| -> Option<f64> {
        let hw = mem_hw?;
        ss.iter().find(|s| s.hw_name == hw && s.has(kw)).map(|s| s.value)
    };
    let used = memv("memory used");
    let avail = memv("memory available");
    let total = match (used, avail) {
        (Some(u), Some(a)) => Some(u + a),
        _ => None,
    };
    push(
        &mut out,
        "内存",
        "mem_pct",
        "内存占用率",
        "%",
        1,
        ss.iter().find(|s| Some(s.hw_name.as_str()) == mem_hw && s.is("Load")).map(|s| s.value),
        "LHS: Total Memory",
    );
    push(&mut out, "内存", "mem_used", "已用内存", "GB", 1, used, "LHS: Memory Used");
    push(&mut out, "内存", "mem_total", "内存总量", "GB", 1, total, "LHS: Used + Available");

    // ---------- 磁盘温度（容量/占用由 win32_fill 补）----------
    // **按物理盘号对**，不能按顺序 nth()：一块物理盘有多个温度传感器，而且
    // 「Warning / Critical Temperature 是**阈值常数**不是读数」（WD 盘上是 79℃ / 84℃，
    // 直接当温度显示会吓人）。LHS 标识符里的序号与 Windows 物理盘号一致（`/nvme/1` ↔ 磁盘 1），
    // 用 `disk_number()` 拿到盘符所属盘号，再把分区对到物理盘上。
    let phys: Vec<(u32, f64, Option<f64>, String)> = {
        // (盘号, 总容量 GB(十进制), 温度, 盘名)
        let mut groups: Vec<(String, Vec<&S>)> = Vec::new();
        for s in ss.iter().filter(|s| s.hw_type == "Storage") {
            match groups.iter_mut().find(|(k, _)| *k == s.hw_name) {
                Some((_, v)) => v.push(s),
                None => groups.push((s.hw_name.clone(), vec![s])),
            }
        }
        groups
            .into_iter()
            .filter_map(|(name, g)| {
                let idx = g.first().and_then(|s| {
                    s.identifier
                        .split('/')
                        .nth(2)
                        .and_then(|x| x.parse::<u32>().ok())
                })?;
                let total = g.iter().find(|s| s.name == "Total Space").map(|s| s.value)?; // 十进制 GB
                // 第一个「真实温度」：跳过阈值类传感器，也跳过明显是坏值的
                let temp = g
                    .iter()
                    .filter(|s| s.is("Temperature"))
                    .find(|s| {
                        let n = s.name.to_ascii_lowercase();
                        !n.contains("warning") && !n.contains("critical") && !n.contains("limit")
                            && (0.0..150.0).contains(&s.value)
                    })
                    .map(|s| s.value);
                Some((idx, total, temp, name))
            })
            .collect()
    };
    let temps: Vec<Option<f64>> = crate::metrics::drives()
        .iter()
        .map(|d| {
            let disk_no = crate::metrics::disk_number(d.letter)?;
            let (_, _, temp, _) = phys.iter().find(|(idx, total, _, _)| {
                // 盘号一致 + 容量对得上（分区不可能比所在物理盘大太多，也不能小得离谱）——
                // 万一同号不同盘（RAID / 外置盒），宁可显示 — 也不显示别人的温度
                *idx == disk_no
                    && *total >= d.total_gb * 0.5
                    && *total <= d.total_gb * 4.0 + 8.0
            })?;
            *temp
        })
        .collect();
    disk_temps(&mut out, &temps, "LHS: Storage Temperature");

    // ---------- 网络 ----------
    // **不在这里出**：网络统一由 `metrics::win32_fill` 从 `GetIfTable2` 取。
    // LHS 会把每个 NDIS 过滤驱动都当成一块网卡（WFP / QoS / Npcap / 火绒 / Hyper-V 扩展…），
    // 这台机上能刷出 260+ 条 net 指标，全是同一块物理网卡的镜像 → 交给 Win32 按
    // HardwareInterface 标志筛，速率也由我们自己算，口径两路一致。

    // ---------- 风扇（主板 SuperIO，RPM）----------
    // 按**通道号**排序（`/fan/0`、`/fan/1`…）而不是按数据到达顺序 —— 数据顺序会变，
    // 排序后「风扇1/2」始终是同一路风扇，素材上的编号才不会漂。
    // 转速 0 的通道（没接风扇 / 停转）不出现，否则一块板子上 11 路里 7 路是 0，白占位。
    let mut fans: Vec<&S> = ss
        .iter()
        .filter(|s| s.is("Fan") && s.hw_type == "SuperIO" && s.value > 0.0)
        .collect();
    fans.sort_by(|a, b| chan(a).cmp(&chan(b)));
    for (i, s) in fans.iter().enumerate().take(6) {
        let n = i + 1;
        push(&mut out, "风扇", &format!("fan{n}"), &format!("风扇{n} ({})", s.name), "RPM", 0, Some(s.value), &src(s));
    }

    // ---------- 共用的 Win32 项 ----------
    win32_fill(&mut out);
    Some(out)
}

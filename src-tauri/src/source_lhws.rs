//! 数据源之二：**LibreHardwareService**（第三方组件，MPL-2.0）—— 读它的共享内存。
//!
//! 独立文件：使用 Win32 共享内存映射与 `serde_json`，不新增依赖。
//! 读取失败或没有有效传感器时返回 `None`，由上层补齐缺值。
//!
//! data 段按 0x00 分隔的 JSON 传感器对象解析，按硬件类型、传感器类型和名称映射指标。

use std::sync::Mutex;

use crate::metrics::{BasicMetric, disk_temps, push, push_text, short_cpu, short_gpu, win32_fill};
use serde_json::Value;

const MMF: &str = "Global\\LibreHardwareService/json/sensors/data";
const MUTEX: &str = "Global\\LibreHardwareService/json/sensors/data/MUTEX";
// 等待与释放都要授权：只有 SYNCHRONIZE 的句柄能取得所有权，却不能 ReleaseMutex。
const SENSOR_MUTEX_ACCESS: u32 = windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE
    | windows_sys::Win32::System::Threading::MUTEX_MODIFY_STATE;
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

const MAX_MAPPING_BYTES: usize = 64 * 1024 * 1024;

/// 所有偏移都限制在映射和本地读取上限内，先检查加法再访问内存。
fn checked_range(off: usize, len: usize, cap: usize) -> Result<std::ops::Range<usize>, String> {
    let end = off.checked_add(len).ok_or("共享内存偏移溢出")?;
    let limit = cap.min(MAX_MAPPING_BYTES).min(isize::MAX as usize);
    if end > limit {
        return Err(format!("共享内存越界（off={off} len={len} cap={limit}）"));
    }
    Ok(off..end)
}

fn mapping_capacity(view: usize, region: usize, size: usize) -> Result<usize, String> {
    let end = region.checked_add(size).ok_or("共享内存区域地址溢出")?;
    if view < region || view >= end {
        return Err("映射地址不在 VirtualQuery 返回的区域内".into());
    }
    Ok((end - view).min(MAX_MAPPING_BYTES).min(isize::MAX as usize))
}

/// 仅解析长度与偏移，不访问 FFI；data 偏移相对于映射起点。
fn data_block_range(block: &[u8]) -> Result<std::ops::Range<usize>, String> {
    let rd = |off: usize| -> Result<u32, String> {
        let bytes = block.get(checked_range(off, 4, block.len())?).ok_or("头部不完整")?;
        let bytes: [u8; 4] = bytes.try_into().map_err(|_| "头部不完整")?;
        Ok(u32::from_le_bytes(bytes))
    };
    let meta_len = rd(0)? as usize;
    let hdr = checked_range(4, meta_len, block.len())?.end;
    let header_end = checked_range(hdr, 20, block.len())?.end;
    let len_off = hdr.checked_add(12).ok_or("data-length 偏移溢出")?;
    let off_off = hdr.checked_add(16).ok_or("data-offset 偏移溢出")?;
    let data_len = rd(len_off)? as usize;
    let data_off = rd(off_off)? as usize;
    if data_len == 0 || data_off < header_end {
        return Err(format!("data 段为空或与头部重叠（off={data_off} len={data_len}）"));
    }
    checked_range(data_off, data_len, block.len())
}

/// 读一段共享内存：必须持有互斥体，映射和句柄通过 RAII 释放。
unsafe fn read_data_block() -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE,
        WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Memory::{
        FILE_MAP_READ, MEM_COMMIT, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile, OpenFileMappingW, PAGE_GUARD, PAGE_NOACCESS, UnmapViewOfFile, VirtualQuery,
    };
    use windows_sys::Win32::System::Threading::{OpenMutexW, ReleaseMutex, WaitForSingleObject};

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct MutexGuard(Handle);
    impl Drop for MutexGuard {
        fn drop(&mut self) {
            if unsafe { ReleaseMutex(self.0.0) } == 0 {
                let code = unsafe { GetLastError() };
                crate::logging::warn(&format!(
                    "LibreHardwareService 释放互斥体失败（ReleaseMutex 错误 {code}）"
                ));
            }
        }
    }

    struct View(MEMORY_MAPPED_VIEW_ADDRESS);
    impl Drop for View {
        fn drop(&mut self) {
            unsafe { UnmapViewOfFile(self.0) };
        }
    }

    let mtx = unsafe { OpenMutexW(SENSOR_MUTEX_ACCESS, 0, wide(MUTEX).as_ptr()) };
    if mtx.is_null() {
        let code = unsafe { GetLastError() };
        return Err(format!("互斥体打不开（OpenMutexW 错误 {code}）"));
    }
    let mtx = Handle(mtx);
    match unsafe { WaitForSingleObject(mtx.0, 2000) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => {}
        WAIT_TIMEOUT => return Err("等待读取互斥体超时（WAIT_TIMEOUT）".into()),
        WAIT_FAILED => {
            let code = unsafe { GetLastError() };
            return Err(format!("等待读取互斥体失败（WAIT_FAILED，错误 {code}）"));
        }
        status => return Err(format!("等待读取互斥体返回异常状态（{status}）")),
    }
    // WAIT_ABANDONED 同样取得所有权；仍须验证映射、头部和数据，并最终释放。
    let _lock = MutexGuard(mtx);
    let map = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide(MMF).as_ptr()) };
    if map.is_null() {
        let code = unsafe { GetLastError() };
        return Err(match code {
            ERROR_FILE_NOT_FOUND => {
                "共享内存不存在（服务进程未运行，或刚启动尚未发布数据）".to_string()
            }
            ERROR_ACCESS_DENIED => "共享内存打不开（权限不足，错误 5）".to_string(),
            c => format!("共享内存打不开（错误 {c}）"),
        });
    }
    let map = Handle(map);
    let view = unsafe { MapViewOfFile(map.0, FILE_MAP_READ, 0, 0, 0) };
    if view.Value.is_null() {
        let code = unsafe { GetLastError() };
        return Err(format!("映射共享内存失败（错误 {code}）"));
    }
    let view = View(view);
    let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let got = unsafe {
        VirtualQuery(
            view.0.Value as *const core::ffi::c_void,
            &mut mbi,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if got == 0 {
        let code = unsafe { GetLastError() };
        return Err(format!("查询共享内存区域失败（VirtualQuery 错误 {code}）"));
    }
    if got != std::mem::size_of::<MEMORY_BASIC_INFORMATION>()
        || mbi.State != MEM_COMMIT
        || mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0
    {
        return Err("共享内存区域信息不完整或不可读".into());
    }
    let cap = mapping_capacity(view.0.Value as usize, mbi.BaseAddress as usize, mbi.RegionSize)?;
    let block = unsafe { std::slice::from_raw_parts(view.0.Value as *const u8, cap) };
    let range = data_block_range(block)?;
    Ok(block[range].to_vec())
}

/// 调试用：把共享内存里解析有效的数值传感器输出为 JSON（`--dump-lhws`）。
/// 某个指标对不上时先看这里——有没有这条传感器、原始值是多少、归属哪块硬件。
pub fn dump_raw_json() -> String {
    let d = match unsafe { read_data_block() } {
        Ok(d) => d,
        Err(e) => return serde_json::json!({ "error": e }).to_string(),
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
    serde_json::to_string_pretty(&arr)
        .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string())
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
        let Some(value) = v.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()) else {
            continue;
        };
        let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        out.push(S {
            identifier: g("identifier"),
            name: g("name"),
            sensor_type: g("sensorType"),
            hw_name: g("hardwareName"),
            hw_type: g("hardwareType"),
            value,
        });
    }
    out
}

/// 只选择 CPU 的整体负载，不把核心、线程或其他带 total 的名称当作整体负载。
fn select_cpu_load(sensors: &[S]) -> Option<f64> {
    sensors
        .iter()
        .find(|s| {
            s.hw_type.eq_ignore_ascii_case("Cpu")
                && s.is("Load")
                && s.name.eq_ignore_ascii_case("CPU Total")
                && s.value.is_finite()
                && s.value >= 0.0
        })
        .map(|s| s.value.min(100.0))
}

/// 数据源故障的合并状态：`Some((原因, 已抑制条数))`。
///
/// **为什么要合并**：`metrics::basic()` 一秒会被调用两次（界面轮询 + 面板 `/metrics`），
/// 数据源一断就是 2 条/秒 WARN 刷屏。同一原因只记第一条，恢复时汇总一条，日志立刻可读。
static FAIL: Mutex<Option<(String, u64)>> = Mutex::new(None);

/// 记录一次失败：同一原因只记第一条，后续只累计；原因变化时另起一条。
fn note_fail(reason: &str) {
    let mut g = FAIL.lock().unwrap_or_else(|e| e.into_inner());
    match g.clone() {
        Some((r, n)) if r == reason => *g = Some((r, n.saturating_add(1))),
        Some((r, n)) => {
            *g = Some((reason.to_string(), 1));
            drop(g);
            crate::logging::warn(&format!(
                "LibreHardwareService 数据源异常（原因变化：{r} → {reason}；此前 {n} 次同类已抑制）：{reason}"
            ));
        }
        None => {
            *g = Some((reason.to_string(), 1));
            drop(g);
            crate::logging::warn(&format!(
                "LibreHardwareService 不可用：{reason}（同一原因的后续失败合并记录，恢复时给汇总）"
            ));
        }
    }
}

/// 记录一次成功：若之前在故障中，汇总一条「已恢复」（带抑制了多少次采样）。
fn note_ok() {
    let hit = FAIL.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((reason, n)) = hit {
        crate::logging::info(&format!(
            "LibreHardwareService 已恢复（此前「{reason}」连续 {n} 次采样未取到，日志已抑制）"
        ));
    }
}

/// 读一遍并映射成指标；读取失败或没有有效传感器时返回 `None`，由 metrics 补齐缺值。
/// 成功结果始终包含 LHS 的 cpu_load（有效值或 None）；自算模式由 metrics 过滤该项。
pub fn read() -> Option<Vec<BasicMetric>> {
    let data = match unsafe { read_data_block() } {
        Ok(d) => d,
        Err(e) => {
            note_fail(&e);
            return None;
        }
    };
    let ss = parse(&data);
    if ss.is_empty() {
        note_fail("共享内存中没有有效传感器数据");
        return None;
    }
    note_ok();
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
    push(
        &mut out,
        "CPU",
        "cpu_load",
        "CPU 占用率",
        "%",
        1,
        select_cpu_load(&ss),
        "LHS: CPU Total",
    );
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

#[cfg(test)]
mod tests {
    use super::{MAX_MAPPING_BYTES, S, checked_range, data_block_range, mapping_capacity, parse, select_cpu_load};

    #[test]
    fn sensor_mutex_access_allows_release_and_next_reader() {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            CreateMutexW, GetCurrentProcessId, GetCurrentThreadId, OpenMutexW, ReleaseMutex,
            WaitForSingleObject,
        };

        struct Handle(HANDLE);
        impl Drop for Handle {
            fn drop(&mut self) {
                unsafe { CloseHandle(self.0) };
            }
        }

        // 独立于真实服务的 Local 对象，不读取/占用 LHS 正在使用的全局互斥体。
        let name = format!("Local\\jiadian-mutex-access-test-{}-{}", unsafe {
            GetCurrentProcessId()
        }, unsafe { GetCurrentThreadId() });
        let wide = super::wide(&name);
        let created = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        assert!(!created.is_null());
        let created = Handle(created);
        let reader = unsafe { OpenMutexW(super::SENSOR_MUTEX_ACCESS, 0, wide.as_ptr()) };
        assert!(!reader.is_null());
        let reader = Handle(reader);
        assert_eq!(unsafe { WaitForSingleObject(reader.0, 1000) }, WAIT_OBJECT_0);
        let released = unsafe { ReleaseMutex(reader.0) };
        if released == 0 {
            // 若访问权限回归，仍用创建时的完整权限清理所有权，避免污染测试环境。
            unsafe { ReleaseMutex(created.0) };
        }
        assert_ne!(released, 0, "传感器读取句柄必须能释放互斥体");

        let next = std::thread::spawn(move || {
            let handle = unsafe { OpenMutexW(super::SENSOR_MUTEX_ACCESS, 0, super::wide(&name).as_ptr()) };
            assert!(!handle.is_null());
            let handle = Handle(handle);
            let status = unsafe { WaitForSingleObject(handle.0, 1000) };
            if status == WAIT_OBJECT_0 {
                assert_ne!(unsafe { ReleaseMutex(handle.0) }, 0);
            }
            status
        }).join().unwrap();
        assert_eq!(next, WAIT_OBJECT_0, "后续线程不能被遗留的所有权阻塞");
    }

    fn sensor(hw_type: &str, sensor_type: &str, name: &str, value: f64) -> S {
        S {
            identifier: "/test/load/0".into(),
            name: name.into(),
            sensor_type: sensor_type.into(),
            hw_name: "测试硬件".into(),
            hw_type: hw_type.into(),
            value,
        }
    }

    fn block(meta: &[u8], data: &[u8]) -> Vec<u8> {
        let hdr = 4 + meta.len();
        let off = hdr + 20;
        let mut out = vec![0; off];
        out[..4].copy_from_slice(&(meta.len() as u32).to_le_bytes());
        out[4..hdr].copy_from_slice(meta);
        out[hdr + 12..hdr + 16].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out[hdr + 16..hdr + 20].copy_from_slice(&(off as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn parse_preserves_zero_and_skips_invalid_values() {
        let data = b" \n{\"name\":\"zero\",\"value\":0}\0{\"value\":null}\0{}\0{\"value\":\"12\"}\0{\"value\":1e999}\0{\"value\":NaN}\0{broken}\0[]\0{\"name\":\"negative\",\"value\":-2.5}\0";
        let sensors = parse(data);
        assert_eq!(sensors.len(), 2);
        assert_eq!(sensors[0].name, "zero");
        assert_eq!(sensors[0].value, 0.0);
        assert_eq!(sensors[1].value, -2.5);
        assert!(parse(b" \0{\"value\":null}\0").is_empty());
    }

    #[test]
    fn parse_maps_sensor_fields() {
        let sensors = parse(br#"{"identifier":"/cpu/0/load/0","name":"CPU Total","sensorType":"Load","hardwareName":"CPU","hardwareType":"Cpu","value":42.5}"#);
        assert_eq!(sensors.len(), 1);
        let s = &sensors[0];
        assert_eq!(s.identifier, "/cpu/0/load/0");
        assert_eq!(s.hw_name, "CPU");
        assert_eq!(s.hw_type, "Cpu");
        assert_eq!(s.sensor_type, "Load");
        assert_eq!(select_cpu_load(&sensors), Some(42.5));
    }

    #[test]
    fn cpu_load_requires_exact_case_insensitive_match() {
        let sensors = vec![
            sensor("Cpu", "Load", "Total", 11.0),
            sensor("Cpu", "Load", "CPU Total Average", 12.0),
            sensor("GpuIntel", "Load", "CPU Total", 13.0),
            sensor("Cpu", "Temperature", "CPU Total", 14.0),
            sensor("Cpu", "Load", "CPU Core #1", 15.0),
            sensor("cPu", "lOaD", "cPu tOtAl", 42.0),
        ];
        assert_eq!(select_cpu_load(&sensors), Some(42.0));
        assert_eq!(select_cpu_load(&sensors[..5]), None);
        assert_eq!(select_cpu_load(&[]), None);
    }

    #[test]
    fn cpu_load_rejects_invalid_values_and_clamps_valid_values() {
        let mut sensors = vec![
            sensor("Cpu", "Load", "CPU Total", f64::NAN),
            sensor("Cpu", "Load", "CPU Total", f64::INFINITY),
            sensor("Cpu", "Load", "CPU Total", f64::NEG_INFINITY),
            sensor("Cpu", "Load", "CPU Total", -1.0),
        ];
        assert_eq!(select_cpu_load(&sensors), None);
        sensors.push(sensor("Cpu", "Load", "CPU Total", 0.0));
        assert_eq!(select_cpu_load(&sensors), Some(0.0));
        sensors.last_mut().unwrap().value = 150.0;
        assert_eq!(select_cpu_load(&sensors), Some(100.0));
    }

    #[test]
    fn ranges_check_exact_end_overflow_and_capacity_limit() {
        assert_eq!(checked_range(12, 4, 16).unwrap(), 12..16);
        assert!(checked_range(13, 4, 16).is_err());
        assert!(checked_range(usize::MAX, 1, usize::MAX).is_err());
        assert!(checked_range(MAX_MAPPING_BYTES, 1, usize::MAX).is_err());
        assert_eq!(checked_range(0, MAX_MAPPING_BYTES, usize::MAX).unwrap(), 0..MAX_MAPPING_BYTES);
    }

    #[test]
    fn mapping_capacity_checks_region_and_limits_read_size() {
        assert_eq!(mapping_capacity(104, 100, 16).unwrap(), 12);
        assert_eq!(mapping_capacity(0, 0, MAX_MAPPING_BYTES + 1).unwrap(), MAX_MAPPING_BYTES);
        assert!(mapping_capacity(99, 100, 16).is_err());
        assert!(mapping_capacity(116, 100, 16).is_err());
        assert!(mapping_capacity(100, 100, 0).is_err());
        assert!(mapping_capacity(usize::MAX, usize::MAX - 1, 4).is_err());
    }

    #[test]
    fn data_range_accepts_metadata_and_exact_end() {
        let data = b"{\"value\":1}\0";
        let bytes = block(b"{}", data);
        let range = data_block_range(&bytes).unwrap();
        assert_eq!(range.end, bytes.len());
        assert_eq!(&bytes[range], data);
    }

    #[test]
    fn data_range_rejects_truncated_header_or_payload() {
        assert!(data_block_range(&[]).is_err());
        assert!(data_block_range(&[0; 3]).is_err());
        let bytes = block(&[], b"x");
        assert!(data_block_range(&bytes[..23]).is_err());
        assert!(data_block_range(&bytes[..24]).is_err());
        assert!(data_block_range(&u32::MAX.to_le_bytes()).is_err());
    }

    #[test]
    fn data_range_rejects_empty_overlap_and_extreme_lengths() {
        assert!(data_block_range(&block(&[], &[])).is_err());
        let mut bytes = block(&[], b"x");
        bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
        assert!(data_block_range(&bytes).is_err());
        bytes[20..24].copy_from_slice(&24u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(data_block_range(&bytes).is_err());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(data_block_range(&bytes).is_err());
    }
}

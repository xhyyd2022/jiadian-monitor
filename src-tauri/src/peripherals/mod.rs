//! 外设电量：JSON 协议库的加载、端点匹配、收发执行与后台轮询。
//!
//! 数据在 exe 同级的 `peripherals/`（开发时是 `app/peripherals/`），格式见那里的 `README.md`。
//! 本模块只做四件事：
//!
//! 1. `dir()` + `load_cached()`：读盘并把 JSON 解析成结构（文件没变就用缓存）；
//! 2. `resolve()`：拿 HID 端点去 `index.json` 查品牌与协议档位，再用 `match` 校验；
//! 3. `run_protocol()`：照协议发指令、挑回包、算电量与附加字段；
//! 4. `spawn()`：后台线程定时枚举 + 轮询，结果放进 `STATE` 给界面和 HTTP API 用。
//!
//! 设计底线：**读不到就报错，不猜**。任何解析失败都变成一条 `error`，界面显示「未知」。

pub mod expr;
pub mod hid;
pub mod model;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use model::{HexByte, MatchSpec, Protocol, RxSpec, Transport, TxFrame};

// ---------------------------------------------------------------- 目录与缓存

/// 协议库目录：exe 同级的 `peripherals` → 当前目录的 `peripherals` → 开发树。
///
/// 这里返回的是**运行时**解析出来的绝对路径，任何地方都不写死盘符或安装目录：
/// 换台机器、换个目录跑，结果自然跟着变。
pub fn dir() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            cands.push(p.join("peripherals"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("peripherals"));
    }
    #[cfg(debug_assertions)]
    {
        cands.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("peripherals"),
        );
    }
    cands.into_iter().find(|p| p.is_dir())
}

/// 相对当前进程的路径标签，形如 `<程序目录>\peripherals`。
///
/// 界面、日志、截图里一律用这个：不把本机的盘符和安装位置写死到任何地方，
/// 换台机器换个目录，标签自动跟着变。要给人真正打开目录时走资源管理器
/// （见 `lib.rs` 的 `peripherals_open_dir` 命令），而不是把绝对路径印在界面上。
pub fn dir_label(dir: &Path) -> String {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(base) = exe_dir {
        if let Ok(rel) = dir.strip_prefix(&base) {
            let rel = rel.to_string_lossy().replace('/', "\\");
            if !rel.is_empty() {
                return format!("<程序目录>\\{rel}");
            }
        }
    }
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "peripherals".to_string())
}

/// 当前可执行文件名（`jiadian-monitor.exe`）。界面上的示例命令行用它，不写死。
pub fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "jiadian-monitor.exe".to_string())
}

/// 读盘结果的缓存。键是「index.json + brands/ 下全部文件的 mtime+大小」。
struct Cache {
    stamp: String,
    loaded: Option<Arc<Loaded>>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    stamp: String::new(),
    loaded: None,
});

/// 解析好的协议库。
pub struct Loaded {
    pub dir: PathBuf,
    pub index: model::Index,
    pub brands: BTreeMap<String, model::BrandFile>,
    pub errors: Vec<String>,
}

fn file_stamp(p: &Path) -> String {
    match std::fs::metadata(p) {
        Ok(m) => {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{t}:{}", m.len())
        }
        Err(_) => "missing".into(),
    }
}

fn dir_stamp(dir: &Path) -> String {
    let mut parts = vec![file_stamp(&dir.join("index.json"))];
    if let Ok(rd) = std::fs::read_dir(dir.join("brands")) {
        let mut v: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| {
                format!(
                    "{}={}",
                    e.file_name().to_string_lossy(),
                    file_stamp(&e.path())
                )
            })
            .collect();
        v.sort();
        parts.extend(v);
    }
    parts.join("|")
}

fn load_dir(dir: &Path) -> Result<Loaded, String> {
    // 报错里只写文件名、不写绝对路径 —— 这些字符串会直接出现在界面上。
    let p = dir.join("index.json");
    let src = std::fs::read_to_string(&p).map_err(|e| format!("读不到 index.json：{e}"))?;
    let index: model::Index =
        serde_json::from_str(&src).map_err(|e| format!("index.json 解析失败：{e}"))?;

    let mut brands = BTreeMap::new();
    let mut errors = Vec::new();
    for (key, r) in &index.brands {
        let bp = dir.join(&r.file);
        match std::fs::read_to_string(&bp) {
            Ok(s) => match serde_json::from_str::<model::BrandFile>(&s) {
                Ok(bf) => {
                    brands.insert(key.clone(), bf);
                }
                Err(e) => errors.push(format!("{} 解析失败：{e}", r.file)),
            },
            Err(e) => errors.push(format!("读不到 {}：{e}", r.file)),
        }
    }
    Ok(Loaded {
        dir: dir.to_path_buf(),
        index,
        brands,
        errors,
    })
}

/// 读盘（带缓存）。文件有改动立刻生效，不需要重启。
pub fn load_cached() -> Result<Arc<Loaded>, String> {
    let dir = dir().ok_or_else(|| {
        "找不到 peripherals 目录（应在 exe 同级，或当前目录下的 peripherals）".to_string()
    })?;
    let stamp = dir_stamp(&dir);
    {
        let c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if c.stamp == stamp {
            if let Some(ld) = &c.loaded {
                return Ok(ld.clone());
            }
        }
    }
    let ld = Arc::new(load_dir(&dir)?);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.stamp = stamp;
    c.loaded = Some(ld.clone());
    Ok(ld)
}

// ---------------------------------------------------------------- 匹配解析

/// 一个 HID 端点匹配到的完整读取方案。
#[derive(Clone, Copy)]
struct Resolved<'a> {
    brand_key: &'a str,
    brand_name: &'a str,
    protocol_name: &'a str,
    protocol: &'a Protocol,
    /// `index.json` 里命中的条目；走公版兜底时为 `None`（没登记就没有型号/分类可继承）。
    entry: Option<&'a model::DeviceEntry>,
}

fn match_endpoint(m: Option<&MatchSpec>, ep: &hid::Endpoint) -> bool {
    let Some(m) = m else { return true };
    if let Some(v) = m.usage_page {
        if ep.usage_page != v.0 {
            return false;
        }
    }
    if let Some(v) = m.usage {
        if ep.usage != v.0 {
            return false;
        }
    }
    if let Some(v) = m.input_length {
        if ep.input_len != v {
            return false;
        }
    }
    if let Some(v) = m.output_length {
        if ep.output_len != v {
            return false;
        }
    }
    if let Some(v) = m.feature_length {
        if ep.feature_len != v {
            return false;
        }
    }
    if let Some(s) = &m.product_like {
        let needle = s.to_lowercase();
        if !ep.product.to_lowercase().contains(&needle) {
            return false;
        }
    }
    true
}

/// 给一个 HID 端点选品牌与协议档位：精确 VID:PID 优先于通配，指名档位优先于其它。
/// 公版兜底：`index.json` 里没登记的设备，只要端点对得上 `brands/generic.json` 的档位就照读。
///
/// 约定是「默认公版，读不到再报错」——不是「没收录就消失」。标准 HID 电量
/// （UsagePage 0006 / Usage 0020）本来就是协议自带的，不少无线键鼠/手柄直接按这个报，
/// 没理由因为查不到 VID:PID 就把整台设备丢掉。
fn generic_fallback<'a>(ld: &'a Loaded, ep: &hid::Endpoint) -> Option<Resolved<'a>> {
    let bf = ld.brands.get("generic")?;
    let brand_name = ld
        .index
        .brands
        .get("generic")
        .map(|b| if b.name.is_empty() { "generic" } else { b.name.as_str() })
        .unwrap_or("通用 HID");
    let mut names: Vec<&str> = Vec::new();
    if let Some(p) = bf.default.as_deref() {
        names.push(p);
    }
    for k in bf.protocols.keys() {
        if !names.iter().any(|n| *n == k.as_str()) {
            names.push(k.as_str());
        }
    }
    for n in names {
        let Some((k, pr)) = bf.protocols.get_key_value(n) else {
            continue;
        };
        if !match_endpoint(pr.match_spec.as_ref(), ep) {
            continue;
        }
        return Some(Resolved {
            brand_key: "generic",
            brand_name,
            protocol_name: k.as_str(),
            protocol: pr,
            entry: None,
        });
    }
    None
}

/// PID 命中、VID 没命中的候选档位。
///
/// 场景：`index.json` 登记的是 `3554:F503`，用户手里是换壳/换接收器的同款 —— PID 认得、VID 没收录。
/// 老逻辑按 VID:PID 精确匹配，一台对不上就整台消失。这里把「同 PID 但不同 VID」的条目的档位
/// 按顺序列出来，交给 `probe_once()` 真机试：谁先读回合法电量就用谁。
fn loose_candidates<'a>(ld: &'a Loaded, ep: &hid::Endpoint) -> Vec<Resolved<'a>> {
    let mut out: Vec<Resolved<'a>> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    // 只认「PID 是写死的具体值」的条目。`pid: "*"` 的含义是「这个 VID 下所有型号」，
    // 拿它去按 PID 猜品牌等于对全机设备乱试——本机实测会把主板灯效、USB 音频都写一遍。
    if !looks_like_peripheral(ep) {
        return out;
    }
    for d in &ld.index.devices {
        if !matches!(d.pid, model::VidPid::Exact(_)) || !d.pid.matches(ep.pid) {
            continue;
        }
        if d.vid.matches(ep.vid) {
            continue;
        }
        let Some(brand_ref) = ld.index.brands.get(&d.brand) else {
            continue;
        };
        let Some(bf) = ld.brands.get(&d.brand) else {
            continue;
        };
        let mut names: Vec<&str> = Vec::new();
        if let Some(p) = d.protocol.as_deref() {
            names.push(p);
        }
        if let Some(p) = bf.default.as_deref() {
            if !names.contains(&p) {
                names.push(p);
            }
        }
        for k in bf.protocols.keys() {
            if !names.iter().any(|n| *n == k.as_str()) {
                names.push(k.as_str());
            }
        }
        for n in names {
            let Some((k, pr)) = bf.protocols.get_key_value(n) else {
                continue;
            };
            if !match_endpoint(pr.match_spec.as_ref(), ep) {
                continue;
            }
            let key = (d.brand.clone(), k.to_string());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            out.push(Resolved {
                brand_key: d.brand.as_str(),
                brand_name: if brand_ref.name.is_empty() {
                    d.brand.as_str()
                } else {
                    brand_ref.name.as_str()
                },
                protocol_name: k.as_str(),
                protocol: pr,
                entry: Some(d),
            });
        }
    }
    out
}

/// 打开设备真试一遍：先试只读档位（Feature / Input），再试要写查询帧的 Output 档位。
/// 返回第一个读回合法电量的方案。
///
/// 只在「PID 命中、VID 没命中」时用，候选集来自同一 PID 的登记条目，不会拿无关设备的协议乱写。
fn probe_once<'a>(ld: &'a Loaded, ep: &hid::Endpoint) -> Option<(Resolved<'a>, Readout)> {
    let (read_only, writes): (Vec<Resolved<'a>>, Vec<Resolved<'a>>) = loose_candidates(ld, ep)
        .into_iter()
        .partition(|c| c.protocol.transport != Transport::Output);
    for c in read_only.into_iter().chain(writes) {
        let Ok(dev) = hid::HidDevice::open(ep) else {
            continue;
        };
        if let Ok(out) = run_protocol(c.protocol, &dev, ep) {
            if out.battery.is_some() {
                return Some((c, out));
            }
        }
    }
    None
}

fn resolve<'a>(ld: &'a Loaded, ep: &hid::Endpoint) -> Option<Resolved<'a>> {
    let mut cands: Vec<&model::DeviceEntry> = ld
        .index
        .devices
        .iter()
        .filter(|d| d.accepts(ep.vid, ep.pid))
        .collect();
    if cands.is_empty() {
        // 没登记 → 先看公版协议认不认这个端点（标准 HID 电量）。
        return generic_fallback(ld, ep);
    }
    cands.sort_by_key(|d| d.rank());

    for d in cands {
        let Some(brand_ref) = ld.index.brands.get(&d.brand) else {
            continue;
        };
        let Some(bf) = ld.brands.get(&d.brand) else {
            continue;
        };
        // 候选档位：index 指名 → 品牌 default → 该文件其余档位。
        let mut names: Vec<&str> = Vec::new();
        if let Some(p) = d.protocol.as_deref() {
            names.push(p);
        }
        if let Some(p) = bf.default.as_deref() {
            if !names.contains(&p) {
                names.push(p);
            }
        }
        for k in bf.protocols.keys() {
            if !names.iter().any(|n| *n == k.as_str()) {
                names.push(k.as_str());
            }
        }
        for n in names {
            let Some(pr) = bf.protocols.get_key_value(n) else {
                continue;
            };
            if !match_endpoint(pr.1.match_spec.as_ref(), ep) {
                continue;
            }
            return Some(Resolved {
                brand_key: d.brand.as_str(),
                brand_name: if brand_ref.name.is_empty() {
                    d.brand.as_str()
                } else {
                    brand_ref.name.as_str()
                },
                protocol_name: pr.0.as_str(),
                protocol: pr.1,
                entry: Some(d),
            });
        }
    }
    // 品牌认得、但私有协议全对不上：最后再看公版（比如这只鼠标同时暴露了标准电量集合）。
    generic_fallback(ld, ep)
}

/// 只认品牌、不认协议：`index.json` 里登记过这个 VID:PID 就把它找出来。
///
/// 用在「品牌认得，但品牌文件里没有档位匹配上它的端点」这种情形——比如本机的
/// 雷柏 24AE:1205。以前这种设备会被 `resolve()` 直接丢掉，整台设备从列表里
/// 消失，用户只看到「什么都没有」，既不知道品牌也不到该改哪个 JSON。现在改成
/// 仍然列出来（品牌 + 型号 + 电量未知 + 一句原因），把问题指回配置文件。
fn entry_only<'a>(
    ld: &'a Loaded,
    vid: u16,
    pid: u16,
) -> Option<(&'a str, &'a str, &'a model::DeviceEntry)> {
    let mut cands: Vec<&model::DeviceEntry> = ld
        .index
        .devices
        .iter()
        .filter(|d| d.accepts(vid, pid))
        .collect();
    if cands.is_empty() {
        return None;
    }
    cands.sort_by_key(|d| d.rank());
    let d = cands[0];
    let brand_name = ld
        .index
        .brands
        .get(&d.brand)
        .map(|b| {
            if b.name.is_empty() {
                d.brand.as_str()
            } else {
                b.name.as_str()
            }
        })
        .unwrap_or(d.brand.as_str());
    Some((d.brand.as_str(), brand_name, d))
}

/// 一个设备暴露的端点摘要，写进「没匹配上」的原因里，方便直接照着补 JSON。
fn endpoint_brief(ep: &hid::Endpoint) -> String {
    let base = format!(
        "UsagePage {:04X}/Usage {:04X}（输入 {} 输出 {} Feature {}）",
        ep.usage_page, ep.usage, ep.input_len, ep.output_len, ep.feature_len
    );
    match &ep.open_error {
        None => base,
        // 端点被别的程序攥着时，就算 match 写对了也读不到 —— 必须写进原因里。
        Some(e) => format!("{base}，且打不开：{e}"),
    }
}

/// 这个端点像不像「外设」（鼠标 / 键盘 / 耳机 / 接收器）。
///
/// 只用来给「未收录设备」提示做过滤：不滤的话，主板灯效、USB 音频、显示器 HID
/// 全都会被当成「未收录的外设」，提示就没意义了。
fn looks_like_peripheral(ep: &hid::Endpoint) -> bool {
    // 标准鼠标 / 键盘集合
    if ep.usage_page == 0x0001 && matches!(ep.usage, 0x02 | 0x06) {
        return true;
    }
    // 厂商自定义页：外设的私有控制接口都在这一段；排掉音频常用的 FFA0 / FFC0
    (0xFF00..=0xFFFE).contains(&ep.usage_page) && !matches!(ep.usage_page, 0xFFA0 | 0xFFC0)
}

// ---------------------------------------------------------------- 结果结构

/// 一台识别到的外设。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// `"1532"`
    pub vid: String,
    /// `"0555"`
    pub pid: String,
    /// 品牌显示名
    pub brand: String,
    /// 品牌 key（与 index.json 一致）
    pub brand_key: String,
    /// 型号
    pub model: String,
    /// 品牌 + 型号，方便直接显示
    pub name: String,
    /// `Mouse`/`Keyboard`/`Headset`/...
    pub category: String,
    /// 协议档位名
    pub protocol: String,
    /// `feature` | `output` | `input`
    pub transport: String,
    /// 电量百分比，`None` = 未知
    pub battery: Option<u8>,
    /// 是否充电，`None` = 未知（协议读不到就别猜）
    pub charging: Option<bool>,
    /// 这一次是否真读到了
    pub online: bool,
    /// 读数来自退避缓存（设备在睡觉）
    pub stale: bool,
    /// `index.json` 里登记了这个 VID:PID，但没有任何协议档位匹配上它的端点。
    /// 这种情况下设备仍然出现在列表里（品牌型号照常显示），电量是未知的，
    /// `error` 会写清楚是哪几个端点、该去补哪份 JSON。
    pub supported: bool,
    pub note: Option<String>,
    pub error: Option<String>,
    /// 协议里其它数值字段（millivolts 等）
    pub extras: BTreeMap<String, f64>,
}

impl Reading {
    fn name_of(brand: &str, model: &str) -> String {
        if model.is_empty() {
            brand.to_string()
        } else if model.starts_with(brand) {
            model.to_string()
        } else {
            format!("{brand} {model}")
        }
    }
}

/// 一次轮询的整体快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub ok: bool,
    /// 协议库目录的绝对路径。
    ///
    /// **只给程序自己用**（打开目录、排查问题），不要在界面上直接显示：
    /// 界面用下面的 `dir_label`，免得把本机盘符写进任何呈现给人看的地方。
    pub dir: String,
    /// 目录的相对标签，形如 `<程序目录>\peripherals`。界面显示这个。
    pub dir_label: String,
    /// 当前可执行文件名，界面拼示例命令用。
    pub exe_name: String,
    pub poll_ms: u64,
    pub devices: Vec<Reading>,
    pub errors: Vec<String>,
    pub updated_ms: u64,
}

impl Snapshot {
    const EMPTY: Snapshot = Snapshot {
        ok: false,
        dir: String::new(),
        dir_label: String::new(),
        exe_name: String::new(),
        poll_ms: 3000,
        devices: Vec::new(),
        errors: Vec::new(),
        updated_ms: 0,
    };
}

static STATE: Mutex<Snapshot> = Mutex::new(Snapshot::EMPTY);

/// 当前快照（不触发读设备）。
pub fn snapshot() -> Snapshot {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 快照的 JSON 形式，给 HTTP API 与 Tauri 命令用。
pub fn json() -> serde_json::Value {
    serde_json::to_value(snapshot()).unwrap_or_else(|e| {
        serde_json::json!({ "ok": false, "devices": [], "errors": [format!("序列化失败：{e}")] })
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hex_prefix(b: &[u8]) -> String {
    b.iter()
        .take(16)
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------- 组帧与求值

fn clamp_byte(v: f64) -> u8 {
    let n = v.round();
    if !n.is_finite() {
        return 0;
    }
    (n as i64 & 0xFF) as u8
}

/// 按 JSON 里的一帧描述组出要发出去的字节。
fn build_frame(f: &TxFrame) -> Vec<u8> {
    let len = f.length.max(1);
    let pad = f.pad.map(|b| b.0).unwrap_or(0);
    let mut buf = vec![pad; len];
    buf[0] = f.report_id.map(|b| b.0).unwrap_or(0);
    for (k, v) in &f.bytes {
        if let Ok(off) = k.trim().parse::<usize>() {
            if off < len {
                buf[off] = v.0;
            }
        }
    }
    if let Some(cs) = &f.checksum {
        if let Ok(v) = expr::eval(&buf, &cs.expr) {
            if cs.put < len {
                buf[cs.put] = clamp_byte(v);
            }
        }
    }
    buf
}

/// 一个数值字段的解析：`valid` 条件 → 求值 → 区间判定 → 取整。
fn parse_number(
    buf: &[u8],
    src: &str,
    round: Option<bool>,
    min: Option<f64>,
    max: Option<f64>,
    valid: Option<&str>,
) -> Result<f64, String> {
    if let Some(v) = valid {
        let ok = expr::eval(buf, v).map_err(|e| format!("valid 求值失败：{e}"))?;
        if ok == 0.0 {
            return Err("valid 条件不满足".into());
        }
    }
    let raw = expr::eval(buf, src)?;
    let lo = min.unwrap_or(0.0);
    let hi = max.unwrap_or(100.0);
    if raw < lo || raw > hi {
        return Err(format!("读数 {raw} 不在 [{lo}, {hi}] 内"));
    }
    Ok(if round.unwrap_or(true) {
        raw.round()
    } else {
        raw
    })
}

/// 单次读数集合。
#[derive(Debug, Clone, Default)]
struct Readout {
    battery: Option<u8>,
    charging: Option<bool>,
    model: Option<String>,
    extras: BTreeMap<String, f64>,
    battery_err: Option<String>,
}

fn apply_field(f: &model::FieldSpec, buf: &[u8], out: &mut Readout) {
    if let Some(a) = &f.ascii {
        let end = (a.offset.saturating_add(a.length)).min(buf.len());
        if a.offset < end {
            let s = String::from_utf8_lossy(&buf[a.offset..end])
                .trim_end_matches('\u{0}')
                .trim()
                .to_string();
            if !s.is_empty() && f.name == "model" {
                out.model = Some(s);
            }
        }
        return;
    }
    let Some(src) = f.expr.as_deref() else {
        return;
    };
    if f.name == "charging" {
        if let Ok(v) = expr::eval(buf, src) {
            out.charging = Some(v != 0.0);
        }
        return;
    }
    if let Ok(v) = parse_number(buf, src, f.round, f.min, f.max, f.valid.as_deref()) {
        out.extras.insert(f.name.clone(), v);
    }
}

/// 从一块已经拿到的回包解析电量与「不需要自己发指令」的附加字段。
fn parse_from(proto: &Protocol, buf: &[u8]) -> Readout {
    let mut out = Readout::default();
    match parse_number(
        buf,
        &proto.battery.expr,
        proto.battery.round,
        proto.battery.min,
        proto.battery.max,
        proto.battery.valid.as_deref(),
    ) {
        Ok(v) => out.battery = Some(v.clamp(0.0, 255.0) as u8),
        Err(e) => out.battery_err = Some(e),
    }
    for f in &proto.fields {
        if f.tx.is_none() && f.rx.is_none() {
            apply_field(f, buf, &mut out);
        }
    }
    out
}

// ---------------------------------------------------------------- 逐个设备读写

fn drain_input(dev: &hid::HidDevice) {
    for _ in 0..8 {
        match dev.read_input(2) {
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
}

fn send_frames(
    dev: &hid::HidDevice,
    transport: Transport,
    frames: &[TxFrame],
    ep: &hid::Endpoint,
) -> Result<(), String> {
    if transport == Transport::Input {
        return Err("transport=input 不发送指令".into());
    }
    if transport == Transport::Output {
        drain_input(dev);
    }
    for fr in frames {
        let mut buf = build_frame(fr);
        match transport {
            Transport::Feature => {
                let want = ep.feature_len as usize;
                if want == 0 {
                    return Err("该端点没有 Feature 报告，检查 match.featureLength".into());
                }
                buf.resize(want, 0);
                dev.set_feature(&buf)?;
            }
            Transport::Output => {
                let want = ep.output_len as usize;
                if want == 0 {
                    return Err("该端点没有 Output 报告".into());
                }
                buf.resize(want, 0);
                dev.write_output(&buf)?;
            }
            Transport::Input => {}
        }
        if let Some(d) = fr.delay_ms {
            if d > 0 {
                std::thread::sleep(Duration::from_millis(d.min(2000)));
            }
        }
    }
    Ok(())
}

fn read_reply(
    dev: &hid::HidDevice,
    transport: Transport,
    rx: &RxSpec,
    report_id: u8,
) -> Result<Vec<u8>, String> {
    let attempts = rx.attempts();
    let timeout = rx.timeout();
    let interval = rx.interval();
    let mut last = String::new();

    for i in 0..attempts {
        if transport == Transport::Feature || i > 0 {
            std::thread::sleep(Duration::from_millis(interval.min(2000)));
        }
        let buf = match transport {
            Transport::Feature => {
                let n = rx.length.unwrap_or(dev.feature_len).max(1);
                let mut b = vec![0u8; n];
                b[0] = report_id;
                dev.get_feature(&mut b)?;
                b
            }
            _ => {
                let wait = timeout.min(5000) as u32;
                match dev.read_input(wait) {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        last = "等待超时".into();
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        if buf.is_empty() {
            last = "空回包".into();
            continue;
        }
        if rx.should_skip(&buf) {
            last = format!("设备忙/等待帧 {}", hex_prefix(&buf));
            continue;
        }
        if rx.accept(&buf) {
            return Ok(buf);
        }
        last = format!("回包不匹配 {}", hex_prefix(&buf));
    }
    Err(format!(
        "{attempts} 次尝试未拿到有效回包（最后一次：{last}）"
    ))
}

fn run_protocol(
    proto: &Protocol,
    dev: &hid::HidDevice,
    ep: &hid::Endpoint,
) -> Result<Readout, String> {
    let mut frames = proto.tx.as_ref().map(|t| t.frames()).unwrap_or_default();
    let report_id = frames
        .last()
        .and_then(|f| f.report_id)
        .map(|b| b.0)
        .unwrap_or(0);

    // ① 前置步骤：查 Feature 索引之类，把回包里的字节填进后面帧。
    for step in &proto.pre {
        if let Some(tx) = &step.tx {
            send_frames(dev, proto.transport, &tx.frames(), ep)?;
        }
        let rx = step.rx.clone().or_else(|| proto.rx.clone());
        let (Some(rx), Some(take)) = (rx, step.take) else {
            continue;
        };
        let buf = read_reply(dev, proto.transport, &rx, report_id)?;
        let v = *buf.get(take.offset).ok_or_else(|| {
            format!(
                "pre 取值偏移 {} 越界（回包 {} 字节）",
                take.offset,
                buf.len()
            )
        })?;
        if let Some(put) = step.put {
            if let Some(fr) = frames.get_mut(put.frame) {
                fr.bytes.insert(put.offset.to_string(), HexByte(v));
            }
        }
    }

    // ② 主指令。
    if !frames.is_empty() {
        send_frames(dev, proto.transport, &frames, ep)?;
    }
    let Some(rx) = proto.rx.clone() else {
        if frames.is_empty() {
            return Err("协议既没有 tx 也没有 rx".into());
        }
        return Err("协议没有 rx，拿不到回包".into());
    };
    let buf = read_reply(dev, proto.transport, &rx, report_id)?;
    let mut out = parse_from(proto, &buf);
    if let Some(e) = out.battery_err.clone() {
        return Err(format!("电量解析失败：{e}"));
    }

    // ③ 需要自己发指令的附加字段（雷蛇查充电就是这样）。
    for f in &proto.fields {
        if f.tx.is_none() && f.rx.is_none() {
            continue;
        }
        let fbuf = (|| -> Result<Vec<u8>, String> {
            if let Some(tx) = &f.tx {
                send_frames(dev, proto.transport, &tx.frames(), ep)?;
            }
            let take_rx = f.rx.clone().or_else(|| proto.rx.clone());
            let Some(frx) = take_rx else {
                return Ok(buf.clone());
            };
            let rid = f
                .tx
                .as_ref()
                .and_then(|t| t.frames().last().and_then(|x| x.report_id))
                .map(|b| b.0)
                .unwrap_or(report_id);
            read_reply(dev, proto.transport, &frx, rid)
        })();
        if let Ok(fbuf) = fbuf {
            apply_field(f, &fbuf, &mut out);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 一轮轮询

struct DevState {
    streak: u32,
    next_due: Instant,
    last: Option<Reading>,
}

struct Poller {
    devs: BTreeMap<String, DevState>,
    /// 「PID 命中、VID 没命中」时试出来的结论：`(vid,pid) -> (品牌 key, 档位名)`。
    /// 缓存住就不必每 3 秒重试一遍，设备拔掉后跟着 `devs` 一起清。
    decided: BTreeMap<(u16, u16), (String, String)>,
}

impl Poller {
    fn new() -> Poller {
        Poller {
            devs: BTreeMap::new(),
            decided: BTreeMap::new(),
        }
    }

    fn run_round(&mut self) -> Snapshot {
        let mut snap = Snapshot {
            updated_ms: now_ms(),
            ..Snapshot::EMPTY
        };

        let ld = match load_cached() {
            Ok(l) => l,
            Err(e) => {
                // 目录或 index.json 有问题：清空缓存，界面显示原因
                self.devs.clear();
                snap.ok = false;
                snap.errors.push(e);
                return snap;
            }
        };
        snap.dir = ld.dir.to_string_lossy().to_string();
        snap.dir_label = dir_label(&ld.dir);
        snap.exe_name = exe_name();
        snap.poll_ms = ld.index.poll.interval_ms.max(500);
        snap.ok = true;
        snap.errors = ld.errors.clone();

        let endpoints = match hid::list_endpoints() {
            Ok(v) => v,
            Err(e) => {
                snap.ok = false;
                snap.errors.push(format!("枚举 HID 失败：{e}"));
                return snap;
            }
        };

        // 同一个 VID:PID 只保留一个方案，品牌专用优先于通用兜底。
        let mut chosen: BTreeMap<(u16, u16), Resolved<'_>> = BTreeMap::new();
        // 品牌认得、但没有档位匹配上端点的设备：照常列出来，只是读数未知。
        let mut unmatched: BTreeMap<(u16, u16), Vec<&hid::Endpoint>> = BTreeMap::new();
        for ep in &endpoints {
            if ep.input_len == 0 && ep.output_len == 0 && ep.feature_len == 0 {
                continue;
            }
            let r = match resolve(&ld, ep) {
                Some(r) => Some(r),
                None => {
                    // 精确匹配失败：如果「PID 命中、VID 没命中」，就真机试一遍候选档位，
                    // 试出来的结论按 (VID,PID) 缓存，后面几轮直接复用，不反复开设备。
                    let key = (ep.vid, ep.pid);
                    let cached = self.decided.get(&key).cloned();
                    let hit = cached.and_then(|(bk, pn)| {
                        loose_candidates(&ld, ep)
                            .into_iter()
                            .find(|c| c.brand_key == bk && c.protocol_name == pn)
                    });
                    match hit {
                        Some(r) => Some(r),
                        None => match probe_once(&ld, ep) {
                            Some((r, _out)) => {
                                self.decided
                                    .insert(key, (r.brand_key.to_string(), r.protocol_name.to_string()));
                                Some(r)
                            }
                            None => None,
                        },
                    }
                }
            };
            let Some(r) = r else {
                if entry_only(&ld, ep.vid, ep.pid).is_some() {
                    unmatched
                        .entry((ep.vid, ep.pid))
                        .or_default()
                        .push(ep);
                }
                continue;
            };
            match chosen.get_mut(&(ep.vid, ep.pid)) {
                Some(cur) => {
                    if cur.brand_key == "generic" && r.brand_key != "generic" {
                        *cur = r;
                    }
                }
                None => {
                    chosen.insert((ep.vid, ep.pid), r);
                }
            }
        }
        // 已经能读的设备不该再出现在「没匹配上」里。
        for k in chosen.keys() {
            unmatched.remove(k);
        }
        // 试出来的结论只对当前还在的设备有效。
        self.decided
            .retain(|k, _| chosen.contains_key(k) || unmatched.contains_key(k));

        // 本轮没出现的设备直接忘掉，下次插上按新设备处理。
        let seen: Vec<String> = chosen
            .keys()
            .chain(unmatched.keys())
            .map(|(v, p)| format!("{v:04X}:{p:04X}"))
            .collect();
        self.devs.retain(|k, _| seen.contains(k));

        let base = Duration::from_millis(snap.poll_ms);
        for ((vid, pid), res) in &chosen {
            let key = format!("{vid:04X}:{pid:04X}");
            let ep = match endpoints.iter().find(|e| {
                e.vid == *vid && e.pid == *pid && match_endpoint(res.protocol.match_spec.as_ref(), e)
            }) {
                Some(e) => e,
                None => continue,
            };

            let now = Instant::now();
            let st = self.devs.entry(key.clone()).or_insert(DevState {
                streak: 0,
                next_due: now,
                last: None,
            });

            if now < st.next_due {
                if let Some(mut r) = st.last.clone() {
                    r.stale = true;
                    snap.devices.push(r);
                }
                continue;
            }

            let model = res
                .entry
                .and_then(|e| e.model.clone())
                .unwrap_or_else(|| ep.product.clone());
            let mut reading = Reading {
                vid: format!("{vid:04X}"),
                pid: format!("{pid:04X}"),
                brand: res.brand_name.to_string(),
                brand_key: res.brand_key.to_string(),
                model: model.clone(),
                name: Reading::name_of(res.brand_name, &model),
                category: res
                    .entry
                    .and_then(|e| e.category.clone())
                    .unwrap_or_else(|| "Other".into()),
                protocol: res.protocol_name.to_string(),
                transport: res.protocol.transport.label().to_string(),
                note: {
                    // 条目认得 PID、但不认得这个 VID：说明档位是「试」出来的，得写在界面上，
                    // 不然用户以为这个 VID 已经被正式收录了。
                    let loose = res
                        .entry
                        .map(|e| !e.accepts(ep.vid, ep.pid))
                        .unwrap_or(false);
                    let base = res
                        .entry
                        .and_then(|e| e.note.clone())
                        .or_else(|| res.protocol.note.clone());
                    if loose {
                        Some(match base {
                            Some(t) => format!("PID 命中、VID 未收录，档位是自动试出来的；{t}"),
                            None => "PID 命中、VID 未收录，档位是自动试出来的".to_string(),
                        })
                    } else {
                        base
                    }
                },
                supported: true,
                ..Reading::default()
            };

            let result = hid::HidDevice::open(ep).and_then(|dev| run_protocol(res.protocol, &dev, ep));
            match result {
                Ok(out) => {
                    reading.online = true;
                    reading.battery = out.battery;
                    reading.charging = out.charging;
                    reading.extras = out.extras;
                    if let Some(m) = out.model {
                        reading.model = m.clone();
                        reading.name = Reading::name_of(res.brand_name, &m);
                    }
                    st.streak = 0;
                    st.next_due = now + base;
                }
                Err(e) => {
                    reading.online = false;
                    reading.charging = None;
                    reading.error = Some(e);
                    st.streak = st.streak.saturating_add(1);
                    // 连续读不到就退避到 3 倍间隔：设备在睡觉，别死磕。
                    st.next_due = now
                        + if st.streak >= 3 {
                            base * 3
                        } else {
                            base
                        };
                }
            }
            st.last = Some(reading.clone());
            snap.devices.push(reading);
        }

        // 品牌认得但协议不匹配的设备：照样列出来，电量留空、原因写清楚。
        for ((vid, pid), eps) in &unmatched {
            let Some((brand_key, brand_name, entry)) = entry_only(&ld, *vid, *pid) else {
                continue;
            };
            // 型号优先用 index 里登记的；没登记就退回端点自报的产品名。
            let model = entry
                .model
                .clone()
                .or_else(|| {
                    eps.iter()
                        .map(|e| e.product.trim().to_string())
                        .find(|s| !s.is_empty())
                })
                .unwrap_or_else(|| "未知型号".to_string());
            let briefs: Vec<String> = eps.iter().map(|e| endpoint_brief(e)).collect();
            let proto_hint = entry.protocol.as_deref().unwrap_or("default");
            snap.devices.push(Reading {
                vid: format!("{vid:04X}"),
                pid: format!("{pid:04X}"),
                brand: brand_name.to_string(),
                brand_key: brand_key.to_string(),
                model: model.clone(),
                name: Reading::name_of(brand_name, &model),
                category: entry.category.clone().unwrap_or_else(|| "Other".into()),
                protocol: String::new(),
                transport: String::new(),
                battery: None,
                charging: None,
                online: false,
                stale: false,
                supported: false,
                note: entry.note.clone(),
                error: Some(format!(
                    "认得品牌，但 brands/{brand_key}.json 里没有匹配这些端点的协议档位，电量未知；\
                     已知端点：{}。要支持它就照 README.md 加一个 match 对得上的档位（当前 index 指向 {proto_hint}）",
                    briefs.join("；")
                )),
                extras: BTreeMap::new(),
            });
        }

        // `index.json` 里查不到 VID:PID 的设备：以前直接静默丢弃，界面只剩一句「没有识别到已收录的外设」，
        // 用户根本分不清「没插上 / 没收录 / 被厂商软件占着」。这里汇总成一行提示，只做展示，不参与读数。
        let mut unknown: BTreeMap<(u16, u16), String> = BTreeMap::new();
        for ep in &endpoints {
            if chosen.contains_key(&(ep.vid, ep.pid)) || unmatched.contains_key(&(ep.vid, ep.pid)) {
                continue;
            }
            if !looks_like_peripheral(ep) {
                continue;
            }
            unknown
                .entry((ep.vid, ep.pid))
                .or_insert_with(|| ep.product.trim().to_string());
        }
        if !unknown.is_empty() {
            let mut items: Vec<String> = unknown
                .iter()
                .map(|((v, p), name)| {
                    if name.is_empty() {
                        format!("{v:04X}:{p:04X}")
                    } else {
                        format!("{v:04X}:{p:04X}（{name}）")
                    }
                })
                .collect();
            let extra = items.len().saturating_sub(6);
            items.truncate(6);
            snap.errors.push(format!(
                "未收录的 HID 设备：{}{}。在 peripherals/index.json 的 devices 里补一条 vid/pid 就能开始识别",
                items.join("、"),
                if extra > 0 {
                    format!(" 等 {} 个", items.len() + extra)
                } else {
                    String::new()
                }
            ));
        }

        snap.devices.sort_by(|a, b| {
            a.brand_key
                .cmp(&b.brand_key)
                .then_with(|| a.model.cmp(&b.model))
        });
        snap
    }
}

/// 立刻重读协议库并重新轮询（界面点「刷新」用）。
static FORCE: AtomicBool = AtomicBool::new(false);

pub fn refresh() {
    FORCE.store(true, Ordering::SeqCst);
}

/// 后台轮询线程。进程内只该调用一次。
pub fn spawn() {
    let _ = std::thread::Builder::new()
        .name("peripherals".into())
        .spawn(move || {
            let mut poller = Poller::new();
            loop {
                let started = Instant::now();
                let snap = poller.run_round();
                let interval = Duration::from_millis(snap.poll_ms.max(500));
                {
                    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
                    *g = snap;
                }
                // 分小段睡，这样外部点刷新能在 200ms 内生效。
                while started.elapsed() < interval {
                    if FORCE.swap(false, Ordering::SeqCst) {
                        // 强制重读盘：清掉缓存戳
                        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                        c.stamp.clear();
                        c.loaded = None;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                let _ = FORCE.swap(false, Ordering::SeqCst);
            }
        });
}

// ---------------------------------------------------------------- 诊断入口

/// `--peripherals-hid`：列出本机所有 HID 端点。
pub fn hid_dump() -> String {
    let eps = match hid::list_endpoints() {
        Ok(v) => v,
        Err(e) => return format!("枚举 HID 失败：{e}"),
    };
    let closed = eps.iter().filter(|e| e.open_error.is_some()).count();
    let mut s = format!("HID 端点数：{}（其中 {closed} 个只能查询、不能读写）\n", eps.len());
    s.push_str(&format!(
        "{:<9} {:<6} {:<6} {:>4} {:>4} {:>4} {:<24} {:<16} {}\n",
        "VID:PID", "UsageP", "Usage", "In", "Out", "Feat", "产品名", "厂商", "状态"
    ));
    for e in &eps {
        s.push_str(&format!(
            "{:04X}:{:04X} {:04X}   {:04X}   {:>4} {:>4} {:>4} {:<24} {:<16} {}\n",
            e.vid,
            e.pid,
            e.usage_page,
            e.usage,
            e.input_len,
            e.output_len,
            e.feature_len,
            if e.product.is_empty() { "-" } else { &e.product },
            if e.manufacturer.is_empty() { "-" } else { &e.manufacturer },
            match &e.open_error {
                None => "可读写".to_string(),
                Some(err) => format!("只读查询（{err}）"),
            }
        ));
    }
    if closed > 0 {
        s.push_str(
            "\n只读查询的端点发不了 Feature/Output 报告，电量读不到；系统独占的鼠标/键盘集合属于这一类。\n\
             如果某只鼠标在这里连一行都没有，说明它没枚举出 HID 集合（没插好 / 只走蓝牙 LE 未连接）。\n",
        );
    }
    s
}

/// `--peripherals-probe`：把所有「精确匹配不上、但 PID 有登记」的设备真机试一遍，
/// 逐个候选档位打印结果。
///
/// 用来解决「知道 PID 但不知道是哪家（VID 没收录）」的情况：跑完就知道哪个档位能读回电量，
/// 照着把 VID 补进 `index.json` 即可（补完就不用再试了）。
pub fn probe_dump() -> String {
    let ld = match load_cached() {
        Ok(l) => l,
        Err(e) => return format!("协议库读取失败：{e}"),
    };
    let eps = match hid::list_endpoints() {
        Ok(v) => v,
        Err(e) => return format!("枚举 HID 失败：{e}"),
    };
    let mut groups: BTreeMap<(u16, u16), Vec<&hid::Endpoint>> = BTreeMap::new();
    for ep in &eps {
        if ep.input_len == 0 && ep.output_len == 0 && ep.feature_len == 0 {
            continue;
        }
        groups.entry((ep.vid, ep.pid)).or_default().push(ep);
    }

    let mut s = String::new();
    for ((vid, pid), list) in &groups {
        // 已经能精确匹配的设备跳过：它们不需要试。
        if list.iter().any(|e| resolve(&ld, e).is_some()) {
            continue;
        }
        let name = list
            .iter()
            .map(|e| e.product.trim())
            .find(|n| !n.is_empty())
            .unwrap_or("-");
        s.push_str(&format!(
            "\n{vid:04X}:{pid:04X}  {name}（{} 个端点）\n",
            list.len()
        ));
        let mut tried = false;
        for ep in list {
            let cands = loose_candidates(&ld, ep);
            if cands.is_empty() {
                continue;
            }
            tried = true;
            s.push_str(&format!("  端点 {}\n", endpoint_brief(ep)));
            for c in cands {
                let r = hid::HidDevice::open(ep).and_then(|dev| run_protocol(c.protocol, &dev, ep));
                match r {
                    Ok(o) => s.push_str(&format!(
                        "    {}/{} → 电量 {:?}，充电 {:?}{}\n",
                        c.brand_key,
                        c.protocol_name,
                        o.battery,
                        o.charging,
                        if o.battery.is_some() { "  ← 可用" } else { "" }
                    )),
                    Err(e) => s.push_str(&format!(
                        "    {}/{} → 失败：{e}\n",
                        c.brand_key, c.protocol_name
                    )),
                }
            }
        }
        if !tried {
            s.push_str("  没有 PID 命中的候选档位：index.json 里没有这个 PID，需要新写一条\n");
        }
    }
    if s.is_empty() {
        s.push_str("没有需要试的设备：所有端点都已精确命中，或协议库里没有任何同 PID 的候选。\n");
    }
    s
}

/// `--peripherals-dump`：跑一轮然后打印结果 JSON。
pub fn dump_text() -> String {
    let mut poller = Poller::new();
    let snap = poller.run_round();
    {
        let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
        *g = snap.clone();
    }
    serde_json::to_string_pretty(&snap).unwrap_or_else(|e| format!("序列化失败：{e}"))
}

/// `--peripherals-selftest`：用每份协议自带的 `sample` 回放，返回 (检查数, 失败明细)。
pub fn selftest() -> (usize, Vec<String>) {
    let mut checked = 0usize;
    let mut fails: Vec<String> = Vec::new();
    let ld = match load_cached() {
        Ok(l) => l,
        Err(e) => return (0, vec![e]),
    };
    for r in ld.errors.clone() {
        fails.push(r);
    }
    for (key, bf) in &ld.brands {
        for (pname, proto) in &bf.protocols {
            // 组帧能跑通（不 panic、checksum 能求值）
            if let Some(tx) = &proto.tx {
                for fr in tx.frames() {
                    let buf = build_frame(&fr);
                    if buf.len() != fr.length.max(1) {
                        fails.push(format!("{key}/{pname}：组帧长度不符"));
                    }
                }
            }
            let Some(sample) = &proto.sample else {
                continue;
            };
            checked += 1;
            let buf = match model::parse_hex_bytes(&sample.rx) {
                Ok(b) => b,
                Err(e) => {
                    fails.push(format!("{key}/{pname}：sample.rx 无法解析（{e}）"));
                    continue;
                }
            };
            let out = parse_from(proto, &buf);
            if let Some(e) = &out.battery_err {
                fails.push(format!("{key}/{pname}：电量解析失败（{e}）"));
            }
            if let Some(exp) = sample.expect.battery {
                match out.battery {
                    Some(v) if (v as f64 - exp).abs() < 0.01 => {}
                    other => fails.push(format!(
                        "{key}/{pname}：电量期望 {exp}，实际 {:?}",
                        other
                    )),
                }
            }
            if let Some(exp) = sample.expect.charging {
                if out.charging != Some(exp) {
                    fails.push(format!(
                        "{key}/{pname}：充电期望 {exp}，实际 {:?}",
                        out.charging
                    ));
                }
            }
            if let Some(exp) = &sample.expect.model {
                if out.model.as_deref() != Some(exp.as_str()) {
                    fails.push(format!(
                        "{key}/{pname}：型号期望 {exp:?}，实际 {:?}",
                        out.model
                    ));
                }
            }
        }
    }
    (checked, fails)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_build() {
        let fr: TxFrame =
            serde_json::from_str(r#"{"length":8,"reportId":"00","bytes":{"2":"1f"},"checksum":{"put":7,"expr":"sum(0, 6) & 0xff"}}"#)
                .unwrap();
        let b = build_frame(&fr);
        assert_eq!(b.len(), 8);
        assert_eq!(b[0], 0x00);
        assert_eq!(b[2], 0x1F);
        assert_eq!(b[7], 0x1F);
    }

    #[test]
    fn all_samples_pass() {
        let (checked, fails) = selftest();
        assert!(checked > 0, "没有任何 sample 可回放");
        assert!(fails.is_empty(), "回放失败：{fails:#?}");
    }
}

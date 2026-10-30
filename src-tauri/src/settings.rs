//! 统一设置文件：exe 同级 `settings.ini` —— **所有设置都在这一个文件里**。
//!
//! 为什么必须是文件（而不是 localStorage）：`silent_start` / `logging` 得**在前端跑起来
//! 之前**就知道（`setup()` 决定窗口显不显示、`init()` 决定日志开不开），那时前端还没跑。
//! 于是让后端做唯一真值：**启动时读进内存**，改设置时**原子写回**；前端起来后通过
//! `settings_get` / `settings_set` 读写，不再各自散一个标记文件。
//!
//! 格式（`0` = 关、`1` = 开；`;` / `#` 起注释；手动改完存盘，下次启动生效）：
//! ```ini
//! [app]
//! silent_start=0
//! logging=1
//! high_fps=0
//! [ui]
//! theme=dark
//! close_to_tray=1
//! devtools=0
//! [units]
//! mode=auto
//! capacity=
//! rate=
//! temperature=
//! duration=
//! [peripherals.names]
//! ; vid:pid = 同型号共用；vid:pid@短指纹 = 只给这一只
//! 1532:0555=主用鼠标
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

/// 全部设置。**加新设置时**：加字段 → `Default` → `from_map` → `to_ini` → `to_json` →
/// `apply_patch`，五处对齐即可。
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub silent_start: bool,
    pub logging: bool,
    /// 日志级别门槛：`info` 全部 / `warn` 只记警告与错误 / `error` 只记错误。
    /// 只在 `logging` 开启时有意义。
    pub log_level: String,
    pub high_fps: bool,
    /// 手动切换 CPU 数据源：关闭用 LHS CPU Total；开启才使用 PDH/系统时间自算。
    /// 开启后可额外输出 P/E 分组；同构核不输出分组但仍遵守手动选择。
    pub hybrid_compat: bool,
    pub theme: String,
    pub close_to_tray: bool,
    pub devtools: bool,
    pub unit_mode: String,
    pub unit_capacity: String,
    pub unit_rate: String,
    pub unit_temperature: String,
    pub unit_duration: String,
    /// 外设显示名。键见 [`normalize_peripheral_key`]：
    /// `vid:pid` = 同型号共用；`vid:pid@短指纹` = 只给这一只。
    pub peripheral_names: BTreeMap<String, String>,
}

/// 外设名字的最大字符数（按 Unicode 字符计，中文算 1 个）。
///
/// 上限不是技术限制，是**显示限制**：面板一行只有那么宽，名字超过 24 个字
/// 在界面上必然被截断，不如在存盘前就截掉，保证「存进去的就是看到的」。
pub const PERIPHERAL_NAME_MAX_CHARS: usize = 24;

/// 清洗用户输入的外设名：去控制字符（换行会把 INI 写坏）、去首尾空白、按字符截断。
pub fn clean_peripheral_name(raw: &str) -> String {
    let filtered: String = raw.chars().filter(|c| !c.is_control()).collect();
    filtered.trim().chars().take(PERIPHERAL_NAME_MAX_CHARS).collect()
}

/// 规范化外设名字的键：`vid:pid` 或 `vid:pid@短指纹`，统一小写十六进制。
///
/// 非法键返回 `None`（手改 ini 写错了就忽略这一行，不让整份设置失效）。
pub fn normalize_peripheral_key(key: &str) -> Option<String> {
    let key = key.trim();
    let (base, inst) = match key.split_once('@') {
        Some((b, i)) => (b, Some(i.trim())),
        None => (key, None),
    };
    let (vid, pid) = base.split_once(':')?;
    if vid.len() != 4 || pid.len() != 4 {
        return None;
    }
    u16::from_str_radix(vid, 16).ok()?;
    u16::from_str_radix(pid, 16).ok()?;
    let base = format!("{}:{}", vid.to_ascii_lowercase(), pid.to_ascii_lowercase());
    match inst {
        None => Some(base),
        Some(i) => {
            let i = i.to_ascii_lowercase();
            if i.is_empty() || i.len() > 16 || !i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return None;
            }
            Some(format!("{base}@{i}"))
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            silent_start: false,
            logging: true,
            log_level: "info".into(),
            high_fps: false,
            hybrid_compat: false,
            theme: "dark".into(),
            close_to_tray: true,
            devtools: false,
            unit_mode: "auto".into(),
            unit_capacity: String::new(),
            unit_rate: String::new(),
            unit_temperature: String::new(),
            unit_duration: String::new(),
            peripheral_names: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// 四族单位（喂给 `units.rs`）。
    pub fn units_map(&self) -> HashMap<String, String> {
        [
            ("capacity", &self.unit_capacity),
            ("rate", &self.unit_rate),
            ("temperature", &self.unit_temperature),
            ("duration", &self.unit_duration),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
    }

    fn from_map(m: &HashMap<String, String>) -> Self {
        let d = Settings::default();
        let s = |k: &str| m.get(k).map(|v| v.as_str());
        let pick = |k: &str, allow: [&str; 2], def: &str| {
            s(k).filter(|v| allow.contains(v))
                .map(|v| v.to_string())
                .unwrap_or_else(|| def.to_string())
        };
        Self {
            silent_start: s("app.silent_start").map(truthy).unwrap_or(d.silent_start),
            logging: s("app.logging").map(truthy).unwrap_or(d.logging),
            log_level: {
                let v = s("app.log_level").unwrap_or("").trim().to_ascii_lowercase();
                if v == "info" || v == "warn" || v == "error" {
                    v
                } else {
                    d.log_level.clone()
                }
            },
            high_fps: s("app.high_fps").map(truthy).unwrap_or(d.high_fps),
            hybrid_compat: s("app.hybrid_compat").map(truthy).unwrap_or(d.hybrid_compat),
            theme: {
                let def = d.theme.clone();
                pick("ui.theme", ["dark", "light"], &def)
            },
            close_to_tray: s("ui.close_to_tray").map(truthy).unwrap_or(d.close_to_tray),
            devtools: s("ui.devtools").map(truthy).unwrap_or(d.devtools),
            unit_mode: {
                let def = d.unit_mode.clone();
                pick("units.mode", ["auto", "manual"], &def)
            },
            unit_capacity: s("units.capacity").unwrap_or("").trim().to_string(),
            unit_rate: s("units.rate").unwrap_or("").trim().to_string(),
            unit_temperature: s("units.temperature").unwrap_or("").trim().to_string(),
            unit_duration: s("units.duration").unwrap_or("").trim().to_string(),
            peripheral_names: {
                let mut names = BTreeMap::new();
                for (k, v) in m {
                    let Some(rest) = k.strip_prefix("peripherals.names.") else {
                        continue;
                    };
                    let Some(key) = normalize_peripheral_key(rest) else {
                        continue;
                    };
                    let name = clean_peripheral_name(v);
                    if !name.is_empty() {
                        names.insert(key, name);
                    }
                }
                names
            },
        }
    }

    /// 序列化成 INI（固定顺序 + 注释，方便手改）。
    fn to_ini(&self) -> String {
        let b = |v: bool| if v { 1 } else { 0 };
        let mut out = format!(
            "; jiadian-monitor 设置 —— 改完保存，程序启动时读取（运行中改请用界面）。\n\
             ; 取值：0 = 关 / 1 = 开；留空 = 跟随素材。\n\
             \n\
             [app]\n\
             ; 启动时不显示窗口，直接收进托盘（左键托盘图标恢复）\n\
             silent_start={}\n\
             ; 是否写 log/jiadian.log（0 = 不写，省磁盘 I/O）\n\
             logging={}\n\
             ; 日志级别：info = 全部 / warn = 只记警告与错误 / error = 只记错误；\n\
             ; 仅在 logging=1 时有效\n\
             log_level={}\n\
             ; 高刷模式：1 = 面板 / 预览 ~30fps，0 = 普通 ~10fps（省 CPU）\n\
             high_fps={}\n\
             ; 大小核兼容模式：0 = LHS CPU Total，1 = 自算 CPU 占用率；\n\
             ; 手动开启；切换后自算需重新采样，缺值显示不可用，不跨模式回退\n\
             hybrid_compat={}\n\
             \n\
             [ui]\n\
             ; dark / light\n\
             theme={}\n\
             ; 关闭按钮收进系统托盘（0 = 直接退出）\n\
             close_to_tray={}\n\
             devtools={}\n\
             \n\
             [units]\n\
             ; auto = 完全跟随素材的 data-unit；manual = 用下面四族统一覆盖\n\
             mode={}\n\
             ; 容量 B/KB/MB/GB/TB · 速率 B/s…GB/s · 温度 ℃/℉ · 时长 秒/分/小时/天\n\
             capacity={}\n\
             rate={}\n\
             temperature={}\n\
             duration={}\n",
            b(self.silent_start),
            b(self.logging),
            self.log_level,
            b(self.high_fps),
            b(self.hybrid_compat),
            self.theme,
            b(self.close_to_tray),
            b(self.devtools),
            self.unit_mode,
            self.unit_capacity,
            self.unit_rate,
            self.unit_temperature,
            self.unit_duration,
        );
        // 外设名字是动态条目（有几只外设就有几行），单独拼在最后；
        // 键一律小写十六进制，排序输出，手改时位置稳定。
        out.push_str(
            "\n[peripherals.names]\n\
             ; 外设显示名。vid:pid = 同型号共用（如 1532:0555）；\n\
             ; vid:pid@短指纹 = 只给这一只改名（短指纹见外设页或 --peripherals-hid）；\n\
             ; 留空或删掉这一行 = 用默认的品牌+型号\n",
        );
        for (k, v) in &self.peripheral_names {
            out.push_str(&format!("{k}={v}\n"));
        }
        out
    }
}

/// `settings.ini` 路径（exe 同级；便携布局里即 `bin/settings.ini`）。
fn path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("settings.ini"))
}

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// 解析 INI → `section.key`（小写）→ 值。
fn parse(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let text = text.trim_start_matches('\u{feff}');
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        out.insert(
            format!("{section}.{}", k.trim().to_ascii_lowercase()),
            v.trim().to_string(),
        );
    }
    out
}

static CURRENT: Mutex<Option<Settings>> = Mutex::new(None);
// 覆盖读-改-写及运行态提交，避免共用临时文件和旧快照互相覆盖。
static TRANSACTION: Mutex<()> = Mutex::new(());

/// 旧的标记文件（本次迁移前的形态），存在就把它的语义搬进 `settings.ini`。
/// 迁移只做一次：写出的 ini 里有值，之后就不再看这两个文件。
fn migrate_legacy() -> Option<Settings> {
    let dir = path()?.parent()?.to_path_buf();
    let silent = dir.join("silent.flag").is_file();
    let nolog = dir.join("nolog.flag").is_file();
    if !silent && !nolog {
        return None;
    }
    let mut s = Settings::default();
    s.silent_start = silent;
    s.logging = !nolog; // `nolog.flag` 在 = 关日志
    Some(s)
}

/// 读盘并填进内存（文件不存在 / 读失败 / 解析失败 → 全默认，不报错）。
///
/// 文件不存在时会**顺手写一份出来**（含从旧标记文件 `silent.flag` / `nolog.flag` 迁移来的值），
/// 之后就以 `settings.ini` 为准。
pub fn load() -> Settings {
    let _transaction = TRANSACTION.lock().unwrap_or_else(|e| e.into_inner());
    load_locked()
}

fn load_locked() -> Settings {
    if let Some(s) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return s;
    }
    let from_file = path().and_then(|p| std::fs::read_to_string(&p).ok());
    let (s, need_write) = match from_file {
        Some(text) => (Settings::from_map(&parse(&text)), false),
        None => (migrate_legacy().unwrap_or_default(), true),
    };
    *CURRENT.lock().unwrap() = Some(s.clone());
    if need_write {
        let _ = write_file(&s);
    }
    // 顺便把单位设置灌进 `units.rs`（面板注入脚本每秒拉 `/units`）
    crate::units::set(s.unit_mode.clone(), s.units_map());
    s
}

/// 当前设置（首次访问自动读盘）。
pub fn current() -> Settings {
    if let Some(s) = CURRENT.lock().unwrap().clone() {
        return s;
    }
    load()
}

/// 写回：**原子写**（先写 `.tmp` 再替换，避免写一半断电留下坏文件）。
fn write_file(s: &Settings) -> Result<(), String> {
    let p = path().ok_or("无法定位 settings.ini，设置未保存")?;
    let tmp = p.with_extension("ini.tmp");
    std::fs::write(&tmp, s.to_ini().as_bytes()).map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("替换 {} 失败：{e}", p.display()))?;
    Ok(())
}

/// 用一段 JSON 局部更新设置（只认传进来的键），写回文件并返回最新值。
pub fn apply_patch(patch: &Value) -> Result<Settings, String> {
    let _transaction = TRANSACTION.lock().unwrap_or_else(|e| e.into_inner());
    let previous = load_locked();
    let s = patched(&previous, patch);
    // 只在成功落盘后提交运行态；相同快照不重复写盘。
    if s != previous {
        write_file(&s)?;
    }
    commit(&s);
    Ok(s)
}

/// 只改一条外设显示名（`name` 为 `None`/空 = 删掉这条覆盖，回落默认名）。
///
/// 单独开一个入口而不是走 `apply_patch`：改名是**读-改-写单条**，
/// 用整表覆盖会在两个入口同时改时互相吞掉对方的名字。
pub fn set_peripheral_name(key: &str, name: Option<&str>) -> Result<Settings, String> {
    let key = normalize_peripheral_key(key)
        .ok_or_else(|| format!("外设键不合法（{key}），应为 vid:pid 或 vid:pid@短指纹"))?;
    let _transaction = TRANSACTION.lock().unwrap_or_else(|e| e.into_inner());
    let previous = load_locked();
    let mut s = previous.clone();
    match name.map(clean_peripheral_name).filter(|n| !n.is_empty()) {
        Some(n) => {
            s.peripheral_names.insert(key, n);
        }
        None => {
            s.peripheral_names.remove(&key);
        }
    }
    if s != previous {
        write_file(&s)?;
    }
    commit(&s);
    Ok(s)
}

/// 把新设置发布到运行态（CPU/日志/刷新率/单位各模块各取所需）。
/// 调用方必须已持有 `TRANSACTION` 锁，且已经成功落盘。
fn commit(s: &Settings) {
    // 持 CURRENT 锁建立模式切换提交点；CPU worker 不在CPU锁内访问settings。
    {
        let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        crate::cpu_usage::set_enabled(s.hybrid_compat);
        *current = Some(s.clone());
    }
    crate::logging::set_enabled(s.logging);
    crate::logging::set_level(&s.log_level);
    crate::pipeline::set_high_fps(s.high_fps);
    crate::units::set(s.unit_mode.clone(), s.units_map());
}

fn patched(previous: &Settings, patch: &Value) -> Settings {
    let mut s = previous.clone();
    let b = |k: &str, cur: bool| patch.get(k).and_then(|v| v.as_bool()).unwrap_or(cur);
    let t = |k: &str, cur: &str| {
        patch
            .get(k)
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .unwrap_or_else(|| cur.to_string())
    };
    s.silent_start = b("silentStart", s.silent_start);
    s.logging = b("logging", s.logging);
    let lv = t("logLevel", &s.log_level);
    if lv == "info" || lv == "warn" || lv == "error" {
        s.log_level = lv;
    }
    s.high_fps = b("highFps", s.high_fps);
    s.hybrid_compat = b("hybridCompat", s.hybrid_compat);
    s.close_to_tray = b("closeToTray", s.close_to_tray);
    s.devtools = b("devtools", s.devtools);
    let theme = t("theme", &s.theme);
    if theme == "dark" || theme == "light" {
        s.theme = theme;
    }
    let mode = t("unitMode", &s.unit_mode);
    if mode == "auto" || mode == "manual" {
        s.unit_mode = mode;
    }
    s.unit_capacity = t("unitCapacity", &s.unit_capacity);
    s.unit_rate = t("unitRate", &s.unit_rate);
    s.unit_temperature = t("unitTemp", &s.unit_temperature);
    s.unit_duration = t("unitDuration", &s.unit_duration);
    // 外设名字表：整表替换（键会先规范化，非法键与非字符串值直接丢弃）。
    if let Some(obj) = patch.get("peripheralNames").and_then(|v| v.as_object()) {
        s.peripheral_names.clear();
        for (k, v) in obj {
            let Some(raw) = v.as_str() else { continue };
            let name = clean_peripheral_name(raw);
            if name.is_empty() {
                continue;
            }
            if let Some(key) = normalize_peripheral_key(k) {
                s.peripheral_names.insert(key, name);
            }
        }
    }

    s
}

/// 给前端的 JSON（键名与 `apply_patch` 一致）。
pub fn to_json(s: &Settings) -> Value {
    json!({
        "silentStart": s.silent_start,
        "logging": s.logging,
        "logLevel": s.log_level,
        "highFps": s.high_fps,
        "hybridCompat": s.hybrid_compat,
        "theme": s.theme,
        "closeToTray": s.close_to_tray,
        "devtools": s.devtools,
        "unitMode": s.unit_mode,
        "unitCapacity": s.unit_capacity,
        "unitRate": s.unit_rate,
        "unitTemp": s.unit_temperature,
        "unitDuration": s.unit_duration,
        "peripheralNames": &s.peripheral_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_mode_defaults_to_lhs() {
        assert!(!Settings::default().hybrid_compat);
        assert!(!Settings::from_map(&parse("[app]\n")).hybrid_compat);
    }

    #[test]
    fn mode_ini_round_trip_and_bom() {
        for enabled in [false, true] {
            let s = Settings { hybrid_compat: enabled, ..Settings::default() };
            assert_eq!(Settings::from_map(&parse(&s.to_ini())), s);
        }
        assert!(Settings::from_map(&parse("\u{feff}[app]\r\nhybrid_compat=1\r\n")).hybrid_compat);
    }

    #[test]
    fn log_level_round_trip_and_fallback() {
        let s = Settings { log_level: "warn".into(), ..Settings::default() };
        assert_eq!(Settings::from_map(&parse(&s.to_ini())), s);
        // 非法值（手改 ini 写错）回落到默认 info，而不是把整份设置判为无效
        assert_eq!(
            Settings::from_map(&parse("[app]\nlog_level=verbose\n")).log_level,
            "info"
        );
    }

    #[test]
    fn partial_patch_preserves_other_fields() {
        let s = Settings { theme: "light".into(), high_fps: true, ..Settings::default() };
        let enabled = patched(&s, &json!({"hybridCompat": true}));
        assert!(enabled.hybrid_compat);
        assert!(enabled.high_fps);
        assert_eq!(enabled.theme, "light");
        assert!(patched(&enabled, &json!({"hybridCompat": "false"})).hybrid_compat);
        assert!(!patched(&enabled, &json!({"hybridCompat": false})).hybrid_compat);
    }

    #[test]
    fn peripheral_names_round_trip() {
        let mut s = Settings::default();
        s.peripheral_names.insert("1532:0555@3f2a".into(), "左手机".into());
        // 值里带等号/分号/空格：INI 只在行首认注释、只按第一个 = 切分，必须原样回来
        s.peripheral_names.insert("0b05:18a5".into(), "键盘 = 一号; 备用".into());
        assert_eq!(Settings::from_map(&parse(&s.to_ini())), s);
    }

    #[test]
    fn peripheral_names_reject_bad_keys_and_blank_values() {
        let m = parse(
            "[peripherals.names]\n\
             not-a-key=坏键\n\
             zzzz:0555=坏键\n\
             1532:0555@=坏指纹\n\
             1532:0555=  主键盘  \n\
             0b05:18a5=\n",
        );
        let s = Settings::from_map(&m);
        assert_eq!(s.peripheral_names.len(), 1);
        assert_eq!(s.peripheral_names.get("1532:0555").unwrap(), "主键盘");
    }

    #[test]
    fn peripheral_name_is_capped_and_control_free() {
        let long = "名".repeat(40);
        let cleaned = clean_peripheral_name(&format!("\n\t{long}\r\n"));
        assert_eq!(cleaned.chars().count(), PERIPHERAL_NAME_MAX_CHARS);
        assert!(!cleaned.chars().any(char::is_control));
        assert_eq!(clean_peripheral_name("   "), "");
        assert_eq!(normalize_peripheral_key("0B05:18A5@3F2A").unwrap(), "0b05:18a5@3f2a");
        assert_eq!(normalize_peripheral_key("1532:0555").unwrap(), "1532:0555");
        assert!(normalize_peripheral_key("1532:05").is_none());
    }

    #[test]
    fn patch_replaces_peripheral_names_table() {
        let mut s = Settings::default();
        s.peripheral_names.insert("1532:0555".into(), "旧名".into());
        let next = patched(
            &s,
            &json!({"peripheralNames": {"1532:0555": "新名", "0B05:18A5@3F2A": "键盘", "bad": "x"}}),
        );
        assert_eq!(next.peripheral_names.get("1532:0555").unwrap(), "新名");
        assert_eq!(next.peripheral_names.get("0b05:18a5@3f2a").unwrap(), "键盘");
        assert!(!next.peripheral_names.contains_key("bad"));
        // 不带这个键的补丁不能把名字清空
        assert_eq!(patched(&s, &json!({"theme": "light"})).peripheral_names, s.peripheral_names);
    }
}

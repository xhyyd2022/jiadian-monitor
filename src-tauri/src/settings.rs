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
//! ```

use std::collections::HashMap;
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
        }
    }

    /// 序列化成 INI（固定顺序 + 注释，方便手改）。
    fn to_ini(&self) -> String {
        let b = |v: bool| if v { 1 } else { 0 };
        format!(
            "; jiaidian-monitor 设置 —— 改完保存，程序启动时读取（运行中改请用界面）。\n\
             ; 取值：0 = 关 / 1 = 开；留空 = 跟随素材。\n\
             \n\
             [app]\n\
             ; 启动时不显示窗口，直接收进托盘（左键托盘图标恢复）\n\
             silent_start={}\n\
             ; 是否写 log/jiaidian.log（0 = 不写，省磁盘 I/O）\n\
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
        )
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
    Ok(s)
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
}

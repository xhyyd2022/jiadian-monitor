//! 单位设置：默认「全自动」，也可切「手动指定」。
//!
//! - **全自动（auto）**：完全按素材自己声明的 `data-unit` 显示（素材写什么单位就换算成什么单位）。
//! - **手动指定（manual）**：按这里的配置，把**所有可换算指标**统一成选定单位（覆盖素材声明）。
//!
//! 四族（与 `MONITOR_METRICS.md`、`check_material.py` 的 A14 一致）：
//! `capacity` 容量 · `rate` 速率 · `temperature` 温度 · `duration` 时长。
//! （功耗固定 `W`，电脑上无可换算，不在其中。）
//! 某个族没选（空串）→ 那一族仍然跟随素材。
//!
//! 面板侧不直接读这个模块：注入脚本每秒拉一次 `GET /units`，所以**改完立刻生效**，不用重开面板。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, serde::Serialize)]
pub struct UnitConfig {
    /// `auto` | `manual`
    pub mode: String,
    /// 族 → 目标单位（空串 = 该族跟随素材）
    pub units: HashMap<String, String>,
}

impl Default for UnitConfig {
    fn default() -> Self {
        Self {
            mode: "auto".into(),
            units: HashMap::new(),
        }
    }
}

fn cell() -> &'static Mutex<UnitConfig> {
    static C: OnceLock<Mutex<UnitConfig>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(UnitConfig::default()))
}

/// 设置（前端设置页改动时调）。`mode` 只认 `auto` / `manual`，其余按 `auto`。
pub fn set(mode: String, units: HashMap<String, String>) {
    let mode = if mode == "manual" { "manual" } else { "auto" };
    let mut c = cell().lock().unwrap();
    c.mode = mode.into();
    // 空值不存（空 = 跟随素材），顺手把键统一成小写
    c.units = units
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
        .collect();
}

/// 当前配置（`GET /units` 用）。
pub fn get() -> UnitConfig {
    cell().lock().unwrap().clone()
}

/// 序列化成 JSON（面板注入脚本拉的那份）。
pub fn json() -> String {
    serde_json::to_string(&get()).unwrap_or_else(|_| r#"{"mode":"auto","units":{}}"#.into())
}

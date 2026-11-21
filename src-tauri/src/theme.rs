//! 应用外观主题：`Theme/` 文件夹 = 一张背景图 + 一份 `theme.toml`。
//!
//! 主题数据（深/浅两套调色板、图片缩放/位置、不透明度）全部写在 TOML 里，
//! 方便用户直接编辑、备份、分享；图片放在同一个文件夹，TOML 只记文件名。
//!
//! 目录固定在 exe 同级 `Theme/`，不跟随 cwd，避免便携版换工作目录后找不到。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const IMAGE_EXTS: [&str; 3] = ["png", "jpg", "webp"];
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// 前端可选的 M3 取色算法；写进 TOML，换版本后仍能还原。
const ALGORITHMS: [&str; 9] = [
    "tonal",
    "vibrant",
    "expressive",
    "fidelity",
    "content",
    "monochrome",
    "neutral",
    "rainbow",
    "fruit",
];

/// 一套模式下的语义颜色。字段为空 = 使用 `styles.css` 的默认值。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Palette {
    pub bg: String,
    pub side: String,
    pub panel: String,
    pub line: String,
    pub fg: String,
    pub sub: String,
    pub accent: String,
    pub button: String,
    pub button_hover: String,
    pub danger: String,
    pub ok: String,
    pub warn: String,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: "#11151c".into(),
            side: "#141a22".into(),
            panel: "#1c2430".into(),
            line: "#344155".into(),
            fg: "#f1f5f9".into(),
            sub: "#bac5d3".into(),
            accent: "#619aff".into(),
            button: "#242e3d".into(),
            button_hover: "#2d3a4b".into(),
            danger: "#f87171".into(),
            ok: "#34d399".into(),
            warn: "#fbbf24".into(),
        }
    }
}

fn light_palette() -> Palette {
    Palette {
        bg: "#f4f6f9".into(),
        side: "#fbfcfe".into(),
        panel: "#ffffff".into(),
        line: "#dce2ea".into(),
        fg: "#202733".into(),
        sub: "#596579".into(),
        accent: "#2563eb".into(),
        button: "#f1f4f8".into(),
        button_hover: "#e6ebf2".into(),
        danger: "#dc2626".into(),
        ok: "#059669".into(),
        warn: "#d97706".into(),
    }
}

/// `theme.toml` 的完整结构。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeConfig {
    pub name: String,
    /// 生成整套配色的种子色（M3 HCT）；用户之后仍可逐项覆盖。
    pub seed: String,
    /// standard = 用取色算法生成；advanced = 全部颜色逐项自定义。
    pub mode: String,
    /// M3 取色算法：tonal / vibrant / expressive / fidelity / content / monochrome / neutral / rainbow / fruit。
    pub algorithm: String,
    /// `Theme/` 内的图片文件名，空 = 纯色背景。
    pub image: String,
    /// cover / contain / fill / tile。
    pub image_fit: String,
    /// 图片位置，0–100（百分比）。
    pub image_x: u8,
    pub image_y: u8,
    pub opacity: u8,
    pub dark: Palette,
    pub light: Palette,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "自定义主题".into(),
            seed: "#619aff".into(),
            mode: "standard".into(),
            algorithm: "tonal".into(),
            image: String::new(),
            image_fit: "cover".into(),
            image_x: 50,
            image_y: 50,
            opacity: 94,
            dark: Palette::default(),
            light: light_palette(),
        }
    }
}

/// 前端一次拿到的主题数据（图片走字节，不暴露任意路径）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeData {
    pub config: ThemeConfig,
    pub image: Option<Vec<u8>>,
    pub dir: String,
    /// 之前是否已有 `theme.toml`（首次打开时前端可据此决定是否提示）。
    pub exists: bool,
}

fn exe_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位程序目录：{e}"))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "无法定位程序目录（exe 没有父目录）".to_string())
}

pub fn dir() -> Result<PathBuf, String> {
    let d = exe_dir()?.join("Theme");
    fs::create_dir_all(&d).map_err(|e| format!("创建 {} 失败：{e}", d.display()))?;
    Ok(d)
}

fn theme_path() -> Result<PathBuf, String> {
    Ok(dir()?.join("theme.toml"))
}

fn ext_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        _ => None,
    }
}

fn clean_color(raw: &str) -> String {
    let v = raw.trim();
    if v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        v.to_ascii_lowercase()
    } else {
        String::new()
    }
}

fn clean_palette(p: &mut Palette) {
    for c in [
        &mut p.bg,
        &mut p.side,
        &mut p.panel,
        &mut p.line,
        &mut p.fg,
        &mut p.sub,
        &mut p.accent,
        &mut p.button,
        &mut p.button_hover,
        &mut p.danger,
        &mut p.ok,
        &mut p.warn,
    ] {
        *c = clean_color(c);
    }
}

/// 只认文件头，避免把改了扩展名的任意文件复制进 Theme/ 后再让前端报错。
fn looks_like_image(bytes: &[u8], ext: &str) -> bool {
    match ext {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" => bytes.starts_with(b"\xff\xd8\xff"),
        "webp" => {
            bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
        }
        _ => false,
    }
}

fn clean_image_name(raw: &str) -> String {
    let name = raw.trim();
    if name.is_empty() || name.len() > 96 || name.contains(['/', '\\']) {
        return String::new();
    }
    let ok = name
        .rsplit_once('.')
        .map(|(_, ext)| IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if ok && !name.starts_with('.') {
        name.to_string()
    } else {
        String::new()
    }
}

impl ThemeConfig {
    fn sanitized(mut self) -> Self {
        self.name = self.name.trim().chars().take(40).collect();
        if self.name.is_empty() {
            self.name = "自定义主题".into();
        }
        self.seed = {
            let v = clean_color(&self.seed);
            if v.is_empty() { "#619aff".into() } else { v }
        };
        self.mode = match self.mode.as_str() {
            "advanced" | "custom" => "advanced".into(),
            _ => "standard".into(),
        };
        if !ALGORITHMS.contains(&self.algorithm.as_str()) {
            self.algorithm = "tonal".into();
        }
        self.image = clean_image_name(&self.image);
        if !matches!(self.image_fit.as_str(), "cover" | "contain" | "fill" | "tile") {
            self.image_fit = "cover".into();
        }
        self.image_x = self.image_x.min(100);
        self.image_y = self.image_y.min(100);
        self.opacity = self.opacity.clamp(60, 100);
        clean_palette(&mut self.dark);
        clean_palette(&mut self.light);
        self
    }
}

/// 读取 `Theme/` 下指定图片的字节（前端转 Blob URL 预览）。
pub fn image_bytes(name: &str) -> Result<Option<Vec<u8>>, String> {
    Ok(read_image(name))
}

fn read_image(name: &str) -> Option<Vec<u8>> {
    let name = clean_image_name(name);
    if name.is_empty() {
        return None;
    }
    fs::read(dir().ok()?.join(name)).ok()
}

fn remove_images() {
    if let Ok(d) = dir() {
        for ext in IMAGE_EXTS {
            let _ = fs::remove_file(d.join(format!("background.{ext}")));
        }
    }
}

/// 首次启动从旧 `settings.ini` / `ui/background.*` 迁移：只在没有 `theme.toml` 时做，
/// 之后主题文件就是唯一真值。
fn migrate_legacy(cfg: &mut ThemeConfig) -> bool {
    let old = crate::settings::load();
    if !old.appearance_accent.is_empty() {
        cfg.seed = old.appearance_accent.clone();
        cfg.dark.accent = old.appearance_accent.clone();
        cfg.light.accent = old.appearance_accent;
    }
    cfg.opacity = old.appearance_opacity;
    if old.appearance_bg.is_empty() {
        return false;
    }
    let Some(ui) = exe_dir().ok().map(|p| p.join("ui")) else {
        return false;
    };
    let src = ui.join(&old.appearance_bg);
    if !src.is_file() {
        return false;
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return false;
    }
    let dst_name = format!("background.{ext}");
    let Ok(dst) = dir().map(|d| d.join(&dst_name)) else {
        return false;
    };
    if fs::copy(&src, &dst).is_ok() {
        cfg.image = dst_name;
        true
    } else {
        false
    }
}

pub fn load() -> Result<ThemeData, String> {
    let d = dir()?;
    let p = theme_path()?;
    if let Ok(text) = fs::read_to_string(&p) {
        let cfg: ThemeConfig = toml::from_str(&text).map_err(|e| format!("解析 theme.toml 失败：{e}"))?;
        let cfg = cfg.sanitized();
        let image = read_image(&cfg.image);
        return Ok(ThemeData {
            config: cfg,
            image,
            dir: d.to_string_lossy().to_string(),
            exists: true,
        });
    }
    let mut cfg = ThemeConfig::default();
    let _ = migrate_legacy(&mut cfg);
    let image = read_image(&cfg.image);
    let _ = save(&cfg);
    Ok(ThemeData {
        config: cfg,
        image,
        dir: d.to_string_lossy().to_string(),
        exists: false,
    })
}

pub fn save(cfg: &ThemeConfig) -> Result<(), String> {
    let cfg = cfg.clone().sanitized();
    let d = dir()?;
    let p = theme_path()?;
    let text = toml::to_string_pretty(&cfg).map_err(|e| format!("生成 theme.toml 失败：{e}"))?;
    let tmp = p.with_extension("toml.tmp");
    fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
    fs::rename(&tmp, &p).map_err(|e| format!("替换 {} 失败：{e}", p.display()))?;
    let _ = d;
    Ok(())
}

/// 复制图片进 `Theme/background.<ext>`；同名旧格式会清掉。
pub fn import_image(src: &str) -> Result<String, String> {
    let src = PathBuf::from(src);
    let meta = fs::metadata(&src).map_err(|e| format!("读取图片失败：{e}"))?;
    if !meta.is_file() {
        return Err("请选择一张图片文件".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err("背景图不能超过 8 MB".into());
    }
    let ext = ext_of(&src).ok_or("只支持 PNG / JPG / WebP 图片")?;
    let bytes = fs::read(&src).map_err(|e| format!("读取图片失败：{e}"))?;
    if !looks_like_image(&bytes, ext) {
        return Err("图片内容与扩展名不符，请选择真正的 PNG / JPG / WebP".into());
    }
    let d = dir()?;
    fs::create_dir_all(&d).map_err(|e| format!("创建 {} 失败：{e}", d.display()))?;
    let name = format!("background.{ext}");
    fs::write(d.join(&name), &bytes).map_err(|e| format!("保存背景图失败：{e}"))?;
    for old in IMAGE_EXTS {
        if old != ext {
            let _ = fs::remove_file(d.join(format!("background.{old}")));
        }
    }
    Ok(name)
}

pub fn clear_image() -> Result<(), String> {
    remove_images();
    Ok(())
}

/// 把当前主题（theme.toml + 背景图）复制到用户指定的文件夹。
pub fn export(target_dir: &str, overwrite: bool) -> Result<String, String> {
    let target = PathBuf::from(target_dir);
    if !target.is_dir() {
        return Err("请选择一个文件夹作为保存位置".into());
    }
    let src_toml = theme_path()?;
    if !src_toml.is_file() {
        return Err("当前还没有可保存的主题".into());
    }
    let dst_toml = target.join("theme.toml");
    if dst_toml.exists() && !overwrite {
        return Err("EXISTS: 这个文件夹里已经有一份主题（theme.toml）".into());
    }
    fs::copy(&src_toml, &dst_toml)
        .map_err(|e| format!("保存 theme.toml 失败：{e}"))?;
    let cfg = load()?.config;
    if !cfg.image.is_empty() {
        let src_dir = dir()?;
        let from = src_dir.join(&cfg.image);
        if from.is_file() {
            let to = target.join(&cfg.image);
            fs::copy(&from, &to).map_err(|e| format!("保存背景图失败：{e}"))?;
        }
    }
    Ok(target.to_string_lossy().to_string())
}

pub fn open_dir() -> Result<(), String> {
    let d = dir()?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&d)
            .spawn()
            .map_err(|e| format!("打开主题目录失败：{e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = d;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_sanitizes_and_round_trips() {
        let mut cfg = ThemeConfig::default();
        cfg.dark.bg = "#ABCDEF".into();
        cfg.dark.fg = "not-a-color".into();
        cfg.image_fit = "bad".into();
        cfg.opacity = 10;
        cfg.image = "..\\evil.png".into();
        let cfg = cfg.sanitized();
        assert_eq!(cfg.dark.bg, "#abcdef");
        assert_eq!(cfg.dark.fg, "");
        assert_eq!(cfg.image_fit, "cover");
        assert_eq!(cfg.opacity, 60);
        assert_eq!(cfg.image, "");
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: ThemeConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.dark.bg, "#abcdef");
        assert_eq!(back.light.accent, "#2563eb");
    }

    #[test]
    fn image_magic_matches_extension() {
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\nrest", "png"));
        assert!(looks_like_image(b"\xff\xd8\xff\xe0rest", "jpg"));
        assert!(looks_like_image(b"RIFF\x00\x00\x00\x00WEBPrest", "webp"));
        assert!(!looks_like_image(b"<html>", "png"));
        assert!(!looks_like_image(b"\x89PNG\r\n\x1a\n", "jpg"));
    }

    #[test]
    fn mode_and_algorithm_sanitize() {
        let mut cfg = ThemeConfig::default();
        cfg.mode = "custom".into();
        cfg.algorithm = "not-an-algorithm".into();
        let cfg = cfg.sanitized();
        assert_eq!(cfg.mode, "advanced");
        assert_eq!(cfg.algorithm, "tonal");
    }
}

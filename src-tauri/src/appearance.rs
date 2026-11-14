//! 外观资源：用户背景图落在 exe 同级 `ui/` 下（便携布局）。
//!
//! 只做两件事：
//! - 把用户通过对话框选中的图片**校验后复制**进 `ui/`，返回安全文件名；
//! - 按 settings.ini 里记录的文件名读取字节，交给前端转 Blob URL 显示。
//!
//! 文件名只允许 `background.<png|jpg|webp>`，不接受任何路径分隔符，避免把
//! settings.ini 手改成 `../../...` 时读到任意文件。

use std::path::{Path, PathBuf};

use crate::settings::clean_bg_name;

const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn ui_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位程序目录：{e}"))?;
    let parent = exe.parent().ok_or("无法定位程序目录（exe 没有父目录）")?;
    let dir = parent.join("ui");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    Ok(dir)
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

/// 复制用户选中的图片到 `ui/background.<ext>`，返回文件名。
pub fn import(src: &str) -> Result<String, String> {
    let src = PathBuf::from(src);
    let meta = std::fs::metadata(&src).map_err(|e| format!("读取图片失败：{e}"))?;
    if !meta.is_file() {
        return Err("请选择一张图片文件".into());
    }
    if meta.len() > MAX_BYTES {
        return Err("背景图不能超过 8 MB".into());
    }
    let ext = ext_of(&src).ok_or("只支持 PNG / JPG / WebP 图片")?;
    let dir = ui_dir()?;
    let name = format!("background.{ext}");
    let dst = dir.join(&name);
    std::fs::copy(&src, &dst).map_err(|e| format!("复制背景图失败：{e}"))?;
    // 换格式时清掉旧扩展名，避免 ui/ 里堆多份
    for old in ["png", "jpg", "webp"] {
        if old != ext {
            let _ = std::fs::remove_file(dir.join(format!("background.{old}")));
        }
    }
    Ok(name)
}

/// 按 settings.ini 里的文件名读取背景图字节；文件不存在返回 `None`。
pub fn read(name: &str) -> Result<Option<Vec<u8>>, String> {
    let name = clean_bg_name(name);
    if name.is_empty() || !name.starts_with("background.") {
        return Ok(None);
    }
    let path = ui_dir()?.join(name);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取背景图失败：{e}")),
    }
}

/// 清除背景图文件（不清 settings.ini；调用方会同时把 appearanceBg 置空）。
pub fn clear() -> Result<(), String> {
    let dir = ui_dir()?;
    for ext in ["png", "jpg", "webp"] {
        let _ = std::fs::remove_file(dir.join(format!("background.{ext}")));
    }
    Ok(())
}

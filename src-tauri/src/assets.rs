//! 主题目录 `assets/`：定位、扫描、导入。
//!
//! 一个主题 = `assets/` 下一层文件夹，文件夹名即主题名（可用中文）。
//! 主题内可按**屏幕尺寸**再分一层子目录（可选）：
//! ```text
//! assets/玻璃拟态/v1p/index.html   → 320×170（V1PRO）
//! assets/玻璃拟态/v2/index.html    → 320×240（V2）
//! ```
//! 单尺寸主题只放其中一个子目录即可；也兼容直接把 `index.html` 放在主题根下
//! （按其**声明尺寸**归类，识别不出算 `any`）。图片与页面同层、按文件名相对引用。

use std::fs;
use std::path::{Path, PathBuf};

/// 导入时随页面一起复制的图片扩展名。
const IMAGE_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp"];

/// 一个尺寸变体：`tag` 是归类标签（`v1p` / `v2` / `any`），`page` 相对 `assets/`。
#[derive(Clone, serde::Serialize)]
pub struct Variant {
    pub tag: String,
    pub page: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// 一个主题：`name` 是文件夹名，`page` 是默认页面（供不关心尺寸的调用方使用），
/// `variants` 是各尺寸下的页面。
#[derive(Clone, serde::Serialize)]
pub struct Theme {
    pub name: String,
    pub page: String,
    pub variants: Vec<Variant>,
}

/// `assets/` 目录：exe 同级 → cwd。
pub fn dir() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.join("assets"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("assets"));
    }
    cands.into_iter().find(|p| p.is_dir())
}

/// 拿或建 `assets/`（exe 同级，兜底 cwd）。
pub fn ensure_dir() -> Result<PathBuf, String> {
    if let Some(d) = dir() {
        return Ok(d);
    }
    let base = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .ok_or("拿不到可执行文件目录")?;
    let d = base.join("assets");
    fs::create_dir_all(&d).map_err(|e| format!("创建 {} 失败：{e}", d.display()))?;
    crate::logging::info(&format!("已创建主题目录 {}", d.display()));
    Ok(d)
}

/// 排序键：带数字的名字按数值排（p1 < p10），其余按名字。
fn natural_key(n: &str) -> (u8, u64, String) {
    let digits: String = n
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    match digits.parse::<u64>() {
        Ok(num) => (0u8, num, n.to_string()),
        Err(_) => (1u8, 0, n.to_string()),
    }
}

/// 目录内的主页面：优先 `index.html`，否则唯一的 `.html`。
fn page_in(dir: &Path) -> Option<String> {
    let rd = fs::read_dir(dir).ok()?;
    let mut htmls: Vec<String> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let is_html = p.is_file()
            && p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("html"));
        if is_html {
            if let Some(n) = p.file_name().and_then(|x| x.to_str()) {
                htmls.push(n.to_string());
            }
        }
    }
    htmls.sort();
    if let Some(i) = htmls.iter().position(|n| n.eq_ignore_ascii_case("index.html")) {
        return Some(htmls[i].clone());
    }
    if htmls.len() == 1 {
        return Some(htmls.remove(0));
    }
    None
}

/// `width:320px` → `320`（必须跟 `px`，避免把 `width:100%` 当像素）。
fn px_after(low: &str, key: &str) -> Option<u32> {
    let rest = low.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    if rest[digits.len()..].trim_start().starts_with("px") {
        Some(n)
    } else {
        None
    }
}

/// 从 HTML 里推断画布尺寸：先找第一个声明 `width:Npx` + `height:Npx` 的规则块，
/// 再退到内嵌清单里的 `"size": [W, H]`。
fn declared_size(p: &Path) -> Option<(u32, u32)> {
    let s = fs::read_to_string(p).ok()?;
    let mut w = None;
    let mut h = None;
    for seg in s.split([';', '{', '}']) {
        let low = seg.trim().to_ascii_lowercase();
        if w.is_none() && low.starts_with("width") {
            w = px_after(&low, "width");
        } else if h.is_none() && low.starts_with("height") {
            h = px_after(&low, "height");
        }
        if w.is_some() && h.is_some() {
            return Some((w.unwrap(), h.unwrap()));
        }
    }
    manifest_size(&s)
}

/// 兜底：从内嵌清单 `"size": [320, 240]` 里读尺寸。
fn manifest_size(s: &str) -> Option<(u32, u32)> {
    let i = s.find("\"size\"")?;
    let tail = &s[i..s.len().min(i + 64)];
    let a = tail.find('[')? + 1;
    let nums: Vec<u32> = tail[a..]
        .split([',', ']'])
        .filter_map(|x| x.trim().parse::<u32>().ok())
        .collect();
    match nums.as_slice() {
        [w, h, ..] => Some((*w, *h)),
        _ => None,
    }
}

/// 尺寸 → 标签。
fn size_tag(w: Option<u32>, h: Option<u32>) -> &'static str {
    match (w, h) {
        (Some(320), Some(170)) => "v1p",
        (Some(320), Some(240)) => "v2",
        _ => "any",
    }
}

/// 子目录名 → 标签（`v1p` / `v2`，其它返回 None 由声明尺寸判定）。
fn dir_tag(sub: &str) -> Option<&'static str> {
    match sub.to_ascii_lowercase().as_str() {
        "v1p" => Some("v1p"),
        "v2" => Some("v2"),
        _ => None,
    }
}

/// 扫一个主题目录，得到它的所有尺寸变体（子目录优先，主题根下的 html 作为兜底）。
fn scan_variants(name: &str, theme_dir: &Path) -> Vec<Variant> {
    let mut out: Vec<Variant> = Vec::new();

    // ① 尺寸子目录（v1p / v2；其它子目录按声明尺寸归类）
    let mut subs: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(rd) = fs::read_dir(theme_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(n) = p.file_name().and_then(|x| x.to_str()) {
                    subs.push((n.to_string(), p));
                }
            }
        }
    }
    subs.sort_by(|a, b| a.0.cmp(&b.0));
    for (sub, sp) in subs {
        let Some(page) = page_in(&sp) else { continue };
        let full = sp.join(&page);
        let dt = dir_tag(&sub);
        let (w, h) = match dt {
            Some("v1p") => (Some(320), Some(170)),
            Some("v2") => (Some(320), Some(240)),
            _ => declared_size(&full)
                .map(|(a, b)| (Some(a), Some(b)))
                .unwrap_or((None, None)),
        };
        let tag = dt.unwrap_or_else(|| size_tag(w, h));
        out.push(Variant {
            tag: tag.to_string(),
            page: format!("{name}/{sub}/{page}"),
            width: w,
            height: h,
        });
    }

    // ② 主题根下的 html（单尺寸主题 / 旧扁平结构）
    if let Some(page) = page_in(theme_dir) {
        let full = theme_dir.join(&page);
        let (w, h) = declared_size(&full)
            .map(|(a, b)| (Some(a), Some(b)))
            .unwrap_or((None, None));
        out.push(Variant {
            tag: size_tag(w, h).to_string(),
            page: format!("{name}/{page}"),
            width: w,
            height: h,
        });
    }

    // v2 → v1p → any，稳定排序（默认页面优先取 v2）
    out.sort_by_key(|v| match v.tag.as_str() {
        "v2" => 0,
        "v1p" => 1,
        _ => 2,
    });
    out
}

/// 组装主题：`page` 取 v2 → v1p → 第一个变体。
fn make_theme(name: String, variants: Vec<Variant>) -> Theme {
    let page = variants
        .iter()
        .find(|v| v.tag == "v2")
        .or_else(|| variants.iter().find(|v| v.tag == "v1p"))
        .or_else(|| variants.first())
        .map(|v| v.page.clone())
        .unwrap_or_default();
    Theme { name, page, variants }
}

/// 文件是否已在 `assets/<主题名>/…` 里？是则返回主题名。
fn theme_of(assets: &Path, p: &Path) -> Option<String> {
    let a = fs::canonicalize(assets).ok()?;
    let q = fs::canonicalize(p).ok()?;
    let rel = q.strip_prefix(&a).ok()?;
    if rel.components().count() < 2 {
        return None; // 直接躺在 assets/ 根下 → 当新主题处理
    }
    rel.components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .map(|s| s.to_string())
}

/// 扫描 `assets/` 下的一级文件夹（主题），根下文件忽略。
pub fn list_themes() -> Vec<Theme> {
    let Some(dir) = dir() else {
        return Vec::new();
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<Theme> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|x| x.to_str()).map(|s| s.to_string()) else {
            continue;
        };
        let variants = scan_variants(&name, &p);
        if variants.is_empty() {
            crate::logging::warn(&format!("主题「{name}」里没有 .html，已忽略"));
            continue;
        }
        v.push(make_theme(name, variants));
    }
    v.sort_by_key(|t| natural_key(&t.name));
    v
}

/// 主题名是否可用作目录名。
/// 拦掉 `.` / `..`（会让 `dir.join(name)` 指到 assets **之外**）、路径分隔符与 Windows 非法字符，
/// 以及以点/空格结尾（Windows 会静默去掉，导致目录名和主题名对不上）。
fn valid_theme_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && n != ".."
        && !n.starts_with('.')
        && !n.ends_with(['.', ' '])
        && !n.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
}

/// 导入 HTML：整理成 `assets/<主题名>/<尺寸>/`，连同同目录图片复制，返回主题。
///
/// 主题名：文件名不是 `index.html` → 用文件名；是 `index.html` → 用所在文件夹名。
/// 尺寸：按 HTML 声明的画布尺寸放进 `v1p/`（320×170）或 `v2/`（320×240）子目录，
/// 识别不出就放主题根下。
pub fn import(src: &str) -> Result<Theme, String> {
    let src_p = PathBuf::from(src);
    if !src_p.is_file() {
        return Err(format!("文件不存在：{src}"));
    }
    let src_dir = src_p.parent().ok_or("文件没有父目录")?.to_path_buf();
    let file_stem = src_p
        .file_stem()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("无效路径：{src}"))?;
    let dir = ensure_dir()?;

    // 已在某个主题目录（含尺寸子目录）里：不复制，直接返回该主题
    if let Some(theme_name) = theme_of(&dir, &src_p) {
        let td = dir.join(&theme_name);
        let variants = scan_variants(&theme_name, &td);
        crate::logging::info(&format!("[ui] 导入：文件已在主题「{theme_name}」内，直接使用"));
        return Ok(make_theme(theme_name, variants));
    }

    let theme_name = if file_stem.eq_ignore_ascii_case("index") {
        src_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .ok_or("无法从文件夹名推断主题名")?
    } else {
        file_stem.clone()
    };
    if !valid_theme_name(&theme_name) {
        return Err(format!(
            "主题名不合法（不能为空、不能是 . 或 ..、不能含 \\ / : * ? \" < > |、不能以点或空格结尾）：{theme_name}"
        ));
    }

    let sub = declared_size(&src_p)
        .map(|(w, h)| size_tag(Some(w), Some(h)))
        .unwrap_or("any");
    let theme_dir = dir.join(&theme_name);
    let dst_dir = match sub {
        "v1p" | "v2" => theme_dir.join(sub),
        _ => theme_dir.clone(),
    };
    fs::create_dir_all(&dst_dir).map_err(|e| format!("创建主题目录失败：{e}"))?;

    let dst = dst_dir.join("index.html");
    if dst.exists() {
        crate::logging::info(&format!("[ui] 导入：覆盖 {}", dst.display()));
    }
    fs::copy(&src_p, &dst).map_err(|e| format!("复制 HTML 失败：{e}"))?;
    let mut copied = 1usize;

    if let Ok(rd) = fs::read_dir(&src_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p == src_p {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.to_ascii_lowercase())
                .unwrap_or_default();
            if !IMAGE_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let Some(fname) = p.file_name() else { continue };
            match fs::copy(&p, dst_dir.join(fname)) {
                Ok(_) => copied += 1,
                Err(e) => crate::logging::warn(&format!("导入图片失败（跳过）{}：{e}", p.display())),
            }
        }
    }

    crate::logging::info(&format!(
        "[ui] 导入完成：{src} → {}（共 {copied} 个文件）",
        dst.display()
    ));
    let variants = scan_variants(&theme_name, &theme_dir);
    Ok(make_theme(theme_name, variants))
}

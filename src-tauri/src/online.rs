//! 在线素材：`online/` 目录下「一条在线页面 = 一个 TOML」。
//!
//! 约定见 `app/ONLINE_MODE.md`：字段 `title` / `description` / `[v1p].url` / `[v2].url`。
//! 目录与 `assets/`、`peripherals/` 同规则：**exe 同级优先，cwd 回退**。
//!
//! 这里只负责「读 / 校验 / 原子写 / 删」，不碰渲染：加载在线页面走现有
//! `render_start`（`html` 传完整 URL），由 `pipeline` 区分本地 file:// 与在线 http(s)。

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 一条在线素材（前端按 snake_case 读：`url_v1p` / `url_v2`）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OnlineSource {
    /// 文件名 slug（= `online/<id>.toml`），保存/删除都靠它定位
    pub id: String,
    pub title: String,
    pub description: String,
    pub url_v1p: Option<String>,
    pub url_v2: Option<String>,
}

const TITLE_MAX: usize = 40;
const DESC_MAX: usize = 2000;
const URL_MAX: usize = 2000;
const ID_MAX: usize = 64;
const EXT: &str = "toml";

/// 可执行文件所在目录（拿不到就返回 None）。
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn cwd_dir() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

/// 现成的 `online/` 目录：exe 同级 → cwd。**不创建**。
pub fn dir() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(d) = exe_dir() {
        cands.push(d.join("online"));
    }
    if let Some(d) = cwd_dir() {
        cands.push(d.join("online"));
    }
    cands.into_iter().find(|p| p.is_dir())
}

/// 拿或建 `online/`（exe 同级优先，cwd 兜底）。
pub fn ensure_dir() -> Result<PathBuf, String> {
    if let Some(d) = dir() {
        return Ok(d);
    }
    let base = exe_dir()
        .or_else(cwd_dir)
        .ok_or("拿不到可执行文件目录，无法定位 online/")?;
    let d = base.join("online");
    fs::create_dir_all(&d).map_err(|e| format!("创建 {} 失败：{e}", d.display()))?;
    crate::logging::info(&format!("已创建在线素材目录 {}", d.display()));
    Ok(d)
}

/// 界面显示的目录标签：尽量相对程序目录（`online/`），避免把本机盘符写进界面。
pub fn dir_label(dir: &Path) -> String {
    if let Some(base) = exe_dir() {
        if let Ok(rel) = dir.strip_prefix(&base) {
            let s = rel.to_string_lossy().replace('\\', "/");
            return if s.is_empty() { ".".to_string() } else { s };
        }
    }
    dir.to_string_lossy().to_string()
}

/// 列表：扫描 `online/*.toml`。坏文件只记日志、不拖垮整张列表。
fn list_in(dir: &Path) -> Result<Vec<OnlineSource>, String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("读取 {} 失败：{e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(EXT))
        {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        if !valid_id(&id) {
            crate::logging::warn(&format!(
                "跳过在线素材 {}：文件名不是合法 id",
                path.display()
            ));
            continue;
        }
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_source(&id, &t))
        {
            Ok(src) => out.push(src),
            Err(e) => crate::logging::warn(&format!(
                "跳过在线素材 {}：{e}",
                path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            )),
        }
    }
    out.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// 解析一份 TOML 文本（纯函数，单测直接调）。
fn parse_source(id: &str, text: &str) -> Result<OnlineSource, String> {
    if !valid_id(id) {
        return Err(format!("文件名不是合法 id：{id}"));
    }
    let table: toml::Table = text.parse().map_err(|e| format!("TOML 解析失败：{e}"))?;
    let title = table
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("缺少 title")?
        .to_string();
    if title.chars().count() > TITLE_MAX {
        return Err(format!("title 超长（>{TITLE_MAX} 字符）"));
    }
    let description = table
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if description.chars().count() > DESC_MAX {
        return Err(format!("description 超长（>{DESC_MAX} 字符）"));
    }
    let url_v1p = parse_url(&table, "v1p")?;
    let url_v2 = parse_url(&table, "v2")?;
    if url_v1p.is_none() && url_v2.is_none() {
        return Err("至少需要一个 [v1p].url 或 [v2].url".into());
    }
    Ok(OnlineSource {
        id: id.to_string(),
        title,
        description,
        url_v1p,
        url_v2,
    })
}

fn parse_url(table: &toml::Table, key: &str) -> Result<Option<String>, String> {
    let Some(v) = table.get(key) else {
        return Ok(None);
    };
    let sibling = v.as_table().ok_or_else(|| format!("[{key}] 不是表"))?;
    let Some(raw) = sibling.get("url") else {
        return Ok(None);
    };
    let s = raw
        .as_str()
        .ok_or_else(|| format!("[{key}].url 不是字符串"))?;
    validate_url(s, &format!("[{key}].url"))
}

/// URL 校验：https:// 任意站点；http:// 仅限本机与局域网地址（设备面板常用局域网 IP）；
/// 拒绝账号密码 / 空白 / 控制字符。
fn validate_url(raw: &str, field: &str) -> Result<Option<String>, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().count() > URL_MAX {
        return Err(format!("{field} 超长（>{URL_MAX} 字符）"));
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("{field} 含空白或控制字符"));
    }
    let lower = s.to_ascii_lowercase();
    let (scheme, rest, is_https) = if lower.starts_with("https://") {
        ("https://", &s[8..], true)
    } else if lower.starts_with("http://") {
        ("http://", &s[7..], false)
    } else {
        ("", "", false)
    };
    if scheme.is_empty() {
        return Err(format!(
            "{field} 只允许 https:// 或 http://（仅本机/局域网地址）"
        ));
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.is_empty() {
        return Err(format!("{field} 缺少主机名"));
    }
    if authority.contains('@') {
        return Err(format!("{field} 不允许带账号密码"));
    }
    if !is_https {
        let host = host_of(authority);
        if !is_private_host(host) {
            return Err(format!(
                "{field} 的 http:// 仅支持本机或局域网地址（localhost / 127.x / 10.x / 172.16-31.x / 192.168.x / 169.254.x / ::1 / fc00::/7 / fe80::/10）；公网请用 https://"
            ));
        }
    }
    Ok(Some(s.to_string()))
}

/// http:// 只放行本机/局域网地址：设备自带面板通常是 192.168.x / 10.x，
/// 公网 http 一律拒绝（也避免在线素材被当成任意站点的明文跳板）。
fn is_private_host(host: &str) -> bool {
    use std::net::IpAddr;
    let h = host.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    if h == "localhost" {
        return true;
    }
    if let Ok(ip) = h.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            IpAddr::V6(v6) => {
                let seg = v6.segments();
                v6.is_loopback()
                    || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                    || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link local
            }
        };
    }
    // mDNS 主机名（如 device.local）只在本机局域网有意义；仍只允许 http。
    h.ends_with(".local")
}

/// 从 `host[:port]` 里取 host（兼容 `[::1]:8766`）。
fn host_of(authority: &str) -> &str {
    if authority.starts_with('[') {
        let end = authority
            .find(']')
            .map(|i| i + 1)
            .unwrap_or(authority.len());
        return &authority[..end];
    }
    authority.split(':').next().unwrap_or(authority)
}

/// id：`[a-z0-9][a-z0-9-]*`，用于文件名。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 标题 → slug；非 ASCII（中文等）无法转写时用时间戳兜底。
fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out = format!("online-{secs}");
    }
    out.truncate(ID_MAX);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "online".to_string()
    } else {
        out
    }
}

/// 新 id 去重：`base`、`base-2`、`base-3`…
fn unique_id(dir: &Path, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2u32;
    while dir.join(format!("{id}.{EXT}")).exists() {
        id = format!("{base}-{n}");
        n += 1;
        if n > 9999 {
            id = format!(
                "{base}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            );
            break;
        }
    }
    id
}

/// 保存（`id = None` 新建，否则覆盖）。会校验并返回落盘后的完整对象。
fn save_in(
    dir: &Path,
    id: Option<String>,
    title: &str,
    description: &str,
    url_v1p: &str,
    url_v2: &str,
) -> Result<OnlineSource, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("标题不能为空".into());
    }
    if title.chars().count() > TITLE_MAX {
        return Err(format!("标题最多 {TITLE_MAX} 个字符"));
    }
    let description = description.trim();
    if description.chars().count() > DESC_MAX {
        return Err(format!("描述最多 {DESC_MAX} 个字符"));
    }
    let url_v1p = validate_url(url_v1p, "V1P URL")?;
    let url_v2 = validate_url(url_v2, "V2 URL")?;
    if url_v1p.is_none() && url_v2.is_none() {
        return Err("V1P / V2 至少填一个 URL".into());
    }
    fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let id = match id {
        Some(raw) => {
            let raw = raw.trim();
            if !valid_id(raw) {
                return Err("id 不合法（只允许小写字母、数字、-）".into());
            }
            raw.to_string()
        }
        None => unique_id(dir, &slugify(title)),
    };
    let src = OnlineSource {
        id: id.clone(),
        title: title.to_string(),
        description: description.to_string(),
        url_v1p,
        url_v2,
    };
    let text = render_source(&src);
    let dst = dir.join(format!("{id}.{EXT}"));
    let tmp = dir.join(format!(".{id}.{EXT}.tmp-{}", std::process::id()));
    fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写临时文件失败：{e}"))?;
    // Windows 上 rename 覆盖已存在的目标：std 会替换；失败再回退到 remove+rename。
    if let Err(e) = fs::rename(&tmp, &dst) {
        let _ = fs::remove_file(&dst);
        fs::rename(&tmp, &dst).map_err(|e2| {
            let _ = fs::remove_file(&tmp);
            format!("写入 {} 失败：{e2}（首次 rename：{e}）", dst.display())
        })?;
    }
    Ok(src)
}

/// 生成 TOML 文本：标题/描述用单行基本字符串（换行转义），两个硬件各一个表。
fn render_source(src: &OnlineSource) -> String {
    let mut s = format!("title = {}\n", toml_string(&src.title));
    if !src.description.is_empty() {
        s.push_str(&format!(
            "description = {}\n",
            toml_string(&src.description)
        ));
    }
    if let Some(url) = &src.url_v1p {
        s.push_str(&format!("\n[v1p]\nurl = {}\n", toml_string(url)));
    }
    if let Some(url) = &src.url_v2 {
        s.push_str(&format!("\n[v2]\nurl = {}\n", toml_string(url)));
    }
    s
}

fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn delete_in(dir: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("id 不合法".into());
    }
    let path = dir.join(format!("{id}.{EXT}"));
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除 {} 失败：{e}", path.display())),
    }
}

// ---------------- 对外（Tauri 命令调用） ----------------

pub fn list() -> Result<Vec<OnlineSource>, String> {
    list_in(&ensure_dir()?)
}

pub fn save(
    id: Option<String>,
    title: String,
    description: String,
    url_v1p: String,
    url_v2: String,
) -> Result<OnlineSource, String> {
    save_in(&ensure_dir()?, id, &title, &description, &url_v1p, &url_v2)
}

pub fn delete(id: String) -> Result<(), String> {
    delete_in(&ensure_dir()?, &id)
}

pub fn dir_label_string() -> Result<String, String> {
    Ok(dir_label(&ensure_dir()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = std::env::temp_dir().join(format!(
            "jiadian-online-test-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parse_minimal_and_full() {
        let a = parse_source(
            "a",
            "title = \"天气\"\n[v1p]\nurl = \"https://example.com/a\"\n",
        )
        .unwrap();
        assert_eq!(a.title, "天气");
        assert_eq!(a.url_v1p.as_deref(), Some("https://example.com/a"));
        assert_eq!(a.url_v2, None);
        let b = parse_source(
            "b",
            "title = \"B\"\ndescription = \"说明\"\n[v1p]\nurl = \"https://x/v1\"\n[v2]\nurl = \"http://localhost:8080/v2\"\n",
        )
        .unwrap();
        assert_eq!(b.description, "说明");
        assert_eq!(b.url_v2.as_deref(), Some("http://localhost:8080/v2"));
    }

    #[test]
    fn url_rules() {
        assert!(
            validate_url("https://example.com/x", "u")
                .unwrap()
                .is_some()
        );
        assert!(
            validate_url("http://localhost:8080/x", "u")
                .unwrap()
                .is_some()
        );
        assert!(validate_url("http://127.0.0.1/x", "u").unwrap().is_some());
        assert!(validate_url("http://example.com/x", "u").is_err());
        assert!(validate_url("http://192.168.100.1:3002/#/", "u").is_ok());
        assert!(validate_url("http://10.0.0.5/a", "u").is_ok());
        assert!(validate_url("http://172.16.3.9/a", "u").is_ok());
        assert!(validate_url("http://172.32.0.1/a", "u").is_err());
        assert!(validate_url("http://[fd00::1]/a", "u").is_ok());
        assert!(validate_url("http://[fe80::1]/a", "u").is_ok());
        assert!(validate_url("ftp://example.com/x", "u").is_err());
        assert!(validate_url("https://user:pw@example.com/", "u").is_err());
        assert!(validate_url("  ", "u").unwrap().is_none());
    }

    #[test]
    fn missing_title_or_urls() {
        assert!(parse_source("x", "[v1p]\nurl=\"https://a\"").is_err());
        assert!(parse_source("x", "title=\"t\"").is_err());
        assert!(parse_source("x", "title=\"t\"\n[v1p]\nurl=1").is_err());
    }

    #[test]
    fn save_list_delete_roundtrip() {
        let d = tmpdir();
        let s = save_in(
            &d,
            None,
            "天气面板",
            "多行\n描述",
            "https://example.com/v1",
            "",
        )
        .unwrap();
        assert!(valid_id(&s.id));
        let list = list_in(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "天气面板");
        assert_eq!(list[0].description, "多行\n描述");
        // 再次保存（覆盖）不新增
        let edited = save_in(
            &d,
            Some(s.id.clone()),
            "天气 2",
            "",
            "https://example.com/v1",
            "https://example.com/v2",
        )
        .unwrap();
        assert_eq!(edited.id, s.id);
        assert_eq!(list_in(&d).unwrap().len(), 1);
        delete_in(&d, &s.id).unwrap();
        assert!(list_in(&d).unwrap().is_empty());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bad_file_does_not_break_list() {
        let d = tmpdir();
        save_in(&d, None, "好", "", "https://example.com/a", "").unwrap();
        fs::write(d.join("bad.toml"), "title = ").unwrap();
        fs::write(d.join("Bad Name.toml"), "title = \"x\"").unwrap();
        let list = list_in(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "好");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn slug_and_unique() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert!(slugify("天气面板").starts_with("online-"));
        let d = tmpdir();
        fs::write(d.join("weather.toml"), "x").unwrap();
        assert_eq!(unique_id(&d, "weather"), "weather-2");
        fs::remove_dir_all(&d).unwrap();
    }
}

//! 简易文件日志：`log/jiadian.log`，单文件上限 1 MB，超过就轮转成 `jiadian.log.1`。
//!
//! 目的：面板切换卡顿 / 需要点多次这类问题，光看 UI 无从下手，落到文件里才能追溯。
//! 同时打到 stderr（CLI 直接可见）。
//!
//! **为什么是 1 MB**：排查「长时间运行后断联」时，每 10s 一条统计就要 ~140 B，
//! 跑 2 小时 ≈ 100 KB —— 原来的 100 KB 上限会在最需要证据的时刻把前半段轮转掉。
//!
//! 目录解析与 `assets/` 一致：先看 cwd，再从 exe 所在目录逐级向上找 `log/`。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

/// 单个日志文件上限。
const MAX_BYTES: u64 = 1024 * 1024;

static FILE: Mutex<Option<File>> = Mutex::new(None);
static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 日志输出总开关。**默认开**（保持原有行为）；关闭后 [`line`] 直接返回——
/// 不写文件、不打 stderr、不做 flush，也就不再产生任何日志相关的 I/O 与内存开销。
static ENABLED: AtomicBool = AtomicBool::new(true);

/// 级别门槛（0 = 全部 / 1 = 警告及以上 / 2 = 仅错误）。低于门槛的行直接丢弃。
///
/// 只在总开关打开时才有意义：关掉日志时三个级别都不输出。
static LEVEL: AtomicU8 = AtomicU8::new(0);

/// 级别名 → 序号。未知按「全部」处理。
fn rank(level: &str) -> u8 {
    match level {
        "WARN" => 1,
        "ERROR" => 2,
        _ => 0,
    }
}

/// 设置级别门槛（`info` / `warn` / `error`，大小写不敏感；未知值按 `info`）。
pub fn set_level(name: &str) {
    let v = match name.trim().to_ascii_lowercase().as_str() {
        "warn" | "warning" => 1,
        "error" => 2,
        _ => 0,
    };
    LEVEL.store(v, Ordering::Relaxed);
}

/// 当前是否允许输出日志。
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 运行时开关日志输出（**立即生效**）。
///
/// 持久化不在这里：真值在 `settings.ini` 的 `[app] logging`（由 `settings::apply_patch` 写），
/// 启动时 [`init`] 从那儿读。
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        // 关闭后释放文件句柄（下次开启时会按需重开）。
        *FILE.lock().unwrap() = None;
    }
}

/// `log/` 目录（cwd，或从 exe 目录逐级向上）。
pub fn dir() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = vec![PathBuf::from("log")];
    if let Ok(exe) = std::env::current_exe() {
        let mut d = exe.parent().map(|x| x.to_path_buf());
        for _ in 0..4 {
            let Some(cur) = d else { break };
            cands.push(cur.join("log"));
            d = cur.parent().map(|x| x.to_path_buf());
        }
    }
    cands.into_iter().find(|p| p.is_dir())
}

/// 打开日志文件（幂等）。找不到 `log/` 就自己建一个。
pub fn init() {
    // 开关与级别都来自 `settings.ini`（`[app] logging` / `[app] log_level`）。
    let s = crate::settings::current();
    if !s.logging {
        ENABLED.store(false, Ordering::Relaxed);
    }
    set_level(&s.log_level);
    if !enabled() {
        return; // 关闭状态：不建目录、不建文件
    }
    if FILE.lock().unwrap().is_some() {
        return;
    }
    let dir = dir().or_else(|| {
        let d = PathBuf::from("log");
        fs::create_dir_all(&d).ok().map(|_| d)
    });
    let Some(dir) = dir else { return };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("jiadian.log");
    let f = OpenOptions::new().create(true).append(true).open(&path).ok();
    *PATH.lock().unwrap() = Some(path);
    *FILE.lock().unwrap() = f;
}

fn timestamp() -> String {
    let mut st = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// 写一行日志。开关关闭时直接返回（不格式化、不写文件、不打 stderr）。
pub fn line(level: &str, msg: &str) {
    if !enabled() {
        return;
    }
    // 级别门槛：`warn` 档不记 info 行，`error` 档只记错误
    if rank(level) < LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let text = format!("{} [{level}] {msg}\n", timestamp());
    eprint!("{text}");
    {
        let mut g = FILE.lock().unwrap();
        if g.is_none() {
            drop(g);
            init();
            g = FILE.lock().unwrap();
        }
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(text.as_bytes());
            let _ = f.flush();
        }
    }
    rotate_if_needed();
}

/// 直接追加一行，**不碰全局锁**（panic 钩子专用）。
///
/// panic 有可能发生在持锁期间，此时再用 `FILE.lock()` 会死锁 —— 那正是最需要日志的时刻。
/// 这里每次重新打开文件追加，代价可忽略（只在 panic 时走一次）。
fn direct_line(text: &str) {
    // 用户关掉日志时连 panic 也不落盘（否则「关了还在写」与开关语义矛盾）。
    // panic 属于 ERROR 级，级别门槛不会拦它，只有总开关能拦。
    if !enabled() {
        return;
    }
    let Some(dir) = dir() else { return };
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("jiadian.log"))
    {
        let _ = f.write_all(format!("{} {text}\n", timestamp()).as_bytes());
        let _ = f.flush();
    }
}

/// 装 panic 钩子：把 panic 写进日志文件。
///
/// **为什么必须装**：日志里出现过「上一次会话直接消失、没有任何退出记录」的情况 ——
/// 不装钩子时，Rust panic 只打到 stderr，而 release 是 GUI 子系统、没有控制台，
/// 于是整个事件在文件里零痕迹，只能靠猜。
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        direct_line(&format!("[PANIC] {info}"));
        default(info);
    }));
}

fn rotate_if_needed() {
    let Some(path) = PATH.lock().unwrap().clone() else { return };
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size <= MAX_BYTES {
        return;
    }
    *FILE.lock().unwrap() = None;
    let bak = PathBuf::from(format!("{}.1", path.to_string_lossy()));
    let _ = fs::rename(&path, &bak);
    let f = OpenOptions::new().create(true).append(true).open(&path).ok();
    *FILE.lock().unwrap() = f;
}

/// `INFO` 快捷方式。
pub fn info(msg: &str) {
    line("INFO", msg);
}

/// `WARN` 快捷方式。
pub fn warn(msg: &str) {
    line("WARN", msg);
}

/// `ERROR` 快捷方式。
pub fn error(msg: &str) {
    line("ERROR", msg);
}

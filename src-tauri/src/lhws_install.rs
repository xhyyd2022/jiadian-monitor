//! LibreHardwareService（第三方组件，MPL-2.0）：检测 / 从官方源下载 / 一键安装。
//!
//! 只做三件事：判断装没装、在不在跑；从**官方发布页**下载并按固定 sha256 校验；用 msiexec 提权安装。
//! **不修改它的任何文件**；界面需展示来源与许可（见 `app/THIRD-PARTY-NOTICES.md`）。

use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// 上游版本 / 官方下载地址 / 固定校验值（换版本时同步改这三项）
pub const VERSION: &str = "0.3.4";
pub const DOWNLOAD_URL: &str =
    "https://github.com/epinter/LibreHardwareService/releases/download/v0.3.4/LibreHardwareService.msi";
pub const SHA256: &str = "30e7c877e4b7979b174f93ac5a3a2cf13e5209c3d64a34ed5040a013f68f4020";
pub const SOURCE_URL: &str = "https://github.com/epinter/LibreHardwareService";
pub const LICENSE: &str = "MPL-2.0";
pub const LICENSE_URL: &str = "https://www.mozilla.org/en-US/MPL/2.0/";

/// 传感器共享内存（存在即服务**在发布数据**）
const MMF_SENSORS: &str = "Global\\LibreHardwareService/json/sensors/data";
/// 服务注册表键
const SVC_KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\LibreHardwareService";
/// 服务名（SCM 用）
const SVC_NAME: &str = "LibreHardwareService";

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 服务是否在发布数据（共享内存打得开）。
pub fn running() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{FILE_MAP_READ, OpenFileMappingW};
    unsafe {
        let h = OpenFileMappingW(FILE_MAP_READ, 0, to_wide(MMF_SENSORS).as_ptr());
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

/// 服务是否已安装（注册表里有服务项）。
pub fn installed() -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            to_wide(SVC_KEY).as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        );
        if r == 0 {
            RegCloseKey(hkey);
            true
        } else {
            false
        }
    }
}

/// **服务进程**在 SCM 里的状态。
///
/// 为什么不用「共享内存存不存在」当唯一判据：那个探针混了三种情况 ——
/// ① 服务没跑；② 服务刚启动、LHM 还没枚举完硬件（约 5~7 秒）；③ 权限/会话不通。
/// 三者该给用户看的话完全不同，所以这里直接问 SCM 要真实状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SvcState {
    /// 注册表/SCM 里没有这个服务 → 没装
    NotInstalled,
    Stopped,
    StartPending,
    Running,
    StopPending,
    Paused,
    /// 连不上 SCM（理论上不会）
    Unknown,
}

impl SvcState {
    fn as_str(self) -> &'static str {
        match self {
            SvcState::NotInstalled => "notInstalled",
            SvcState::Stopped => "stopped",
            SvcState::StartPending => "startPending",
            SvcState::Running => "running",
            SvcState::StopPending => "stopPending",
            SvcState::Paused => "paused",
            SvcState::Unknown => "unknown",
        }
    }
}

/// 查服务在 SCM 里的真实状态。
pub fn svc_state() -> SvcState {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
        SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
        SERVICE_PAUSED, SERVICE_START_PENDING, SERVICE_STOP_PENDING, SERVICE_STOPPED,
        SERVICE_STATUS_PROCESS,
    };
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return SvcState::Unknown;
        }
        let svc = OpenServiceW(scm, to_wide(SVC_NAME).as_ptr(), SERVICE_QUERY_STATUS);
        if svc.is_null() {
            // 1060 = ERROR_SERVICE_DOES_NOT_EXIST → 没装
            let e = GetLastError();
            CloseServiceHandle(scm);
            return if e == 1060 { SvcState::NotInstalled } else { SvcState::Unknown };
        }
        let mut ssp: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut need = 0u32;
        let ok = QueryServiceStatusEx(
            svc,
            SC_STATUS_PROCESS_INFO,
            &mut ssp as *mut _ as *mut u8,
            std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut need,
        );
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        if ok == 0 {
            return SvcState::Unknown;
        }
        match ssp.dwCurrentState {
            SERVICE_RUNNING => SvcState::Running,
            SERVICE_STOPPED => SvcState::Stopped,
            SERVICE_START_PENDING => SvcState::StartPending,
            SERVICE_STOP_PENDING => SvcState::StopPending,
            SERVICE_PAUSED => SvcState::Paused,
            _ => SvcState::Unknown,
        }
    }
}

/// 状态（给界面用，含来源/许可信息，便于一处展示合规声明）。
///
/// `status` 是**给界面直接显示的合并态**（把"服务进程状态"与"数据发布状态"合起来判）：
/// | status | 含义 |
/// |---|---|
/// | `notInstalled` | 未安装 |
/// | `stopped` | 已安装，但服务进程没在跑（**本机就是这种情况**：开机竞态导致崩溃后没人拉起） |
/// | `starting` | 服务在跑，但还没发布共享内存（LHM 初始化约 5~7 秒，属瞬态） |
/// | `running` | 服务在跑且在发布数据 → 一切正常 |
/// | `unknown` | 拿不到状态 |
pub fn state() -> serde_json::Value {
    let svc = svc_state();
    let publishing = running();
    let (status, text) = match svc {
        SvcState::NotInstalled => ("notInstalled", "未安装"),
        _ if publishing => ("running", "运行中"),
        SvcState::Running | SvcState::StartPending => ("starting", "服务已启动，数据未就绪"),
        SvcState::Stopped => ("stopped", "已安装，服务未启动"),
        SvcState::StopPending => ("stopped", "服务正在停止…"),
        SvcState::Paused => ("stopped", "服务已暂停"),
        SvcState::Unknown => ("unknown", "状态未知"),
    };
    json!({
        "version": VERSION,
        "installed": svc != SvcState::NotInstalled,
        "running": publishing,
        "serviceState": svc.as_str(),
        "publishing": publishing,
        "status": status,
        "statusText": text,
        "source": SOURCE_URL,
        "license": LICENSE,
        "licenseUrl": LICENSE_URL,
        "msi": local_msi().map(|p| p.to_string_lossy().to_string()),
        // 下载/安装进度一并带出：界面一次 invoke 就能同时拿到"状态 + 进度"，
        // 不用两个接口互相等，也就不会出现"状态已刷新、进度还是上一轮"的错位。
        "progress": progress(),
    })
}

/// 用户自己放在 exe 同级的离线 MSI。
///
/// **和下载缓存分开对待**：这里的文件是用户主动放进来的，校验不过要明确报错让人去换，
/// 绝不能"悄悄改成从网上下载"——那会让用户以为自己放的离线包生效了。
fn offline_msi() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let d = exe.parent()?;
    let cands = [
        d.join("LibreHardwareService.msi"),
        d.join("lhws").join("LibreHardwareService.msi"),
    ];
    cands.into_iter().find(|p| p.is_file())
}

/// 缓存目录候选：先 `%TEMP%\jiadian-monitor`，写不进去再退到 exe 同级的 `cache/`，
/// 最后退到当前工作目录下的 `cache/`。
///
/// 为什么要退：`%TEMP%` 在某些环境下会被安全策略拦掉（企业管控、把 TEMP 指向不可写路径、
/// 或者进程被降权启动），这时如果只有一条路，用户看到的就是"装不了"，而且跟网络毫无关系。
/// 多给两个候选，绝大多数受限环境都能自动落到能写的地方。
fn cache_dirs() -> Vec<PathBuf> {
    let mut v = vec![std::env::temp_dir().join("jiadian-monitor")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            let p = d.join("cache");
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("cache");
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// 目录能不能真的写进去（建目录 + 写一个探针文件 + 立刻删掉）。
fn dir_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 本进程实际使用的缓存目录（**只探测一次**，之后固定）。
///
/// 结果缓存下来，是因为 `state()` 会被界面每 300ms 拉一次；每次都探测就成了
/// 每 300ms 写删一个文件，没必要。
fn cache_dir() -> &'static Path {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        for d in cache_dirs() {
            if dir_writable(&d) {
                return d;
            }
        }
        // 全都写不进去：退回首选，让后面给出准确的错误（而不是在这里 panic）
        cache_dirs().into_iter().next().unwrap_or_else(std::env::temp_dir)
    })
}

/// 下载缓存路径（`<缓存目录>\lhws-<版本>.msi`）。
fn cache_path() -> PathBuf {
    cache_dir().join(format!("lhws-{VERSION}.msi"))
}

/// 临时文件路径：先写 `<名字>.part`，**校验通过才改名成正式名字**。
///
/// 这一步是必须的。以前直接往正式路径写，一旦断流（57MB 的文件在这台机器上被截到
/// 815KB）就会留下半截文件；下次点安装时 `local_msi()` 看到"文件存在"就当成可用的
/// 离线包，跳过下载直接校验，于是永远报「校验失败（期望 30e7c877…，实际 5b96889f…）」
/// ——用户看到的就是这个错，而且点多少次都一样。
fn part_path(dst: &Path) -> PathBuf {
    let mut s = dst.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// 清掉可能残留的半截文件（正式路径与临时路径都清）。
fn drop_partial(dst: &Path) {
    for p in [part_path(dst), dst.to_path_buf()] {
        if p.is_file() {
            match std::fs::remove_file(&p) {
                Ok(_) => crate::logging::info(&format!("已删除无效文件 {}", p.display())),
                Err(e) => crate::logging::warn(&format!("删除 {} 失败：{e}", p.display())),
            }
        }
    }
}

/// 当前本机存在的 MSI 文件（**只查存在，不做校验**）。
///
/// 只给界面显示用，不能拿它决定"要不要下载"——界面上有文件不代表文件是好的。
/// 真正的可用性判断在 `install()` 里做（校验不过就删掉重下）。
fn local_msi() -> Option<PathBuf> {
    if let Some(p) = offline_msi() {
        return Some(p);
    }
    let p = cache_path();
    p.is_file().then_some(p)
}

/// 下载进度（给界面画进度条；`state()` 里也带一份，方便一次性拿全）。
///
/// 之前这里没有任何进度：界面只能显示"安装中…"，用户对着一个不动的字等两分钟，
/// 分不清是"在下"还是"卡死了"。断流时更是完全没有反馈。
#[derive(Clone)]
pub struct Prog {
    /// 有安装/下载任务在进行
    pub active: bool,
    /// preparing / downloading / verifying / installing / done / failed
    pub phase: String,
    /// 当前用的下载源标签（"GitHub 官方" / "gh-proxy.com" …）
    pub source: String,
    /// 本次是第几次尝试（换源也算一次）
    pub attempt: u32,
    /// 一共可能尝试几次
    pub total_attempts: u32,
    /// 已下载字节
    pub got: u64,
    /// 总字节（拿不到就是 0，界面画成不确定态）
    pub total: u64,
    /// 实时速度（字节/秒）
    pub speed: f64,
    /// 给界面直接显示的一句话
    pub message: String,
}

impl Prog {
    const fn new() -> Prog {
        Prog {
            active: false,
            phase: String::new(),
            source: String::new(),
            attempt: 0,
            total_attempts: 0,
            got: 0,
            total: 0,
            speed: 0.0,
            message: String::new(),
        }
    }
}

static PROG: Mutex<Prog> = Mutex::new(Prog::new());

/// 拿进度锁。**不用 unwrap**：持锁期间 panic 过也不能让整个安装流程瘫掉。
fn plock() -> std::sync::MutexGuard<'static, Prog> {
    PROG.lock().unwrap_or_else(|e| e.into_inner())
}

fn prog_set(f: impl FnOnce(&mut Prog)) {
    f(&mut plock());
}

/// 进度 JSON（前端 `lhws_progress` 命令直接返回它）。
pub fn progress() -> serde_json::Value {
    let p = plock().clone();
    json!({
        "active": p.active,
        "phase": p.phase,
        "source": p.source,
        "attempt": p.attempt,
        "totalAttempts": p.total_attempts,
        "got": p.got,
        "total": p.total,
        "percent": if p.total > 0 { (p.got.min(p.total) * 100 / p.total) as u32 } else { 0 },
        "speed": p.speed,
        "message": p.message,
    })
}

/// 官方源 + 国内加速站，顺序即尝试顺序。
///
/// 为什么需要加速站：这台机器上从 GitHub 直连下这个 59.7 MB 的 MSI 反复断流
/// （只拿到 815 KB 就断了），重试多少次都是同一个截断结果。加速站做的是字节流
/// 转发，断流概率低得多。**无论从哪下，最后都过同一个 sha256**，所以中转站即使
/// 篡改内容也装不进去。
fn mirrors() -> Vec<(&'static str, String)> {
    vec![
        ("GitHub 官方", DOWNLOAD_URL.to_string()),
        ("gh-proxy.com", format!("https://gh-proxy.com/{DOWNLOAD_URL}")),
        ("ghproxy.net", format!("https://ghproxy.net/{DOWNLOAD_URL}")),
        (
            "hk.gh-proxy.com",
            format!("https://hk.gh-proxy.com/{DOWNLOAD_URL}"),
        ),
    ]
}

/// **速度阈值**：低于它就认为是"太慢"，不值得继续等。
const MIN_SPEED: u64 = 12 * 1024;
/// 连续这么多秒都慢于阈值就放弃这个源、换下一个。
const SLOW_SECS: u64 = 20;
/// 连续这么多秒一个字节都没多，判定卡死（连不上 / 服务器不回数据 / 断流）。
const STALL_SECS: u64 = 60;
/// 进度轮询间隔。
const TICK_MS: u64 = 400;

/// 先问一下总大小，界面才能画百分比。拿不到（超时/没有 Content-Length）返回 0。
fn head_total(url: &str) -> Option<u64> {
    let agent = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .new_agent();
    let resp = agent.head(url).call().ok()?;
    resp.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

/// 一次下载的被中止原因（区分开好写日志与提示）。
enum Aborted {
    /// 卡住：STALL_SECS 秒没有新字节
    Stalled,
    /// 太慢：SLOW_SECS 秒平均速度低于 MIN_SPEED
    TooSlow,
}

impl Aborted {
    fn reason(&self) -> String {
        match self {
            Aborted::Stalled => format!("{STALL_SECS} 秒没拿到数据（判定卡死）"),
            Aborted::TooSlow => format!(
                "速度连续 {SLOW_SECS} 秒低于 {}/s（判定过慢）",
                human_rate(MIN_SPEED as f64)
            ),
        }
    }
}

fn human_rate(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bps / 1024.0 / 1024.0)
    } else if bps >= 1024.0 {
        format!("{:.0} KB", bps / 1024.0)
    } else {
        format!("{bps:.0} B")
    }
}

/// 用系统 `curl.exe` 下载一次（Windows 10 1803 起自带）。
///
/// 中止策略由我们自己的旁路线程判定 —— **不设"总时长上限"**，而是看"还有没有在动"：
///   * `STALL_SECS` 秒一个字节没多 → 判卡死，kill 掉换源；
///   * `SLOW_SECS` 秒平均速度低于 `MIN_SPEED` → 判过慢，kill 掉换源。
/// 这样慢但在动的连接不会被无谓掐断，而卡死/极慢的连接一分钟内就能换源。
/// curl 自己也带 `--speed-limit/--speed-time` 做同一件事，双保险。
fn curl_download(url: &str, tmp: &Path, total: u64) -> Result<(), String> {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // curl 的 stderr 走管道收集，**不落临时文件**。
    // 以前写 `<临时名>.err` 落盘，在受限目录里 `File::create` 会返回
    // 「拒绝访问（os error 5）」，于是每个源都在真正联网之前就判失败，
    // 界面只会看到"所有下载源都失败"，完全误导。
    let mut child = Command::new("curl.exe")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "15",
            "--speed-limit",
            &MIN_SPEED.to_string(),
            "--speed-time",
            &SLOW_SECS.to_string(),
        ])
        .arg("-o")
        .arg(tmp)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;

    // stderr 必须在别的线程里边读边丢，否则管道写满后 curl 会卡住。
    let mut err_lines: Vec<String> = Vec::new();
    let err_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut out: Vec<String> = Vec::new();
            let reader = std::io::BufReader::new(&mut pipe);
            for line in reader.lines().map_while(Result::ok) {
                let t = line.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
                if out.len() > 40 {
                    out.remove(0);
                }
            }
            out
        })
    });

    let child = Arc::new(Mutex::new(child));
    let stop = Arc::new(AtomicBool::new(false));
    let aborted: Arc<Mutex<Option<Aborted>>> = Arc::new(Mutex::new(None));

    let monitor = {
        let child = child.clone();
        let stop = stop.clone();
        let aborted = aborted.clone();
        let path = tmp.to_path_buf();
        std::thread::spawn(move || {
            let mut got = 0u64;
            // 距离"上次有进展"过了多久
            let mut last_gain = std::time::Instant::now();
            let mut win = std::time::Instant::now();
            let mut win_got = 0u64;
            let mut last_tick = std::time::Instant::now();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(TICK_MS));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let now = std::time::Instant::now();
                let cur = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let dt = now.duration_since(last_tick).as_secs_f64();
                let inst = if dt > 0.05 {
                    cur.saturating_sub(got) as f64 / dt
                } else {
                    0.0
                };
                last_tick = now;
                if cur > got {
                    got = cur;
                    last_gain = now;
                }
                prog_set(|p| {
                    p.got = got;
                    if total > 0 {
                        p.total = total;
                    }
                    // 速度做一次平滑，别让数字乱跳
                    p.speed = if p.speed <= 0.0 {
                        inst
                    } else {
                        p.speed * 0.7 + inst * 0.3
                    };
                });

                // 判定①：卡死（含"根本没开始下载"——一分钟一个字节都没有）
                if now.duration_since(last_gain).as_secs() >= STALL_SECS {
                    *aborted.lock().unwrap_or_else(|e| e.into_inner()) = Some(Aborted::Stalled);
                    let _ = child
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .kill();
                    return;
                }
                // 判定②：过慢（只在已经开动之后判，避免把连接握手期误判）
                if now.duration_since(win).as_secs() >= SLOW_SECS {
                    let avg = (got.saturating_sub(win_got)) as f64 / SLOW_SECS as f64;
                    win = now;
                    win_got = got;
                    if got > 0 && avg < MIN_SPEED as f64 {
                        *aborted
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(Aborted::TooSlow);
                        let _ = child
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .kill();
                        return;
                    }
                }
            }
        })
    };

    // 等子进程结束
    let status = loop {
        let done = child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_wait()
            .map_err(|e| format!("等待 curl 失败：{e}"))?;
        if let Some(st) = done {
            break st;
        }
        std::thread::sleep(Duration::from_millis(120));
    };
    stop.store(true, Ordering::Relaxed);
    let _ = monitor.join();

    let why = aborted
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|a| a.reason());
    if let Some(h) = err_reader {
        err_lines = h.join().unwrap_or_default();
    }

    if let Some(w) = why {
        return Err(w);
    }
    if !status.success() {
        let msg = err_lines.last().cloned().unwrap_or_default();
        return Err(format!(
            "退出码 {:?}{}",
            status.code(),
            if msg.is_empty() {
                String::new()
            } else {
                format!("：{msg}")
            }
        ));
    }
    let n = std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0);
    if n < 1_000_000 {
        return Err(format!("只拿到 {n} 字节，明显被拦截或断流"));
    }
    if total > 0 && n < total {
        return Err(format!("下载不完整（拿到 {n} / 应为 {total} 字节）"));
    }
    prog_set(|p| p.got = n);
    Ok(())
}

/// 内置 HTTP 客户端的下载实现（**只在系统没有 curl.exe 时兜底**）。
///
/// 同样写进 `.part`、同样上报进度。这条路径没有旁路监控线程，所以给
/// `timeout_recv_body` 设一个 3 分钟的总上限当保险丝 —— 正常网络 59.7 MB
/// 一分钟内下得完，超过 3 分钟说明确实有问题，直接失败换源。
fn ureq_download(url: &str, tmp: &Path) -> Result<(), String> {
    let agent = ureq::config::Config::builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(180)))
        .build()
        .new_agent();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("请求失败：{e}"))?;
    let mut body = resp.into_body();
    let mut f = std::fs::File::create(tmp).map_err(|e| format!("创建临时文件失败：{e}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0u64;
    let mut reader = body.as_reader();
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("读取响应失败（已收 {got} 字节）：{e}"))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])
            .map_err(|e| format!("写入失败：{e}"))?;
        got += n as u64;
        prog_set(|p| p.got = got);
    }
    f.sync_all().map_err(|e| format!("落盘失败：{e}"))?;
    if got < 1_000_000 {
        return Err(format!("只拿到 {got} 字节，明显被拦截或断流"));
    }
    Ok(())
}

/// 这个失败是"本机写不进文件"，还是"网络上没拿到"？
///
/// curl 的 23 号退出码 = `client returned ERROR on write`，只可能是落盘失败
/// （磁盘满、目录无权限、路径不可写）。这类错误**换多少个源都没用**——
/// 必须停下来直接告诉用户去修目录，而不是跑完 8 次再报"所有下载源都失败"
/// （那会把人引去查网络，方向完全错了）。
fn is_local_write_err(e: &str) -> bool {
    e.contains("Some(23)") || e.contains("on write of") || e.contains("建错误日志失败")
}

/// 下载前再确认一次目标目录真能写，不能等到 curl 跑到一半才发现。
///
/// `cache_dir()` 已经挑过可写目录了，这里是兜底：那之后磁盘可能被写满、目录可能被删掉。
fn probe_writable(dir: &Path) -> Result<(), String> {
    if dir_writable(dir) {
        return Ok(());
    }
    Err(format!(
        "下载缓存目录不可写：{}。请检查该目录的写权限，或清理磁盘空间。",
        dir.display()
    ))
}

/// 依次试官方源与各加速站，每个源下完都过一遍 sha256，**通过才算成功**。
///
/// 这是"下载失败自动换中转站重试"的实现：卡死 / 过慢 / 报错 / 哈希不符，全都算
/// 这个源失败，立刻换下一个；一轮跑完还没成功就再来一轮，最后把所有失败原因一起报出来。
fn fetch_verified(dst: &Path) -> Result<(), String> {
    if let Some(d) = dst.parent() {
        probe_writable(d)?;
    }
    let list = mirrors();
    let total_attempts = (list.len() * 2) as u32;
    let mut n = 0u32;
    let mut errs: Vec<String> = Vec::new();

    for round in 0..2 {
        for (label, url) in &list {
            n += 1;
            prog_set(|p| {
                p.active = true;
                p.phase = "downloading".into();
                p.source = (*label).to_string();
                p.attempt = n;
                p.total_attempts = total_attempts;
                p.got = 0;
                p.speed = 0.0;
                p.message = format!("正在从 {label} 下载（第 {n}/{total_attempts} 次尝试）");
            });
            if round > 0 {
                // 第二轮：给第一轮失败的源再一次机会，稍等避免立刻撞同一个故障
                std::thread::sleep(Duration::from_secs(2));
                crate::logging::warn(&format!("第二轮重试：{label}"));
            }
            let total = head_total(url).unwrap_or(0);
            prog_set(|p| p.total = total);

            let tmp = part_path(dst);
            let _ = std::fs::remove_file(&tmp);
            crate::logging::info(&format!(
                "下载尝试 {n}/{total_attempts}：{label} → {url}"
            ));
            let dl = match curl_download(url, &tmp, total) {
                Ok(()) => Ok(()),
                Err(e) if e.starts_with("调用 curl 失败") => {
                    // 系统没有 curl.exe（极老的 Win10）→ 用内置客户端兜底
                    crate::logging::warn(&format!("{label}：{e}，改用内置下载"));
                    ureq_download(url, &tmp)
                }
                Err(e) => Err(e),
            };
            match dl {
                Ok(()) => {}
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    crate::logging::warn(&format!("{label} 放弃：{e}"));
                    // 落盘失败跟源没关系，再换下去只是把同一个错误重复 8 遍。
                    if is_local_write_err(&e) {
                        prog_set(|p| {
                            p.active = false;
                            p.phase = "failed".into();
                            p.speed = 0.0;
                            p.message = "本机写不进文件，已停止重试".into();
                        });
                        return Err(format!(
                            "本机无法写入下载目录（不是网络问题，换源无效）：{e}"
                        ));
                    }
                    errs.push(format!("{label}：{e}"));
                    continue;
                }
            }

            prog_set(|p| {
                p.phase = "verifying".into();
                p.message = format!("{label} 下载完成，正在校验 sha256…");
            });
            match sha256_file(&tmp) {
                Ok(got) if got.eq_ignore_ascii_case(SHA256) => {
                    std::fs::rename(&tmp, dst).map_err(|e| format!("改名失败：{e}"))?;
                    let size = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
                    crate::logging::info(&format!("{label} 下载完成并校验通过（{size} 字节）"));
                    prog_set(|p| {
                        p.got = size;
                        p.total = size;
                        p.source = (*label).to_string();
                        p.message = format!("{label} 下载完成，校验通过");
                    });
                    return Ok(());
                }
                Ok(got) => {
                    let _ = std::fs::remove_file(&tmp);
                    crate::logging::warn(&format!("{label} 给的字节校验不符（{got}）"));
                    errs.push(format!("{label}：内容校验不符（实际 {got}）"));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    errs.push(format!("{label}：校验失败（{e}）"));
                }
            }
        }
        if round == 0 {
            crate::logging::warn("第一轮所有下载源都失败，开始第二轮重试");
        }
    }

    prog_set(|p| {
        p.active = false;
        p.phase = "failed".into();
        p.speed = 0.0;
        p.message = format!("{n} 次尝试全部失败");
    });
    Err(format!(
        "所有下载源都失败（共试了 {n} 次）：{}",
        errs.join("；")
    ))
}

/// 只下载 + 校验，不安装（诊断用：`--lhws-fetch`）。
///
/// 单独拎出来是为了能在不弹 UAC 的前提下验证"下载链路 + 换源重试 + 进度上报"。
pub fn fetch_only() -> Result<String, String> {
    let p = cache_path();
    if p.is_file() && sha256_file(&p).map_or(true, |h| !h.eq_ignore_ascii_case(SHA256)) {
        crate::logging::warn("下载缓存校验不通过，删除后重新下载");
        drop_partial(&p);
    }
    let reused = p.is_file();
    if !reused {
        fetch_verified(&p)?;
    }
    let got = sha256_file(&p)?;
    if !got.eq_ignore_ascii_case(SHA256) {
        drop_partial(&p);
        return Err(format!("校验失败（期望 {SHA256}，实际 {got}），已删除"));
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "{}：{}（{size} 字节）",
        if reused { "复用缓存" } else { "已下载并校验通过" },
        p.display()
    ))
}

/// 文件 sha256（BCrypt，流式读取，不占内存）。
fn sha256_file(path: &Path) -> Result<String, String> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider,
        BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData,
        BCryptOpenAlgorithmProvider,
    };
    unsafe {
        let mut alg: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
        if BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0) < 0 {
            return Err("BCrypt 打开 SHA256 失败".into());
        }
        let mut hash: BCRYPT_HASH_HANDLE = std::ptr::null_mut();
        if BCryptCreateHash(alg, &mut hash, std::ptr::null_mut(), 0, std::ptr::null(), 0, 0) < 0 {
            BCryptCloseAlgorithmProvider(alg, 0);
            return Err("BCrypt 建哈希失败".into());
        }
        let mut f = std::fs::File::open(path).map_err(|e| format!("打开文件失败：{e}"))?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).map_err(|e| format!("读文件失败：{e}"))?;
            if n == 0 {
                break;
            }
            if BCryptHashData(hash, buf.as_ptr(), n as u32, 0) < 0 {
                BCryptDestroyHash(hash);
                BCryptCloseAlgorithmProvider(alg, 0);
                return Err("BCrypt 计算失败".into());
            }
        }
        let mut out = [0u8; 32];
        let ok = BCryptFinishHash(hash, out.as_mut_ptr(), 32, 0) >= 0;
        BCryptDestroyHash(hash);
        BCryptCloseAlgorithmProvider(alg, 0);
        if !ok {
            return Err("BCrypt 收尾失败".into());
        }
        Ok(out.iter().map(|b| format!("{b:02x}")).collect())
    }
}

/// 提权执行一个程序并等它退出，返回退出码（UAC 被取消 / 超时 → `Err`）。
///
/// 装卸、重启服务都需要管理员：普通交互用户的服务默认 DACL 里没有 `SERVICE_START`，
/// 所以一律走 `runas`（会弹一次 UAC）。
fn run_elevated(file: &str, params: &str, timeout_ms: u32) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb = to_wide("runas");
    let f = to_wide(file);
    let p = to_wide(params);
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = f.as_ptr();
    info.lpParameters = p.as_ptr();
    info.nShow = SW_HIDE as i32;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err("提权被取消或失败（UAC 未通过？）".into());
    }
    crate::logging::info(&format!("已提权启动 {file} {params}，等待完成…"));
    unsafe {
        let r = WaitForSingleObject(info.hProcess, timeout_ms);
        if r != 0 {
            CloseHandle(info.hProcess);
            return Err("等待超时（可能有 UAC 弹窗未确认）".into());
        }
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(code)
    }
}

/// 提权跑 `msiexec /i <msi> /qn /norestart`，返回退出码（0=成功，3010=成功需重启）。
fn run_msiexec(msi: &Path) -> Result<u32, String> {
    run_elevated(
        "msiexec.exe",
        &format!("/i \"{}\" /qn /norestart", msi.to_string_lossy()),
        600_000,
    )
}

/// **不提权**直接重启服务（停 → 起）。能成就省掉一次 UAC。
///
/// 关键安全点：**先把 `SERVICE_STOP | SERVICE_START` 两个权限一起申请**，
/// 打不开就直接放弃去走提权 —— 绝不会出现"停下来了却起不回去"。
fn restart_direct() -> bool {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
        SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS,
        SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOPPED,
        StartServiceW,
    };
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return false;
        }
        let svc = OpenServiceW(
            scm,
            to_wide(SVC_NAME).as_ptr(),
            SERVICE_STOP | SERVICE_START | SERVICE_QUERY_STATUS,
        );
        if svc.is_null() {
            CloseServiceHandle(scm);
            return false; // 没有停/起权限（交互用户默认就没有）→ 交给提权路径
        }
        let read_state = || -> u32 {
            let mut ssp: SERVICE_STATUS_PROCESS = std::mem::zeroed();
            let mut need = 0u32;
            if QueryServiceStatusEx(
                svc,
                SC_STATUS_PROCESS_INFO,
                &mut ssp as *mut _ as *mut u8,
                std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
                &mut need,
            ) == 0
            {
                u32::MAX
            } else {
                ssp.dwCurrentState
            }
        };
        let ok = if read_state() != SERVICE_STOPPED {
            let mut st: SERVICE_STATUS = std::mem::zeroed();
            if ControlService(svc, SERVICE_CONTROL_STOP, &mut st) == 0 {
                CloseServiceHandle(svc);
                CloseServiceHandle(scm);
                return false;
            }
            // 等它真停稳（最多 15 秒）
            for _ in 0..30 {
                if read_state() == SERVICE_STOPPED {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            StartServiceW(svc, 0, std::ptr::null()) != 0
        } else {
            StartServiceW(svc, 0, std::ptr::null()) != 0
        };
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        ok
    }
}

/// 等共享内存出现（LHM 启动后要枚举硬件，通常 5~7 秒）。
fn wait_publishing() -> bool {
    for _ in 0..40 {
        if running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// **重新启动**服务（停 → 起），并等到数据就绪。
///
/// 为什么是"重启"而不是"启动"：三种情况一次搞定 ——
/// ① 服务已停止 → 直接拉起；② 服务在跑但卡住没发布数据 → 停掉重来；
/// ③ 服务崩过一次（开机竞态）→ 重启即恢复。先试不提权，失败才弹一次 UAC。
pub fn restart() -> Result<serde_json::Value, String> {
    if running() {
        // 已经在正常发布数据：仍按用户意图重启一次（可治"数值卡住"）
        crate::logging::info("重新启动 LibreHardwareService（当前在运行）");
    }
    if !restart_direct() {
        // 提权一次性搞定：`net stop` 对"没在跑"的服务会报错，但 `&` 会继续执行 `net start`
        crate::logging::info("无权限直接重启，改为提权执行 net stop & net start");
        let rc = run_elevated(
            "cmd.exe",
            &format!("/c net stop {SVC_NAME} & net start {SVC_NAME}"),
            180_000,
        )?;
        if rc != 0 {
            // `net stop` 对"本来就没在跑"的服务会返回非 0，但 `&` 之后 `net start` 照样执行。
            // 这是预期内的正常路径，不是警告：留在 warn 级只会让「仅警告及以上」档出现假报警。
            crate::logging::info(&format!("提权重启命令退出码 {rc}（服务本来可能是停止的，属正常）"));
        }
    }
    if wait_publishing() {
        crate::logging::info("LibreHardwareService 已重新启动并就绪");
        return Ok(state());
    }
    // 没起来：给准确原因（区分"又崩了"与"还在初始化"）
    Err(match svc_state() {
        SvcState::Stopped | SvcState::NotInstalled => {
            "服务启动后又退出了。原因通常是**开机早期的网卡枚举竞态**（LHM 的已知缺陷）——\
             再点一次「重新启动」基本都能起来。"
                .into()
        }
        SvcState::Running | SvcState::StartPending => {
            "服务已在运行，但还没发布数据（可能仍在初始化）。稍等几秒后刷新看看。".into()
        }
        s => format!("重启未成功，当前服务状态：{}", s.as_str()),
    })
}

/// 找 MSI 的 ProductCode（即 Uninstall 下的子键名，形如 `{GUID}`）。
fn find_product_code() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
        RegQueryValueExW, REG_SZ,
    };
    const ROOTS: [&str; 2] = [
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    ];
    unsafe {
        for root in ROOTS {
            let mut hkey: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, to_wide(root).as_ptr(), 0, KEY_READ, &mut hkey) != 0
            {
                continue;
            }
            let mut i = 0u32;
            loop {
                let mut name = [0u16; 256];
                let mut len = name.len() as u32;
                let r = RegEnumKeyExW(
                    hkey,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if r != 0 {
                    break; // ERROR_NO_MORE_ITEMS
                }
                i += 1;
                let key = String::from_utf16_lossy(&name[..len as usize]);
                let mut sub: HKEY = std::ptr::null_mut();
                if RegOpenKeyExW(hkey, name.as_ptr(), 0, KEY_READ, &mut sub) != 0 {
                    continue;
                }
                // 读 DisplayName（REG_SZ）
                let mut dn = String::new();
                let prop = to_wide("DisplayName");
                let mut ty = 0u32;
                let mut size = 0u32;
                if RegQueryValueExW(
                    sub,
                    prop.as_ptr(),
                    std::ptr::null_mut(),
                    &mut ty,
                    std::ptr::null_mut(),
                    &mut size,
                ) == 0
                    && ty == REG_SZ
                    && size > 2
                    && size < 8192
                {
                    let mut buf = vec![0u8; size as usize];
                    if RegQueryValueExW(
                        sub,
                        prop.as_ptr(),
                        std::ptr::null_mut(),
                        &mut ty,
                        buf.as_mut_ptr(),
                        &mut size,
                    ) == 0
                    {
                        let u: Vec<u16> = buf
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        let end = u.iter().position(|&c| c == 0).unwrap_or(u.len());
                        dn = String::from_utf16_lossy(&u[..end]);
                    }
                }
                RegCloseKey(sub);
                if dn.contains("LibreHardwareService") && key.starts_with('{') {
                    RegCloseKey(hkey);
                    return Some(key);
                }
            }
            RegCloseKey(hkey);
        }
        None
    }
}

/// **卸载** LibreHardwareService（提权 `msiexec /x`），并清掉我们自己下载的 MSI 缓存。
///
/// 与 `scripts/uninstall-lhws.bat` 同口径：**只卸载、只删我们自己的缓存**，
/// 不动用户其他任何东西。
pub fn uninstall() -> Result<serde_json::Value, String> {
    let code = find_product_code()
        .ok_or("找不到 LibreHardwareService 的安装记录（可能不是用 MSI 安装的，请到「应用和功能」里卸载）")?;
    crate::logging::info(&format!("卸载 LibreHardwareService（ProductCode {code}）"));
    let rc = run_elevated(
        "msiexec.exe",
        &format!("/x {code} /qn /norestart"),
        600_000,
    )?;
    if rc != 0 && rc != 3010 {
        return Err(format!(
            "msiexec 退出码 {rc}（1605=未安装该产品，1603=卸载失败）"
        ));
    }
    for _ in 0..40 {
        if svc_state() == SvcState::NotInstalled && !installed() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // 清掉我们自己的下载缓存（与 bat 脚本一致）
    let cache = cache_path();
    if cache.is_file() {
        match std::fs::remove_file(&cache) {
            Ok(_) => crate::logging::info(&format!("已清理下载缓存 {}", cache.display())),
            Err(e) => crate::logging::warn(&format!("清理缓存失败：{e}")),
        }
    }
    crate::logging::info("LibreHardwareService 已卸载");
    Ok(state())
}

/// 一键安装：用户放的离线包优先，否则用（校验通过的）下载缓存，再否则下载
/// → 校验 sha256 → 提权安装 → 复检。
pub fn install() -> Result<serde_json::Value, String> {
    let msi = if let Some(p) = offline_msi() {
        // 用户主动放进来的离线包：校验不过必须明确报错让他去换，
        // 绝不能悄悄改成联网下载（那会让人以为自己放的包生效了）。
        let got = sha256_file(&p)?;
        if !got.eq_ignore_ascii_case(SHA256) {
            return Err(format!(
                "本地离线包校验不通过（期望 {SHA256}，实际 {got}）：{}。\
                 这个文件不是官方发布的那个 MSI，请删掉它或改名，然后点「一键安装」让程序自己下。",
                p.display()
            ));
        }
        crate::logging::info(&format!("使用离线 MSI：{}", p.display()));
        p
    } else {
        let p = cache_path();
        // 缓存里若躺着上次断流留下的半截文件，先清掉重下，否则会一直是同一个校验失败。
        if p.is_file() && sha256_file(&p).map_or(true, |h| !h.eq_ignore_ascii_case(SHA256)) {
            crate::logging::warn("下载缓存校验不通过，删除后重新下载");
            drop_partial(&p);
        }
        if !p.is_file() {
            // 官方源 + 各加速站轮着试；卡死/过慢/报错/哈希不符都会自动换下一个源。
            fetch_verified(&p)?;
        } else {
            crate::logging::info(&format!("使用下载缓存：{}", p.display()));
        }
        p
    };
    let got = sha256_file(&msi)?;
    if !got.eq_ignore_ascii_case(SHA256) {
        // 走到这里说明下载链路反复给出错误内容：把坏文件清掉，避免下次又踩同一个坑。
        drop_partial(&msi);
        return Err(format!("校验失败（期望 {SHA256}，实际 {got}），已删除无效文件"));
    }
    crate::logging::info("sha256 校验通过，开始安装");
    prog_set(|p| {
        p.active = true;
        p.phase = "installing".into();
        p.message = "正在安装（会弹一次 UAC，请在弹窗上点「是」）".into();
    });
    let code = match run_msiexec(&msi) {
        Ok(c) => c,
        Err(e) => {
            prog_set(|p| {
                p.active = false;
                p.phase = "failed".into();
                p.speed = 0.0;
                p.message = e.clone();
            });
            return Err(e);
        }
    };
    if code != 0 && code != 3010 {
        let e = format!("msiexec 退出码 {code}（1603=安装失败，1625=策略禁止）");
        prog_set(|p| {
            p.active = false;
            p.phase = "failed".into();
            p.speed = 0.0;
            p.message = e.clone();
        });
        return Err(e);
    }
    prog_set(|p| p.message = "安装完成，正在等服务发布数据…".into());
    for _ in 0..40 {
        if running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    prog_set(|p| {
        p.active = false;
        p.phase = "done".into();
        p.speed = 0.0;
        p.message = if running() {
            "安装完成，服务已在发布数据".into()
        } else {
            "安装完成，但服务还没发布数据（可点「重新启动」）".into()
        };
    });
    crate::logging::info(&format!(
        "安装完成：installed={} running={}",
        installed(),
        running()
    ));
    Ok(state())
}

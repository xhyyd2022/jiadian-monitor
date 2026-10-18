//! LibreHardwareService（第三方组件，MPL-2.0）：检测 / 从官方源下载 / 一键安装。
//!
//! 只做三件事：判断装没装、在不在跑；从**官方发布页**下载并按固定 sha256 校验；用 msiexec 提权安装。
//! **不修改它的任何文件**；界面需展示来源与许可（见 `app/THIRD-PARTY-NOTICES.md`）。

use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};
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
    })
}

/// 本地已有的 MSI（离线回退：用户把 MSI 放到 exe 同级）。
fn local_msi() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.join("LibreHardwareService.msi"));
            cands.push(d.join("lhws").join("LibreHardwareService.msi"));
        }
    }
    cands.push(cache_path());
    cands.into_iter().find(|p| p.is_file())
}

/// 下载缓存路径（`%TEMP%\jiaidian-monitor\lhws-<版本>.msi`）。
fn cache_path() -> PathBuf {
    std::env::temp_dir()
        .join("jiaidian-monitor")
        .join(format!("lhws-{VERSION}.msi"))
}

/// 从官方发布页下载（超时 15 分钟，57MB）。
fn download(dst: &Path) -> Result<(), String> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("建目录失败：{e}"))?;
    }
    crate::logging::info(&format!("开始下载 {DOWNLOAD_URL}"));
    let agent = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(900)))
        .build()
        .new_agent();
    let resp = agent
        .get(DOWNLOAD_URL)
        .call()
        .map_err(|e| format!("下载失败：{e}"))?;
    let mut body = resp.into_body();
    let mut f = std::fs::File::create(dst).map_err(|e| format!("创建临时文件失败：{e}"))?;
    let n = std::io::copy(&mut body.as_reader(), &mut f).map_err(|e| format!("写入失败：{e}"))?;
    crate::logging::info(&format!("下载完成：{}（{n} 字节）", dst.display()));
    if n < 1_000_000 {
        return Err(format!("下载内容过小（{n} 字节），可能被拦截或断流"));
    }
    Ok(())
}

/// 兜底下载：交给系统自带的 `curl.exe`（某些网络/CDN 对 rustls 会突然断连）。
fn download_with_curl(dst: &Path) -> Result<(), String> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("建目录失败：{e}"))?;
    }
    crate::logging::info("改用系统 curl.exe 下载…");
    let out = std::process::Command::new("curl.exe")
        .args(["-L", "--fail", "--retry", "2", "--max-time", "900", "-o"])
        .arg(dst)
        .arg(DOWNLOAD_URL)
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl 下载失败（退出码 {:?}）：{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")
        ));
    }
    let n = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    if n < 1_000_000 {
        return Err(format!("curl 下载内容过小（{n} 字节）"));
    }
    crate::logging::info(&format!("curl 下载完成（{n} 字节）"));
    Ok(())
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

/// 一键安装：本地有就用本地的，没有就下载 → 校验 sha256 → 提权安装 → 复检。
pub fn install() -> Result<serde_json::Value, String> {
    let msi = match local_msi() {
        Some(p) => {
            crate::logging::info(&format!("使用本地 MSI：{}", p.display()));
            p
        }
        None => {
            let p = cache_path();
            if let Err(e) = download(&p) {
                // 内置 HTTP 客户端失败 → 走系统 curl，这是设计好的回退链，属正常路径。
                // 真正失败会在 download_with_curl 里以 Err 抛出（调用方负责报错）。
                crate::logging::info(&format!("内置下载失败（{e}），改用 curl 兜底"));
                download_with_curl(&p)?;
            }
            p
        }
    };
    let got = sha256_file(&msi)?;
    if !got.eq_ignore_ascii_case(SHA256) {
        return Err(format!("校验失败（期望 {SHA256}，实际 {got}）"));
    }
    crate::logging::info("sha256 校验通过，开始安装");
    let code = run_msiexec(&msi)?;
    if code != 0 && code != 3010 {
        return Err(format!("msiexec 退出码 {code}（1603=安装失败，1625=策略禁止）"));
    }
    for _ in 0..40 {
        if running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    crate::logging::info(&format!(
        "安装完成：installed={} running={}",
        installed(),
        running()
    ));
    Ok(state())
}

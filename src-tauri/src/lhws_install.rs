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

/// 传感器共享内存（存在即服务在跑）
const MMF_SENSORS: &str = "Global\\LibreHardwareService/json/sensors/data";
/// 服务注册表键
const SVC_KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\LibreHardwareService";

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

/// 状态（给界面用，含来源/许可信息，便于一处展示合规声明）。
pub fn state() -> serde_json::Value {
    json!({
        "version": VERSION,
        "installed": installed(),
        "running": running(),
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

/// 提权跑 `msiexec /i <msi> /qn /norestart`，返回退出码（0=成功，3010=成功需重启）。
fn run_msiexec(msi: &Path) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb = to_wide("runas");
    let file = to_wide("msiexec.exe");
    let params = to_wide(&format!(
        "/i \"{}\" /qn /norestart",
        msi.to_string_lossy()
    ));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE as i32;
    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok == 0 {
        return Err("提权安装被取消或失败（UAC 未通过？）".into());
    }
    crate::logging::info("已提权启动 msiexec，等待安装完成（最多 10 分钟）…");
    unsafe {
        let r = WaitForSingleObject(info.hProcess, 600_000);
        if r != 0 {
            CloseHandle(info.hProcess);
            return Err("等待安装超时（可能有 UAC 弹窗未确认）".into());
        }
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(code)
    }
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
                crate::logging::warn(&format!("内置下载失败（{e}），改用 curl 兜底"));
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

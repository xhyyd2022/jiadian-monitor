//! 开机自启动：写/删 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\jiadian-monitor`。
//!
//! 用 HKCU（当前用户）而不是 HKLM，免管理员；值 = 可执行文件完整路径（带引号）。

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "jiadian-monitor";
/// 旧版本（拼音拼错那一版）写的值名。改名后必须搬走，否则它一直指向已经不存在的旧 exe。
const LEGACY_VALUE_NAME: &str = "jiaidian-monitor";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 是否已设置开机自启动。
pub fn enabled() -> bool {
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        ) != 0
        {
            return false;
        }
        let mut buf = [0u8; 1024];
        let mut len = buf.len() as u32;
        let mut ty = 0u32;
        let r = RegQueryValueExW(
            hkey,
            wide(VALUE_NAME).as_ptr(),
            std::ptr::null_mut(),
            &mut ty,
            buf.as_mut_ptr(),
            &mut len,
        );
        RegCloseKey(hkey);
        r == 0
    }
}

/// 删一个自启动值（不存在算成功）。
fn delete_value(name: &str) -> bool {
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        ) != 0
        {
            return false;
        }
        let r = RegDeleteValueW(hkey, wide(name).as_ptr());
        RegCloseKey(hkey);
        r == 0 || r == 2 // 2 = ERROR_FILE_NOT_FOUND
    }
}

/// 把旧版（`jiaidian-monitor`）的自启动项迁到新值名。
///
/// 改名不能只改常量：老值还留在 `Run` 里指向旧 exe 路径，用户那边一旦删掉旧 exe，
/// 开机就会报「找不到文件」；两个值同时存在还会重复启动。这里在启动时做一次性迁移。
pub fn migrate_legacy() {
    // 旧值不存在 → 什么都不做（绝大多数情况）。
    let exists = unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        ) != 0
        {
            return;
        }
        let mut buf = [0u8; 1024];
        let mut len = buf.len() as u32;
        let mut ty = 0u32;
        let r = RegQueryValueExW(
            hkey,
            wide(LEGACY_VALUE_NAME).as_ptr(),
            std::ptr::null_mut(),
            &mut ty,
            buf.as_mut_ptr(),
            &mut len,
        );
        RegCloseKey(hkey);
        r == 0
    };
    if !exists {
        return;
    }
    // 先按当前 exe 路径写新值，再删旧值：中途失败也不会把自启动弄丢。
    let _ = set(true);
    let _ = delete_value(LEGACY_VALUE_NAME);
}

/// 设置 / 取消开机自启动。
pub fn set(on: bool) -> Result<String, String> {
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if rc != 0 {
            return Err(format!("打开注册表失败 code={rc}"));
        }
        let r = if on {
            let path = format!("\"{}\"", exe_path());
            let data = wide(&path);
            RegSetValueExW(
                hkey,
                wide(VALUE_NAME).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(hkey, wide(VALUE_NAME).as_ptr())
        };
        RegCloseKey(hkey);
        // 2 = ERROR_FILE_NOT_FOUND：删一个本来就没有的值，视为成功
        if r != 0 && !(!on && r == 2) {
            return Err(format!("写注册表失败 code={r}"));
        }
        if !on {
            // 关掉时连旧值一起清掉，别让改名前的残留项又把它拉起来。
            let _ = delete_value(LEGACY_VALUE_NAME);
        }
        Ok(if on {
            "已开启开机自启动".into()
        } else {
            "已关闭开机自启动".into()
        })
    }
}

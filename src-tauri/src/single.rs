//! 单实例：用命名互斥体（named mutex）保证同一会话只跑一个本程序。
//!
//! 判据是**内核对象是否存在**（`CreateMutexW` + `ERROR_ALREADY_EXISTS`），
//! 不用端口/文件锁：端口会被别的程序占用，文件锁会因强杀留下脏文件。
//! 互斥体随进程销毁自动释放，所以被强杀后下次启动照常。

use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};

/// 句柄存起来不关：进程活着就持有这个互斥体。
static MUTEX: AtomicIsize = AtomicIsize::new(0);

/// `Local\` = 每个登录会话一个（不同 Windows 用户各能开一个；`Global\` 需要特权，普通用户建不出来）。
const NAME: &str = "Local\\jiaidian-monitor-single";

/// 抢单实例锁。`true` = 本次是唯一实例；`false` = 已有实例在跑。
pub fn acquire() -> bool {
    unsafe {
        let name: Vec<u16> = NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let h: HANDLE = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if h.is_null() {
            // 创建失败就不拦（宁可多开也别打不开）
            return true;
        }
        MUTEX.store(h as isize, Ordering::SeqCst);
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// 请已在跑的实例把窗口显示出来。返回是否把请求递出去了。
pub fn wake_existing() -> bool {
    use std::io::Write;
    let addr = format!("127.0.0.1:{}", crate::api::PORT);
    let Ok(sa) = addr.parse() else { return false };
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&sa, std::time::Duration::from_millis(800))
    else {
        return false;
    };
    let _ = s.set_write_timeout(Some(std::time::Duration::from_millis(800)));
    s.write_all(
        b"POST /app/show HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .is_ok()
}

/// 唤不起已有实例时的兜底提示（它可能还在启动中，或本地 API 端口被别的程序占了）。
pub fn notify_already_running() {
    unsafe {
        let text: Vec<u16> = "程序已在运行（窗口可能在系统托盘里）。\0".encode_utf16().collect();
        let cap: Vec<u16> = "jiaidian-monitor\0".encode_utf16().collect();
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            cap.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

//! 系统托盘：关闭按钮收进托盘、左键恢复主窗口、右键菜单「显示主界面 / 退出」。
//!
//! 托盘图标优先从 `assets/icon.png` / `assets/icon.ico`（当前目录）加载 —— 换图标不用重编译；
//! 找不到或读取失败则回退 exe 内置图标（并记一条日志）。
//! 托盘创建失败时关闭按钮回退「最小化到任务栏」，避免窗口藏起来找不回。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// 托盘是否可用（创建成功后置 true；关闭按钮据此决定 hide 还是 minimize）。
static AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn available() -> bool {
    AVAILABLE.load(Ordering::SeqCst)
}

/// 创建托盘（在 app setup 里调用一次）。
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id("tray-show", "显示主界面").build(app)?;
    let quit = MenuItemBuilder::with_id("tray-quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&quit)
        .build()?;

    let custom = load_custom_icon();
    let icon = custom.clone().or_else(|| app.default_window_icon().cloned());

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("佳点监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "tray-show" => show_main(app),
            "tray-quit" => {
                crate::logging::info("[ui] 托盘菜单：退出");
                // 先发退出帧并归还设备，再退出
                crate::pipeline::release(1500);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(ic) = icon {
        builder = builder.icon(ic);
    }
    let tray = builder.build(app)?;
    // 参考同为 Tauri 的 TieZ（jimuzhe/tiez-clipboard）：build 后显式 set_visible(true)，
    // 并把 TrayIcon 句柄交给 app.manage 托管，避免“创建成功但图标不在通知区”。
    let _ = tray.set_visible(true);
    app.manage(tray);
    AVAILABLE.store(true, Ordering::SeqCst);

    // 自定义图标同时挂到主窗口（任务栏图标跟随）
    if let Some(ic) = custom {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_icon(ic);
        }
    }
    crate::logging::info("系统托盘就绪（左键显示 / 右键菜单）");
    // Windows 11 默认把新图标塞进“其他系统托盘图标”（溢出区）。这里在图标注册后
    // 尝试把本程序的 NotifyIconSettings 条目标记为 IsPromoted，尽量让它常驻任务栏；
    // 若系统仍不生效，用户可在 任务栏设置 → 其他系统托盘图标 里手动打开。
    std::thread::spawn(promote_notify_icon);
    Ok(())
}

/// 让本程序的托盘图标尽量“常驻”，而不是折叠进 Windows 11 的“其他系统托盘图标”。
///
/// 背景：Windows 没有公开 API 控制通知区域图标的“显示/折叠”，任务栏设置界面自己也是写
/// `HKCU\Control Panel\NotifyIconSettings\<hash>` 下的 `IsPromoted=1`。这里用 Win32 注册表
/// API 直接做同样的事（不再起 PowerShell，启动时不会闪控制台）。
fn promote_notify_icon() {
    use std::ptr::null;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_SZ,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = exe.to_string_lossy().to_string();
    let mut promoted = false;
    unsafe {
        // 预定义 HKEY 在 64 位上必须是符号扩展后的 0xFFFFFFFF80000001；
        // 直接 i32 → 指针的隐式行为在某些工具链/FFI 下可能变成零扩展，导致 ERROR_ACCESS_DENIED。
        let hkcu: HKEY = 0xFFFF_FFFF_8000_0001usize as HKEY;
        let mut root: HKEY = std::ptr::null_mut();
        let sub = wide("Control Panel\\NotifyIconSettings");
        let rc = RegOpenKeyExW(hkcu, sub.as_ptr(), 0, KEY_READ | KEY_WRITE, &mut root);
        if rc != 0 {
            let code = windows_sys::Win32::Foundation::GetLastError();
            crate::logging::warn(&format!(
                "托盘图标常驻设置：打开 NotifyIconSettings 失败（rc={rc} last_error={code}，不影响托盘）"
            ));
            return;
        }
        let mut index = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let rc = RegEnumKeyExW(root, index, name.as_mut_ptr(), &mut len, null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if rc != 0 { break; }
            index += 1;
            let mut sk: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(root, name.as_ptr(), 0, KEY_READ | KEY_WRITE, &mut sk) != 0 { continue; }
            let value_name = wide("ExecutablePath");
            let mut ty = 0u32;
            let mut buf = [0u8; 4096];
            let mut size = buf.len() as u32;
            let rc = RegQueryValueExW(sk, value_name.as_ptr(), null(), &mut ty, buf.as_mut_ptr(), &mut size);
            if rc == 0 && (ty == REG_SZ || ty == REG_EXPAND_SZ) {
                let units = (size as usize / 2).saturating_sub(1);
                let path = String::from_utf16_lossy(std::slice::from_raw_parts(buf.as_ptr() as *const u16, units));
                if path.trim().eq_ignore_ascii_case(&exe) {
                    let one: u32 = 1;
                    let promoted_name = wide("IsPromoted");
                    let _ = RegSetValueExW(sk, promoted_name.as_ptr(), 0, REG_DWORD, &one as *const u32 as *const u8, 4);
                    let tooltip = wide("佳点监控");
                    let tooltip_name = wide("InitialTooltip");
                    let _ = RegSetValueExW(sk, tooltip_name.as_ptr(), 0, REG_SZ, tooltip.as_ptr() as *const u8, (tooltip.len() * 2) as u32);
                    promoted = true;
                }
            }
            RegCloseKey(sk);
        }
        RegCloseKey(root);
    }
    if promoted {
        crate::logging::info("托盘图标已设为常驻（IsPromoted=1；资源管理器重排后生效）");
        return;
    }
    // 原生写失败时不再调用 PowerShell（避免额外进程/控制台）。
    // 常见原因：本进程以受限令牌运行（如从其他进程/沙箱拉起），或系统策略限制。
    // 以普通方式双击/开机自启时通常有权限；实在写不进去，用户可在
    // 设置 → 个性化 → 任务栏 → 其他系统托盘图标 里手动打开。
    crate::logging::warn("托盘图标常驻设置未生效：注册表写入被拒绝；可在任务栏设置里手动打开「佳点监控」");
}

/// 显示并聚焦主窗口（托盘左键 / 菜单「显示主界面」/ 第二实例唤起）。
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        // Windows 限制后台进程抢前台：先置顶再取消置顶，能把窗口真正带到最前面。
        let _ = w.set_always_on_top(true);
        let _ = w.set_always_on_top(false);
        let _ = w.set_focus();
        crate::logging::info("[ui] 恢复主窗口");
    }
}

/// `assets/icon.png`（或 `icon.ico`）→ 托盘/窗口图标；没有就 None（用内置图标）。
fn load_custom_icon() -> Option<Image<'static>> {
    let dir = crate::assets::dir()?;
    for name in ["icon.ico", "icon.png"] {
        let p = dir.join(name);
        if !p.is_file() {
            continue;
        }
        match Image::from_path(&p) {
            Ok(img) => {
                crate::logging::info(&format!("图标 ← {}", p.display()));
                return Some(img);
            }
            Err(e) => crate::logging::warn(&format!("图标 {} 读取失败：{e}", p.display())),
        }
    }
    None
}

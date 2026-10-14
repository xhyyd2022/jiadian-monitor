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
        .tooltip("jiaidian-monitor")
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
    builder.build(app)?;
    AVAILABLE.store(true, Ordering::SeqCst);

    // 自定义图标同时挂到主窗口（任务栏图标跟随）
    if let Some(ic) = custom {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_icon(ic);
        }
    }
    crate::logging::info("系统托盘就绪（左键显示 / 右键菜单）");
    Ok(())
}

/// 显示并聚焦主窗口（托盘左键 / 菜单「显示主界面」/ 第二实例唤起）。
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        crate::logging::info("[ui] 恢复主窗口");
    }
}

/// `assets/icon.png`（或 `icon.ico`）→ 托盘/窗口图标；没有就 None（用内置图标）。
fn load_custom_icon() -> Option<Image<'static>> {
    let dir = crate::assets::dir()?;
    for name in ["icon.png", "icon.ico"] {
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

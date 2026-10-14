mod api;
mod assets;
mod autostart;
mod device;
mod diff;
mod handoff;
mod live_frame;
mod lhws_install;
mod source_lhws;
mod logging;
mod metrics;
mod units;

/// 离屏面板 WebView2 的 CDP 端口（`main.rs` 设环境变量时要用）。
pub use render::PANEL_CDP_PORT;
mod pipeline;
mod render;
pub mod single;
mod tray;
mod winusb;

use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::Manager;

/// 主 AppHandle（`setup` 时存入）：本地 API 线程靠它切到主线程干活。
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// LibreHardwareService（第三方组件，MPL-2.0）状态：装没装、在不在跑 + 来源/许可信息。
#[tauri::command]
fn lhws_state() -> serde_json::Value {
    lhws_install::state()
}

/// 一键安装 LibreHardwareService：官方源下载 → sha256 校验 → msiexec 提权安装（会弹一次 UAC）。
#[tauri::command]
async fn lhws_install() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(lhws_install::install)
        .await
        .map_err(|e| e.to_string())?
}

/// 当前**启用了哪几路**数据源（前端据此决定要不要提示装 LibreHardwareService）。
#[tauri::command]
fn data_sources() -> serde_json::Value {
    serde_json::json!({
        "lhws": crate::metrics::lhws_enabled(),
    })
}

/// 单位设置：`auto` 全自动（按素材 `data-unit`）/ `manual` 手动指定（按 `units` 覆盖）。
/// 面板注入脚本每秒拉 `GET /units` → 改完立刻生效，不用重开面板。
#[tauri::command]
fn set_unit_config(
    mode: String,
    units: std::collections::HashMap<String, String>,
) -> Result<String, String> {
    crate::units::set(mode.clone(), units.clone());
    let mut kv: Vec<String> = units
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    kv.sort();
    logging::info(&format!(
        "[ui] 单位设置 = {mode}{}",
        if kv.is_empty() {
            String::new()
        } else {
            format!("（{}）", kv.join(" "))
        }
    ));
    Ok("ok".into())
}

/// 用系统默认浏览器打开链接。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    tauri_plugin_opener::open_url(url.as_str(), None::<&str>).map_err(|e| e.to_string())
}

pub fn app_handle() -> Option<&'static tauri::AppHandle> {
    APP.get()
}

/// 给 `main.rs` 用的日志入口（logging 模块是私有的）。
pub fn log_info(msg: &str) {
    logging::info(msg);
}

/// CLI：打印 LibreHardwareService 状态（JSON）。
pub fn lhws_state_json() -> String {
    serde_json::to_string_pretty(&lhws_install::state()).unwrap_or_default()
}

/// CLI：一键安装 LibreHardwareService（官方源下载 + sha256 校验 + 提权安装）。
pub fn lhws_install_now() -> Result<String, String> {
    let v = lhws_install::install()?;
    Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
}

/// 唤起主窗口（第二实例启动时用）。
pub fn show_main_window() {
    if let Some(app) = app_handle() {
        tray::show_main(app);
    }
}

/// 优雅退出：先发退出帧并归还设备，再退出（编译脚本/外部调用用，避免被强杀留下租约）。
pub fn request_quit() {
    if let Some(app) = app_handle() {
        let app = app.clone();
        std::thread::spawn(move || {
            pipeline::release(2000);
            app.exit(0);
        });
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// 基础指标（LibreHardwareService + Windows API 汇总）。
#[tauri::command]
fn basic_metrics() -> Vec<metrics::BasicMetric> {
    metrics::basic()
}

/// 前端把关键动作/结果写进同一个日志文件，方便和后台时序对齐。
#[tauri::command]
fn log_event(msg: String) {
    logging::info(&format!("[ui] {msg}"));
}

/// 打开主窗口的 DevTools（设置页的开关打开后才允许调用）。
#[tauri::command]
fn open_devtools(app: tauri::AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        w.open_devtools();
        logging::info("[ui] 打开 DevTools");
    }
}

/// 设置窗口主题（Windows 上会一并把**原生标题栏**切深/浅色）。
#[tauri::command]
fn set_window_theme(app: tauri::AppHandle, dark: bool) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let t = if dark { tauri::Theme::Dark } else { tauri::Theme::Light };
        let _ = w.set_theme(Some(t));
    }
}

/// 设备信息：SN / 能力(JED) / 电压 / 显示状态。
#[tauri::command]
fn device_info(serial: Option<String>) -> Result<serde_json::Value, String> {
    device::info(serial)
}

/// 设亮度 0..=255（0 = 息屏）。
#[tauri::command]
fn device_set_brightness(serial: Option<String>, value: u8) -> Result<String, String> {
    device::set_brightness(serial, value)
}

/// 设旋转 0..=3。
#[tauri::command]
fn device_set_rotation(serial: Option<String>, value: u8) -> Result<String, String> {
    device::set_rotation(serial, value)
}

/// 跟随息屏 开/关。
#[tauri::command]
fn device_set_follow_off(serial: Option<String>, value: bool) -> Result<String, String> {
    device::set_follow_off(serial, value)
}

/// 完整下发一次设备配置（亮度 + 旋转 + 跟随息屏），改任一项都调它。
#[tauri::command]
fn device_apply_config(
    serial: Option<String>,
    brightness: u8,
    rotation: u8,
    follow_off: bool,
) -> Result<serde_json::Value, String> {
    device::apply_config(serial, brightness, rotation, follow_off)
}

/// 是否已设置开机自启动。
#[tauri::command]
fn autostart_get() -> bool {
    autostart::enabled()
}

/// 设置 / 取消开机自启动。
#[tauri::command]
fn autostart_set(value: bool) -> Result<String, String> {
    let r = autostart::set(value)?;
    logging::info(&format!("[ui] {r}"));
    Ok(r)
}

/// 静默启动标记文件：存在 = 启动时不显示主窗口（exe 同级，便携布局里即 `bin/silent.flag`）。
///
/// 用文件而不是前端 localStorage：窗口显示与否必须在 Tauri `setup()` 里就决定，
/// 那时前端还没跑起来；文件是唯一能在启动瞬间读到的地方。
fn silent_flag_path() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("silent.flag"))
}

/// 是否开启静默启动。
pub fn silent_start_enabled() -> bool {
    silent_flag_path().map(|p| p.is_file()).unwrap_or(false)
}

/// 读静默启动设置。
#[tauri::command]
fn silent_start_get() -> bool {
    silent_start_enabled()
}

/// 设置静默启动（写 / 删 exe 同级的标记文件，**下次启动生效**）。
#[tauri::command]
fn set_silent_start(on: bool) -> Result<String, String> {
    let p = silent_flag_path().ok_or("拿不到可执行文件目录")?;
    if on {
        std::fs::write(&p, b"1").map_err(|e| format!("写入 {} 失败：{e}", p.display()))?;
    } else if p.is_file() {
        std::fs::remove_file(&p).map_err(|e| format!("删除 {} 失败：{e}", p.display()))?;
    }
    let m = if on {
        "已开启静默启动（下次启动不显示窗口，直接进托盘）"
    } else {
        "已关闭静默启动"
    };
    logging::info(&format!("[ui] {m}"));
    Ok(m.into())
}

/// 初始化文件日志（CLI 无界面模式也用）。
pub fn init_logging() {
    logging::init();
}

/// 枚举 WinUSB 设备（path + serial）。
#[tauri::command]
fn usb_list() -> Result<Vec<serde_json::Value>, String> {
    Ok(winusb::list_devices()?
        .into_iter()
        .map(|(p, sn)| json!({ "path": p, "serial": sn }))
        .collect())
}

/// 设备列表：本地 WinUSB 物理枚举 + GUI `/usb/status` 的 `devices[]` 富化（尺寸 / 机型）。
///
/// 物理枚举保证「没点连接也能看到设备」；GUI 那份顺带给出 `panelWidth/Height` 与
/// `hardwareVariant`，**不用逐台开 USB 读 JED** 就能知道是 320×240 还是 320×170。
#[tauri::command]
fn device_list() -> Result<Vec<serde_json::Value>, String> {
    let phys = winusb::list_devices().unwrap_or_default();
    let gui_devs: Vec<serde_json::Value> = handoff::status()
        .ok()
        .and_then(|v| v.get("devices").and_then(|d| d.as_array()).cloned())
        .unwrap_or_default();
    // SN 忽略大小写匹配：GUI 与本地枚举的来源可能大小写不同，直接用 `==` 会漏富化
    let find_gui = |sn: &str| {
        gui_devs.iter().find(|d| {
            d.get("serial")
                .and_then(|s| s.as_str())
                .is_some_and(|x| x.eq_ignore_ascii_case(sn))
        })
    };
    let enrich = |d: Option<&serde_json::Value>| {
        (
            d.and_then(|d| d.get("panelWidth")).and_then(|v| v.as_u64()),
            d.and_then(|d| d.get("panelHeight")).and_then(|v| v.as_u64()),
            d.and_then(|d| d.get("hardwareVariant"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            d.and_then(|d| d.get("connected"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        )
    };

    let mut out: Vec<serde_json::Value> = Vec::new();
    for (path, sn) in &phys {
        let (w, h, variant, connected) = enrich(sn.as_deref().and_then(find_gui));
        out.push(json!({
            "serial": sn,
            "path": path,
            "width": w,
            "height": h,
            "variant": variant,
            "connected": connected,
        }));
    }
    // GUI 报告但本地没枚举到的（少见，兜底）
    for d in &gui_devs {
        let sn = d.get("serial").and_then(|s| s.as_str()).unwrap_or("");
        if sn.is_empty()
            || out.iter().any(|o| {
                o.get("serial")
                    .and_then(|s| s.as_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case(sn))
            })
        {
            continue;
        }
        let (w, h, variant, connected) = enrich(Some(d));
        out.push(json!({
            "serial": sn,
            "path": serde_json::Value::Null,
            "width": w,
            "height": h,
            "variant": variant,
            "connected": connected,
        }));
    }
    Ok(out)
}

/// 查询 GUI 的 `/usb/status`（GUI 未运行时返回 Err）。
#[tauri::command]
fn usb_gui_status() -> Result<serde_json::Value, String> {
    handoff::status()
}

/// 走「移交 → 打开 → 发一帧全屏测试图 → 读回执 → 结束移交」验证 USB 下发。
#[tauri::command]
fn usb_test_pattern(
    serial: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<String, String> {
    usb_test(serial, width.unwrap_or(320), height.unwrap_or(240))
}

/// 启动实时渲染管线（app 自带 WebView2 离屏窗口 → 差分 → USB 下发），每帧推 `preview` 事件。
/// **必须 async**：里面要创建 WebView2 窗口，同步命令跑在主线程上会死锁。
#[tauri::command]
async fn render_start(
    app: tauri::AppHandle,
    html: String,
    serial: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<String, String> {
    pipeline::start(app, html, serial, width.unwrap_or(320), height.unwrap_or(240))
}

/// 停止管线并等设备归还。返回给界面提示。
#[tauri::command]
async fn render_stop() -> Result<String, String> {
    Ok(if pipeline::stop_and_wait(1500).await {
        "已停止".into()
    } else {
        "已停止（仍在收尾）".into()
    })
}

/// **取消接管**：发 LIVE 退出帧（设备回默认素材）+ 归还控制权给官方上位机。
#[tauri::command]
async fn render_release() -> Result<String, String> {
    let ok = tauri::async_runtime::spawn_blocking(|| pipeline::release(1500))
        .await
        .unwrap_or(false);
    Ok(if ok {
        "已取消接管，控制权已归还".into()
    } else {
        "已取消接管（仍在收尾）".into()
    })
}

/// 扫描 `assets/` 下的主题（一级子文件夹），返回 主题名 + 相对页面路径（已排序）。
#[tauri::command]
fn list_themes() -> Vec<assets::Theme> {
    assets::list_themes()
}

/// 当前 USB 链路状态：`open` = 已接管、`ready` = 管线已做过接管尝试、`serial` = 接管中的 SN。
/// 前端用它等「管线真正接管完」再读设备信息，避免和管线并发抢设备。
#[tauri::command]
fn link_state() -> serde_json::Value {
    serde_json::json!({
        "open": pipeline::link_open(),
        "ready": pipeline::link_ready(),
        "serial": pipeline::link_serial(),
    })
}

/// 「手动导入」对话框的默认定位目录（`assets/` 的绝对路径）。
#[tauri::command]
fn assets_dir() -> Option<String> {
    assets::dir().map(|p| p.to_string_lossy().to_string())
}

/// 手动导入：把选中的 HTML（连同同目录图片）整理成主题文件夹，返回落位后的主题。
#[tauri::command]
fn import_panel(src: String) -> Result<assets::Theme, String> {
    assets::import(&src)
}

/// 关闭按钮（设置开启时）：收进系统托盘；托盘不可用则回退任务栏最小化。
#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let w = app.get_webview_window("main").ok_or("主窗口不存在")?;
    if !tray::available() {
        w.minimize().map_err(|e| e.to_string())?;
        logging::info("[ui] 托盘不可用 → 已回退任务栏最小化");
        return Ok("minimized".into());
    }
    w.hide().map_err(|e| e.to_string())?;
    logging::info("[ui] 关闭 → 已收进系统托盘（左键托盘图标恢复）");
    Ok("hidden".into())
}

/// 无界面：跑 N 秒渲染管线（验证用）。
pub fn render_headless(html: String, seconds: f64) -> Result<String, String> {
    pipeline::run_headless(html, None, 320, 240, seconds)
}

/// 无界面：打开面板并截图（诊断用）。
pub fn capture_shot(html: String, out: String) -> Result<String, String> {
    pipeline::capture_shot(&html, &out)
}

pub fn usb_test(serial: Option<String>, w: u32, h: u32) -> Result<String, String> {
    let gui = handoff::gui_running();

    let target = match serial {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ if gui => {
            let st = handoff::status()?;
            st.get("devices")
                .and_then(|d| d.as_array())
                .and_then(|a| {
                    a.iter()
                        .find_map(|e| e.get("serial").and_then(|s| s.as_str()))
                })
                .or_else(|| st.get("serial").and_then(|s| s.as_str()))
                .ok_or("GUI 未列出设备 SN")?
                .to_string()
        }
        _ => winusb::list_devices()?
            .into_iter()
            .find_map(|(_, sn)| sn)
            .ok_or("没有可用设备")?,
    };

    let client_id = format!("jiaidian-monitor-{}", std::process::id());
    let mut lease: Option<String> = None;
    if gui {
        let r = handoff::handoff_start(&target, &client_id, 90)?;
        if r.get("ok").and_then(|b| b.as_bool()) != Some(true) {
            return Err(format!("移交失败：{r}"));
        }
        lease = r
            .get("leaseId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        std::thread::sleep(Duration::from_millis(1500));
    }

    let result = (|| -> Result<String, String> {
        let dev = winusb::find_by_serial(&target)?;
        let frame = build_test_frame(w, h);
        let sent = dev.write(&frame)?;
        let ack = dev.read(256)?;
        let (kind, reason) = live_frame::parse_ack(ack.as_deref());
        Ok(format!(
            "SN={:?} sent={}B ack={} {}",
            dev.serial,
            sent,
            kind,
            reason.unwrap_or_default()
        ))
    })();

    if let Some(id) = lease {
        let _ = handoff::handoff_finish(&id, &target);
    }
    result
}

/// 8 段彩色竖条测试图（全屏 mode 0）。
fn build_test_frame(w: u32, h: u32) -> Vec<u8> {
    let bars: [[u8; 3]; 8] = [
        [255, 255, 255],
        [255, 255, 0],
        [0, 255, 255],
        [0, 255, 0],
        [255, 0, 255],
        [255, 0, 0],
        [0, 0, 255],
        [0, 0, 0],
    ];
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let b = bars[(((x * 8) / w).min(7)) as usize];
            let o = ((y * w + x) * 3) as usize;
            rgb[o] = b[0];
            rgb[o + 1] = b[1];
            rgb[o + 2] = b[2];
        }
    }
    let payload = live_frame::rgb888_to_rgb565_be(&rgb);
    live_frame::frame_full(&payload)
}

/// 启动本地指标 API（CLI 无界面模式下也能用）。
pub fn start_api() {
    api::spawn();
}

/// 无界面用途：把 23 项基础指标序列化成 JSON（供 `--dump-metrics` 命令行使用）。
/// **采样两次**（间隔 1.2 s）：网络速率要两次采样才推得出来，只采一次永远是 `—`。
pub fn dump_metrics_json() -> String {
    let _ = metrics::basic();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    match serde_json::to_string_pretty(&metrics::basic()) {
        Ok(s) => s,
        Err(e) => format!("{{\"error\": \"{e}\"}}"),
    }
}

/// 无界面用途：LibreHardwareService 共享内存里的**全部原始传感器**（供 `--dump-lhws` 排查用）。
pub fn dump_lhws_json() -> String {
    crate::source_lhws::dump_raw_json()
}

/// 无界面用途：Win32 看到的**物理网卡**原样列表（供 `--dump-nics` 排查用）。
pub fn dump_nics_json() -> String {
    metrics::dump_nics_json()
}

/// 无界面用途：当前单位设置 JSON（供 `--dump-units` 排查用）。
pub fn dump_units_json() -> String {
    units::json()
}

/// 无界面用途：设置单位配置（供 `--units` 用）。
/// 写法：`auto`（全自动）或 `manual:capacity=MB,rate=MB/s,temperature=℃,duration=分`
/// （冒号后没写的族 = 跟随素材）。
pub fn set_units_cli(spec: &str) -> Result<String, String> {
    let (mode, rest) = match spec.split_once(':') {
        Some((m, r)) => (m.trim(), r),
        None => (spec.trim(), ""),
    };
    let mut map = std::collections::HashMap::new();
    for kv in rest.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("单位条目要写成 族=单位：{kv}"))?;
        map.insert(k.trim().to_string(), v.trim().to_string());
    }
    units::set(mode.to_string(), map);
    Ok(units::json())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    logging::info("=== jiaidian-monitor 启动 ===");
    logging::info(&format!("数据源：{}", metrics::source_hint()));

    // 本地指标 API：面板网页直接 fetch，无需注入。
    api::spawn();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let _ = APP.set(app.handle().clone());
            // 窗口图标 = 应用图标（任务栏/Alt+Tab 都用它）
            if let Some(w) = app.get_webview_window("main") {
                if let Some(ic) = app.default_window_icon() {
                    let _ = w.set_icon(ic.clone());
                }
            }
            if let Err(e) = tray::setup(app.handle()) {
                logging::warn(&format!(
                    "系统托盘创建失败：{e}（关闭按钮回退为最小化到任务栏）"
                ));
            }
            // 主窗口在配置里是 `visible:false`（避免启动瞬间闪一下）。这里按「静默启动」决定显示与否：
            // 开启 → 保持隐藏、只留托盘（左键恢复）；未开启 → 立即显示并聚焦。
            // 托盘不可用时**必须显示窗口**，否则用户无处可点、找不到程序。
            if let Some(w) = app.get_webview_window("main") {
                if silent_start_enabled() && tray::available() {
                    logging::info("静默启动：主窗口保持隐藏（托盘左键恢复）");
                } else {
                    if silent_start_enabled() {
                        logging::warn("静默启动已开启，但托盘不可用 → 显示主窗口（避免找不到）");
                    }
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            basic_metrics,
            log_event,
            open_devtools,
            set_window_theme,
            device_info,
            device_set_brightness,
            device_set_rotation,
            device_set_follow_off,
            device_apply_config,
            autostart_get,
            autostart_set,
            silent_start_get,
            set_silent_start,
            lhws_state,
            lhws_install,
            data_sources,
            set_unit_config,
            open_url,
            usb_list,
            device_list,
            usb_gui_status,
            usb_test_pattern,
            render_start,
            render_stop,
            render_release,
            list_themes,
            link_state,
            assets_dir,
            import_panel,
            hide_to_tray
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // 退出：停管线，释放设备
            tauri::RunEvent::ExitRequested { .. } => {
                pipeline::shutdown(app);
            }
            tauri::RunEvent::Exit => {
                pipeline::shutdown(app);
            }
            // 主窗口销毁即释放（面板窗口还在时应用不退出，等不到 ExitRequested）
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } if label == "main" => {
                logging::info("主窗口已销毁：发退出帧并释放设备");
                pipeline::release(1500);
                // 关键：离屏面板窗口若还开着，Tauri 认为「还有窗口」→ 不发 ExitRequested →
                // 进程挂着退不掉（用户以为已关闭）。这里把它一并关掉，让应用真正退出。
                if let Some(w) = app.get_webview_window("panel") {
                    let _ = w.close();
                    logging::info("主窗口销毁：一并关闭离屏面板窗口");
                }
            }
            _ => {}
        });
}

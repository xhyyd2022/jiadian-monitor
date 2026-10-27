// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebView2 的 CDP 端口与禁用遮挡参数现在显式写进
    // `tauri.conf.json` 的 `app.windows[0].additionalBrowserArgs` 和
    // `pipeline::ensure_panel_webview`，**不能再依赖环境变量**：
    // wry 0.57 总会给 WebView2 设置 additionalBrowserArguments，
    // 而 WebView2 在设置了该项时会忽略 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`。
    //
    // ⚠️ **不要加 `--disable-web-security`**：它会让 WebView2 不再发送 `Origin` 头，
    // 而 Tauri IPC 强制校验 Origin（tauri/src/ipc/protocol.rs，缺了直接返回
    // "missing Origin header"）→ 前端所有 invoke 全部失败、界面表现为「没反应」。
    // 面板要 fetch 本地指标 API（file:// 页面 origin 为 null）靠的是 API 自己回的
    // `Access-Control-Allow-Origin: *`（见 api.rs），与浏览器安全开关无关；
    // 主题素材也是同目录自包含、无跨目录引用，因此也不需要 `--allow-file-access-from-files`。
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        force_utf8_console();
        jiaidian_monitor_lib::init_logging();
    }
    if args.iter().any(|a| a == "--dump-metrics") {
        println!("{}", jiaidian_monitor_lib::dump_metrics_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-lhws") {
        println!("{}", jiaidian_monitor_lib::dump_lhws_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-nics") {
        println!("{}", jiaidian_monitor_lib::dump_nics_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-units") {
        println!("{}", jiaidian_monitor_lib::dump_units_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-cpu") {
        println!("{}", jiaidian_monitor_lib::dump_cpu_json());
        return;
    }
    // 单位设置：与 `--shot` / `--render` 同一条命令行时**只设置不返回**（同一个进程内生效），
    // 方便无界面验证「手动指定单位」的效果。
    if let Some(i) = args.iter().position(|a| a == "--units") {
        let spec = args.get(i + 1).cloned().unwrap_or_default();
        match jiaidian_monitor_lib::set_units_cli(&spec) {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        if !args.iter().any(|a| a == "--shot" || a == "--render") {
            return;
        }
    }
    if args.iter().any(|a| a == "--lhws-check") {
        println!("{}", jiaidian_monitor_lib::lhws_state_json());
        return;
    }
    // 外设协议库诊断：列 HID 端点 / 跑一轮识别 / 回放所有 sample。
    if args.iter().any(|a| a == "--peripherals-hid") {
        println!("{}", jiaidian_monitor_lib::dump_peripherals_hid());
        return;
    }
    if args.iter().any(|a| a == "--peripherals-dump") {
        println!("{}", jiaidian_monitor_lib::dump_peripherals_json());
        return;
    }
    if args.iter().any(|a| a == "--peripherals-selftest") {
        match jiaidian_monitor_lib::peripherals_selftest() {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--lhws-install") {
        match jiaidian_monitor_lib::lhws_install_now() {
            Ok(m) => println!("OK://n{m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--shot") {
        jiaidian_monitor_lib::start_api();
        let html = args.get(i + 1).cloned().unwrap_or_default();
        let out = args
            .get(i + 2)
            .cloned()
            .unwrap_or_else(|| "shot.png".to_string());
        match jiaidian_monitor_lib::capture_shot(html, out) {
            Ok(m) => println!("OK:\n{m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--render") {
        jiaidian_monitor_lib::start_api();
        let Some(html) = args.get(i + 1).cloned() else {
            println!("ERR: --render 需要面板路径，例如 assets/玻璃拟态/index.html");
            return;
        };
        let secs = args
            .iter()
            .position(|a| a == "--seconds")
            .and_then(|j| args.get(j + 1))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(8.0);
        match jiaidian_monitor_lib::render_headless(html, secs) {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--usb-test") {
        match jiaidian_monitor_lib::usb_test(None, 320, 240) {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    // 单实例：已有实例在跑就唤起它的窗口并退出（两个进程会抢 USB 与端口）
    // 放在 CLI 分支之后：`--dump-*` / `--shot` / `--render` 允许在程序运行时执行
    if !jiaidian_monitor_lib::single::acquire() {
        jiaidian_monitor_lib::init_logging();
        let woke = jiaidian_monitor_lib::single::wake_existing();
        jiaidian_monitor_lib::log_info(if woke {
            "检测到已有实例：已唤起它的窗口，本次启动退出"
        } else {
            "检测到已有实例：唤起失败（端口未就绪），本次启动退出"
        });
        if !woke {
            jiaidian_monitor_lib::single::notify_already_running();
        }
        return;
    }
    jiaidian_monitor_lib::run()
}

/// 把当前控制台切到 UTF-8（65001）。内部读取已把 ANSI/GBK 转成 UTF-8 字符串，
/// 这里保证直接写控制台时也不会被系统 936 代码页再解释一遍。
/// 只改输出代码页，不动输入代码页（避免污染调用方控制台）。
fn force_utf8_console() {
    use windows_sys::Win32::System::Console::SetConsoleOutputCP;
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

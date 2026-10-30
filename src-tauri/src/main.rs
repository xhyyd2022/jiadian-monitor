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
        // 顺序有讲究：先挂上父控制台，再改它的输出代码页。反了就是「设了个不存在的控制台」。
        attach_parent_console();
        force_utf8_console();
        jiadian_monitor_lib::init_logging();
    }
    if args.iter().any(|a| a == "--dump-metrics") {
        println!("{}", jiadian_monitor_lib::dump_metrics_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-lhws") {
        println!("{}", jiadian_monitor_lib::dump_lhws_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-nics") {
        println!("{}", jiadian_monitor_lib::dump_nics_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-units") {
        println!("{}", jiadian_monitor_lib::dump_units_json());
        return;
    }
    if args.iter().any(|a| a == "--dump-cpu") {
        println!("{}", jiadian_monitor_lib::dump_cpu_json());
        return;
    }
    // 单位设置：与 `--shot` / `--render` 同一条命令行时**只设置不返回**（同一个进程内生效），
    // 方便无界面验证「手动指定单位」的效果。
    if let Some(i) = args.iter().position(|a| a == "--units") {
        let spec = args.get(i + 1).cloned().unwrap_or_default();
        match jiadian_monitor_lib::set_units_cli(&spec) {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        if !args.iter().any(|a| a == "--shot" || a == "--render") {
            return;
        }
    }
    if args.iter().any(|a| a == "--lhws-check") {
        println!("{}", jiadian_monitor_lib::lhws_state_json());
        return;
    }
    // 外设协议库诊断：列 HID 端点 / 跑一轮识别 / 回放所有 sample。
    if args.iter().any(|a| a == "--peripherals-hid") {
        emit_cli("peripherals-hid.txt", &jiadian_monitor_lib::dump_peripherals_hid());
        return;
    }
    // 逐 HID 集合展开：只在排查「某只鼠标有哪几个集合」时才需要，默认输出是聚合过的。
    if args.iter().any(|a| a == "--peripherals-hid-all") {
        emit_cli("peripherals-hid-all.txt", &jiadian_monitor_lib::dump_peripherals_hid_all());
        return;
    }
    if args.iter().any(|a| a == "--peripherals-dump") {
        emit_cli("peripherals-dump.json", &jiadian_monitor_lib::dump_peripherals_json());
        return;
    }
    if args.iter().any(|a| a == "--peripherals-probe") {
        emit_cli("peripherals-probe.txt", &jiadian_monitor_lib::dump_peripherals_probe());
        return;
    }
    // 设备信息诊断：拿一次 JED / PONG / STA 原文，用来区分「设备回包问题」和「界面问题」。
    if args.iter().any(|a| a == "--device-info") {
        emit_cli("device-info.json", &jiadian_monitor_lib::dump_device_info());
        return;
    }
    if args.iter().any(|a| a == "--peripherals-selftest") {
        let text = match jiadian_monitor_lib::peripherals_selftest() {
            Ok(m) => format!("OK: {m}"),
            Err(e) => format!("ERR: {e}"),
        };
        emit_cli("peripherals-selftest.txt", &text);
        return;
    }
    // 只下载 + 校验（不装、不弹 UAC）：验证换源重试与进度上报。
    if args.iter().any(|a| a == "--lhws-fetch") {
        let t = std::thread::spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                println!("{}", jiadian_monitor_lib::lhws_progress_json());
            }
        });
        let r = jiadian_monitor_lib::lhws_fetch_only();
        // 进程直接退出，不用管那个打印线程
        std::mem::forget(t);
        match r {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--lhws-install") {
        match jiadian_monitor_lib::lhws_install_now() {
            Ok(m) => println!("OK://n{m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--shot") {
        jiadian_monitor_lib::start_api();
        let html = args.get(i + 1).cloned().unwrap_or_default();
        let out = args
            .get(i + 2)
            .cloned()
            .unwrap_or_else(|| "shot.png".to_string());
        match jiadian_monitor_lib::capture_shot(html, out) {
            Ok(m) => println!("OK:\n{m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--render") {
        jiadian_monitor_lib::start_api();
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
        match jiadian_monitor_lib::render_headless(html, secs) {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--usb-test") {
        match jiadian_monitor_lib::usb_test(None, 320, 240) {
            Ok(m) => println!("OK: {m}"),
            Err(e) => println!("ERR: {e}"),
        }
        return;
    }
    // 单实例：已有实例在跑就唤起它的窗口并退出（两个进程会抢 USB 与端口）
    // 放在 CLI 分支之后：`--dump-*` / `--shot` / `--render` 允许在程序运行时执行
    if !jiadian_monitor_lib::single::acquire() {
        jiadian_monitor_lib::init_logging();
        let woke = jiadian_monitor_lib::single::wake_existing();
        jiadian_monitor_lib::log_info(if woke {
            "检测到已有实例：已唤起它的窗口，本次启动退出"
        } else {
            "检测到已有实例：唤起失败（端口未就绪），本次启动退出"
        });
        if !woke {
            jiadian_monitor_lib::single::notify_already_running();
        }
        return;
    }
    jiadian_monitor_lib::run()
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

use std::sync::atomic::{AtomicIsize, Ordering};

/// 直接拿到的控制台句柄（`CONOUT$`）。0 = 没拿到。
///
/// 有它就不依赖 `SetStdHandle` + Rust 的 `println!`（std 可能缓存旧句柄），
/// 直接用 `WriteConsoleW` 写 UTF-16，中日文和 emoji 都不会受代码页影响。
static CONSOLE_OUT: AtomicIsize = AtomicIsize::new(0);

fn console_handle() -> Option<isize> {
    let v = CONSOLE_OUT.load(Ordering::Relaxed);
    if v != 0 {
        Some(v)
    } else {
        None
    }
}

/// 往控制台写一段文本（自动切成 UTF-16）。失败返回 false。
fn write_console(h: isize, text: &str) -> bool {
    use windows_sys::Win32::System::Console::WriteConsoleW;
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut written = 0u32;
    unsafe {
        WriteConsoleW(
            h as *mut std::ffi::c_void,
            wide.as_ptr(),
            wide.len() as u32,
            &mut written,
            std::ptr::null(),
        ) != 0
    }
}

/// stdout 是否已经被调用方重定向成管道 / 文件。
fn stdout_is_redirected() -> bool {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_DISK, FILE_TYPE_PIPE};
    use windows_sys::Win32::System::Console::GetStdHandle;
    unsafe {
        let h: HANDLE = GetStdHandle(windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return false;
        }
        let t = GetFileType(h);
        t == FILE_TYPE_PIPE || t == FILE_TYPE_DISK
    }
}

/// 打印命令行结果，并在「控制台写不进去」时给一份文件兜底。
///
/// 为什么需要兜底：release 是 GUI 子系统，被 cmd 直接拉起时标准句柄可能是空的，
/// `println!` 出错也不会报（std 忽略写失败），用户只看到提示符回来、屏幕上什么都没有。
/// 所以只要 stdout 不是管道/文件，就把同一份内容再写一份到 exe 同级 `log/<name>`。
fn emit_cli(name: &str, text: &str) {
    // 先判断 stdout 有没有被重定向：`> hid.txt` 时 stdout 是文件、stderr 还是控制台，
    // 这时必须走 println!（写到那个文件），不能因为拿到了 CONOUT$ 句柄就往屏幕上写，
    // 否则重定向出来的文件是空的。
    let redirected = stdout_is_redirected();
    if !redirected {
        if let Some(h) = console_handle() {
            let _ = write_console(h, &format!("{text}\r\n"));
        } else {
            println!("{text}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    } else {
        println!("{text}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }

    if redirected {
        return;
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("log")))
        .unwrap_or_else(|| std::path::PathBuf::from("log"));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let p = dir.join(name);
    if std::fs::write(&p, text).is_ok() {
        let line = format!("（同样内容已写入 {}）\r\n", p.display());
        match console_handle() {
            Some(h) => {
                let _ = write_console(h, &line);
            }
            None => println!("{line}"),
        }
    }
}

/// 让命令行参数模式的输出真的落到调用方控制台里。
///
/// release 构建是 **GUI 子系统**（见文件头 `windows_subsystem = "windows"`），从 cmd /
/// PowerShell 里敲命令时拿到的是「父控制台的句柄、但本进程没挂到那个控制台上」，
/// `println!` 每次 WriteFile 都静默失败 —— 用户看到的就是「敲回车、提示符立刻回来、
/// 一个字都没有」，而 `> out.txt` 或 `| more`（句柄换成文件/管道）反倒正常。
/// 实测：`--peripherals-hid` 重定向到文件 1176 字节，直接在控制台跑 0 字节。
///
/// 处理办法：句柄是管道/磁盘文件（调用方自己重定向了）就原样保留；否则挂到父进程控制台，
/// 再把 stdout / stderr 指向 `CONOUT$`。双击启动（没有父控制台）时该调用直接失败，
/// 不影响 GUI 路径。
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::{
        GetLastError, HANDLE, INVALID_HANDLE_VALUE, GENERIC_READ, GENERIC_WRITE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileType, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK, FILE_TYPE_PIPE,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    fn ok(h: HANDLE) -> bool {
        !h.is_null() && h != INVALID_HANDLE_VALUE
    }
    fn redirected(h: HANDLE) -> bool {
        if !ok(h) {
            return false;
        }
        let t = unsafe { GetFileType(h) };
        t == FILE_TYPE_PIPE || t == FILE_TYPE_DISK
    }

    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let err = GetStdHandle(STD_ERROR_HANDLE);
        // 两个都已经被调用方重定向 → 说明输出有人接，别动它，也别抢父控制台。
        if redirected(out) && redirected(err) {
            return;
        }
        // 已经挂在控制台上时返回 0 + ERROR_ACCESS_DENIED；没有父控制台（双击启动）返回别的值。
        // 两种情况都不影响下面的 CONOUT$ 尝试。
        let attached = AttachConsole(ATTACH_PARENT_PROCESS);
        cli_debug(&format!("AttachConsole={attached} err={}", GetLastError()));
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let con = CreateFileW(
            name.as_ptr(),
            // 必须是 GENERIC_READ|GENERIC_WRITE。用 FILE_GENERIC_WRITE 会在控制台设备上
            // 直接返回 INVALID_HANDLE_VALUE + ERROR_INVALID_HANDLE(6)：它里面带了
            // FILE_WRITE_ATTRIBUTES / FILE_WRITE_EA，控制台不支持这两项。
            // 之前「cmd 里敲命令没输出」就是栽在这里（实测 CONOUT$ 打开失败 err=6）。
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if !ok(con) {
            cli_debug(&format!("CONOUT$ 打开失败 err={}", GetLastError()));
            return;
        }
        CONSOLE_OUT.store(con as isize, Ordering::Relaxed);
        if !redirected(out) {
            SetStdHandle(STD_OUTPUT_HANDLE, con);
        }
        if !redirected(err) {
            SetStdHandle(STD_ERROR_HANDLE, con);
        }
        cli_debug(&format!(
            "ok out={out:p} err={err:p} attach_ok={attached} console={con:p}"
        ));
    }
}

/// 设 `JIADIAN_CLI_DEBUG=1` 时，把控制台探测结果写到 exe 同级 `log/cli-console.txt`。
/// 排查「命令行没输出」这类只在别人机器上复现的问题用，平时零开销。
fn cli_debug(msg: &str) {
    if std::env::var_os("JIADIAN_CLI_DEBUG").is_none() {
        return;
    }
    use std::io::Write;
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("log")))
        .unwrap_or_else(|| std::path::PathBuf::from("log"));
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("cli-console.txt"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

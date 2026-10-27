//! 实时流管线：离屏浏览器抓帧 → RGB565 → 差分 → LIVE 帧 → USB 下发，并把每帧推给前端预览。
//!
//! 首帧 mode 0 立底座，之后只推脏矩形（mode 1 / mode 2 ≤6 区）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager};

use crate::{diff, handoff, live_frame, render, winusb};

static CANCEL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
/// 管线是否在跑（退出时用来等它收尾，好把租约还回去）。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 管线线程的收尾守卫：无论从哪条路径退出（正常运行 / `?` 提前返回 / 线程内 panic 展开），
/// 都要把 `RUNNING` 放下，避免 `stop_blocking` 白等 4s、下一轮启动误判「上一轮卡住」。
struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
        // 兜底：管线退出后也算「就绪」，免得前端在接管失败/提前退出时白等
        LINK_READY.store(true, Ordering::SeqCst);
    }
}

/// 前端预览通道（前端通过 `preview_subscribe` 交进来）。
///
/// **不要改回 `app.emit`**：`emit` 的实现是把整个 payload 内联进一段 JS 源码再 `eval`
/// 到渲染进程 —— 每帧都在 V8 里编译一个新脚本（预览帧 ~50KB base64），30fps 下渲染
/// 进程工作集 ~6MB/s 持续上涨（实测）。`Channel` 对 ≥1KB 的负载走 IPC 队列 + 固定小
/// 脚本取回，且**传输的是原始字节**，既不再逐帧编译、也省掉 base64。
static PREVIEW_CH: Mutex<Option<Channel<InvokeResponseBody>>> = Mutex::new(None);

/// 前端订阅预览（一次即可；重新连接会覆盖旧的）。
pub fn set_preview_channel(ch: Channel<InvokeResponseBody>) {
    *PREVIEW_CH.lock().unwrap() = Some(ch);
    crate::logging::info("预览通道已建立（Channel，二进制直传）");
}

/// 把一帧 PNG 原始字节推给前端预览；没订阅 / 已断开就丢掉（不排队、不累积）。
pub fn preview_send(png: Vec<u8>) {
    let ch = PREVIEW_CH.lock().unwrap().clone();
    if let Some(ch) = ch {
        let _ = ch.send(InvokeResponseBody::Raw(png));
    }
}
/// 下一帧是否强制整屏重发（旋转/息屏后设备会回默认背景，脏区差分补不回来）。
static FORCE_FULL: AtomicBool = AtomicBool::new(false);
/// 暂停下发（调显示设置时用）：置位后管线跳过发送，`resume()` 后自动继续。
static PAUSED: AtomicBool = AtomicBool::new(false);
pub fn pause() {
    PAUSED.store(true, Ordering::SeqCst);
}
pub fn resume() {
    PAUSED.store(false, Ordering::SeqCst);
}

/// 让下一帧整屏重发（mode 0）。
pub fn force_full_frame() {
    FORCE_FULL.store(true, Ordering::SeqCst);
}

/// 目标帧率两档：**高刷** ~30fps（面板有动画时贴上限，再快对 320×240 没意义、白烧 USB
/// 带宽和 CPU），**普通** ~10fps（省 CPU / 内存 / 带宽）。指标本身每秒才刷新一次，
/// 普通档看数值完全够用；变化小的帧天然发得少。
const INTERVAL_HIGH: Duration = Duration::from_millis(33);
const INTERVAL_NORMAL: Duration = Duration::from_millis(100);

/// 高刷模式：开 = ~30fps，关 = 普通 ~10fps（**默认关**，省资源）。
///
/// 热路径（每帧）读它，所以用原子量。**不落文件**：前端在启动时（`boot()` 之前）把它推过来，
/// 之后改设置也即时推 —— 与主题 / 单位等设置同一套路（后端不需要在启动瞬间知道它）。
static HIGH_FPS: AtomicBool = AtomicBool::new(false);

/// 当前是否高刷模式。
pub fn high_fps() -> bool {
    HIGH_FPS.load(Ordering::SeqCst)
}

/// 设置高刷档位（**立即生效**——下一帧就按新节奏）。
pub fn set_high_fps(on: bool) {
    HIGH_FPS.store(on, Ordering::SeqCst);
}

/// 当前帧间隔（高刷 33ms / 普通 100ms）。
pub fn frame_interval() -> Duration {
    if high_fps() {
        INTERVAL_HIGH
    } else {
        INTERVAL_NORMAL
    }
}

/// 静止退避：连续这么多帧画面没动 → 抓屏降到 [`INTERVAL_IDLE`]（省 CPU）；
/// 一旦检出变化**立刻**回到目标档位。指标本身每秒才刷新一次，静止时 10fps 也是浪费。
const IDLE_AFTER: u32 = 3;
/// 静止时的抓屏间隔（~3fps）。不宜再拉长：最坏情况是「值刚变」要多等这么久才被抓到。
const INTERVAL_IDLE: Duration = Duration::from_millis(300);

/// 单次等 CDP 响应的上限。`wait_result` 只按 id 匹配、原本没有超时：
/// 页面导航、渲染进程被回收、WebView2 卡死时，这一等就是永远 ——
/// 管线不再抓帧、也不再查 `cancel`，只能杀进程。5s 足够覆盖一次正常截图（~25ms）。
const CDP_RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// 停止当前管线（若有）。
pub fn stop() {
    if let Some(c) = CANCEL.lock().unwrap().take() {
        c.store(true, Ordering::SeqCst);
    }
}

/// 停止管线并等设备归还（同步；托盘菜单等不能 await 的地方用）。返回是否已归还。
pub fn stop_blocking(timeout_ms: u64) -> bool {
    let was_running = RUNNING.load(Ordering::SeqCst);
    stop();
    let t0 = std::time::Instant::now();
    while RUNNING.load(Ordering::SeqCst) && t0.elapsed().as_millis() < timeout_ms as u128 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let stopped = !RUNNING.load(Ordering::SeqCst);
    if stopped {
        close_link();
    }
    crate::logging::info(match (was_running, stopped) {
        (false, _) => "停止：管线未在运行",
        (true, true) => "停止：已归还设备",
        (true, false) => "停止：超时未收尾（退出流程会再兜底）",
    });
    stopped
}

/// 异步版（命令用）。
pub async fn stop_and_wait(timeout_ms: u64) -> bool {
    tauri::async_runtime::spawn_blocking(move || stop_blocking(timeout_ms))
        .await
        .unwrap_or(false)
}

/// **取消接管**：暂停下发 → 发 LIVE 退出帧（设备回到它自己默认播放的素材）→ 停管线 → 归还租约。
/// 「停止」按钮与退出流程都走这里；返回 true = 租约已归还。
pub fn release(timeout_ms: u64) -> bool {
    // 已经释放过了（退出流程会多次调用：托盘/API + ExitRequested + Exit）
    if !link_open() && !RUNNING.load(Ordering::SeqCst) {
        return true;
    }
    pause();
    // 等在途那一帧发完（帧间隔 33ms），否则退出帧后面还会跟一帧画面
    std::thread::sleep(Duration::from_millis(80));
    match with_link(|l| {
        let f = live_frame::frame_exit();
        l.dev()?.write(&f)?;
        let ack = l.dev()?.read(256)?;
        Ok(live_frame::parse_ack(ack.as_deref()).0.to_string())
    }) {
        Ok(a) => crate::logging::info(&format!("已发 LIVE 退出帧（{a}），设备回默认素材")),
        Err(e) => crate::logging::warn(&format!("LIVE 退出帧未发出：{e}")),
    }
    let ok = stop_blocking(timeout_ms);
    resume();
    ok
}

/// 退出时调用：发退出帧 + 归还设备（`UsbLink` 的 `Drop` 释放移交租约），最后关掉离屏面板窗口。
pub fn shutdown(app: &tauri::AppHandle) {
    release(2000);
    if let Some(w) = app.get_webview_window("panel") {
        let _ = w.close();
        crate::logging::info("退出：关闭离屏面板窗口");
    }
    // 兜底：任何情况下退出都要把租约还回去
    close_link();
}

/// 启动管线：用 **app 自带的 WebView2**（一个 320×240 的离屏窗口）渲染面板。
/// 不再另起外部 Edge —— 省掉冷启动，也少一个浏览器进程。
pub fn start(
    app: AppHandle,
    html: String,
    serial: Option<String>,
    w: u32,
    h: u32,
) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    crate::logging::info(&format!("render_start 请求：{html}"));
    resume(); // 清掉上一轮可能残留的暂停
    stop();
    // 等上一轮真正收尾再启动（两个渲染任务不能抢同一个离屏窗口）
    let mut waited = false;
    for _ in 0..80 {
        if !RUNNING.load(Ordering::SeqCst) {
            break;
        }
        waited = true;
        std::thread::sleep(Duration::from_millis(50));
    }
    if RUNNING.load(Ordering::SeqCst) {
        // 上一轮卡住没收尾：**强制释放它的链路**，让它的下发立刻报错退出，
        // 否则它会一直占着 `LINK`（新一轮只能复用同一句柄），收尾时还会把它关掉 → 新链路被误关。
        crate::logging::warn("上一轮管线 4s 内未停止，强制释放其链路后继续启动");
        close_link();
    } else if waited {
        crate::logging::info(&format!("已等上一轮收尾 {}ms", t0.elapsed().as_millis()));
    }

    let url = panel_url(&html);
    let needle = panel_file_name(&html);
    ensure_panel_webview(&app, &url, w, h)?;
    let ws = render::page_ws_url_matching(render::PANEL_CDP_PORT, &needle)?;
    crate::logging::info(&format!("面板 CDP 就绪 +{}ms", t0.elapsed().as_millis()));

    let cancel = Arc::new(AtomicBool::new(false));
    *CANCEL.lock().unwrap() = Some(cancel.clone());
    RUNNING.store(true, Ordering::SeqCst);
    LINK_READY.store(false, Ordering::SeqCst); // 本轮 USB 还没接管，前端先别抢

    // USB 接管放在管线线程**之外**同步做：
    // `UsbLink::open` 内部是同步 USB / HTTP（含移交重试，最坏受 `OPEN_BUDGET` 约束）。
    // 放在渲染线程里会让 `render_start` 已经返回、USB 却还没接管；放在 Tauri async
    // 任务里则会占住 tokio worker。这里由 `render_start` 的阻塞池调用，返回前就能
    // 确定是「已接管」还是「仅预览」。
    let link_gen = match UsbLink::open(serial, w, h) {
        Ok(g) => {
            crate::logging::info(&format!("USB 接管完成 +{}ms", t0.elapsed().as_millis()));
            g
        }
        Err(e) => {
            crate::logging::warn(&format!("USB 不可用，仅预览：{e}"));
            None
        }
    };
    // 接管尝试已结束（无论成败）：前端可以安全读信息了（成功则复用句柄、失败则自己开）
    LINK_READY.store(true, Ordering::SeqCst);
    // 线程创建失败时闭包会被丢弃，链路不会有人收尾 —— 先留一份代数用于兜底。
    let link_gen_for_error = link_gen;

    // 管线跑在**独立线程 + current_thread 运行时**里，不再占用 Tauri 的 async worker。
    // `with_link` 里的同步 USB 读写（含最长 3s 的管道超时）只阻塞这一条线程；
    // 界面、命令、API 轮询都不会被它拖住。CDP 断开后的重连也在这里自愈。
    let spawn = std::thread::Builder::new()
        .name("render-pipeline".into())
        .spawn(move || {
            let _running = RunningGuard;
            // 预览限频到目标帧率：不影响 USB 下发，只砍掉超过帧率的 IPC / 前端解码开销
            let app_preview = app.clone();
            let mut last: Option<std::time::Instant> = None;
            let cb = move |png: &[u8], _fw: u32, _fh: u32, _frames: u64| {
                // 主窗口收进托盘 / 最小化时看不到预览 → 直接不发，省掉这一整趟编码、传输与解码；
                // 窗口恢复后下一帧自动续上，无需任何手工操作。
                if let Some(win) = app_preview.get_webview_window("main") {
                    let hidden =
                        !win.is_visible().unwrap_or(true) || win.is_minimized().unwrap_or(false);
                    if hidden {
                        return;
                    }
                }
                let now = std::time::Instant::now();
                if let Some(t) = last {
                    if now.duration_since(t) < frame_interval() {
                        return;
                    }
                }
                last = Some(now);
                // 走 Channel 推原始 PNG 字节（不是 base64、不是事件）
                preview_send(png.to_vec());
            };
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    crate::logging::error(&format!("创建管线运行时失败：{e}"));
                    // 线程已拿到接管权，但还没进监督循环：这里自己把链路还掉，避免泄漏。
                    if let Some(g) = link_gen {
                        close_link_if_gen(g);
                    }
                    return;
                }
            };
            if let Err(e) = rt.block_on(run_supervised(
                cb,
                ws,
                url,
                w,
                h,
                cancel,
                None,
                Some(app),
                render::PANEL_CDP_PORT,
                link_gen,
            )) {
                crate::logging::error(&format!("render 出错：{e}"));
            }
        });
    if let Err(e) = spawn {
        RUNNING.store(false, Ordering::SeqCst);
        LINK_READY.store(true, Ordering::SeqCst);
        if let Some(g) = link_gen_for_error {
            close_link_if_gen(g);
        }
        return Err(format!("创建渲染线程失败：{e}"));
    }
    Ok("started".into())
}

/// 面板文件名（在 CDP 目标里区分面板页 vs UI 页）。
fn panel_file_name(html: &str) -> String {
    html.rsplit(['/', '\\']).next().unwrap_or(html).to_string()
}

/// WebView2 启动参数（必须与 `tauri.conf.json` 里主窗口的
/// `additionalBrowserArgs` **完全一致**）：同一个 user-data-dir 下，
/// 主窗口和面板窗口的参数不一致时 WebView2 会拒绝复用环境。
///
/// 为什么不能只设 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`：wry 0.57 总会调用
/// `SetAdditionalBrowserArguments`，WebView2 在设置了该项后会忽略同名环境变量。
fn webview2_args() -> String {
    format!(
        "--remote-debugging-port={} \
         --disable-features=CalculateNativeWinOcclusion,msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --autoplay-policy=no-user-gesture-required",
        render::PANEL_CDP_PORT
    )
}

/// 复用 / 创建离屏面板窗口（320×240，放到屏幕外 —— 隐藏窗口 Chromium 会停渲染）。
fn ensure_panel_webview(app: &AppHandle, url: &str, w: u32, h: u32) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let parsed: tauri::Url = url.parse().map_err(|e| format!("URL 解析失败：{e}"))?;
    if let Some(win) = app.get_webview_window("panel") {
        let _ = win.hide();
        // 复用窗口时把尺寸对齐到本次设备尺寸（换 170/240 设备时窗口建时尺寸可能不同）。
        // 截图真正依据的是 `Emulation.setDeviceMetricsOverride`，这里只是让窗口与视口一致、少一层隐患。
        let _ = win.set_size(tauri::LogicalSize::new(w as f64, h as f64));
        win.navigate(parsed)
            .map_err(|e| format!("面板导航失败：{e}"))?;
        crate::logging::info("复用离屏面板窗口（已导航，已隐藏，已对齐尺寸）");
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "panel", WebviewUrl::External(parsed))
        .title("jiaidian-panel")
        .inner_size(w as f64, h as f64)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .visible(false) // 真·无头：不显示窗口（隐藏后 captureScreenshot 仍可用，实测过）
        .additional_browser_args(&webview2_args())
        .build()
        .map_err(|e| format!("创建离屏面板窗口失败：{e}"))?;
    crate::logging::info("已创建离屏面板窗口（隐藏）");
    Ok(())
}

/// 注入到页面里的「自取本地指标」脚本：等 `__setSlot` 就绪后，每秒拉一次 `/metrics` 喂进去。
/// 面板只要定义了 `__setSlot` + 标准 `data-slot`（如 `cpu_load`），就自动拿到真值，
/// **不需要面板自己写 fetch**。闸门直接用 `file://` 打开（不注入）→ 结果确定。
///
/// **两条数据通道**：`/metrics`（本机硬件指标）+ `/peripherals`（外设电量），
/// 都经同一段脚本注入到同一个 `__setSlot`，面板侧无感。外设槽名见
/// `app/MONITOR_METRICS.md` 的「外设」一节（`mouse1_pct` / `kbd1_pct` …）。
///
/// **单位两档**（脚本每秒同时拉 `GET /units`，所以改设置立刻生效、不用重开面板）：
/// - **全自动（auto）**：素材元素上写 `data-unit="MB/s"` 表示"这个槽我要 MB/s"，
///   脚本把 `/metrics` 的规范值换算成该单位再喂进去（不在换算表里的单位原样透传）。
/// - **手动指定（manual）**：按设置里选的单位，把该族**所有**槽统一换算过去（覆盖素材声明），
///   同时把新单位写回元素的 `data-suffix`（否则会出现「16384GB」这种数值与单位打架）。
///
/// 换算表按**单位族**组织（容量 / 速率 / 温度 / 时长；功耗固定 W、不换算），跨族不换算
/// （给容量声明速率 → 原样）。数值按目标单位取「合适的位数」（时长给 1~2 位、字节给 2 位、B 给整数），
/// 素材自己的 `data-fmt` 仍可再决定显示小数位。
fn pull_js() -> String {
    const JS: &str = r#"(function(){
var MURL='__BASE__/metrics';
var UURL='__BASE__/units';
var PURL='__BASE__/peripherals';
// 单位族：同一族内可换算（×倍数）。规范单位：容量 GB / 显存 MB、速率 KB/s、温度 ℃、功耗 W、时长 s（秒）。
var BYTES={B:1,KB:1024,MB:1048576,GB:1073741824,TB:1099511627776};
var RATES={'B/s':1,'KB/s':1024,'MB/s':1048576,'GB/s':1073741824,'KBps':1024,'MBps':1048576};
var DURS={'秒':1,'s':1,'分':60,'min':60,'小时':3600,'h':3600,'天':86400,'d':86400};
// 目标单位 → 小数位（"输出合适数字"：大单位给小数、小单位给整数）
var DEC={B:0,KB:1,MB:2,GB:2,TB:2,'B/s':0,'KB/s':1,'MB/s':2,'GB/s':2,'℃':1,'℉':1,'%':1,'RPM':0,'秒':0,'s':0,'分':1,'min':1,'小时':1,'h':1,'天':2,'d':2};
var U={};                        // 素材声明：槽 → data-unit
var CFG={mode:'auto',units:{}};  // 上位机设置：全自动 / 手动指定
var LAST={};                     // 上次给某槽应用过的单位（避免每秒重复写 DOM）
function scan(){
  U={};
  var els=document.querySelectorAll('[data-unit]');
  for(var i=0;i<els.length;i++){
    var s=els[i].getAttribute('data-slot'),u=els[i].getAttribute('data-unit');
    if(s&&u&&!U[s])U[s]=u;
  }
}
function fam(u){
  u=String(u||'');
  if(BYTES[u])return 'capacity';
  if(RATES[u])return 'rate';
  if(u==='℃'||u==='℉')return 'temperature';
  if(DURS[u])return 'duration';
  return '';
}
function rnd(v,t){var d=DEC[t];d=(d===undefined)?2:d;var p=Math.pow(10,d);return Math.round(v*p)/p;}
function conv(v,f,t){
  f=String(f||'');t=String(t||'');
  if(!f||!t||f===t)return v;
  if(v===null||v===undefined||isNaN(v))return v;
  if(BYTES[f]&&BYTES[t])return rnd(v*BYTES[f]/BYTES[t],t);
  if(RATES[f]&&RATES[t])return rnd(v*RATES[f]/RATES[t],t);
  if(f==='℃'&&t==='℉')return rnd(v*9/5+32,t);
  if(f==='℉'&&t==='℃')return rnd((v-32)*5/9,t);
  if(DURS[f]&&DURS[t])return rnd(v*DURS[f]/DURS[t],t);   // 时长规范 = 秒
  return v;
}
// 把单位写回元素的 data-suffix；记下原始后缀，切回全自动时还原。
// 间距沿用素材的写法：素材原来带空格就带空格；没写过后缀时，字母开头的单位（GB / KB/s / RPM…）
// 留一个空格，`℃ ℉ % 秒 分 小时 天` 紧贴数值。
function unitSuffix(el,unit){
  var orig=el.__origSfx;
  if(orig!==null&&orig!==undefined&&orig!==''&&!/^\s/.test(orig))return unit;
  return /^[A-Za-z]/.test(unit)?(' '+unit):unit;
}
function suffix(slot,unit){
  var els=document.querySelectorAll('[data-slot="'+slot+'"]');
  for(var i=0;i<els.length;i++){
    var el=els[i];
    if(!el.__sfxInit){el.__sfxInit=true;el.__origSfx=el.getAttribute('data-suffix');}
    var want=unit?unitSuffix(el,unit):el.__origSfx, cur=el.getAttribute('data-suffix');
    if(want===null){ if(cur!==null)el.removeAttribute('data-suffix'); }
    else if(cur!==want){ el.setAttribute('data-suffix',want); }
  }
}
var CPU_LOADS=['cpu_load','cpu_load_p','cpu_load_e'];
function clearCpuLoad(key){
  // 仍走素材 setter：已抽查的素材透传非数字文本；只接受数字的旧 setter 可能无法显示「—」。
  // 不猜测其内部解析规则，也不绕过 setter 写 innerHTML，以免破坏条形/圆环结构。
  try{window.__setSlot(key,'—');}catch(e){}
}
function clearCpuLoads(){
  for(var i=0;i<CPU_LOADS.length;i++)clearCpuLoad(CPU_LOADS[i]);
}
function apply(list,cfg){
  if(cfg&&cfg.mode)CFG=cfg;
  var available={};
  for(var i=0;i<list.length;i++){
    var m=list[i];
    if(CPU_LOADS.indexOf(m.key)!==-1&&typeof m.value==='number'&&isFinite(m.value))available[m.key]=true;
  }
  // 缺项、null、非有限值均清理；不存在的 P/E 不作为数值 0 喂给素材。
  for(var i=0;i<CPU_LOADS.length;i++){
    if(!available[CPU_LOADS[i]])clearCpuLoad(CPU_LOADS[i]);
  }
  for(var i=0;i<list.length;i++){
    var m=list[i];
    if(CPU_LOADS.indexOf(m.key)!==-1&&!available[m.key])continue;
    if(m.kind==='text'){
      if(m.text!==null&&m.text!==undefined)window.__setSlot(m.key,String(m.text));
      continue;
    }
    if(m.value===null||m.value===undefined)continue;
    var f=fam(m.unit),tgt=null,manual=false;
    if(CFG.mode==='manual'&&f&&CFG.units&&CFG.units[f]){tgt=CFG.units[f];manual=true;}
    else if(U[m.key]){tgt=U[m.key];}
    var want=manual?tgt:null;
    if(!(m.key in LAST)||LAST[m.key]!==want){LAST[m.key]=want;suffix(m.key,want);}
    window.__setSlot(m.key,String(tgt?conv(m.value,m.unit,tgt):m.value));
  }
}
// ── 外设电量（鼠标 / 键盘 / 耳机…）────────────────────────────────────────
// 数据来自 `GET /peripherals`（另一条通道，不是 /metrics），同样由上位机注入，
// 面板照样不用写 fetch。槽名规则与硬件指标一致：类别 + 编号（从 1 起）+ 指标。
//   mouse1_pct / mouse1_name / mouse1_status     键盘 kbd1_*；耳机 headset1_*；
//   手柄 gamepad1_*；其余 peri1_*；另有 peri_count（当前认到几台）。
// 编号在同一类别内按快照顺序（上位机已按品牌 + 型号排序），设备插拔会让编号前移。
// 读不到电量一律喂「—」，**绝不喂 0** —— 0% 是「没电」，未知是「不知道」，不能混。
var PCAT={Mouse:'mouse',Keyboard:'kbd',Headset:'headset',Earbuds:'headset',
          Gamepad:'gamepad',Dongle:'dongle',Generic:'peri'};
var PFED={};   // 上一轮喂过的外设槽 → 设备消失时清掉，避免残留旧电量
function pset(k,v){PFED[k]=1;try{window.__setSlot(k,String(v));}catch(e){}}
function pstatus(d){
  if(!d.supported)return '未支持';
  if(d.error&&!d.online)return '未连接';
  if(d.charging===true)return '充电中';
  if(d.charging===false)return '放电中';
  if(!d.online)return '离线';
  return '—';
}
function applyPeri(snap){
  // 请求失败（null）时保持上一轮读数，不要把整屏外设槽刷成「—」——
  // 外设是 3 秒一轮的慢通道，偶发一次超时不该让画面闪空。
  if(!snap||typeof snap!=='object')return;
  var devs=Array.isArray(snap.devices)?snap.devices:[];
  var seen={},n={};
  for(var i=0;i<devs.length;i++){
    var d=devs[i],p=PCAT[d.category]||'peri';
    n[p]=(n[p]||0)+1;
    var b=p+n[p];
    var has=(d.supported&&d.battery!==null&&d.battery!==undefined);
    pset(b+'_pct',has?d.battery:'—');
    pset(b+'_name',d.model||d.brand||'—');
    pset(b+'_status',pstatus(d));
    // PFED 里记的是完整槽名（`kbd1_pct` 这种），清理时也按完整槽名比对；
    // 只记基名 `kbd1` 会让下面那圈把所有槽都判成「本轮没出现」而当场清空。
    seen[b+'_pct']=1; seen[b+'_name']=1; seen[b+'_status']=1;
  }
  pset('peri_count',devs.length);
  seen['peri_count']=1;
  for(var k in PFED){
    if(!seen[k]){ try{window.__setSlot(k,'—');}catch(e){} delete PFED[k]; }
  }
}
function request(url){
  var controller=new AbortController();
  var timeout=setTimeout(function(){controller.abort();},4000);
  return fetch(url,{cache:'no-store',signal:controller.signal}).then(function(r){
    if(!r.ok)throw new Error('HTTP '+r.status);
    return r.json();
  }).catch(function(){return null;}).finally(function(){clearTimeout(timeout);});
}
var t=setInterval(function(){
  if(typeof window.__setSlot!=='function')return;
  clearInterval(t);
  clearCpuLoads();
  (function p(){
    scan();
    Promise.all([request(MURL),request(UURL),request(PURL)]).then(function(res){
      if(Array.isArray(res[0]))apply(res[0],res[1]);
      else clearCpuLoads();
      // 外设请求失败（null）时只清外设槽，不牵连硬件指标。
      applyPeri(res[2]);
    }).catch(function(){clearCpuLoads();}).finally(function(){
      // 故障请求有超时，且上一轮结束后才调度下一轮，避免堆积与旧响应覆盖。
      setTimeout(p,1000);
    });
  })();
},100);
})();"#;
    // 基址取「实际在听的回环地址」（IPv4 或 IPv6），不写死 127.0.0.1。
    JS.replace("__BASE__", &crate::api::base_url())
}

/// 等页面 `document.readyState === 'complete'`（最多 `ms` 毫秒）。
/// 比死等固定时长快得多 —— 这段直接决定面板切换延迟。
async fn wait_page_ready(cdp: &mut render::Cdp, ms: u64) -> bool {
    let t0 = std::time::Instant::now();
    loop {
        if let Ok(v) = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
            )
            .await
        {
            if v.get("result")
                .and_then(|r| r.get("value"))
                .and_then(|s| s.as_str())
                == Some("complete")
            {
                return true;
            }
        }
        if t0.elapsed().as_millis() as u64 >= ms {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 等取消标志置位 —— 给 `select!` 用，让 `stop()` 立刻生效（不用等下一次收帧）。
async fn wait_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 诊断：打开面板，取页面 URL 并截一张图存盘（看浏览器到底渲染了什么）。
pub fn capture_shot(html: &str, out_png: &str) -> Result<String, String> {
    let exe = render::find_browser().ok_or("找不到 Edge / Chrome")?;
    let url = panel_url(html);
    let profile = std::env::temp_dir()
        .join("jiaidian-shotprof")
        .to_string_lossy()
        .to_string();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut browser = render::Browser::launch(&exe, 320, 240, &url, 9335, &profile)?;
        let ws = browser.page_ws_url()?;
        let mut cdp = render::Cdp::connect(&ws).await?;
        cdp.cmd("Page.enable", json!({})).await?;
        // 注入自取脚本必须在导航之前，否则首帧拿不到真值
        cdp.cmd("Page.addScriptToEvaluateOnNewDocument", json!({ "source": pull_js() }))
            .await?;
        let _ = cdp.call("Page.navigate", json!({ "url": url })).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        cdp.cmd(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": 320, "height": 240, "deviceScaleFactor": 1, "mobile": false }),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let href = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": "document.location.href + ' | ' + document.title", "returnByValue": true }),
            )
            .await
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("value")).and_then(|s| s.as_str()).map(|s| s.to_string()))
            .unwrap_or_default();
        let shot = cdp
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let b64 = shot.get("data").and_then(|d| d.as_str()).unwrap_or("");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| e.to_string())?;
        std::fs::write(out_png, &bytes).map_err(|e| e.to_string())?;
        browser.kill();
        let _ = render::close_browser(9335).await;
        Ok(format!("url={url}\nhref={href}\nsaved={out_png} ({} bytes)", bytes.len()))
    })
}

/// 无界面运行（供 `--render` 命令行验证）。CLI 里没有 Tauri app 上下文，
/// 拿不到 WebView2，所以这条路径仍用外部 headless Edge。
pub fn run_headless(
    html: String,
    serial: Option<String>,
    w: u32,
    h: u32,
    seconds: f64,
) -> Result<String, String> {
    let exe = render::find_browser().ok_or("找不到 Edge / Chrome")?;
    let cancel = Arc::new(AtomicBool::new(false));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let first = std::env::temp_dir().join("jiaidian_first_frame.png");
    let mut n = 0u64;
    let cb = |png: &[u8], fw: u32, fh: u32, frames: u64| {
        println!("  frame {frames}  {fw}x{fh}");
        if n == 0 {
            let _ = std::fs::write(&first, png);
        }
        n += 1;
    };
    let (frames, sent) = rt.block_on(async move {
        let url = panel_url(&html);
        let profile = std::env::temp_dir()
            .join("jiaidian-cli-prof")
            .to_string_lossy()
            .to_string();
        let port = render::CLI_CDP_PORT;
        let mut browser = render::Browser::launch(&exe, w, h, &url, port, &profile)?;
        let ws = render::page_ws_url(port)?;
        // 和 GUI 路径一致：USB 先接管，再进管线；拿不到就只预览。
        let link_gen = match UsbLink::open(serial, w, h) {
            Ok(g) => g,
            Err(e) => {
                crate::logging::warn(&format!("USB 不可用，仅预览：{e}"));
                None
            }
        };
        let r = run_supervised(
            cb,
            ws,
            url,
            w,
            h,
            cancel,
            Some(seconds),
            None,
            port,
            link_gen,
        )
        .await;
        browser.kill();
        let _ = render::close_browser(port).await;
        r
    })?;
    Ok(format!("frames={frames} usb_sent={sent}"))
}

/// 一次下发的回执摘要（只用于统计与日志，不参与控制逻辑）。
struct Sent {
    /// `ok` / `timeout` / `exit` / `err` / `other`（见 `live_frame::parse_ack`）
    kind: &'static str,
    reason: Option<String>,
    /// 本帧实际写出的字节数（含帧头）
    bytes: usize,
    /// 写 bulk OUT 耗时
    write_ms: u64,
    /// 等 ACK 耗时（设备没回就是管道超时值）
    ack_ms: u64,
}

/// 下发观测统计（**只用于日志**，不参与任何控制逻辑）。
///
/// 为什么要有：现场出现过「跑一两个小时后黑屏、自动回默认素材，日志里一条错误都没有」。
/// 原因是旧代码把每帧的 ACK 直接丢掉（`let _ = read(256)?`），而"画面没动"的帧也会算一次
/// 成功下发，于是「失败了多少帧、ACK 是 ok 还是 err、设备还有没有在回话」全都没有痕迹。
/// 这里每 10s 打一条增量统计，让「设备什么时候开始不回话」在日志里直接可读。
#[derive(Default)]
struct TxStats {
    /// 本区间（自上次统计以来）的计数
    frames: u64,
    sent: u64,
    skip: u64,
    fail: u64,
    bytes: u64,
    write_ms: u64,
    ack_ms: u64,
    ack_ok: u64,
    ack_timeout: u64,
    ack_bad: u64,
    black: u64,
    last: Option<Instant>,
    last_fail_warn: Option<Instant>,
    last_ack_warn: Option<Instant>,
}

impl TxStats {
    /// 限流：同类告警最多 5s 一条（异常持续时不能让日志被刷爆，计数看 stats 行）。
    fn warn_throttled(slot: &mut Option<Instant>, now: Instant, msg: &str) {
        let due = match *slot {
            None => true,
            Some(t) => now.duration_since(t) >= Duration::from_secs(5),
        };
        if due {
            *slot = Some(now);
            crate::logging::warn(msg);
        }
    }

    fn log_if_due(&mut self, now: Instant) {
        let due = match self.last {
            None => true,
            Some(t) => now.duration_since(t) >= Duration::from_secs(10),
        };
        if !due {
            return;
        }
        let span = self
            .last
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        self.last = Some(now);
        if span <= 0.0 {
            return;
        }
        let avg = |total: u64, n: u64| {
            if n == 0 {
                0.0
            } else {
                total as f64 / n as f64
            }
        };
        crate::logging::info(&format!(
            "[stats] {:.0}s 帧={} 发={} 跳过={} 失败={} 帧率={:.1}fps 平均帧={:.0}B 写={:.1}ms ACK={:.1}ms ACK(ok={},超时={},异常={}) 黑帧={}",
            span,
            self.frames,
            self.sent,
            self.skip,
            self.fail,
            self.frames as f64 / span,
            avg(self.bytes, self.sent),
            avg(self.write_ms, self.sent),
            avg(self.ack_ms, self.sent),
            self.ack_ok,
            self.ack_timeout,
            self.ack_bad,
            self.black,
        ));
        // 区间计数清零（累计量在调用方的 frames/sent 里）
        self.frames = 0;
        self.sent = 0;
        self.skip = 0;
        self.fail = 0;
        self.bytes = 0;
        self.write_ms = 0;
        self.ack_ms = 0;
        self.ack_ok = 0;
        self.ack_timeout = 0;
        self.ack_bad = 0;
        self.black = 0;
    }
}

/// 整帧是否全黑（RGB565 大端，全 0）。抽样即可：真·黑帧是整片 0，误报代价只是一行日志。
fn looks_black(rgb565: &[u8]) -> bool {
    rgb565.iter().step_by(256).all(|&b| b == 0)
}

/// 建立一次 CDP 会话（连接、导航、等页面就绪、注入自取脚本、锁视口）。
/// 会话失败只影响这一轮，由 [`run_supervised`] 重连。
async fn setup_session(ws: &str, url: &str, w: u32, h: u32) -> Result<render::Cdp, String> {
    let mut cdp = render::Cdp::connect(ws).await?;
    cdp.cmd("Page.enable", json!({})).await?;
    // 先导航到面板并等页面就绪 —— 否则首帧会抓到别的页面
    let _ = cdp.call("Page.navigate", json!({ "url": url })).await;
    // 等「真的加载完」而不是死等固定时长：file:// 通常 <100ms，这段直接决定切换延迟
    tokio::time::sleep(Duration::from_millis(80)).await;
    let ready = wait_page_ready(&mut cdp, 3000).await;
    crate::logging::info(&format!("页面就绪 ready={ready}"));
    // 注入自取脚本：**导航之后**注入。导航会重建 JS 上下文，旧定时器自然消失，
    // 所以复用时不累积；脚本等 `__setSlot` 就绪后自动从本地 API 取真值。
    let _ = cdp.call("Runtime.evaluate", json!({ "expression": pull_js() })).await;
    // 视口锁死 W×H
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": w, "height": h, "deviceScaleFactor": 1, "mobile": false }),
    )
    .await?;
    // 等这次 resize 生效
    tokio::time::sleep(Duration::from_millis(150)).await;
    crate::logging::info("视口锁定");
    Ok(cdp)
}

/// 重连时找一个可用的面板 CDP 页面。先短探（页面通常还在，直接重连最快）；
/// 找不到再让离屏窗口重新导航；仍不行才关掉窗口重建（覆盖渲染进程被回收）。
fn acquire_panel_ws(
    app: Option<&AppHandle>,
    port: u16,
    needle: &str,
    url: &str,
    w: u32,
    h: u32,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("用户停止".into());
    }
    if let Ok(ws) = render::page_ws_url_matching_timeout(port, needle, Duration::from_secs(3)) {
        return Ok(ws);
    }
    let Some(app) = app else {
        return Err("找不到面板页面（CLI 模式无法重建窗口）".into());
    };
    if cancel.load(Ordering::SeqCst) {
        return Err("用户停止".into());
    }
    crate::logging::warn("面板页面丢失，尝试重新导航…");
    ensure_panel_webview(app, url, w, h)?;
    if let Ok(ws) = render::page_ws_url_matching_timeout(port, needle, Duration::from_secs(8)) {
        return Ok(ws);
    }
    if cancel.load(Ordering::SeqCst) {
        return Err("用户停止".into());
    }
    crate::logging::warn("面板页面仍未恢复，关闭并重建离屏窗口…");
    if let Some(win) = app.get_webview_window("panel") {
        let _ = win.close();
        std::thread::sleep(Duration::from_millis(200));
    }
    ensure_panel_webview(app, url, w, h)?;
    render::page_ws_url_matching_timeout(port, needle, Duration::from_secs(10))
}

fn reconnect_backoff(fails: u32) -> Duration {
    let shift = fails.min(4);
    Duration::from_millis(500u64.saturating_mul(1u64 << shift).min(5_000))
}

async fn wait_or_cancel(cancel: Arc<AtomicBool>, dur: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => false,
        _ = wait_cancel(cancel) => true,
    }
}

/// 管线监督循环：CDP 会话断开（渲染进程被回收、页面导航、WebView2 卡死）时自动重连，
/// **不重新移交 USB** —— 设备句柄由本函数持有，跨会话复用；只有用户停止 / 到达运行时长 / 设备
/// 被拔掉（`run_inner` 内部关链路、退回仅预览）才结束。
#[allow(clippy::too_many_arguments)]
async fn run_supervised<F: FnMut(&[u8], u32, u32, u64)>(
    mut on_frame: F,
    initial_ws: String,
    url: String,
    w: u32,
    h: u32,
    cancel: Arc<AtomicBool>,
    seconds: Option<f64>,
    app: Option<AppHandle>,
    cdp_port: u16,
    link_gen: Option<u64>,
) -> Result<(u64, u64), String> {
    let _link_guard = LinkGuard(link_gen);
    let needle = panel_file_name(&url);
    let started = Instant::now();
    let mut next_ws = Some(initial_ws);
    let mut frames = 0u64;
    let mut sent = 0u64;
    let mut fails = 0u32;
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let remaining = seconds.map(|s| s - started.elapsed().as_secs_f64());
        if let Some(r) = remaining {
            if r <= 0.0 {
                break;
            }
        }
        let ws = match next_ws.take() {
            Some(ws) => ws,
            None => match acquire_panel_ws(
                app.as_ref(),
                cdp_port,
                &needle,
                &url,
                w,
                h,
                cancel.as_ref(),
            ) {
                Ok(ws) => ws,
                Err(e) => {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    fails += 1;
                    if fails == 1 || fails % 5 == 0 {
                        crate::logging::warn(&format!(
                            "面板页面不可用（{e}），第 {fails} 次重试…"
                        ));
                    }
                    if wait_or_cancel(cancel.clone(), reconnect_backoff(fails)).await {
                        break;
                    }
                    continue;
                }
            },
        };
        match run_inner(&mut on_frame, ws, url.clone(), w, h, cancel.clone(), remaining).await {
            Ok((f, s)) => {
                frames += f;
                sent += s;
                break;
            }
            Err(e) => {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                fails += 1;
                if fails == 1 || fails % 5 == 0 {
                    crate::logging::warn(&format!("渲染会话中断（{e}），第 {fails} 次重连…"));
                }
                if wait_or_cancel(cancel.clone(), reconnect_backoff(fails)).await {
                    break;
                }
            }
        }
    }
    Ok((frames, sent))
}

#[allow(clippy::too_many_arguments)]
async fn run_inner<F: FnMut(&[u8], u32, u32, u64)>(
    mut on_frame: F,
    ws: String,
    url: String,
    w: u32,
    h: u32,
    cancel: Arc<AtomicBool>,
    seconds: Option<f64>,
) -> Result<(u64, u64), String> {
    let t0 = std::time::Instant::now();
    let mut cdp = setup_session(&ws, &url, w, h).await?;
    // 设备句柄由 `run_supervised` 统一持有：CDP 会话断开重连时**不重新移交 USB**。
    // 每轮重新读一次 `LINK_OPEN`：上一轮若因拔线被关掉，这里自然退回仅预览。
    let mut has_usb = link_open();
    if !has_usb {
        crate::logging::warn("USB 未接管，本轮仅预览");
    }

    // WebView2 **不支持** `Page.startScreencast`（实测收不到 screencastFrame），只能轮询截屏。
    // 抓屏节奏 = 下发节奏：把下一张截图提前 `SHOT_EST` 发出去，让它正好在「到点」前后回来。
    // 相比「一拿到就立刻发下一张」，这样抓屏次数 ≈ 下发次数，不再连续空转 PNG 编码（CPU 大头）。
    const SHOT_EST: Duration = Duration::from_millis(25); // 截图耗时估计（~25ms）
    let mut prev: Vec<u8> = Vec::new();
    let mut sent = 0u64;
    let mut frames = 0u64;
    let mut still = 0u32; // 连续「画面没动」的帧数（静止退避用）
    let mut st = TxStats::default();
    let mut black_warned = false;
    let mut next_send = std::time::Instant::now();
    let start = std::time::Instant::now();
    let mut req_id = cdp.cmd("Page.captureScreenshot", shot_params()).await?;

    // 退出原因带出循环：日志里必须能区分「用户停止 / CDP 断开 / 出错」。
    // 旧版只有一个 `管线退出 frames=...`，事后完全看不出是不是断联。
    let reason: &str = loop {
        if cancel.load(Ordering::SeqCst) {
            break "用户停止";
        }
        if let Some(secs) = seconds {
            if start.elapsed().as_secs_f64() > secs {
                break "到达运行时长";
            }
        }
        // 等这一张的响应（其间的事件丢掉）；select 让 stop() 立刻生效。
        // **必须套超时**：页面导航/渲染进程被回收时 CDP 可能永远不回，旧版会卡死在这里。
        let data = tokio::select! {
            r = tokio::time::timeout(CDP_RECV_TIMEOUT, wait_result(&mut cdp, req_id)) => match r {
                Ok(Some(d)) => d,
                Ok(None) => {
                    crate::logging::warn("CDP 收帧通道关闭（渲染进程可能已崩溃/被回收）");
                    break "CDP 断开";
                }
                Err(_) => {
                    crate::logging::warn(&format!(
                        "CDP 等截屏响应超时（{}ms），准备重连",
                        CDP_RECV_TIMEOUT.as_millis()
                    ));
                    break "CDP 超时";
                }
            },
            _ = wait_cancel(cancel.clone()) => break "用户停止",
        };

        let now = std::time::Instant::now();
        if PAUSED.load(Ordering::SeqCst) {
            // 调整显示设置时暂停下发；抓屏仍按节奏走（不空转），resume 后自动继续
            next_send = now + frame_interval();
        } else if now >= next_send {
            // 到点才解码下发（省掉超目标帧率的解码 + 差分 + USB）
            // base64 只解一次：原始 PNG 给预览（走 Channel 二进制），RGB565 给面板。
            // 解码/转换失败不再 `?` 打死整条管线，而是交给 `run_supervised` 重连。
            let png = match render::decode_b64(&data) {
                Ok(p) => p,
                Err(e) => {
                    crate::logging::warn(&format!("截屏 base64 解码失败，准备重连：{e}"));
                    break "帧解码失败";
                }
            };
            let (rgb565, fw, fh) = match render::png_to_rgb565(&png) {
                Ok(v) => v,
                Err(e) => {
                    crate::logging::warn(&format!("PNG→RGB565 失败，准备重连：{e}"));
                    break "帧转换失败";
                }
            };
            if fw != w || fh != h {
                TxStats::warn_throttled(
                    &mut st.last_fail_warn,
                    now,
                    &format!("丢弃尺寸不符的帧 {fw}x{fh}（期望 {w}x{h}）"),
                );
            } else {
                // 旋转/息屏之后设备会清屏回默认背景，脏区差分补不回来 → 强制整屏重发
                if FORCE_FULL.swap(false, Ordering::SeqCst) {
                    prev.clear();
                    crate::logging::info("强制整屏重发");
                }
                // 差分只算一次：`None` = 首帧 / 强制整屏（整帧）；`Some(空)` = 画面没动
                let rects = if prev.is_empty() {
                    None
                } else {
                    Some(diff::dirty_rects(&prev, &rgb565, fw as usize, fh as usize))
                };
                let changed = rects.as_ref().map(|r| !r.is_empty()).unwrap_or(true);
                if changed {
                    still = 0;
                } else {
                    still += 1;
                }
                if looks_black(&rgb565) {
                    st.black += 1;
                    if !black_warned {
                        black_warned = true;
                        crate::logging::warn("抓到全黑帧（面板白屏，或渲染进程刚重启）");
                    }
                }
                if has_usb {
                    let r = with_link(|u| match &rects {
                        None => u.send_full(&rgb565),
                        Some(rs) => u.send_rects(&rgb565, fw as usize, rs),
                    });
                    match r {
                        Ok(Some(s)) => {
                            sent += 1;
                            prev = rgb565;
                            st.sent += 1;
                            st.bytes += s.bytes as u64;
                            st.write_ms += s.write_ms;
                            st.ack_ms += s.ack_ms;
                            match s.kind {
                                "ok" => st.ack_ok += 1,
                                "timeout" => {
                                    st.ack_timeout += 1;
                                    TxStats::warn_throttled(
                                        &mut st.last_ack_warn,
                                        now,
                                        "设备未回 ACK（画面可能已不在 LIVE 模式）",
                                    );
                                }
                                _ => {
                                    st.ack_bad += 1;
                                    let reason = s.reason.clone().unwrap_or_default();
                                    TxStats::warn_throttled(
                                        &mut st.last_ack_warn,
                                        now,
                                        &format!("设备回执异常：{}（{reason}）", s.kind),
                                    );
                                }
                            }
                        }
                        // 画面没动：`send_rects` 直接返回，一个字节都没发（USB 流量为零）
                        Ok(None) => st.skip += 1,
                        Err(e) => {
                            st.fail += 1;
                            // **必须进日志文件**：旧版这里是 eprintln，release 无控制台 → 断联零痕迹
                            TxStats::warn_throttled(
                                &mut st.last_fail_warn,
                                now,
                                &format!("USB 下发失败（仍在重试）：{e}"),
                            );
                            // 设备被拔掉：句柄已失效，继续每帧重试只会让每帧等满管道
                            // 超时（3s），设备命令也会排在同一把 I/O 锁后面 →
                            // 界面表现为死机。立刻释放链路、退回「仅预览」，等用户重新插好。
                            if link_unusable(&e) {
                                crate::logging::warn(&format!("设备已断开，停止下发（仅预览）：{e}"));
                                close_link();
                                has_usb = false;
                            }
                        }
                    }
                } else {
                    prev = rgb565;
                }
                frames += 1;
                st.frames += 1;
                if frames == 1 {
                    crate::logging::info(&format!(
                        "首帧已下发（usb_sent={sent}）+{}ms",
                        t0.elapsed().as_millis()
                    ));
                }
                // 预览「变化才发」：画面没动就不推给前端（首帧 / 强制整屏必发），
                // 静止面板预览流量直接归零
                if changed {
                    on_frame(&png, fw, fh, sent);
                }
            }
            // 静止退避：连续没动就降到 ~3fps；一有变化立刻回目标档位（只影响抓屏节奏）
            next_send = now
                + if still >= IDLE_AFTER {
                    INTERVAL_IDLE
                } else {
                    frame_interval()
                };
        }
        // 请求下一张：提前 `SHOT_EST` 发，让抓屏与「等到 next_send」重叠；不空转。
        let shoot_at = next_send.checked_sub(SHOT_EST).unwrap_or(next_send);
        let delay = shoot_at.saturating_duration_since(std::time::Instant::now());
        if !delay.is_zero() {
            let interrupted = tokio::select! {
                _ = tokio::time::sleep(delay) => false,
                _ = wait_cancel(cancel.clone()) => true,
            };
            if interrupted {
                break "用户停止";
            }
        }
        // 心跳统计：每 10s 一条。**它同时是"管线还活着"的证据** ——
        // 日志里统计行突然中断，就说明循环卡在收帧或 USB 上。
        st.log_if_due(std::time::Instant::now());
        req_id = match cdp.cmd("Page.captureScreenshot", shot_params()).await {
            Ok(id) => id,
            Err(e) => {
                crate::logging::warn(&format!("请求截屏失败：{e}"));
                break "CDP 发送失败";
            }
        };
    };

    crate::logging::info(&format!(
        "渲染会话结束（{reason}）frames={frames} usb_sent={sent} +{}ms",
        t0.elapsed().as_millis()
    ));
    // 「用户停止 / 到达运行时长」= 正常结束；其余（CDP 断开/超时/发送失败/解码失败）
    // 交给 `run_supervised` 重连，不能直接结束整条管线。
    if reason == "用户停止" || reason == "到达运行时长" {
        Ok((frames, sent))
    } else {
        Err(format!("{reason}"))
    }
}

/// 截屏参数：PNG + `optimizeForSpeed`（编码走快路径，省 CPU；文件略大，320×240 无所谓）。
/// 不用 JPEG —— 实测画质不佳（文字边缘糊），PNG 无损更适合面板。
fn shot_params() -> Value {
    json!({ "format": "png", "captureBeyondViewport": false, "optimizeForSpeed": true })
}

/// 等指定 id 的 CDP 响应，返回截屏的 base64（其间的事件丢掉）。
async fn wait_result(cdp: &mut render::Cdp, id: u64) -> Option<String> {
    loop {
        let m = cdp.recv().await?;
        if m.get("id").and_then(|x| x.as_u64()) == Some(id) {
            if let Some(err) = m.get("error") {
                crate::logging::warn(&format!("CDP 错误：{err}"));
                return None;
            }
            return Some(m["result"]["data"].as_str().unwrap_or("").to_string());
        }
    }
}

/// 相对路径解析：`assets/` 目录 → cwd → exe 同级。
fn resolve_panel(path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.exists() {
        return p.to_path_buf();
    }
    // `assets/<主题>/index.html` → 用 assets 目录拼绝对路径
    let rel = p.strip_prefix("assets").unwrap_or(p);
    if let Some(assets) = crate::assets::dir() {
        let cand = assets.join(rel);
        if cand.exists() {
            return cand;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let cand = cwd.join(p);
        if cand.exists() {
            return cand;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            let cand = d.join(p);
            if cand.exists() {
                return cand;
            }
        }
    }
    p.to_path_buf()
}

// 主题目录扫描 / 手动导入在 `assets` 模块（约定：一个主题 = `assets/<主题名>/`，HTML 与图片同层）。


/// 面板实际渲染用的 URL：带上 `?__api=<port>`，页面据此开启「自取本地指标」。
/// 检查闸门直接用 `file://` 打开（无此参数）→ 不自取，注入结果确定。
fn panel_url(html: &str) -> String {
    format!("{}?__api={}", file_url(html), crate::api::PORT)
}

/// 本地路径 → `file://` URL（去 `\\?\` 前缀，非 ASCII / 空格做百分号编码）。
fn file_url(path: &str) -> String {
    let abs = std::fs::canonicalize(resolve_panel(path))
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            s.strip_prefix(r"\\?\").map(|x| x.to_string()).unwrap_or(s)
        })
        .unwrap_or_else(|_| path.to_string());
    let s = abs.replace('\\', "/");
    let mut out = String::from("file:///");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'.' | b'-' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// USB 链路（含 GUI 移交租约）。**全局共享**：WinUSB 不允许同一设备开两个句柄，
/// 所以下发帧和设备命令（亮度/旋转…）必须复用这一个。
pub struct UsbLink {
    /// `Option` 是为了能在归还租约**之前**先关掉句柄（顺序反了 GUI 会认为设备仍被占用）。
    dev: Option<winusb::Device>,
    pub serial: String,
    lease: Option<String>,
    /// 链路代数（见 `LINK_GEN`）。只有代数为「最新」的那条链路才会被管线收尾关闭，
    /// 免得上一轮超时未收尾的管线在收尾时把新一轮正在用的链路关掉。
    /// （字段名不用 `gen` —— 那是 edition 2024 的保留字。）
    gen_id: u64,
}

impl UsbLink {
    /// 设备句柄（已释放则报错）。
    pub fn dev(&self) -> Result<&winusb::Device, String> {
        self.dev.as_ref().ok_or_else(|| "设备句柄已释放".to_string())
    }
}

/// 全局唯一的已接管设备。
///
/// 外层锁只保护「当前是哪条链路」；真正的 USB 读写由 `UsbLink` 内层锁串行化。
/// `with_link` 会先克隆 `Arc`、放开外层锁，再进内层锁 —— 这样 `close_link` / 链路查询
/// 不会被一次 3s 的 USB 管道超时堵死，设备命令之间仍然严格串行。
pub static LINK: Mutex<Option<Arc<Mutex<UsbLink>>>> = Mutex::new(None);

/// 链路快照（**只给前端轮询用**，不参与任何 USB 操作）。
///
/// 为什么要单独存一份：链路查询要**永不阻塞**。`with_link` 现在虽然不再持外层锁做 USB，
/// 但前端每 120ms 查一次，走原子量 + 只做 clone 的短锁仍然最稳，
/// 不会因为 USB 管道超时（3s）把 tokio worker 占满 → 所有 invoke 一起卡住。
/// 这里用原子量 + 一把只做 clone 的短锁，轮询永远不会被 USB 阻塞。
static LINK_OPEN: AtomicBool = AtomicBool::new(false);
static LINK_SN: Mutex<Option<String>> = Mutex::new(None);

/// 更新链路快照（每次 `LINK` 变化后调用；只做一次 clone，不碰 USB）。
fn publish_link_state(open: bool, serial: Option<String>) {
    LINK_OPEN.store(open, Ordering::SeqCst);
    *LINK_SN.lock().unwrap() = serial;
}

/// 串行化「打开设备」：**同一时刻只允许一次移交 + 打开**。
/// 否则 `render_start` 返回后管线线程还在做 USB 接管，前端紧接着的 `device_info`
/// 会看到 `link_open()==false` 也去开一次 → 两次并发 `CreateFileW` 撞 `ERROR_ACCESS_DENIED(5)`
/// → 一方失败「USB 不可用，仅预览」，租约互相顶掉 → 切设备卡顿/丢帧。
static OPEN_LOCK: Mutex<()> = Mutex::new(());

/// 一次「接管设备」的总预算。超过就放弃并报错。
///
/// 为什么要有：向 GUI 申请移交是**重试 + 退避**的（8 次，最坏约 13.6s），之后还要轮询
/// 「打开成功」最多 6s。设备被拔掉 / 租约卡住时，这二十来秒里界面一直停在「读取中」，
/// 用户感知就是死机。给一个硬预算，宁可报错让用户重试，也不要长时间无响应。
const OPEN_BUDGET: Duration = Duration::from_secs(8);

/// 管线是否已经**尝试过**接管 USB（成功或失败都算）。前端据此避免和管线抢设备。
static LINK_READY: AtomicBool = AtomicBool::new(false);

/// 链路「代数」：每新打开一次 +1。管线收尾只关**自己那一代**的链路。
/// 场景：上一轮管线 4s 内没停（`start` 里超时仍继续启动）时，新一轮会重新接管；
/// 若上一轮随后才收尾、无条件 `close_link`，就会把**新一轮正在用的链路**一起关掉 →
/// 实时下发直接断掉。按代数隔离后，旧一轮的收尾会变成 no-op。
static LINK_GEN: AtomicU64 = AtomicU64::new(0);

/// 是否已接管设备（**读快照，不抢 `LINK` 锁**）。
pub fn link_open() -> bool {
    LINK_OPEN.load(Ordering::SeqCst)
}

/// 管线是否已经做过 USB 接管尝试。新版 `render_start` 返回时已完成；
/// 前端保留这个查询作为后台重连/异常退出的兜底。
pub fn link_ready() -> bool {
    LINK_READY.load(Ordering::SeqCst)
}

/// 当前接管中的设备 SN（没接管则 None）。多设备下用来判断命令该发给哪台。
pub fn link_serial() -> Option<String> {
    LINK_SN.lock().unwrap().clone()
}

/// 用共享设备执行一段操作。
///
/// 只在取链路时短暂持外层 `LINK` 锁，USB 读写期间持的是内层锁 —— 前端轮询 /
/// `close_link` 不会因为管道超时（最长 3s）被拖住；并发的设备命令仍按内层锁串行。
pub fn with_link<T>(f: impl FnOnce(&UsbLink) -> Result<T, String>) -> Result<T, String> {
    let link = {
        let g = LINK.lock().unwrap();
        g.as_ref().cloned().ok_or("设备未接管")?
    };
    let guard = link.lock().unwrap();
    f(&guard)
}

/// 下发失败是否意味着「这条链路已经不能用了」：
/// 设备物理断开（`[GONE]`），或链路已被释放（未接管 / 句柄已释放）。
/// 命中就应停止下发并退回「仅预览」，不要对着一个废句柄每帧重试 —— 那样每帧都要等满
/// 管道超时，设备命令也会排在同一把 I/O 锁后面，界面像卡死。
fn link_unusable(e: &str) -> bool {
    winusb::is_gone(e) || e.contains("设备未接管") || e.contains("设备句柄已释放")
}

/// 接管设备（必要时先向 GUI 申请移交）。
/// 返回 `Some(gen)` = 本次**新打开**（携带链路代数，调用方负责按代数 `close_link_if_gen`）；
/// `None` = 已有人接管、**复用了现有句柄**（调用方**不能**关，否则会把别人的链路关掉）。
pub fn open_link(serial: Option<String>) -> Result<Option<u64>, String> {
    UsbLink::open(serial, 0, 0)
}

/// 释放设备（`UsbLink::Drop` 归还租约）。**无条件关**：用于「停止」/退出等确实要交还设备的场景。
pub fn close_link() {
    let old = LINK.lock().unwrap().take();
    publish_link_state(false, None);
    // 等当前 USB 操作结束（最多一次管道超时），在锁内立刻关句柄：
    // 这样紧接着的新一轮 `CreateFileW` 不会撞上还没关掉的旧句柄。
    if let Some(link) = old.as_ref() {
        let mut g = link.lock().unwrap();
        drop(g.dev.take());
    }
    drop(old); // 出锁再析构：归还租约要走一次 HTTP
}

/// **只关指定代数**的链路：若当前链路已被更新的一轮接管（代数不同），则原样不动。
/// 返回是否真的关了。用于管线收尾 / 临时接管收尾，避免误关别人的链路。
pub fn close_link_if_gen(gen_id: u64) -> bool {
    // 先克隆 Arc、放开外层锁，再读代数：绝不在持外层锁时等一次 3s 的 USB 操作。
    let link = { LINK.lock().unwrap().as_ref().cloned() };
    let Some(link) = link else {
        return false;
    };
    if link.lock().unwrap().gen_id != gen_id {
        return false;
    }
    let old = {
        let mut g = LINK.lock().unwrap();
        let same = g
            .as_ref()
            .map(|cur| Arc::ptr_eq(cur, &link))
            .unwrap_or(false);
        if same {
            g.take()
        } else {
            None
        }
    };
    let hit = old.is_some();
    if hit {
        publish_link_state(false, None);
        if let Some(link) = old.as_ref() {
            // 与 `close_link` 一样：等在途 I/O 结束，锁内立刻关句柄，避免新旧句柄重叠。
            let mut g = link.lock().unwrap();
            drop(g.dev.take());
        }
    }
    drop(old);
    hit
}

/// 离开作用域即释放设备（覆盖 `?` 提前返回）。携带「本轮自己打开的链路代数」，
/// `None` = 本轮没新开（复用/未接管）→ 收尾时什么都不关。
struct LinkGuard(Option<u64>);
impl Drop for LinkGuard {
    fn drop(&mut self) {
        if let Some(g) = self.0 {
            close_link_if_gen(g);
        }
    }
}

/// 租约续期线程的停止信号（`Some((leaseId, stop))` = 有线程在跑）。
/// 带上租约号是为了 `stop_keepalive` 能**只停自己的那条线程**：若旧链接先被替换、
/// 其 `Drop` 无条件停当前线程，会把新链接的续期线程一起杀掉 → 新租约不再续期 → 过期 409。
static KA_STOP: Mutex<Option<(String, Arc<AtomicBool>)>> = Mutex::new(None);

/// 给移交租约续期。**必须做**：租约 TTL 只有 120s，不续期就会过期——
/// 过期后 `finish` 报 **409**（租约已失效），GUI 也仍认为设备被占着，
/// 于是下一次 `start` 连续 409（8 次退避共 ~13.6s）→ 切设备卡十几秒甚至失败。
/// 实测：管线跑 107s 时 finish 成功、跑 264s 时 finish 409。
/// 每 20s 续一次（TTL 120s，留足余量）。
fn start_keepalive(lease_id: String, serial: String) {
    stop_keepalive(None); // 换租约前先无条件停掉旧线程
    let stop = Arc::new(AtomicBool::new(false));
    *KA_STOP.lock().unwrap() = Some((lease_id.clone(), stop.clone()));
    std::thread::spawn(move || {
        let mut first = true;
        // 连续失败次数：**去重 + 升级**。每 20s 一条同样的 WARN 会在几小时里刷满日志，
        // 反而把真正的断联证据挤掉；这里只在第 1 次、每 3 次（60s）记一条，≥3 次升为 ERROR。
        let mut fails = 0u32;
        loop {
            // 每 100ms 查一次停止信号，够 20s 就续期（停止最多延迟 100ms）
            for _ in 0..200 {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let outcome = match handoff::handoff_keepalive(&lease_id, &serial) {
                Ok(v) if v.get("ok").and_then(|b| b.as_bool()) != Some(false) => Ok(()),
                Ok(v) => Err(format!("被拒：{v}")),
                Err(e) => Err(format!("请求失败：{e}")),
            };
            match outcome {
                Ok(()) => {
                    if first {
                        first = false;
                        crate::logging::info("租约续期正常（keepalive）");
                    } else if fails > 0 {
                        crate::logging::info(&format!("租约续期已恢复（此前连续失败 {fails} 次）"));
                    }
                    fails = 0;
                }
                Err(why) => {
                    fails += 1;
                    if fails == 1 || fails % 3 == 0 {
                        let msg = format!(
                            "租约续期{why}（连续 {fails} 次，租约 {}s 后过期）",
                            fails * 20
                        );
                        if fails >= 3 {
                            crate::logging::error(&msg);
                        } else {
                            crate::logging::warn(&msg);
                        }
                    }
                    // 6 次 = 120s = 租约 TTL 已过：GUI 现在可以合法抢回设备，
                    // 再写下去只会拿到「拒绝访问」。给出唯一有效的处理办法。
                    if fails == 6 {
                        crate::logging::error(
                            "租约已过期：GUI 可能已抢回设备。请点「停止」再点「确定」重新接管（否则只能看预览）",
                        );
                    }
                }
            }
        }
    });
}

/// 停止租约续期线程。`lease` = 只停这一个租约的线程；`None` = 无条件停当前线程。
/// 按租约号匹配，避免旧链接 `Drop` 误停新链接的续期线程。
fn stop_keepalive(lease: Option<&str>) {
    let mut g = KA_STOP.lock().unwrap();
    let hit = match (g.as_ref(), lease) {
        (Some((id, _)), Some(want)) => id.as_str() == want,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if hit {
        if let Some((_, s)) = g.take() {
            s.store(true, Ordering::SeqCst);
        }
    }
}

/// 从 `/usb/status` 里取一个**非空** SN（`serial` / `sn` / `devices[].serial`）。
fn gui_serial(st: &serde_json::Value) -> Option<String> {
    let pick = |v: &serde_json::Value| {
        ["serial", "sn", "deviceSerial"].iter().find_map(|k| {
            v.get(*k)
                .and_then(|s| s.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    };
    pick(st).or_else(|| st.get("devices")?.as_array()?.iter().find_map(pick))
}

impl UsbLink {
    /// 接管设备并把句柄存进全局 `LINK`。
    /// 返回 `Some(gen)` = 本次新打开（携带代数）；`None` = 已接管、复用了现有句柄（**别关它**）。
    fn open(serial: Option<String>, _w: u32, _h: u32) -> Result<Option<u64>, String> {
        // 串行化：同一时刻只允许一次「移交 + 打开」，避免并发 CreateFileW 互撞（ERROR_ACCESS_DENIED）
        let _open_guard = OPEN_LOCK.lock().unwrap();
        let gui = handoff::gui_running();
        let st = if gui {
            handoff::status().unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };
        let gui_connected = st.get("connected").and_then(|b| b.as_bool()).unwrap_or(false);
        let gui_busy = st.get("busy").and_then(|b| b.as_bool()).unwrap_or(false);

        // 目标 SN：显式指定 → GUI 列表 → 本地枚举。**空 SN 一律当没有**
        // （GUI 未连设备时 /usb/status 的 serial 是空串，拿它去申请移交只会被拒 409）
        let target = match serial.filter(|s| !s.trim().is_empty()) {
            Some(s) => s.trim().to_string(),
            None => gui_serial(&st)
                .or_else(|| {
                    winusb::list_devices()
                        .ok()
                        .and_then(|v| v.into_iter().find_map(|(_, sn)| sn))
                })
                .ok_or("没有可用设备（GUI 未连接设备，本地也没枚举到）")?,
        };
        crate::logging::info(&format!(
            "USB 目标 {target}（GUI={gui} 已连={gui_connected} 忙={gui_busy}）"
        ));

        // 拿到锁后重新确认：若此刻已被（另一个线程）接管 → 直接复用，绝不开第二次
        // （管线线程与 device_info 并发时，这里把后到的一方变成复用，消除竞态）
        let existing = { LINK.lock().unwrap().as_ref().cloned() };
        if let Some(cur) = existing {
            let cur = cur.lock().unwrap();
            if cur.serial.eq_ignore_ascii_case(target.trim()) {
                crate::logging::info("设备已接管，复用现有句柄（跳过重复打开）");
                return Ok(None); // 复用 → 调用方不要关
            }
            return Err(format!("设备 {} 正在接管中，无法接管 {}", cur.serial, target));
        }

        // GUI 没占着设备（或根本没运行）：直接接管，省掉一次移交（GUI 409/429 多半出在这里）
        let gui_idle = !gui || (!gui_connected && !gui_busy);
        if gui_idle {
            match winusb::find_by_serial(&target) {
                Ok(dev) => {
                    crate::logging::info(if gui {
                        "GUI 未占用设备：直接接管"
                    } else {
                        "GUI 未运行：直接接管"
                    });
                    let gen_id = LINK_GEN.fetch_add(1, Ordering::SeqCst) + 1;
                    *LINK.lock().unwrap() = Some(Arc::new(Mutex::new(UsbLink {
                        dev: Some(dev),
                        serial: target.clone(),
                        lease: None,
                        gen_id,
                    })));
                    publish_link_state(true, Some(target));
                    return Ok(Some(gen_id));
                }
                // GUI 也说没连着、本地又打不开 → 这台设备**确实不在了**（多半已被拔掉）。
                // **必须立刻返回**：否则会掉进下面的移交重试（8 次退避 ≈ 13.6s，再加 6s 轮询），
                // 这段时间界面一直停在「读取中」，用户看到的就是卡死。
                Err(e) => return Err(format!("设备已断开或不可用：{e}")),
            }
        }

        if gui {
            let client_id = format!("jiaidian-monitor-{}", std::process::id());
            crate::logging::info(&format!("USB 移交：target={target}（GUI 在跑）"));
            // 移交要重试：GUI 可能刚被别的操作占着，或上一次租约还没释放（429）
            let r = {
                // 总预算：GUI 回什么都别耗太久（这段时间界面一直停在「读取中」）
                let deadline = std::time::Instant::now() + OPEN_BUDGET;
                let mut last = String::new();
                let mut ok = None;
                for attempt in 0..8u64 {
                    if std::time::Instant::now() >= deadline {
                        last = format!("{last}（接管总耗时超过 {}s，放弃）", OPEN_BUDGET.as_secs());
                        break;
                    }
                    match handoff::handoff_start(&target, &client_id, 120) {
                        Ok(x) => {
                            if x.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                                ok = Some(x);
                                break;
                            }
                            last = x.to_string();
                        }
                        Err(e) => last = e,
                    }
                    let hint = if last.contains("429") {
                        "（429：GUI 侧限流或上一次租约未释放，等它超时）"
                    } else if last.contains("409") {
                        "（409：设备忙碌 / 目标不匹配 / 租约失效；若是异常退出残留的旧租约，约 1~2 分钟自动恢复）"
                    } else {
                        ""
                    };
                    crate::logging::warn(&format!("USB 移交重试 {}/8：{last}{hint}", attempt + 1));
                    let wait = Duration::from_millis(300 + 400 * attempt);
                    if std::time::Instant::now() + wait >= deadline {
                        break;
                    }
                    std::thread::sleep(wait);
                }
                match ok {
                    Some(x) => x,
                    None => {
                        crate::logging::error(&format!("USB 移交失败：{last}"));
                        return Err(format!("移交失败：{last}"));
                    }
                }
            };
            let lease = r.get("leaseId").and_then(|v| v.as_str()).map(|s| s.to_string());
            // 等 GUI 真正放开设备：轮询「打开成功」而不是死等 1.5s（通常 100–300ms 就够）
            let t0 = std::time::Instant::now();
            let dev = loop {
                match winusb::find_by_serial(&target) {
                    Ok(d) => break Ok(d),
                    Err(e) => {
                        // 3s 足够 GUI 真正放开设备（通常 100–300ms）；再长只会让界面白等
                        if t0.elapsed() > Duration::from_secs(3) {
                            break Err(e);
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            };
            let dev = match dev {
                Ok(d) => d,
                Err(e) => {
                    // 打不开就别占着租约：否则 GUI 一直认为设备被占用，下次 start 会被拒
                    if let Some(id) = &lease {
                        let _ = handoff::handoff_finish(id, &target);
                        crate::logging::warn(&format!("移交后打不开设备，已归还租约：{e}"));
                    }
                    return Err(e);
                }
            };
            crate::logging::info(&format!("移交后打开设备成功 +{}ms", t0.elapsed().as_millis()));
            // 拿到租约就开始续期（否则 120s 后过期 → finish/start 双双 409）
            if let Some(id) = &lease {
                start_keepalive(id.clone(), target.clone());
            }
            let gen_id = LINK_GEN.fetch_add(1, Ordering::SeqCst) + 1;
            *LINK.lock().unwrap() = Some(Arc::new(Mutex::new(UsbLink {
                dev: Some(dev),
                serial: target.clone(),
                lease,
                gen_id,
            })));
            publish_link_state(true, Some(target));
            return Ok(Some(gen_id));
        }

        // 走到这里说明 GUI 在跑但设备已被它连着、且移交没成 —— 上面已把错误返回了
        Err("设备被 GUI 占用且移交失败".to_string())
    }

    fn send_full(&self, rgb565: &[u8]) -> Result<Option<Sent>, String> {
        let f = live_frame::frame_full(rgb565);
        let bytes = f.len();
        let t0 = Instant::now();
        self.dev()?.write(&f)?;
        let write_ms = t0.elapsed().as_millis() as u64;
        // ACK **不能丢**：设备自己退出 LIVE 时，只有这里能看出来（`LIVE,exit` / `LIVE,err`）。
        let raw = self.dev()?.read(256)?;
        let ack_ms = t0.elapsed().as_millis() as u64 - write_ms;
        let (kind, reason) = live_frame::parse_ack(raw.as_deref());
        Ok(Some(Sent {
            kind,
            reason,
            bytes,
            write_ms,
            ack_ms,
        }))
    }

    fn send_rects(
        &self,
        frame: &[u8],
        w: usize,
        rects: &[(usize, usize, usize, usize)],
    ) -> Result<Option<Sent>, String> {
        if rects.is_empty() {
            // 画面没动：**一个字节都不发**。返回 `None` 让统计能区分「跳过」和「发了」。
            return Ok(None);
        }
        let f = if rects.len() == 1 {
            let (x, y, rw, rh) = rects[0];
            let payload = live_frame::extract_region565(frame, w as u32, x as u32, y as u32, rw as u32, rh as u32);
            live_frame::frame_region(x as u16, y as u16, rw as u16, rh as u16, &payload)
        } else {
            let regions: Vec<_> = rects
                .iter()
                .map(|&(x, y, rw, rh)| {
                    let payload = live_frame::extract_region565(frame, w as u32, x as u32, y as u32, rw as u32, rh as u32);
                    (x as u16, y as u16, rw as u16, rh as u16, payload)
                })
                .collect();
            live_frame::frame_batch(&regions)
        };
        let bytes = f.len();
        let t0 = Instant::now();
        self.dev()?.write(&f)?;
        let write_ms = t0.elapsed().as_millis() as u64;
        let raw = self.dev()?.read(256)?;
        let ack_ms = t0.elapsed().as_millis() as u64 - write_ms;
        let (kind, reason) = live_frame::parse_ack(raw.as_deref());
        Ok(Some(Sent {
            kind,
            reason,
            bytes,
            write_ms,
            ack_ms,
        }))
    }
}

impl Drop for UsbLink {
    fn drop(&mut self) {
        // 先关 USB 句柄，再归还租约
        drop(self.dev.take());
        stop_keepalive(self.lease.as_deref()); // 只停本链接的续期线程（避免误停新链接的）
        if let Some(id) = self.lease.take() {
            match handoff::handoff_finish(&id, &self.serial) {
                Ok(_) => crate::logging::info(&format!("归还租约 {id}（{}）", self.serial)),
                Err(e) => crate::logging::warn(&format!("归还租约失败：{e}")),
            }
        }
    }
}

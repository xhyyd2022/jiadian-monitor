//! 实时流管线：离屏浏览器抓帧 → RGB565 → 差分 → LIVE 帧 → USB 下发，并把每帧推给前端预览。
//!
//! 首帧 mode 0 立底座，之后只推脏矩形（mode 1 / mode 2 ≤6 区）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::{diff, handoff, live_frame, render, winusb};

static CANCEL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
/// 管线是否在跑（退出时用来等它收尾，好把租约还回去）。
static RUNNING: AtomicBool = AtomicBool::new(false);
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

/// 目标帧率 ~30fps。面板里有动画（非全静态）时要能跑到 30 附近；
/// 再快对 320×240 没意义，只会白烧 USB 带宽和 CPU。变化小的帧天然发得少。
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

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

    tauri::async_runtime::spawn(async move {
        // 预览限频到目标帧率：不影响 USB 下发，只砍掉超过 30fps 的 IPC / 前端解码开销
        let mut last: Option<std::time::Instant> = None;
        let cb = move |png: &str, fw: u32, fh: u32, frames: u64| {
            let now = std::time::Instant::now();
            if let Some(t) = last {
                if now.duration_since(t) < FRAME_INTERVAL {
                    return;
                }
            }
            last = Some(now);
            let _ = app.emit("preview", json!({ "png": png, "w": fw, "h": fh, "frames": frames }));
        };
        if let Err(e) = run_inner(cb, ws, url, serial, w, h, cancel, None).await {
            crate::logging::error(&format!("render 出错：{e}"));
        }
        RUNNING.store(false, Ordering::SeqCst);
        // 兜底：管线退出后也算「就绪」，免得前端在接管失败/提前退出时白等
        LINK_READY.store(true, Ordering::SeqCst);
    });
    Ok("started".into())
}

/// 面板文件名（在 CDP 目标里区分面板页 vs UI 页）。
fn panel_file_name(html: &str) -> String {
    html.rsplit(['/', '\\']).next().unwrap_or(html).to_string()
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
        .build()
        .map_err(|e| format!("创建离屏面板窗口失败：{e}"))?;
    crate::logging::info("已创建离屏面板窗口（隐藏）");
    Ok(())
}

/// 注入到页面里的「自取本地指标」脚本：等 `__setSlot` 就绪后，每秒拉一次 `/metrics` 喂进去。
/// 面板只要定义了 `__setSlot` + 标准 `data-slot`（如 `cpu_load`），就自动拿到真值，
/// **不需要面板自己写 fetch**。闸门直接用 `file://` 打开（不注入）→ 结果确定。
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
var PORT='__PORT__';
var MURL='http://127.0.0.1:'+PORT+'/metrics';
var UURL='http://127.0.0.1:'+PORT+'/units';
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
function apply(list,cfg){
  if(cfg&&cfg.mode)CFG=cfg;
  for(var i=0;i<list.length;i++){
    var m=list[i];
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
var t=setInterval(function(){
  if(typeof window.__setSlot!=='function')return;
  clearInterval(t);
  (function p(){
    scan();
    var a=fetch(MURL,{cache:'no-store'}).then(function(r){return r.json();}).catch(function(){return null;});
    var b=fetch(UURL,{cache:'no-store'}).then(function(r){return r.json();}).catch(function(){return null;});
    Promise.all([a,b]).then(function(res){ if(res[0])apply(res[0],res[1]); });
    setTimeout(p,1000);
  })();
},100);
})();"#;
    JS.replace("__PORT__", &crate::api::PORT.to_string())
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
    let cb = |png: &str, fw: u32, fh: u32, frames: u64| {
        println!("  frame {frames}  {fw}x{fh}");
        if n == 0 {
            if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(png) {
                let _ = std::fs::write(&first, &b);
            }
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
        let r = run_inner(cb, ws, url, serial, w, h, cancel, Some(seconds)).await;
        browser.kill();
        let _ = render::close_browser(port).await;
        r
    })?;
    Ok(format!("frames={frames} usb_sent={sent}"))
}

#[allow(clippy::too_many_arguments)]
async fn run_inner<F: FnMut(&str, u32, u32, u64)>(
    mut on_frame: F,
    ws: String,
    url: String,
    serial: Option<String>,
    w: u32,
    h: u32,
    cancel: Arc<AtomicBool>,
    seconds: Option<f64>,
) -> Result<(u64, u64), String> {
    let t0 = std::time::Instant::now();
    let mut cdp = render::Cdp::connect(&ws).await?;
    cdp.cmd("Page.enable", json!({})).await?;
    // 先导航到面板并等页面就绪 —— 否则首帧会抓到别的页面
    let _ = cdp.call("Page.navigate", json!({ "url": url })).await;
    // 等「真的加载完」而不是死等固定时长：file:// 通常 <100ms，这段直接决定切换延迟
    tokio::time::sleep(Duration::from_millis(80)).await;
    let ready = wait_page_ready(&mut cdp, 3000).await;
    crate::logging::info(&format!("页面就绪 ready={ready} +{}ms", t0.elapsed().as_millis()));
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
    crate::logging::info(&format!("视口锁定 +{}ms", t0.elapsed().as_millis()));

    // USB（拿不到就只预览、不下发）。
    // `has_usb` = 接管成功（**新开或复用同一台都算**）；`link_gen` = 仅当本轮**自己新打开**时才是
    // `Some(gen)`（收尾只关这一代，避免误关新一轮的链路）。
    let (has_usb, link_gen) = match UsbLink::open(serial, w, h) {
        Ok(g) => {
            crate::logging::info(&format!("USB 已接管 +{}ms", t0.elapsed().as_millis()));
            (true, g)
        }
        Err(e) => {
            crate::logging::warn(&format!("USB 不可用，仅预览：{e}"));
            (false, None)
        }
    };
    // 任何退出路径都释放设备（只释放**本轮自己打开**的那一代）
    let _link_guard = LinkGuard(link_gen);
    // 接管尝试已结束（无论成败）：前端可以安全读信息了（成功则复用句柄、失败则自己开）
    LINK_READY.store(true, Ordering::SeqCst);

    // WebView2 **不支持** `Page.startScreencast`（实测收不到 screencastFrame），只能轮询截屏。
    // 做法：**先发下一张、再处理这一张** —— 截屏耗时（~25ms）和解码/差分/USB 重叠，
    // 否则两者相加会把 30fps 压到 ~20fps。
    let mut prev: Vec<u8> = Vec::new();
    let mut sent = 0u64;
    let mut frames = 0u64;
    let mut next_send = std::time::Instant::now();
    let start = std::time::Instant::now();
    let mut req_id = cdp
        .cmd("Page.captureScreenshot", json!({ "format": "png" }))
        .await?;

    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if let Some(secs) = seconds {
            if start.elapsed().as_secs_f64() > secs {
                break;
            }
        }
        // 等这一张的响应（其间的事件丢掉）；select 让 stop() 立刻生效
        let data = tokio::select! {
            r = wait_result(&mut cdp, req_id) => match r {
                Some(d) => d,
                None => { eprintln!("[dbg] cdp 关闭或出错"); break; }
            },
            _ = wait_cancel(cancel.clone()) => break,
        };
        // 立刻发下一张，让截屏和下面的处理重叠
        req_id = cdp
            .cmd(
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": false }),
            )
            .await?;

        // 到点才解码下发（省掉超 30fps 的解码 + 差分 + USB）
        if std::time::Instant::now() < next_send {
            continue;
        }
        // 调整显示设置时暂停：跳过发送，resume 后自动继续
        if PAUSED.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(15)).await;
            continue;
        }
        let send_at = std::time::Instant::now();
        let (rgb565, fw, fh) = render::decode_screencast_frame(&data)?;
        if fw != w || fh != h {
            eprintln!("[dbg] 丢弃 {fw}x{fh} 帧");
            continue;
        }
        // 旋转/息屏之后设备会清屏回默认背景，脏区差分补不回来 → 强制整屏重发
        if FORCE_FULL.swap(false, Ordering::SeqCst) {
            prev.clear();
            crate::logging::info("强制整屏重发");
        }
        if has_usb {
            let r = with_link(|u| {
                if prev.is_empty() {
                    u.send_full(&rgb565)
                } else {
                    let rects = diff::dirty_rects(&prev, &rgb565, fw as usize, fh as usize);
                    u.send_rects(&rgb565, fw as usize, &rects)
                }
            });
            match r {
                Ok(()) => {
                    sent += 1;
                    prev = rgb565;
                }
                Err(e) => eprintln!("[pipeline] USB 下发失败：{e}"),
            }
        } else {
            prev = rgb565;
        }
        frames += 1;
        if frames == 1 {
            crate::logging::info(&format!(
                "首帧已下发（usb_sent={sent}）+{}ms",
                t0.elapsed().as_millis()
            ));
        }
        on_frame(&data, fw, fh, sent);
        next_send = send_at + FRAME_INTERVAL;
    }

    // 收尾：只关**本轮自己打开的**那一代链路（若已被新一轮接管则不碰）
    if let Some(g) = link_gen {
        close_link_if_gen(g);
    }
    crate::logging::info(&format!(
        "管线退出 frames={frames} usb_sent={sent} +{}ms",
        t0.elapsed().as_millis()
    ));
    Ok((frames, sent))
}

/// 等指定 id 的 CDP 响应，返回截屏的 base64（其间的事件丢掉）。
async fn wait_result(cdp: &mut render::Cdp, id: u64) -> Option<String> {
    loop {
        let m = cdp.recv().await?;
        if m.get("id").and_then(|x| x.as_u64()) == Some(id) {
            if let Some(err) = m.get("error") {
                eprintln!("[pipeline] CDP 错误：{err}");
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
pub static LINK: Mutex<Option<UsbLink>> = Mutex::new(None);

/// 串行化「打开设备」：**同一时刻只允许一次移交 + 打开**。
/// 否则 `render_start` 返回后管线线程还在做 USB 接管，前端紧接着的 `device_info`
/// 会看到 `link_open()==false` 也去开一次 → 两次并发 `CreateFileW` 撞 `ERROR_ACCESS_DENIED(5)`
/// → 一方失败「USB 不可用，仅预览」，租约互相顶掉 → 切设备卡顿/丢帧。
static OPEN_LOCK: Mutex<()> = Mutex::new(());

/// 管线是否已经**尝试过**接管 USB（成功或失败都算）。前端据此避免和管线抢设备。
static LINK_READY: AtomicBool = AtomicBool::new(false);

/// 链路「代数」：每新打开一次 +1。管线收尾只关**自己那一代**的链路。
/// 场景：上一轮管线 4s 内没停（`start` 里超时仍继续启动）时，新一轮会重新接管；
/// 若上一轮随后才收尾、无条件 `close_link`，就会把**新一轮正在用的链路**一起关掉 →
/// 实时下发直接断掉。按代数隔离后，旧一轮的收尾会变成 no-op。
static LINK_GEN: AtomicU64 = AtomicU64::new(0);

/// 是否已接管设备。
pub fn link_open() -> bool {
    LINK.lock().unwrap().is_some()
}

/// 管线是否已经做过 USB 接管尝试（`render_start` 返回 ≠ USB 已就绪，需等这个）。
pub fn link_ready() -> bool {
    LINK_READY.load(Ordering::SeqCst)
}

/// 当前接管中的设备 SN（没接管则 None）。多设备下用来判断命令该发给哪台。
pub fn link_serial() -> Option<String> {
    LINK.lock().unwrap().as_ref().map(|l| l.serial.clone())
}

/// 用共享设备执行一段操作。
pub fn with_link<T>(f: impl FnOnce(&UsbLink) -> Result<T, String>) -> Result<T, String> {
    let g = LINK.lock().unwrap();
    let l = g.as_ref().ok_or("设备未接管")?;
    f(l)
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
    drop(old); // 出锁再析构：归还租约要走一次 HTTP
}

/// **只关指定代数**的链路：若当前链路已被更新的一轮接管（代数不同），则原样不动。
/// 返回是否真的关了。用于管线收尾 / 临时接管收尾，避免误关别人的链路。
pub fn close_link_if_gen(gen_id: u64) -> bool {
    let old = {
        let mut g = LINK.lock().unwrap();
        match g.as_ref() {
            Some(l) if l.gen_id == gen_id => g.take(),
            _ => None,
        }
    };
    let hit = old.is_some();
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
        loop {
            // 每 100ms 查一次停止信号，够 20s 就续期（停止最多延迟 100ms）
            for _ in 0..200 {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            match handoff::handoff_keepalive(&lease_id, &serial) {
                Ok(v) => {
                    if v.get("ok").and_then(|b| b.as_bool()) == Some(false) {
                        crate::logging::warn(&format!("租约续期被拒：{v}"));
                    } else if first {
                        first = false;
                        crate::logging::info("租约续期正常（keepalive）");
                    }
                }
                Err(e) => crate::logging::warn(&format!("租约续期失败：{e}")),
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
        if let Some(cur) = LINK.lock().unwrap().as_ref() {
            if cur.serial.eq_ignore_ascii_case(target.trim()) {
                crate::logging::info("设备已接管，复用现有句柄（跳过重复打开）");
                return Ok(None); // 复用 → 调用方不要关
            }
            return Err(format!("设备 {} 正在接管中，无法接管 {}", cur.serial, target));
        }

        // GUI 没占着设备：直接接管，省掉一次移交（GUI 409/429 多半出在这里）
        if !gui || (!gui_connected && !gui_busy) {
            match winusb::find_by_serial(&target) {
                Ok(dev) => {
                    crate::logging::info(if gui {
                        "GUI 未占用设备：直接接管"
                    } else {
                        "GUI 未运行：直接接管"
                    });
                    let gen_id = LINK_GEN.fetch_add(1, Ordering::SeqCst) + 1;
                    *LINK.lock().unwrap() = Some(UsbLink {
                        dev: Some(dev),
                        serial: target,
                        lease: None,
                        gen_id,
                    });
                    return Ok(Some(gen_id));
                }
                Err(e) if !gui => return Err(e),
                Err(e) => crate::logging::warn(&format!("直接接管失败，改走移交：{e}")),
            }
        }

        if gui {
            let client_id = format!("jiaidian-monitor-{}", std::process::id());
            crate::logging::info(&format!("USB 移交：target={target}（GUI 在跑）"));
            // 移交要重试：GUI 可能刚被别的操作占着，或上一次租约还没释放（429）
            let r = {
                let mut last = String::new();
                let mut ok = None;
                for attempt in 0..8u64 {
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
                    std::thread::sleep(Duration::from_millis(300 + 400 * attempt));
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
                        if t0.elapsed() > Duration::from_secs(6) {
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
            *LINK.lock().unwrap() = Some(UsbLink {
                dev: Some(dev),
                serial: target,
                lease,
                gen_id,
            });
            return Ok(Some(gen_id));
        }

        // 走到这里说明 GUI 在跑但设备已被它连着、且移交没成 —— 上面已把错误返回了
        Err("设备被 GUI 占用且移交失败".to_string())
    }

    fn send_full(&self, rgb565: &[u8]) -> Result<(), String> {
        let f = live_frame::frame_full(rgb565);
        self.dev()?.write(&f)?;
        let _ = self.dev()?.read(256)?;
        Ok(())
    }

    fn send_rects(
        &self,
        frame: &[u8],
        w: usize,
        rects: &[(usize, usize, usize, usize)],
    ) -> Result<(), String> {
        if rects.is_empty() {
            return Ok(());
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
        self.dev()?.write(&f)?;
        let _ = self.dev()?.read(256)?;
        Ok(())
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

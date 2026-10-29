<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from "vue";
import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon, addCollection } from "@iconify/vue";
import { useSettings } from "./composables/useSettings";
import { cleanupMissingDevice, createIntentGuard, createPreviewRenderer, createRenderSession, withTimeout } from "./renderSession";
import {
  prog as lhwsProg,
  pct as lhwsPct,
  indeterminate as lhwsIndet,
  sizeText as lhwsSize,
  speedText as lhwsSpeed,
  attemptText as lhwsAttempt,
  watchProgress,
  unwatchProgress,
} from "./composables/useLhwsProgress";
import lineMd from "./line-md-icons";
import TitleBar from "./components/TitleBar.vue";
import Sidebar from "./components/Sidebar.vue";
import SettingsPage from "./components/SettingsPage.vue";
import MaterialsPage from "./components/MaterialsPage.vue";
import PeripheralsPage from "./components/PeripheralsPage.vue";

addCollection(lineMd);

const appWindow = getCurrentWindow();
// 只有后端 state=idle 才能确认归还；字符串（包括“仍在收尾”）不是释放凭据。
async function releaseLease(tag: string) {
  const idle = await session.stop();
  logEvent(`${tag}：${session.view().message}`);
  return idle;
}
async function onClose() {
  if (closeToTray.value) {
    logEvent("点击关闭 → 收进系统托盘");
    try {
      await invoke<string>("hide_to_tray");
    } catch {
      await appWindow.minimize();
    }
    return;
  }
  intent.invalidate();
  infoRevision++;
  previewRenderer.invalidate();
  await releaseLease("退出");
  await appWindow.destroy();
}
function goPage(id: string) {
  page.value = id as typeof page.value;
}

interface ThemeVariant {
  /** 归类标签：`v1p`（320×170）/ `v2`（320×240）/ `any` */
  tag: string;
  /** 相对 `assets/` 的页面路径，如 `玻璃拟态/v2/index.html` */
  page: string;
  width: number | null;
  height: number | null;
}
interface Theme {
  /** 主题名 = `assets/` 下的文件夹名（可中文） */
  name: string;
  /** 默认页面（后端给的第一个变体） */
  page: string;
  /** 各尺寸变体 */
  variants: ThemeVariant[];
}
/** 一台设备（本地物理枚举 + GUI 状态富化） */
interface DeviceEntry {
  serial: string | null;
  path: string | null;
  width: number | null;
  height: number | null;
  variant: string | null;
  connected: boolean;
}
interface BasicMetric {
  key: string;
  group: string;
  label: string;
  value: number | null;
  unit: string;
  text: string;
  source: string;
  kind: string;
}
interface DeviceInfo {
  serial: string | null;
  jed: string;
  pong: string;
  sta: string;
  flash_id: string;
  flash_model: string | null;
  flash_mbit: number | null;
  anim_mb: number | null;
  frames: number | null;
  width: number | null;
  height: number | null;
  caps: string[];
  hw: string;
  millivolts: number | null;
  brightness: number | null;
  screen_off: number | null;
  rotation: number | null;
  follow_off: number | null;
}

const page = ref<"home" | "perf" | "peripherals" | "materials" | "settings">("home");
const error = ref("");
const {
  theme,
  closeToTray,
  hybridCompat,
  settingsSaving,
  settingsReady,
  loadSettings,
  loadAutostart,
} = useSettings();

/**
 * 主题真正生效的地方是 `<html>` 上的 `light` class（CSS 变量与 `color-scheme`
 * 都挂在 `:root` / `:root.light`，见 styles.css 顶部注释）。
 *
 * 切换分三步，**跨两帧**：
 *   ① 本帧：先挂 `.theme-switching`（`transition: none !important`）；
 *   ② 下一帧：才改 `light` class —— 此时过渡已经被关掉，是一次干净的跳变；
 *   ③ 再下一帧：把过渡还回去（颜色已落定，恢复 transition 不会补动画）。
 *
 * **为什么换色要延后一帧**：①和②挤在同一帧的话，浏览器只做一次样式计算，
 * 「本帧 transition 被覆盖成 none」这种边缘情况各家引擎处理不完全一致，
 * 有的仍会按旧 transition 插值 —— 按钮就又开始慢半拍。拆到两次样式计算里
 * 就没有歧义了。代价是 1 帧（≈16ms），远低于人眼能察觉的阈值。
 *
 * 注意：延迟一帧**不能替代**关过渡。只延迟不关过渡，按钮照样渐变 120ms，
 * 只是整体都晚一帧开始，依然不同步。
 *
 * `themeToken` 防连点；两处 `setTimeout` 是兜底 —— 窗口被隐藏 / 遮挡时
 * rAF 可能不跑，不能让主题卡在旧颜色、或让屏蔽类永久留在 html 上。
 */
let themeToken = 0;
watch(
  theme,
  (t) => {
    const root = document.documentElement;
    const token = ++themeToken;
    let committed = false;

    root.classList.add("theme-switching");

    const commit = () => {
      if (committed || token !== themeToken) return;
      committed = true;
      root.classList.toggle("light", t === "light");
      const restore = () => {
        if (token === themeToken) root.classList.remove("theme-switching");
      };
      requestAnimationFrame(restore);
      setTimeout(restore, 120);
    };
    requestAnimationFrame(commit);
    setTimeout(commit, 120);
  },
  // sync：点下去立刻进第①步，不再等一次组件更新；immediate：启动时按 ini 里的值对齐
  { flush: "sync", immediate: true },
);

// ---------------- 指标 ----------------
const metrics = ref<BasicMetric[]>([]);
const cpuLoadKeys = new Set(["cpu_load", "cpu_load_p", "cpu_load_e"]);
let timer: number | undefined;
let refreshBusy = false;
let refreshRevision = 0;
let metricsDisposed = false;
let cpuModePending = false;
function invalidateCpuLoads() {
  metrics.value = metrics.value.map((m) => cpuLoadKeys.has(m.key)
    ? { ...m, value: null, text: "—" }
    : m);
}
watch(hybridCompat, () => {
  cpuModePending = settingsReady.value;
  refreshRevision++;
  invalidateCpuLoads();
}, { flush: "sync" });
watch(settingsSaving, (saving) => {
  if (!saving && cpuModePending) {
    cpuModePending = false;
    refreshRevision++;
    invalidateCpuLoads();
    void refresh();
  }
}, { flush: "sync" });
async function refresh() {
  if (refreshBusy || metricsDisposed) return;
  refreshBusy = true;
  const revision = refreshRevision;
  try {
    const next = await invoke<BasicMetric[]>("basic_metrics");
    if (metricsDisposed || revision !== refreshRevision) return;
    metrics.value = next.map((m) => cpuLoadKeys.has(m.key) && (
      cpuModePending || m.value === null || !Number.isFinite(m.value) ||
      (!hybridCompat.value && m.key !== "cpu_load")
    ) ? { ...m, value: null, text: "—" } : m);
    error.value = "";
  } catch (e) {
    if (metricsDisposed || revision !== refreshRevision) return;
    invalidateCpuLoads();
    error.value = String(e);
  } finally {
    refreshBusy = false;
  }
}
// 指标显示：单位由后端按规范给出（素材要什么单位由素材自己声明，见 data-unit）
function display(m: BasicMetric): string {
  return m.text;
}
const GROUP_ICON: Record<string, string> = {
  CPU: "line-md:gauge",
  GPU: "line-md:monitor",
  内存: "line-md:computer",
  磁盘: "line-md:folder",
  网络: "line-md:switch",
  风扇: "line-md:sunny",
  系统: "line-md:lightbulb",
};
const sections = computed(() => {
  const out: { title: string; icon: string; items: BasicMetric[] }[] = [];
  for (const m of metrics.value) {
    const last = out[out.length - 1];
    if (last && last.title === m.group) last.items.push(m);
    else out.push({ title: m.group, icon: GROUP_ICON[m.group] || "line-md:list", items: [m] });
  }
  return out;
});

// ---------------- 面板下发 ----------------
const themes = ref<Theme[]>([]);
const selected = ref("");
const htmlPath = ref("");
const assetsDir = ref("");
const renderMsg = ref("");
const running = ref(false);
const busy = ref(false);
/// 预览画布（取代 `<img src="data:...">`）：一块 canvas 反复重画，避免 Chromium 给
/// 每个唯一 data URL 解码并缓存位图 —— 那是主 UI 内存高涨的元凶。
const previewCanvas = ref<HTMLCanvasElement | null>(null);
/// 是否已经有画面（控制「占位提示」与画布的显隐）。
const hasPreview = ref(false);
const intent = createIntentGuard();
let appDisposed = false;
let infoRevision = 0;
let scanRevision = 0;
let pollingTimer: number | undefined;
let pollingStopped = false;
const session = createRenderSession({
  invoke,
  changed: (view) => {
    if (appDisposed) return;
    const oldPreview = sessionView.value.previewSession;
    sessionView.value = view;
    running.value = view.active;
    renderMsg.value = view.message;
    if (oldPreview !== view.previewSession) previewRenderer.invalidate();
  },
});
const sessionView = ref(session.view());
const canChoose = computed(() => sessionView.value.canStart || (
  sessionView.value.known && sessionView.value.state?.phase === "running" && !sessionView.value.pending
));
const previewRenderer = createPreviewRenderer<ImageBitmap>({
  session: () => sessionView.value.previewSession,
  decode: (png) => createImageBitmap(new Blob([new Uint8Array(png)], { type: "image/png" })),
  clear: () => { hasPreview.value = false; previewLogged = false; },
  paint: (bmp) => {
    const cv = previewCanvas.value;
    const ctx = cv?.getContext("2d");
    if (!cv || !ctx) return;
    if (cv.width !== bmp.width || cv.height !== bmp.height) {
      cv.width = bmp.width;
      cv.height = bmp.height;
    }
    ctx.drawImage(bmp, 0, 0);
    hasPreview.value = true;
    if (!previewLogged) { previewLogged = true; logEvent("首帧预览到达"); }
  },
});
// Serial timeout-based polling: no interval backlog and at most one raw state IPC.
async function pollSession() {
  if (pollingStopped) return;
  await session.refresh();
  if (!pollingStopped) pollingTimer = window.setTimeout(pollSession, 1000);
}
async function runIntent(work: (token: number) => Promise<void>) {
  if (busy.value || ctlBusy.value || appDisposed) return;
  const token = intent.begin();
  busy.value = true;
  infoRevision++;
  try { await work(token); }
  catch (e) { if (intent.current(token)) renderMsg.value = String(e); }
  finally { if (intent.current(token)) busy.value = false; }
}
/// 预览通道：订阅一次，Rust 每帧把 PNG **原始字节**直接推过来（**不走事件**）。
/// 事件通道（`emit`）会把 payload 内联进 JS 源码再 `eval` → 每帧在渲染进程编译一段
/// 新脚本 → 内存持续上涨；`Channel` 走 IPC 队列 + 二进制传输，没有这个问题。
const previewCh = new Channel<ArrayBuffer>();
previewCh.onmessage = (buf) => {
  void drawPreview(buf);
};

// ---------------- 设备（多设备切换 + 尺寸识别） ----------------
// 只做两种尺寸：320×240（V2）/ 320×170（V1PRO）。素材按尺寸分目录 assets/<主题>/v1p|v2/。
const devices = ref<DeviceEntry[]>([]);
const selSerial = ref("");
const devSize = ref<{ w: number; h: number } | null>(null);
/**
 * 上一次**确实探到**的设备尺寸。刷新/切设备时 `devSize` 会先被清空，
 * 若直接回退到 320×240 兜底，标题与预览框就会「240 → 170」跳一下 → 界面抖动。
 * 这里记住上次的值，清空期间沿用它，只有新尺寸真到了才变。
 */
const lastSize = ref<{ w: number; h: number } | null>(null);
/** 当前正在渲染的尺寸标签，用来判断「换设备后要不要重新选素材」 */
const curTag = ref("");

function tagOf(w: number | null | undefined, h: number | null | undefined): string {
  if (w === 320 && h === 170) return "v1p";
  if (w === 320 && h === 240) return "v2";
  return "any";
}
/** 有效尺寸：没探到设备时沿用上次已知尺寸，都没有才按 320×240 兜底 */
const effSize = computed(() => devSize.value ?? lastSize.value ?? { w: 320, h: 240 });
// 只要探到真实尺寸就记下来（刷新/切设备时 devSize 会被清空，靠它维持布局不跳）
watch(devSize, (v) => {
  if (v) lastSize.value = v;
});
const selTag = computed(() => tagOf(effSize.value.w, effSize.value.h));
/** SN 尾号：去掉固定后缀 `ABCD`，再取剩余部分的最后 6 位（`112233ABCD` → `112233`） */
function sn6(sn: string | null | undefined): string {
  if (!sn) return "—";
  const s = sn.trim();
  const body = s.length > 4 && s.slice(-4).toUpperCase() === "ABCD" ? s.slice(0, -4) : s;
  return body.slice(-6);
}
/** 每台设备记住一次「尺寸 + 机型」。机型原本只靠上位机（GUI）的 `/usb/status` 富化，
 *  GUI 没连 / 设备刚插入时那份数据会缺 → 标签退化成只有 6 位数字。缓存兜底让型号始终显示。 */
function cacheDev(serial: string | null | undefined, w: number | null, h: number | null, variant: string | null): void {
  if (!serial) return;
  const has = !!variant || (!!w && !!h);
  if (!has) return;
  try {
    localStorage.setItem(`jiadian.dev.${serial}`, JSON.stringify({ w, h, variant }));
  } catch {
    /* 存储满等异常忽略 */
  }
}
function cachedDev(serial: string | null | undefined): { w: number | null; h: number | null; variant: string | null } | null {
  if (!serial) return null;
  try {
    const s = localStorage.getItem(`jiadian.dev.${serial}`);
    return s ? JSON.parse(s) : null;
  } catch {
    return null;
  }
}
/** 由尺寸推机型（320×170→V1P、320×240→V2；其它尺寸给 `W×H`，没有则空） */
function modelBySize(w: number | null | undefined, h: number | null | undefined): string {
  if (w === 320 && h === 170) return "V1P";
  if (w === 320 && h === 240) return "V2";
  return h ? `${w}×${h}` : "";
}
/** 机型：GUI variant → 尺寸推 → 本地缓存（不显示 170/240 这种裸数字） */
function modelOf(d: DeviceEntry): string {
  if (d.variant) return d.variant;
  const bySize = modelBySize(d.width, d.height);
  if (bySize) return bySize;
  const c = cachedDev(d.serial);
  if (c?.variant) return c.variant;
  return modelBySize(c?.w, c?.h);
}
function devLabel(d: DeviceEntry): string {
  const m = modelOf(d);
  return m ? `${sn6(d.serial)} · ${m}` : sn6(d.serial);
}
/** 按 SN 取显示名（`后六位 · 机型`）：在扫描列表里就用列表项，否则用缓存推机型 */
function labelOfSerial(sn: string | null | undefined): string {
  if (!sn) return "默认设备";
  const d = devices.value.find((x) => x.serial === sn);
  if (d) return devLabel(d);
  const c = cachedDev(sn);
  const m = c?.variant || modelBySize(c?.w, c?.h);
  return m ? `${sn6(sn)} · ${m}` : sn6(sn);
}
/** 主题在当前设备尺寸下该用哪个页面（无匹配 → null） */
function resolvePage(t: Theme): string | null {
  const tag = selTag.value;
  const byTag = t.variants.find((v) => v.tag === tag);
  if (byTag) return byTag.page;
  const sz = devSize.value;
  if (sz) {
    const bySize = t.variants.find((v) => v.width === sz.w && v.height === sz.h);
    if (bySize) return bySize.page;
  }
  const any = t.variants.find((v) => v.tag === "any");
  return any ? any.page : null;
}
function pageOf(name: string): string | null {
  const t = themes.value.find((x) => x.name === name);
  return t ? resolvePage(t) : null;
}
const selectedPage = computed(() => (selected.value ? pageOf(selected.value) : null));
/** 每尺寸记住上次用的主题（170 / 240 各一份） */
function getLastTheme(tag: string): string {
  return localStorage.getItem(`jiadian.lastTheme.${tag}`) || "";
}
function setLastTheme(tag: string, name: string): void {
  localStorage.setItem(`jiadian.lastTheme.${tag}`, name);
}
/** 上次用的设备（SN）——下次启动默认选它 */
function getLastDevice(): string {
  return localStorage.getItem("jiadian.lastDevice") || "";
}
function setLastDevice(sn: string | null | undefined): void {
  if (sn) localStorage.setItem("jiadian.lastDevice", sn);
}
let unlistenResize: (() => void) | undefined;
let unlistenClose: (() => void) | undefined;
let resizeTimer: number | undefined;
let previewLogged = false;

// 窗口尺寸写日志：拖好后从 log/jiadian.log 里读，回写 tauri.conf.json 的整数宽高
async function logWindowSize(tag: string) {
  try {
    const [size, sf] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()]);
    const lw = Math.round(size.width / sf);
    const lh = Math.round(size.height / sf);
    logEvent(`窗口尺寸（${tag}）：${lw}×${lh} 逻辑 / ${size.width}×${size.height} 物理 / scale=${sf}`);
  } catch {
    /* 忽略 */
  }
}

function logEvent(msg: string) {
  invoke("log_event", { msg }).catch(() => {});
}

// JDP1 + u64 sessionId + PNG; reject stale sessions before and after bitmap decoding.
async function drawPreview(buf: ArrayBuffer) {
  await previewRenderer.draw(buf);
}

// ---------------- 数据源自检 ----------------
// 启动后查一次：LibreHardwareService 不在跑就弹窗引导（未安装 → 一键安装）。
interface LhwsState {
  version: string;
  installed: boolean;
  running: boolean;
  /** SCM 真实服务状态 */
  serviceState: string;
  /** 是否在发布数据（共享内存存在） */
  publishing: boolean;
  /** 合并态：notInstalled / stopped / starting / running / unknown */
  status: string;
  /** 中文文案 */
  statusText: string;
  source: string;
  license: string;
  licenseUrl: string;
  msi: string | null;
}
const srcDlg = ref(false);
const srcState = ref<LhwsState | null>(null);
const srcBusy = ref(false);
const srcMsg = ref("");

/// 弹窗标题：把"没装"和"装了但没跑"分开说清楚
function srcTitle(s: LhwsState | null): string {
  if (!s) return "LibreHardwareService 不可用";
  switch (s.status) {
    case "notInstalled":
      return "未安装 LibreHardwareService";
    case "stopped":
      return "LibreHardwareService 已停止";
    case "starting":
      return "LibreHardwareService 数据未就绪";
    default:
      return "LibreHardwareService 不可用";
  }
}

/// 弹窗正文
function srcTip(s: LhwsState | null): string {
  if (!s) return "";
  switch (s.status) {
    case "notInstalled":
      return "CPU / GPU / 内存 / 温度 / 风扇这些指标靠它提供，现在读不到。点「一键安装」即可恢复（会弹一次 UAC）。";
    case "stopped":
      return "组件装好了，但它的服务进程没在运行 —— 常见原因是开机时它与网卡初始化撞车而退出，之后不会被自动拉起。点「重新启动」即可恢复（会弹一次 UAC）。";
    case "starting":
      return "服务进程已经起来了，但还没发布数据（初始化通常 5~7 秒）。先稍等几秒「重新检测」；一直不行就「重新启动」。";
    default:
      return "读不到它的运行状态，可以先「重新检测」，或直接「重新启动」（会弹一次 UAC）。";
  }
}

async function checkDataSource() {
  try {
    const en = await invoke<{ lhws: boolean }>("data_sources");
    if (!en.lhws) return;
    const s = await invoke<LhwsState>("lhws_state");
    srcState.value = s;
    if (!s.running) {
      srcMsg.value = "";
      srcDlg.value = true;
      logEvent(`数据源自检：LibreHardwareService status=${s.status}（service=${s.serviceState} publishing=${s.publishing}）→ 弹窗引导`);
    }
  } catch (e) {
    logEvent(`数据源自检失败：${e}`);
  }
}

async function installDataSource() {
  srcBusy.value = true;
  srcMsg.value = "";
  watchProgress();
  try {
    srcState.value = await invoke<LhwsState>("lhws_install");
    srcMsg.value = srcState.value.running ? "" : "已安装，但服务还没在发布数据 —— 可点「重新启动」。";
    if (srcState.value.running) {
      srcDlg.value = false;
      logEvent("数据源已就绪");
      refresh();
    }
  } catch (e) {
    srcMsg.value = `安装失败：${e}`;
  } finally {
    unwatchProgress();
    srcBusy.value = false;
  }
}

/// 重新启动服务（停 → 起），用于"装好了但没在跑"的情况
async function restartDataSource() {
  srcBusy.value = true;
  srcMsg.value = "正在重新启动服务（会弹一次 UAC，请确认）…";
  try {
    srcState.value = await invoke<LhwsState>("lhws_restart");
    srcMsg.value = "";
    srcDlg.value = false;
    logEvent("数据源已重新启动");
    refresh();
  } catch (e) {
    srcMsg.value = String(e);
    // 失败也要拉一次真实状态，别让弹窗显示过期信息
    try {
      srcState.value = await invoke<LhwsState>("lhws_state");
    } catch {
      /* 忽略 */
    }
  } finally {
    srcBusy.value = false;
  }
}

async function recheckDataSource() {
  try {
    const s = await invoke<LhwsState>("lhws_state");
    srcState.value = s;
    if (s.running) {
      srcDlg.value = false;
      srcMsg.value = "";
      logEvent("数据源已就绪（重新检测）");
      refresh();
    } else {
      srcMsg.value = `仍未就绪：${s.statusText}。`;
    }
  } catch (e) {
    srcMsg.value = String(e);
  }
}

function openSrcUrl(url: string) {
  if (url) invoke("open_url", { url }).catch(() => {});
}

// 全局禁止复制 / 刷新
function blockKey(e: KeyboardEvent) {
  const k = e.key.toLowerCase();
  if (k === "f5" || ((e.ctrlKey || e.metaKey) && ["r", "c", "x", "a"].includes(k))) {
    e.preventDefault();
    e.stopPropagation();
  }
}
function blockEvt(e: Event) {
  e.preventDefault();
}
async function loadThemes(token: number): Promise<boolean> {
  try {
    const next = await withTimeout(invoke<Theme[]>("list_themes"), 15000, "读取主题");
    if (!intent.current(token)) return false;
    themes.value = next;
    if (!selected.value || !next.some((x) => x.name === selected.value)) {
      selected.value = next.find((x) => resolvePage(x))?.name || next[0]?.name || "";
    }
    return true;
  } catch (e) {
    if (intent.current(token)) renderMsg.value = `读取主题失败：${e}`;
    return false;
  }
}
async function clearMissingDevice(sn: string, token: number) {
  // Always request backend cleanup first, even when the missing device was only a new selection.
  await cleanupMissingDevice({
    stop: () => releaseLease("设备断开"),
    current: () => intent.current(token),
    clear: () => {
      infoRevision++;
      previewRenderer.invalidate();
      info.value = null;
      devSize.value = null;
      selSerial.value = ""; // never silently redirect an old operation to another device
      infoMsg.value = `未连接：设备 ${sn6(sn)} 已断开，请重新插好后刷新`;
      renderMsg.value = `${infoMsg.value}；${session.view().message}`;
    },
  });
}
async function startRender(token: number) {
  if (!intent.current(token)) return;
  // The caller owns busy before theme/scan awaits; stop invalidates the entire preparation.
  if (!await loadThemes(token) || !intent.current(token)) return;
  if (!await loadDevices(token) || !intent.current(token)) return;
  const serial = selSerial.value;
  if (serial && !devices.value.some((d) => d.serial === serial)) {
    await clearMissingDevice(serial, token);
    return;
  }
  if (!selected.value) { renderMsg.value = "先选一个主题"; return; }
  const page = pageOf(selected.value);
  if (!page) { renderMsg.value = `该主题没有 ${effSize.value.w}×${effSize.value.h} 的素材`; return; }
  if (!session.view().canStart) {
    // Replacing a known running theme is supported, but unknown/starting/stopping is not takeover permission.
    if (!session.view().known || session.view().state?.phase !== "running") {
      renderMsg.value = "后台尚未确认释放，可继续点「停止」";
      return;
    }
    if (!await releaseLease("切换主题") || !intent.current(token)) return;
  }
  if (!intent.current(token) || !session.view().canStart || serial !== selSerial.value) return;
  const name = selected.value;
  const tag = selTag.value;
  htmlPath.value = `assets/${page}`;
  const started = await session.start({ html: htmlPath.value, serial: serial || null,
    width: effSize.value.w, height: effSize.value.h });
  if (!intent.current(token) || !started) return;
  curTag.value = tag;
  setLastTheme(tag, name);
  localStorage.setItem("jiadian.lastTheme", name);
  logEvent(`render_start 已确认：${name}（${labelOfSerial(serial)}）`);
}
async function chooseSelected() {
  if (!selected.value || !canChoose.value) return;
  await runIntent(startRender);
}
async function importPanel() {
  if (!canChoose.value) return;
  await runIntent(async (token) => {
    const f = await open({ multiple: false,
      filters: [{ name: "HTML 面板", extensions: ["html", "htm"] }],
      defaultPath: assetsDir.value || undefined });
    if (!intent.current(token) || typeof f !== "string" || !f) return;
    const t = await withTimeout(invoke<Theme>("import_panel", { src: f }), 30000, "导入素材");
    if (!intent.current(token) || !await loadThemes(token) || !intent.current(token)) return;
    selected.value = t.name;
    await startRender(token);
  });
}
async function releaseTakeover() {
  intent.invalidate();
  infoRevision++;
  scanRevision++;
  busy.value = false;
  previewRenderer.invalidate();
  logEvent("点击「停止」：取消接管");
  await releaseLease("停止");
}

// ---------------- 设备参数 ----------------
const info = ref<DeviceInfo | null>(null);
const infoMsg = ref("");
const bright = ref(200);
const rot = ref(0);
const followOff = ref(false);
const ctlBusy = ref(false);

// Never treat an unknown state as permission to open a USB handle.
async function waitLinkReady(serial: string, valid: () => boolean, timeoutMs = 8000): Promise<boolean> {
  const t0 = performance.now();
  while (valid() && performance.now() - t0 < timeoutMs) {
    const view = session.view();
    if (view.canStart) return true;
    if (!view.known || view.state?.serial !== serial || view.state.phase === "stopping") return false;
    if (view.state.phase === "running" && view.state.ready) return true;
    await session.refresh();
    if (valid()) await new Promise((resolve) => setTimeout(resolve, 120));
  }
  return false;
}
async function loadInfo(token: number) {
  const serial = selSerial.value;
  const revision = ++infoRevision;
  const valid = () => intent.current(token) && revision === infoRevision && serial === selSerial.value;
  if (!serial || !valid()) return;
  infoMsg.value = "读取中…";
  if (!await waitLinkReady(serial, valid) || !valid()) {
    if (valid()) infoMsg.value = "后台尚未就绪或状态未知，请稍后刷新（不会尝试抢占设备）";
    return;
  }
  try {
    const d = await withTimeout(invoke<DeviceInfo>("device_info", { serial }), 20000, "读取设备信息");
    if (!valid()) return;
    if (d.serial && d.serial !== serial) throw new Error("设备信息序列号不匹配");
    info.value = d;
    if (d.brightness !== null) bright.value = d.brightness;
    if (d.rotation !== null) rot.value = d.rotation;
    followOff.value = d.follow_off === 1;
    if (d.width && d.height) devSize.value = { w: d.width, h: d.height };
    cacheDev(serial, d.width, d.height, d.hw);
    infoMsg.value = "";
    logEvent(`设备信息：${sn6(serial)} · ${d.hw} 帧数=${d.frames}`);
  } catch (e) {
    if (!valid()) return;
    info.value = null;
    infoMsg.value = `读取失败：${e}；请刷新确认设备是否仍连接`;
  }
}
const canConfigure = computed(() => {
  const view = sessionView.value;
  return !busy.value && !ctlBusy.value && !!selSerial.value && info.value?.serial === selSerial.value
    && (view.canStart || (view.known && view.state?.phase === "running"
      && view.state.serial === selSerial.value && view.state.ready));
});
// Snapshot the explicit serial and all fields before invoking: no default-device fallback after an await.
async function applyConfig() {
  if (!canConfigure.value) return;
  const serial = selSerial.value;
  const revision = infoRevision;
  const args = { serial, brightness: bright.value, rotation: rot.value, followOff: followOff.value };
  ctlBusy.value = true;
  try {
    await withTimeout(invoke("device_apply_config", args), 20000, "下发配置");
    if (!appDisposed && revision === infoRevision && serial === selSerial.value) logEvent(`下发配置 ${sn6(serial)} 成功`);
  } catch (e) {
    if (!appDisposed && revision === infoRevision && serial === selSerial.value) infoMsg.value = `下发配置失败：${e}`;
  } finally {
    ctlBusy.value = false;
  }
}

// ---------------- 设备扫描 / 切换 ----------------
/// 扫描设备：本地 WinUSB 物理枚举 + GUI `/usb/status` 的 devices[] 富化（尺寸/机型）。
async function loadDevices(token: number): Promise<boolean> {
  const revision = ++scanRevision;
  try {
    const raw = await withTimeout(invoke<DeviceEntry[]>("device_list"), 15000, "扫描设备");
    if (!intent.current(token) || revision !== scanRevision) return false;
    const list = raw.filter((d) => !!d.serial && !(d.path === null && !d.connected));
    for (const d of list) cacheDev(d.serial, d.width, d.height, d.variant);
    devices.value = list;
    // Pick an initial default only while confirmed idle. A missing selection must go through cleanup.
    if (!selSerial.value && session.view().canStart) {
      selSerial.value = (list.find((d) => d.serial === getLastDevice()) || list[0])?.serial || "";
    }
    if (list.some((d) => d.serial === selSerial.value)) setLastDevice(selSerial.value);
    logEvent(`扫描设备：${list.length} 台（${list.map((d) => devLabel(d)).join("、") || "无"}）`);
    return true;
  } catch (e) {
    // Enumeration failure is not evidence of absence: preserve both list and selection.
    if (intent.current(token) && revision === scanRevision) {
      infoMsg.value = `扫描失败（保留原设备）：${e}`;
      logEvent(infoMsg.value);
    }
    return false;
  }
}
async function resolveDevSize(token: number): Promise<void> {
  if (!intent.current(token)) return;
  const d = devices.value.find((x) => x.serial === selSerial.value);
  if (d?.width && d?.height) { devSize.value = { w: d.width, h: d.height }; return; }
  if (!selSerial.value) { devSize.value = null; return; }
  await loadInfo(token);
}
async function refreshDevices(): Promise<void> {
  await runIntent(async (token) => {
    if (!await loadDevices(token) || !intent.current(token)) return;
    const sn = selSerial.value;
    if (sn && !devices.value.some((d) => d.serial === sn)) { await clearMissingDevice(sn, token); return; }
    await loadInfo(token);
  });
}
async function onDeviceChange(): Promise<void> {
  await runIntent(async (token) => {
    const sn = selSerial.value;
    if (!await loadDevices(token) || !intent.current(token)) return;
    if (sn && !devices.value.some((d) => d.serial === sn)) {
      await clearMissingDevice(sn, token);
      return;
    }
    if (!session.view().canStart && !await releaseLease("切换设备")) return;
    if (!intent.current(token) || !session.view().canStart || sn !== selSerial.value) return;
    setLastDevice(sn);
    previewRenderer.invalidate();
    info.value = null;
    devSize.value = null;
    await resolveDevSize(token);
    if (!intent.current(token)) return;
    const tag = selTag.value;
    if (tag !== curTag.value) {
      const last = getLastTheme(tag);
      selected.value = last && themes.value.some((x) => x.name === last && resolvePage(x))
        ? last : themes.value.find((x) => resolvePage(x))?.name || "";
    }
    await startRender(token);
    if (intent.current(token)) await loadInfo(token);
  });
}
// Startup is itself one guarded intent, including theme and device discovery.
async function boot(token: number) {
  if (!await session.refresh() || !intent.current(token)) return;
  if (!await loadThemes(token) || !await loadDevices(token) || !intent.current(token)) return;
  await resolveDevSize(token);
  if (!intent.current(token)) return;
  const tag = selTag.value;
  const saved = getLastTheme(tag) || localStorage.getItem("jiadian.lastTheme") || "";
  const pick = saved && themes.value.some((x) => x.name === saved && resolvePage(x))
    ? saved : themes.value.find((x) => resolvePage(x))?.name || "";
  if (!pick) { logEvent(`没有可用于 ${tag} 的主题`); return; }
  selected.value = pick;
  await startRender(token);
  if (intent.current(token)) await loadInfo(token);
}

onMounted(async () => {
  const initialToken = intent.begin();
  busy.value = true;
  // 先读设置（`settings.ini`）：主题、单位、帧率档位、静默启动都在里面，后面 boot() 要用
  await loadSettings();
  if (appDisposed) return;
  // 窗口主题不在这里设：`useSettings` 里有 watch(theme) 会跟着变（含初始化那次），
  // 这里再调一次只会多一次 IPC。
  loadAutostart();
  // 全局禁止复制 / 刷新（F5、Ctrl+R、右键菜单、copy/cut）
  window.addEventListener("keydown", blockKey, true);
  window.addEventListener("contextmenu", blockEvt, true);
  document.addEventListener("copy", blockEvt);
  document.addEventListener("cut", blockEvt);
  if (appDisposed) return;
  refresh();
  void pollSession();
  assetsDir.value = (await withTimeout(invoke<string | null>("assets_dir"), 3000, "素材目录").catch(() => null)) || "";
  if (appDisposed) return;
  if (intent.current(initialToken)) {
    try { await boot(initialToken); }
    catch (e) { if (intent.current(initialToken)) renderMsg.value = String(e); }
    finally { if (intent.current(initialToken)) busy.value = false; }
  }
  if (appDisposed) return;
  // 数据源自检：等启动流程安定后再查，LibreHardwareService 不在跑就弹窗引导
  setTimeout(checkDataSource, 2500);
  timer = window.setInterval(refresh, 1000);
  // 窗口尺寸日志：启动记一次；拖动停下 600ms 后记一次（供回写 tauri.conf.json）
  logWindowSize("启动");
  appWindow
    .onResized(() => {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => logWindowSize("拖动后"), 600);
    })
    .then((u) => {
      unlistenResize = u;
    });
  // 订阅预览通道：之后 Rust 每帧把 PNG 原始字节直接推过来（不走事件，避免逐帧 eval）
  invoke("preview_subscribe", { channel: previewCh }).catch(() => {
    logEvent("预览通道订阅失败");
  });
  // 系统关闭（X / Alt+F4 / 任务栏）：按设置收进托盘，或先归还设备再退出
  appWindow
    .onCloseRequested(async (e) => {
      e.preventDefault();
      await onClose();
    })
    .then((u) => {
      unlistenClose = u;
    });
});
onUnmounted(() => {
  appDisposed = true;
  intent.dispose();
  session.dispose();
  previewRenderer.dispose();
  infoRevision++;
  scanRevision++;
  pollingStopped = true;
  if (pollingTimer) clearTimeout(pollingTimer);
  metricsDisposed = true;
  refreshRevision++;
  if (timer) clearInterval(timer);
  if (resizeTimer) clearTimeout(resizeTimer);
  unlistenResize?.();
  unlistenClose?.();
  window.removeEventListener("keydown", blockKey, true);
  window.removeEventListener("contextmenu", blockEvt, true);
  document.removeEventListener("copy", blockEvt);
  document.removeEventListener("cut", blockEvt);
});
</script>

<template>
  <div class="app">
    <!-- 自定义标题栏（隐藏了 Windows 原生标题栏） -->
    <TitleBar @close="onClose" />


    <div class="main">
      <Sidebar
        :page="page"
        :theme="theme"
        @nav="goPage"
        @theme="theme = theme === 'dark' ? 'light' : 'dark'"
      />


    <main class="body">
      <p v-if="error" class="err">调用失败：{{ error }}</p>

      <!-- ============ 首页 ============ -->
      <div v-show="page === 'home'" class="cols">
        <!-- 左：设备参数 -->
        <section class="col">
          <div class="card">
            <h2>
              <Icon icon="line-md:computer" />
              <span class="ttl">设备</span>
              <span class="dot" :class="{ ok: !!info }" :title="info ? '已连接' : '未连接'"></span>
              <select
                class="devsel"
                v-model="selSerial"
                @change="onDeviceChange"
                :disabled="busy || ctlBusy"
                :title="selSerial || '未发现设备'"
              >
                <option v-if="!devices.length" value="">无设备</option>
                <option v-for="d in devices" :key="d.serial || d.path || ''" :value="d.serial || ''">
                  {{ devLabel(d) }}
                </option>
              </select>
              <button class="mini" @click="refreshDevices" :disabled="busy || ctlBusy">
                <Icon icon="line-md:backup-restore" />刷新
              </button>
            </h2>
            <!-- 常驻一行：v-if 会让提示出现/消失时整张卡高度跳一下（左列最后一张卡是 flex:1，
                 一跳整页都动）。空内容时靠 .hint.keep 的 min-height 占位。 -->
            <p class="hint keep">{{ infoMsg }}</p>
            <div class="row">
              <Icon icon="line-md:hash" /><span class="lbl">SN</span
              ><span class="val" :title="info?.serial || ''">{{ info ? sn6(info.serial) : "—" }}</span>
            </div>
            <div class="row">
              <Icon icon="line-md:computer" /><span class="lbl">机型</span
              ><span class="val">{{ info?.hw || "—" }}</span>
            </div>
            <div class="row">
              <Icon icon="line-md:monitor" /><span class="lbl">分辨率</span
              ><span class="val">{{ info ? `${info.width}×${info.height}` : "—" }}</span>
            </div>
            <div class="row">
              <Icon icon="line-md:folder" /><span class="lbl">Flash</span
              ><span class="val">{{
                info ? `${info.flash_model || info.flash_id} · ${info.flash_mbit} Mbit` : "—"
              }}</span>
            </div>
            <div class="row">
              <Icon icon="line-md:image" /><span class="lbl">可存帧数</span
              ><span class="val">{{
                info ? `${info.frames}（动画区 ${info.anim_mb} MB）` : "—"
              }}</span>
            </div>
            <div class="row">
              <Icon icon="line-md:lightbulb" /><span class="lbl">电压</span
              ><span class="val">{{ info ? `${info.millivolts} mV` : "—" }}</span>
            </div>
          </div>

          <div class="card">
            <h2><Icon icon="line-md:sunny" />显示</h2>
            <div class="row">
              <span class="lbl">亮度 {{ bright }}</span>
              <input
                class="rng"
                type="range"
                min="0"
                max="255"
                v-model.number="bright"
                @change="applyConfig"
                :disabled="!canConfigure"
              />
            </div>
            <div class="btns two">
              <button @click="bright = 0; applyConfig()" :disabled="!canConfigure">
                <Icon icon="line-md:monitor-off" />息屏
              </button>
              <button @click="bright = 255; applyConfig()" :disabled="!canConfigure">
                <Icon icon="line-md:sunny-filled" />最亮
              </button>
            </div>
            <div class="row">
              <Icon icon="line-md:rotate-90" /><span class="lbl">屏幕方向</span>
              <select v-model.number="rot" @change="applyConfig" :disabled="!canConfigure">
                <option :value="0">0°</option>
                <option :value="1">90°</option>
                <option :value="2">180°</option>
                <option :value="3">270°</option>
              </select>
            </div>
            <div class="row">
              <Icon icon="line-md:switch" /><span class="lbl">跟随息屏</span>
              <label class="sw">
                <input type="checkbox" v-model="followOff" @change="applyConfig" :disabled="!canConfigure" />
                <span>{{ followOff ? "开" : "关" }}</span>
              </label>
            </div>
            <p class="hint">改任一项都会完整下发一次配置。</p>
          </div>
        </section>

        <!-- 右：切换（上）+ 预览（下） -->
        <section class="col right">
          <div class="card">
            <h2><Icon icon="line-md:image" />主题切换</h2>
            <select v-model="selected" class="wide" :disabled="busy">
              <option v-for="t in themes" :key="t.name" :value="t.name">
                {{ t.name }}{{ pageOf(t.name) ? "" : "（缺此尺寸）" }}
              </option>
            </select>
            <div class="btns">
              <button class="primary" @click="chooseSelected" :disabled="busy || ctlBusy || !canChoose || !selectedPage">
                <Icon icon="line-md:confirm-circle" />确定
              </button>
              <button class="danger" @click="releaseTakeover" :disabled="!running && !busy && !sessionView.pending">
                <Icon icon="line-md:close-circle" />停止
              </button>
              <button @click="importPanel" :disabled="busy || ctlBusy || !canChoose">
                <Icon icon="line-md:file-import" />导入素材
              </button>
            </div>
            <p class="hint keep">{{
              selected && !selectedPage
                ? `「${selected}」没有 ${effSize.w}×${effSize.h} 的素材，换个主题或导入`
                : ""
            }}</p>
            <!-- 同上：常驻一行，避免下发结果反复出现/消失时卡片高度跳动 -->
            <p class="hint keep">{{ renderMsg }}</p>
          </div>
          <div class="card">
            <h2><Icon icon="line-md:monitor" />预览 {{ effSize.w }}×{{ effSize.h }}</h2>
            <div class="preview">
              <canvas
                v-show="hasPreview"
                ref="previewCanvas"
                class="pv-canvas"
                :style="{ width: effSize.w + 'px', height: effSize.h + 'px' }"
              ></canvas>
              <div
                v-if="!hasPreview"
                class="ph"
                :style="{ width: effSize.w + 'px', height: effSize.h + 'px' }"
              >
                选好主题点「确定」
              </div>
            </div>
          </div>
        </section>
      </div>

      <!-- ============ 性能 ============ -->
      <div v-show="page === 'perf'">
        <section v-for="s in sections" :key="s.title" class="card">
          <h2><Icon :icon="s.icon" />{{ s.title }}</h2>
          <ul>
            <li v-for="m in s.items" :key="m.key" :title="m.source">
              <span class="lbl">{{ m.label }}</span>
              <span class="val" :class="{ off: m.kind === 'number' && m.value === null }">
                {{ display(m) }}
              </span>
            </li>
          </ul>
        </section>
      </div>

      <!-- ============ 外设 ============ -->
      <PeripheralsPage v-show="page === 'peripherals'" />

      <!-- ============ 设置 ============ -->
      <SettingsPage v-show="page === 'settings'" />
      <MaterialsPage v-show="page === 'materials'" />
      </main>
    </div>

    <!-- 数据源自检：LibreHardwareService 不在跑时引导安装 -->
    <div v-if="srcDlg" class="modal-mask">
      <div class="modal">
        <h3>
          <Icon :icon="srcState?.status === 'notInstalled' ? 'line-md:cloud-alt-download' : 'line-md:alert'" />
          {{ srcTitle(srcState) }}
        </h3>
        <p class="hint">{{ srcTip(srcState) }}</p>
        <div v-if="lhwsProg?.active" class="prog">
          <div class="prog-head">
            <span class="grow">{{ lhwsProg.message }}</span>
            <span v-if="lhwsSize">{{ lhwsSize }}</span>
            <span v-if="lhwsSpeed">{{ lhwsSpeed }}</span>
          </div>
          <div class="prog-track">
            <div
              class="prog-bar"
              :class="{ indet: lhwsIndet, fail: lhwsProg.phase === 'failed' }"
              :style="lhwsIndet ? undefined : { width: lhwsPct + '%' }"
            ></div>
          </div>
          <div class="prog-head">
            <span class="grow">{{ lhwsAttempt }}</span>
            <span v-if="!lhwsIndet">{{ lhwsPct }}%</span>
          </div>
        </div>
        <p v-if="srcMsg" class="hint">{{ srcMsg }}</p>
        <p class="hint">
          当前状态：{{ srcState?.statusText }}（服务 {{ srcState?.serviceState }} · 数据 {{ srcState?.publishing ? "已发布" : "未发布" }}）
        </p>
        <p class="hint">
          第三方开源组件 {{ srcState?.version }}（{{ srcState?.license }}）·
          <a @click="openSrcUrl(srcState?.source || '')">来源</a> ·
          <a @click="openSrcUrl(srcState?.licenseUrl || '')">许可</a>
        </p>
        <div class="btns">
          <button
            v-if="srcState?.status === 'notInstalled'"
            class="primary"
            :disabled="srcBusy"
            @click="installDataSource"
          >
            {{ srcBusy ? "安装中…" : "一键安装" }}
          </button>
          <button v-else class="primary" :disabled="srcBusy" @click="restartDataSource">
            {{ srcBusy ? "处理中…" : "重新启动" }}
          </button>
          <button :disabled="srcBusy" @click="recheckDataSource">重新检测</button>
          <button :disabled="srcBusy" @click="srcDlg = false">稍后</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style src="./styles.css"></style>


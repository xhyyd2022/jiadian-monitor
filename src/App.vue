<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from "vue";
import { Channel, invoke } from "@tauri-apps/api/core";
import { ElMessage } from "element-plus";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon, addCollection } from "@iconify/vue";
import { useSettings } from "./composables/useSettings";
import { useOnline, urlFor } from "./composables/useOnline";
import { useAppearance } from "./composables/useAppearance";
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

/** 下拉项的统一结构：Element Plus 用 label/value 渲染。 */
interface UIOption {
  value: string | number;
  label: string;
  disabled?: boolean;
}

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
  // 进入素材页（含在线素材管理）时重扫 online/：手改 TOML 后不用重启程序
  if (page.value === "materials") void refreshOnline();
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
// 启动时应用用户外观（背景/强调色/透明度），并保持与 settings.ini 同步。
useAppearance();

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
      // Element Plus 的暗色变量靠 html.dark；两边同步，避免 EP 组件与自绘控件主题不一致。
      root.classList.toggle("dark", t === "dark");
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
/// 最近一次成功采样的时间戳：性能页就地标注「最后更新 / 旧值」，不改变采集逻辑
const metricsAt = ref(0);
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
    metricsAt.value = Date.now();
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
interface MetricSection {
  title: string;
  icon: string;
  items: BasicMetric[];
}
const sections = computed<MetricSection[]>(() => {
  const out: MetricSection[] = [];
  for (const m of metrics.value) {
    const last = out[out.length - 1];
    if (last && last.title === m.group) last.items.push(m);
    else out.push({ title: m.group, icon: GROUP_ICON[m.group] || "line-md:list", items: [m] });
  }
  return out;
});
// ---------------- 性能页：核心指标 + 分组详情（P02–P04） ----------------
type PerfKind = "cpu" | "gpu" | "mem" | "disk" | "net" | "fan" | "sys" | "plain";
interface CoreView {
  m: BasicMetric;
  label: string;
  note: string;
  title: string;
}
interface DiskRowView {
  key: string;
  drive: string;
  title: string;
  temp: BasicMetric | null;
  pct: BasicMetric | null;
  used: BasicMetric | null;
  total: BasicMetric | null;
  aggregate: boolean;
}
interface NetRowView {
  key: string;
  name: string;
  title: string;
  down: BasicMetric | null;
  up: BasicMetric | null;
}
interface PerfGroupView {
  title: string;
  icon: string;
  subtitle: string;
  kind: PerfKind;
  core: CoreView[];
  secondary: { label: string; m: BasicMetric }[];
  disks: DiskRowView[];
  nets: NetRowView[];
  details: BasicMetric[];
  detailNote: string;
}
const PERF_KIND: Record<string, PerfKind> = {
  CPU: "cpu", GPU: "gpu", 内存: "mem", 磁盘: "disk", 网络: "net", 风扇: "fan", 系统: "sys",
};
/** 读不到与真实 0 分开：只有数值型且 value=null 才算缺失，界面显示“—”；0 仍按正常值展示。 */
function isMissing(m: BasicMetric | null): boolean {
  return !!m && m.kind === "number" && m.value === null;
}
/** 显存占用率（LHS GPU Memory）描述的是活动/负载，不是容量占比：统一改口，避免误导。 */
function metricLabel(m: BasicMetric): string {
  return m.key.endsWith("_mem_pct") ? m.label.replace("显存占用率", "显存活动率") : m.label;
}
function metricTitle(m: BasicMetric): string {
  if (m.key.endsWith("_mem_pct")) {
    return `${m.source}｜显存活动率：LHS GPU Memory 给出的显存负载/活动读数，不等于「已用/总量」的容量占比`;
  }
  return m.source;
}
function parenName(label: string): string {
  const hit = label.match(/[（(]([^（）()]+)[)）]/);
  return hit ? hit[1].trim() : "";
}
function bareName(label: string): string {
  return label.replace(/\s*[（(][^（）()]*[)）]/g, " ").trim();
}
function coreOf(m: BasicMetric | null, label?: string, note?: string): CoreView | null {
  return m ? { m, label: label || m.label, note: note || "", title: metricTitle(m) } : null;
}
function coreFor(kind: PerfKind, items: BasicMetric[], used: Set<string>): CoreView[] {
  const take = (key: string): CoreView | null => {
    const m = items.find((x) => x.key === key);
    if (!m) return null;
    used.add(m.key);
    return coreOf(m);
  };
  const picked: (CoreView | null)[] = [];
  if (kind === "cpu") picked.push(take("cpu_load"), take("cpu_temp"));
  else if (kind === "gpu") picked.push(take("gpu1_load"), take("gpu1_temp"));
  else if (kind === "mem") picked.push(take("mem_pct"));
  else if (kind === "sys") picked.push(take("sys_uptime"));
  else if (kind === "fan") {
    // 风扇：主区先给前两路，其余进详情；通道名做副说明
    const fans = items
      .filter((x) => /^fan\d+$/.test(x.key))
      .sort((a, b) => Number(a.key.slice(3)) - Number(b.key.slice(3)));
    for (const f of fans.slice(0, 2)) {
      used.add(f.key);
      picked.push(coreOf(f, bareName(f.label), parenName(f.label)));
    }
  }
  const core = picked.filter((x): x is CoreView => !!x);
  if (core.length) return core;
  // 磁盘 / 网络用结构化行展示主数据（按盘符、按网卡），不再把前两个数值重复成核心指标
  if (kind === "disk" || kind === "net") return [];
  // 未知分组兜底：先给前两个数值指标，其余进详情，保证一项都不丢
  for (const m of items) {
    if (core.length >= 2) break;
    if (m.kind !== "number" || used.has(m.key)) continue;
    used.add(m.key);
    core.push({ m, label: m.label, note: "", title: metricTitle(m) });
  }
  return core;
}
/** 磁盘按盘符成行：温度 + 已用/总量 + 占比 同一行，合计单列一行 */
function diskRowsOf(items: BasicMetric[], used: Set<string>): DiskRowView[] {
  const map = new Map<string, DiskRowView>();
  for (const m of items) {
    const hit = m.key.match(/^disk(\d+)_(temp|pct|used|total)$/);
    if (!hit) continue;
    const n = hit[1];
    const row = map.get(n) || {
      key: n, drive: "", title: "", temp: null, pct: null, used: null, total: null, aggregate: false,
    };
    row[hit[2] as "temp" | "pct" | "used" | "total"] = m;
    row.drive = row.drive || parenName(m.label) || `磁盘${n}`;
    row.title = row.title || bareName(m.label);
    used.add(m.key);
    map.set(n, row);
  }
  const rows = [...map.values()].sort((a, b) => Number(a.key) - Number(b.key));
  return rows;
}
/** 网络按网卡分组：主区只放“当前速率”，累计流量进详情，两种口径不混 */
function netRowsOf(items: BasicMetric[], used: Set<string>): NetRowView[] {
  const map = new Map<string, NetRowView>();
  for (const m of items) {
    const hit = m.key.match(/^net(\d+)_(down|up|total_down|total_up)$/);
    if (!hit) continue;
    const n = hit[1];
    const row = map.get(n) || { key: n, name: `网卡${n}`, title: "", down: null, up: null };
    if (hit[2] === "down") { row.down = m; used.add(m.key); }
    else if (hit[2] === "up") { row.up = m; used.add(m.key); }
    const name = parenName(m.label);
    if (name) row.name = name;
    row.title = row.title || bareName(m.label);
    map.set(n, row);
  }
  return [...map.values()].sort((a, b) => Number(a.key) - Number(b.key));
}
function buildPerfGroup(sec: MetricSection): PerfGroupView {
  const kind = PERF_KIND[sec.title] || "plain";
  const used = new Set<string>();
  const g: PerfGroupView = {
    title: sec.title, icon: sec.icon, subtitle: "", kind,
    core: [], secondary: [], disks: [], nets: [], details: [], detailNote: "",
  };
  // 型号（文本型 *_name）移到分组副标题，不再和数值抢同一层级
  const model = sec.items.find((m) => m.kind === "text" && m.key.endsWith("_name"));
  if (model) {
    g.subtitle = model.text;
    used.add(model.key);
  }
  g.core = coreFor(kind, sec.items, used);
  if (kind === "mem") {
    for (const [key, label] of [["mem_used", "已用"], ["mem_total", "总量"]] as const) {
      const m = sec.items.find((x) => x.key === key);
      if (!m || used.has(key)) continue;
      used.add(key);
      g.secondary.push({ label, m });
    }
  }
  if (kind === "disk") g.disks = diskRowsOf(sec.items, used);
  if (kind === "net") g.nets = netRowsOf(sec.items, used);
  if (kind === "gpu") {
    g.detailNote = "显存活动率来自 LHS GPU Memory，表示显存负载/活动程度；容量占比请看「显存已用 / 显存总量」。";
  }
  g.details = sec.items.filter((m) => !used.has(m.key));
  return g;
}
const perfGroups = computed<PerfGroupView[]>(() => sections.value.map((sec) => buildPerfGroup(sec)));
/** 每张卡收起时的大致高度：只用于两列均衡，不参与渲染 */
function perfGroupHeight(g: PerfGroupView): number {
  let h = 37 + 17;
  if (g.core.length) h += 44;
  if (g.secondary.length) h += 22;
  if (g.disks.length) h += g.disks.length * 23;
  if (g.nets.length) h += g.nets.length * 46;
  if (g.details.length) h += 26;
  return h;
}
/// 性能页按估算高度做双列均衡：不把磁盘卡和风扇卡强行拉成等高，每列独立堆叠。
const perfColumns = computed<PerfGroupView[][]>(() => {
  const cols: PerfGroupView[][] = [[], []];
  const heights = [0, 0];
  for (const g of perfGroups.value) {
    const i = heights[0] <= heights[1] ? 0 : 1;
    cols[i].push(g);
    heights[i] += perfGroupHeight(g) + 12;
  }
  return cols;
});
const perfOpen = ref<Record<string, boolean>>({});
function togglePerf(title: string): void {
  perfOpen.value = { ...perfOpen.value, [title]: !perfOpen.value[title] };
}
function barWidth(m: BasicMetric | null): string {
  const v = typeof m?.value === "number" && Number.isFinite(m.value) ? m.value : 0;
  return `${Math.max(0, Math.min(100, v))}%`;
}
// ---------------- 面板下发 ----------------
const themes = ref<Theme[]>([]);
const selected = ref("");
const htmlPath = ref("");
const assetsDir = ref("");
const renderMsg = ref("");
const running = ref(false);
const busy = ref(false);
/// 正在运行的素材 / 来源：只用于首页状态行与预览来源行（纯展示，不参与下发逻辑）
const runningName = ref("");
const runningMode = ref<"local" | "online">("local");
const runningKey = ref("");
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
    // 运行中换主题会经历 running→starting→running：中间 previewSession 会短暂为 null，
    // 这时保留上一帧，等新会话首帧覆盖；只有回到 idle（停止/断开）才真正清空画布。
    if (oldPreview !== view.previewSession && view.previewSession === null && view.state?.phase === "idle") {
      previewRenderer.invalidate();
    }
  },
});
const sessionView = ref(session.view());
/// 运行中但 USB 链路失效 → 状态行明确提示「正在自动重连」，不再只显示「运行中」。
const linkReconnecting = computed(
  () => sessionView.value.state?.phase === "running" && !!sessionView.value.state?.reconnecting,
);
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
const info = ref<DeviceInfo | null>(null);
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
/// 本地主题下拉选项（缺当前尺寸页面的也标出来；点启动会被 canConfirm 拦住）
const themeOptions = computed<UIOption[]>(() =>
  themes.value.map((t) => ({
    value: t.name,
    label: `${t.name}${pageOf(t.name) ? "" : "（缺此尺寸）"}`,
  })),
);
/// 设备下拉选项（无设备时保留「无设备」占位）
const deviceOptions = computed<UIOption[]>(() => {
  const model = info.value;
  const list = devices.value.map((d) => {
    // GUI 富化缺失时 device_list 只有 SN；device_info 回来后补上机型，避免下拉一直只有编号。
    const known = model && model.serial === d.serial && model.hw;
    return { value: d.serial || "", label: known ? `${sn6(d.serial)} · ${model.hw}` : devLabel(d) };
  });
  return list.length ? list : [{ value: "", label: "无设备" }];
});

// ---------------- 在线素材 / 本地素材 ----------------
// 「主题切换」卡有两种模式：本地 = `assets/` 下打包的 HTML，在线 = 用户添加的完整 URL。
// 二者共用同一条渲染管线（`render_start`）：后端只看 `html` 是否以 http(s) 开头。
// 在线列表与侧栏「在线」页共用 useOnline 的共享状态，保存/删除后两边同时刷新。
const themeMode = ref<"local" | "online">("local");
/// 当前选中的在线素材 id（= 后端 slug）
const onlineSelected = ref("");
/// 显式选择（下拉 change / 素材页“应用”）但尚未应用成功的素材；分段浏览不写这两个值
const pendingKey = ref("");
const pendingName = ref("");
const autoErr = ref("");
const {
  sources: onlineSources,
  loading: onlineLoading,
  error: onlineError,
  loaded: onlineLoaded,
  refresh: refreshOnline,
} = useOnline();
const onlineSel = computed(() => onlineSources.value.find((s) => s.id === onlineSelected.value) || null);
/// 在线素材下拉选项：只显示标题，缺当前尺寸 URL 的标出来
const onlineOptions = computed<UIOption[]>(() =>
  onlineSources.value.map((s) => ({
    value: s.id,
    label: `${s.title}${urlFor(s, selTag.value) ? "" : "（缺此尺寸）"}`,
  })),
);
/// 素材页导入成功后：重扫主题列表并把新导入主题选上，回首页立即能选。
function onMaterialImported(name: string): void {
  void runIntent(async (token) => {
    if (await loadThemes(token) && intent.current(token)) {
      selected.value = name;
      renderMsg.value = `已导入素材「${name}」`;
    }
  });
}
/// 素材页「应用」：切到本地素材、选中该主题，并直接走“选择即应用”同一条路径
function applyThemeFromMaterials(name: string): void {
  themeMode.value = "local";
  selected.value = name;
  goPage("home");
  void autoApplySelection();
}
/// 当前设备尺寸对应的在线 URL；缺该尺寸的 URL 时是 null（此时「启动」禁用）
const onlineUrl = computed(() => (onlineSel.value ? urlFor(onlineSel.value, selTag.value) : null));
/// 在线模式下拉下方的一行提示：出错 / 缺尺寸 / 空列表都在这说清楚
const onlineHint = computed(() => {
  if (onlineError.value) return onlineError.value;
  if (onlineLoading.value && !onlineSources.value.length) return "正在读取在线素材…";
  if (!onlineSources.value.length) return "还没有在线素材，去「素材 → 在线素材」添加";
  if (!onlineSel.value) return "先选一个在线素材";
  if (!onlineUrl.value) {
    return `「${onlineSel.value.title}」没有 ${effSize.value.w}×${effSize.value.h} 的 URL，去「素材 → 在线素材」补上`;
  }
  return "在「素材 → 在线素材」管理在线素材";
});

function toggleTheme(): void {
  theme.value = theme.value === "dark" ? "light" : "dark";
}
function onModeChange(v: string | number | boolean): void {
  if (v === "local" || v === "online") void setMode(v);
}
/// 「启动」是否可点：本地看当前尺寸有没有页面，在线看当前尺寸有没有 URL
const canConfirm = computed(() => (themeMode.value === "local" ? !!selectedPage.value : !!onlineUrl.value));
/// 列表刷新后如果选中项被删掉/改名，自动落到上次用过的或第一个（不让下拉悬空）
watch(onlineSources, (list) => {
  if (list.some((s) => s.id === onlineSelected.value)) return;
  const last = localStorage.getItem("jiadian.lastOnline") || "";
  onlineSelected.value = list.some((s) => s.id === last) ? last : list[0]?.id || "";
});
/// 切换本地/在线模式：进在线模式时重扫 `online/`（手改 TOML 也能马上看到）
async function setMode(mode: "local" | "online") {
  if (themeMode.value === mode) return;
  themeMode.value = mode;
  // 分段只切“浏览来源”：清掉显式选择留下的待应用/失败态，不自动应用（H01）
  pendingKey.value = "";
  pendingName.value = "";
  autoErr.value = "";
  if (mode === "online") void refreshOnline();
}

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
/// 文本输入框（含 el-input 内部的 input / textarea / 可编辑区）保留系统右键菜单与
/// Ctrl+C / Ctrl+V / Ctrl+X / Ctrl+A —— 只拦截输入框以外的按键与右键，避免连带屏蔽粘贴。
function isTextField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || el.isContentEditable === true;
}
function blockKey(e: KeyboardEvent) {
  if (isTextField(e.target)) return;
  const k = e.key.toLowerCase();
  if (k === "f5" || ((e.ctrlKey || e.metaKey) && ["r", "c", "x", "a"].includes(k))) {
    e.preventDefault();
    e.stopPropagation();
  }
}
function blockEvt(e: Event) {
  if (isTextField(e.target)) return;
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
/// USB 重新枚举期间，device_list 偶尔会漏一台设备；单次枚举失败不足以判定“设备已断开”。
/// 参考 Windows USB 处理经验（PnP 通知与 I/O 错误双通道、不要把单次失败当终点）：
/// 这里先等一个短暂窗口再强制复核一次，只有确认真没了才走断开清理。
async function deviceGoneAfterGrace(token: number): Promise<boolean> {
  if (!intent.current(token)) return false;
  await new Promise((r) => window.setTimeout(r, 800));
  if (!intent.current(token)) return false;
  if (!await loadDevices(token, true)) return false; // 枚举本身失败：状态未知，保持现状
  if (!intent.current(token)) return false;
  return !!selSerial.value && !devices.value.some((d) => d.serial === selSerial.value);
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
  // 本地模式重新扫主题；在线模式只保证列表读过一次（进在线模式时已刷新过）。
  if (themeMode.value === "local") {
    if (!await loadThemes(token) || !intent.current(token)) return;
  } else if (!onlineLoaded.value) {
    await refreshOnline();
    if (!intent.current(token)) return;
  }

  const serial = selSerial.value;
  if (serial && !devices.value.some((d) => d.serial === serial)) {
    const gone = await deviceGoneAfterGrace(token);
    if (!intent.current(token)) return;
    if (gone) {
      // 复核确认设备真的不在了 → 走断开清理
      await clearMissingDevice(serial, token);
      return;
    }
    // 复核发现设备还在（USB 枚举抖动）→ 继续正常下发
  }
  // 选出这次要下发的东西：本地 = `assets/` 下的页面；在线 = 当前尺寸对应的完整 URL。
  let html: string;
  let material: string;
  let onlineId = "";
  if (themeMode.value === "local") {
    if (!selected.value) { renderMsg.value = "先选一个主题"; return; }
    const resolved = pageOf(selected.value);
    if (!resolved) { renderMsg.value = `该主题没有 ${effSize.value.w}×${effSize.value.h} 的素材`; return; }
    html = `assets/${resolved}`;
    material = selected.value;
  } else {
    const src = onlineSel.value;
    if (!src) { renderMsg.value = "先选一个在线素材"; return; }
    const url = urlFor(src, selTag.value);
    if (!url) { renderMsg.value = `「${src.title}」没有 ${effSize.value.w}×${effSize.value.h} 的 URL`; return; }
    html = url;
    material = src.title;
    onlineId = src.id;
  }
  // 运行中切主题：不先 stop/release，直接 render_start（后端 prepare_start 取消旧会话并接管），
  // 旧画面保留到新画面到达 → 用户无感切换，不会看到“正在运行/未运行”来回跳。
  if (!session.view().canStart) {
    const st = session.view().state;
    const replaceable = session.view().known && st?.phase === "running" && !session.view().pending;
    if (!replaceable) {
      renderMsg.value = "后台尚未确认释放，可继续点「停止」";
      return;
    }
  }
  if (!intent.current(token) || serial !== selSerial.value) return;
  const tag = selTag.value;
  htmlPath.value = html;
  const started = await session.start({ html, serial: serial || null,
    width: effSize.value.w, height: effSize.value.h });
  if (!intent.current(token) || !started) return;
  // 记录这次真正下发成功的素材，供首页“正在运行”与预览来源行使用（不影响下发逻辑）
  runningName.value = material;
  runningMode.value = html.startsWith("http") ? "online" : "local";
  runningKey.value = runningMode.value === "local" ? `local:${material}` : `online:${onlineId}`;
  curTag.value = tag;
  if (themeMode.value === "local") {
    setLastTheme(tag, material);
    localStorage.setItem("jiadian.lastTheme", material);
  } else {
    localStorage.setItem("jiadian.lastOnline", onlineId);
  }
  logEvent(`render_start 已确认：${material}（${labelOfSerial(serial)}）`);
}
async function chooseSelected() {
  if (!canChoose.value || !canConfirm.value) return;
  await runIntent(startRender);
}
/// 首页只保留一个「启动 / 停止」按钮：运行中显示停止图标，空闲显示播放图标。
const renderActive = computed(
  () => running.value || sessionView.value.state?.phase === "running" || sessionView.value.pending,
);
const runHint = computed(() => {
  if (renderActive.value) return "停止";
  if (renderMsg.value) return renderMsg.value;
  if (themeMode.value === "online") return onlineHint.value;
  if (!selected.value) return "先选一个主题";
  if (!selectedPage.value) return `「${selected.value}」没有 ${effSize.value.w}×${effSize.value.h} 的素材`;
  return "启动";
});
async function toggleRender(): Promise<void> {
  if (renderActive.value) {
    await releaseTakeover();
    return;
  }
  await autoApplySelection();
}
async function releaseTakeover() {
  intent.invalidate();
  infoRevision++;
  scanRevision++;
  busy.value = false;
  previewRenderer.invalidate();
  // 用户主动停止：待应用/失败态一并清掉，避免停止后还挂着“重试”
  pendingKey.value = "";
  pendingName.value = "";
  autoErr.value = "";
  logEvent("点击「停止」：取消接管");
  await releaseLease("停止");
}

// ---------------- 首页：运行状态 / 选择即应用 / 预览来源（纯展示） ----------------
/** 自动应用状态：idle / start / switch。applying 期间按钮显示加载态并阻止重复触发。 */
const applying = ref(false);
const applyState = ref<"idle" | "start" | "switch">("idle");
/** 当前下拉选择的素材名（本地主题 / 在线素材标题） */
const currentName = computed(() =>
  themeMode.value === "local" ? selected.value : onlineSel.value?.title || "",
);
const currentKey = computed(() =>
  themeMode.value === "local"
    ? selected.value ? `local:${selected.value}` : ""
    : onlineSelected.value ? `online:${onlineSelected.value}` : "",
);
/** 只有“显式选择后没应用成功”才算待应用；分段浏览（本地↔在线）不进入这个状态（H01） */
const hasPending = computed(() =>
  renderActive.value && applyState.value === "idle"
  && !!pendingKey.value && pendingKey.value === currentKey.value
  && !!runningKey.value && pendingKey.value !== runningKey.value,
);
const runningSourceText = computed(() => (runningMode.value === "local" ? "本地素材" : "在线素材"));
/// 运行状态行只给状态词：主题名只出现在下拉框与预览卡底部（各一次）
const runningLine = computed(() => {
  if (!renderActive.value) return "未运行";
  return linkReconnecting.value ? "重连中" : "正在运行";
});
/// 过渡态：选择即应用时说明目标（必要的临时信息，完成后立刻消失）
const applyLine = computed(() => {
  if (applyState.value === "idle") return "";
  const target = currentName.value || "当前选择";
  return applyState.value === "switch" ? `正在切换到 ${target}…` : `正在启动 ${target}…`;
});
/// 待应用说明：A 仍在运行，B 未应用（重试只针对 B）
const pendingHint = computed(() => {
  if (!hasPending.value) return "";
  return `${runningName.value || "当前素材"} 仍在运行，${pendingName.value || "所选素材"} 未应用`;
});
/// 失败提示：只来自“选择即应用”的失败，且当前没有可重试的待应用目标
const failureHint = computed(() =>
  applyState.value === "idle" && !renderActive.value && !hasPending.value ? autoErr.value : "",
);
/** 未运行时按钮为什么点不了：就地说明，不只靠 tooltip（H02 / G6） */
const startBlockReason = computed(() => {
  if (renderActive.value) return "";
  if (busy.value || ctlBusy.value) return "正在处理上一步操作，请稍候";
  if (!selSerial.value) return "未连接屏幕设备：插好后点「刷新」再启动";
  if (themeMode.value === "online") {
    if (!onlineSources.value.length) return "";
    return onlineHint.value;
  }
  if (!selected.value) return "还没有选择素材";
  if (!selectedPage.value) return `「${selected.value}」没有 ${effSize.value.w}×${effSize.value.h} 的素材`;
  if (!canChoose.value) return "后台尚未确认释放，可继续点「停止」";
  return "";
});

/// 顶部设备条右侧的一句提示：优先显示设备消息；没有消息时说明“未运行时为什么点不了启动”。
/// 播放卡里不再重复显示这行提示（只在顶部说一次）。
const devbarHint = computed(() => {
  if (infoMsg.value && !infoAlert.value) return infoMsg.value;
  if (!renderActive.value && !applying.value && startBlockReason.value) return startBlockReason.value;
  return "";
});
const canStartNow = computed(
  () => !renderActive.value && !applying.value && !busy.value && !ctlBusy.value
    && canChoose.value && canConfirm.value && !!selSerial.value,
);
/** 预览卡底部（一行）：主题名 · 来源 · 机型 · 分辨率 */
const previewName = computed(() => (renderActive.value ? runningName.value : "") || currentName.value || "—");
const previewSource = computed(() =>
  renderActive.value
    ? runningSourceText.value
    : themeMode.value === "local" ? "本地素材" : "在线素材",
);
const previewModel = computed(() => info.value?.hw || modelBySize(effSize.value.w, effSize.value.h) || "—");
const previewMeta = computed(
  () => `${previewSource.value} · ${previewModel.value} · ${effSize.value.w}×${effSize.value.h}`,
);
/// 占位文案跟着“选择即应用”走：运行中未收到帧 = 正在接收；停止后才提示点启动
const previewPlaceholder = computed(() => {
  if (renderActive.value) return "正在接收画面…";
  const hasMaterial = themeMode.value === "local" ? !!selectedPage.value : !!onlineUrl.value;
  return hasMaterial ? "已停止，点右侧「启动」继续下发" : "先在右侧选择一个主题";
});
const previewBadge = computed(() => (linkReconnecting.value ? "重连中" : renderActive.value ? "运行中" : "未运行"));
const previewBadgeTone = computed(() => (linkReconnecting.value ? "warn" : renderActive.value ? "ok" : "idle"));
/** 选择即应用：下拉显式选择后直接启动；A 在跑则复用 startRender 的释放/接管逻辑 */
let autoAgain = false;
async function autoApplySelection(): Promise<void> {
  if (applying.value) { autoAgain = true; return; }
  if (!selSerial.value || !canConfirm.value) return;
  if (renderActive.value && currentKey.value === runningKey.value) return;
  autoAgain = false;
  // 显式选择：先记下目标，失败时才能说明“A 仍在运行、B 未应用”
  pendingKey.value = currentKey.value;
  pendingName.value = currentName.value;
  autoErr.value = "";
  applyState.value = renderActive.value ? "switch" : "start";
  applying.value = true;
  renderMsg.value = "";
  try {
    await chooseSelected();
  } finally {
    if (!running.value && renderMsg.value) ElMessage.warning(renderMsg.value);
    applying.value = false;
    applyState.value = "idle";
    const msg = renderMsg.value;
    const reason = msg && msg !== sessionView.value.message ? msg : "";
    if (running.value && runningKey.value === currentKey.value) {
      pendingKey.value = "";
      pendingName.value = "";
      autoErr.value = "";
    } else if (renderActive.value) {
      // A 还在跑：保留待应用目标，重试只针对 B
      autoErr.value = reason || "未能切换，可重试";
    } else {
      pendingKey.value = "";
      pendingName.value = "";
      autoErr.value = reason || "未能启动，可重试";
    }
    const again = autoAgain;
    autoAgain = false;
    // 切换期间又改了选择：若上一次没有失败信息，按最新选择再应用一次（失败不重试）
    if (again && !autoErr.value) await autoApplySelection();
  }
}
/// 素材页暴露的编程式切标签入口（首页在线素材空态 → 去素材页在线标签）
interface MaterialsApi {
  goTab(main: string, sub?: string): void;
}
const materialsRef = ref<MaterialsApi | null>(null);

/// 下拉的 change 只在用户显式选择时触发；boot / 设备切换的程序化修正不会走到这里
function onThemePicked(): void {
  void autoApplySelection();
}
/// 在线素材空态的去向：素材页 → 在线素材
function goOnlineMaterials(): void {
  goPage("materials");
  requestAnimationFrame(() => materialsRef.value?.goTab("素材管理", "在线素材"));
}

// ---------------- 设备参数 ----------------
const infoMsg = ref("");
/// 设备详情弹窗：SN / Flash / 可存帧数 / 电压等低频规格移到这里，字段一个不少
const devDlg = ref(false);
const bright = ref(200);
/// 滑杆内部仍是 0–255（硬件范围），界面显示换算后的百分比（H04）
const brightPct = computed(() => Math.round((bright.value / 255) * 100));
const applyFailMsg = computed(() => (infoMsg.value.startsWith("下发配置失败") ? infoMsg.value : ""));

/// 顶部警报条：设备断开 / 下发失败 / 指标读取失败统一收口到页面最上方，可手动关闭。
/// 只在文案变化（新问题）时重新出现；避免同一句在设备条、播放卡里各说一遍。
const alertDismissed = ref(false);
const infoAlert = computed(() => /未连接|已断开|读取失败|扫描失败|下发配置失败/.test(infoMsg.value));
const alertText = computed(
  () => error.value || (infoAlert.value ? infoMsg.value : "") || failureHint.value || "",
);
watch(alertText, (v, prev) => {
  if (v && v !== prev) alertDismissed.value = false;
});
const rot = ref(0);
const rotOptions: UIOption[] = [
  { value: 0, label: "0°" },
  { value: 1, label: "90°" },
  { value: 2, label: "180°" },
  { value: 3, label: "270°" },
];
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
    if (typeof d.brightness === "number") bright.value = d.brightness;
    if (typeof d.rotation === "number") rot.value = d.rotation;
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
/// `force = true`（「刷新」按钮）：强制以物理枚举为准，不合并 GUI 残留的幽灵设备 ——
/// 设备拔掉/重插后，点刷新能立刻反映真实在线设备，而不是继续显示缓存里的旧设备。
async function loadDevices(token: number, force = false): Promise<boolean> {
  const revision = ++scanRevision;
  try {
    const raw = await withTimeout(invoke<DeviceEntry[]>("device_list", { force }), 15000, "扫描设备");
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
    if (!await loadDevices(token, true) || !intent.current(token)) return;
    const sn = selSerial.value;
    if (sn && !devices.value.some((d) => d.serial === sn)) {
      // 设备正在自动重连时不要因为「这次扫描没枚举到」就把会话停掉/清空选择：
      // 让后台继续按 SN 重试接管，插回后自动恢复。
      if (linkReconnecting.value) {
        infoMsg.value = "设备已断开，后台正在自动重连；设备列表已刷新";
        logEvent(infoMsg.value);
        return;
      }
      await clearMissingDevice(sn, token);
      return;
    }
    await loadInfo(token);
  });
}
async function onDeviceChange(): Promise<void> {
  await runIntent(async (token) => {
    const sn = selSerial.value;
    if (!await loadDevices(token) || !intent.current(token)) return;
    if (sn && !devices.value.some((d) => d.serial === sn)) {
      const gone = await deviceGoneAfterGrace(token);
      if (!intent.current(token)) return;
      if (gone) {
        await clearMissingDevice(sn, token);
        return;
      }
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
    <div class="app-bg" aria-hidden="true"></div>
    <!-- 自定义标题栏（隐藏了 Windows 原生标题栏） -->
    <TitleBar @close="onClose" />

    <div class="shell">
      <Sidebar :page="page" :theme="theme" @nav="goPage" @theme="toggleTheme" />

      <main class="workspace">
        <div v-if="alertText && !alertDismissed" class="alert-bar" role="alert">
          <Icon icon="line-md:alert-circle" class="alert-ic" aria-hidden="true" />
          <span class="alert-text" :title="alertText">{{ alertText }}</span>
          <button
            type="button"
            class="alert-close"
            aria-label="关闭提示"
            title="关闭提示（出现新问题时还会再提示）"
            @click="alertDismissed = true"
          >
            <Icon icon="line-md:close" />
          </button>
        </div>

        <div class="view" tabindex="0" aria-label="页面内容">

          <!-- ============ 首页 ============ -->
          <div v-show="page === 'home'" class="page home-page">
            <!-- 顶部设备状态条：身份 / 连接 / 分辨率 / 刷新 / 设备详情（H03：规格移入弹窗） -->
            <el-card shadow="never" class="panel devbar-card">
              <div class="devbar">
                <span class="devbar-icon" aria-hidden="true"><Icon icon="line-md:monitor" /></span>
                <el-select
                  v-model="selSerial"
                  class="devbar-sel"
                  :disabled="busy || ctlBusy"
                  :title="selSerial || '未发现设备'"
                  placeholder="无设备"
                  aria-label="选择设备"
                  @change="onDeviceChange"
                >
                  <el-option
                    v-for="d in deviceOptions"
                    :key="d.value"
                    :label="d.label"
                    :value="d.value"
                    :disabled="d.disabled"
                  />
                </el-select>
                <span class="devbar-status" :title="info ? '屏幕设备已连接' : '屏幕设备未连接'">
                  <span class="dot" :class="{ ok: !!info }" aria-hidden="true"></span>
                  <span>{{ info ? "已连接" : "未连接" }}</span>
                </span>
                <span v-if="devbarHint" class="devbar-hint" :title="devbarHint">{{ devbarHint }}</span>
                <span class="devbar-grow"></span>
                <el-button text class="devbar-btn" :disabled="busy || ctlBusy" @click="refreshDevices">
                  <Icon icon="line-md:backup-restore" /><span>刷新</span>
                </el-button>
                <span class="vsep" aria-hidden="true"></span>
                <el-button text class="devbar-btn detail" @click="devDlg = true">
                  <span>设备详情</span><Icon icon="line-md:chevron-right" />
                </el-button>
              </div>
            </el-card>

            <div class="home-body">
              <!-- 左列：完整屏幕预览（真实比例，尽量大） -->
              <div class="home-l">
                <el-card shadow="never" class="panel preview-card">
                  <template #header>
                    <div class="card-head">
                      <span class="card-title">屏幕预览</span>
                      <span class="run-badge" :class="previewBadgeTone" :title="`播放状态：${previewBadge}`">
                        <span class="b-dot" aria-hidden="true"></span>{{ previewBadge }}
                      </span>
                    </div>
                  </template>
                  <div class="preview" :style="{ aspectRatio: `${effSize.w} / ${effSize.h}` }">
                    <canvas v-show="hasPreview" ref="previewCanvas" class="pv-canvas"></canvas>
                    <div v-if="!hasPreview" class="ph">{{ previewPlaceholder }}</div>
                  </div>
                  <div class="preview-foot" :title="`${previewName} · ${previewMeta}`">
                    <Icon icon="line-md:image" aria-hidden="true" />
                    <span class="pf-name">{{ previewName }}</span>
                    <span class="pf-sep" aria-hidden="true">·</span>
                    <span class="pf-meta">{{ previewMeta }}</span>
                  </div>
                </el-card>
              </div>

              <!-- 右列：选素材 → 播放 → 调显示 -->
              <div class="home-r">
                <el-card shadow="never" class="panel themed">
                  <template #header>
                    <div class="card-head">
                      <span class="card-title">主题与播放</span>
                      <button type="button" class="head-link" @click="goPage('materials')">管理素材 ›</button>
                    </div>
                  </template>
                  <el-radio-group
                    v-model="themeMode"
                    class="mode-switch"
                    aria-label="主题来源"
                    :disabled="applying"
                    @change="onModeChange"
                  >
                    <el-radio-button value="local">
                      <Icon icon="line-md:folder" aria-hidden="true" /><span>本地素材</span>
                    </el-radio-button>
                    <el-radio-button value="online">
                      <Icon icon="line-md:cloud-alt-download" aria-hidden="true" /><span>在线素材</span>
                    </el-radio-button>
                  </el-radio-group>

                  <el-select
                    v-if="themeMode === 'local'"
                    v-model="selected"
                    class="theme-select"
                    :disabled="busy || applying"
                    placeholder="无主题"
                    aria-label="选择本地素材"
                    @change="onThemePicked"
                  >
                    <el-option
                      v-for="t in themeOptions"
                      :key="t.value"
                      :label="t.label"
                      :value="t.value"
                      :disabled="t.disabled"
                    />
                  </el-select>
                  <div v-else-if="onlineLoading && !onlineSources.length" class="online-state">
                    <p class="os-title">正在读取在线素材…</p>
                  </div>
                  <div v-else-if="!onlineSources.length" class="online-state">
                    <p class="os-title">还没有在线素材</p>
                    <p class="os-note">在线素材用网址显示，添加到「素材 → 在线素材」后回到这里即可选用。</p>
                    <button type="button" class="os-link" @click="goOnlineMaterials">添加在线素材 ›</button>
                  </div>
                  <el-select
                    v-else
                    v-model="onlineSelected"
                    class="theme-select"
                    :disabled="busy || applying || !onlineSources.length"
                    placeholder="无在线素材"
                    aria-label="选择在线素材"
                    @change="onThemePicked"
                  >
                    <el-option
                      v-for="o in onlineOptions"
                      :key="o.value"
                      :label="o.label"
                      :value="o.value"
                      :disabled="o.disabled"
                    />
                  </el-select>

                  <div class="run-status">
                    <span class="dot" :class="{ ok: renderActive, warn: linkReconnecting }" aria-hidden="true"></span>
                    <span class="run-text" :title="applying ? applyLine : runningLine">{{ applying ? applyLine : runningLine }}</span>
                    <el-button v-if="applying" type="primary" plain class="run-action" loading disabled>
                      {{ applyState === "switch" ? "切换中…" : "启动中…" }}
                    </el-button>
                    <el-button
                      v-else-if="renderActive"
                      type="danger"
                      plain
                      class="run-action"
                      :disabled="busy || ctlBusy"
                      :title="runHint"
                      @click="toggleRender"
                    >
                      ■ 停止
                    </el-button>
                    <el-button
                      v-else
                      type="primary"
                      class="run-action"
                      :disabled="!canStartNow"
                      :title="runHint"
                      @click="toggleRender"
                    >
                      ▶ 启动
                    </el-button>
                  </div>
                  <div v-if="!applying && hasPending" class="state-row">
                    <span class="state-text" :title="autoErr || pendingHint">{{ pendingHint }}</span>
                    <el-button
                      class="state-action"
                      :disabled="busy || ctlBusy"
                      :title="autoErr || `重试应用 ${pendingName}`"
                      @click="autoApplySelection"
                    >
                      重试
                    </el-button>
                  </div>
                </el-card>

                <el-card shadow="never" class="panel display-card">
                  <template #header>
                    <div class="card-head"><span class="card-title">显示控制</span></div>
                  </template>
                  <div class="ctl-row">
                    <span class="ctl-label" :title="`原始值 ${bright} / 255`">亮度 {{ brightPct }}%</span>
                    <el-slider
                      v-model="bright"
                      class="ctl-slider"
                      :min="0"
                      :max="255"
                      :show-tooltip="false"
                      :disabled="!canConfigure"
                      aria-label="亮度"
                      @change="applyConfig"
                    />
                  </div>
                  <div class="ctl-actions">
                    <el-button :disabled="!canConfigure" @click="bright = 0; applyConfig()">
                      <Icon icon="line-md:monitor-off" aria-hidden="true" /><span>息屏</span>
                    </el-button>
                    <el-button :disabled="!canConfigure" @click="bright = 255; applyConfig()">
                      <Icon icon="line-md:sunny-filled" aria-hidden="true" /><span>最亮</span>
                    </el-button>
                  </div>
                  <div class="ctl-row">
                    <span class="ctl-label">屏幕方向</span>
                    <el-select
                      v-model="rot"
                      class="ctl-select"
                      :disabled="!canConfigure"
                      aria-label="屏幕方向"
                      @change="applyConfig"
                    >
                      <el-option v-for="r in rotOptions" :key="r.value" :label="r.label" :value="r.value" />
                    </el-select>
                  </div>
                  <div class="ctl-row follow-row">
                    <span class="ctl-label">跟随电脑息屏</span>
                    <el-switch
                      v-model="followOff"
                      :disabled="!canConfigure"
                      aria-label="跟随电脑息屏"
                      @change="applyConfig"
                    />
                  </div>
                  <p v-if="applyFailMsg" class="ctl-note bad">{{ applyFailMsg }}</p>
                </el-card>
              </div>
            </div>
          </div>

          <!-- ============ 性能 ============ -->
          <div v-show="page === 'perf'" class="page perf-page">
            <div class="perf-cols">
              <div v-for="(col, ci) in perfColumns" :key="ci" class="perf-col">
                <el-card v-for="(g, gi) in col" :key="g.title" shadow="never" class="panel perf-card">
                  <template #header>
                    <div class="card-head">
                      <span class="card-title">
                        <Icon :icon="g.icon" aria-hidden="true" /><span>{{ g.title }}</span>
                      </span>
                      <span v-if="g.subtitle" class="card-sub" :title="g.subtitle">{{ g.subtitle }}</span>
                    </div>
                  </template>
                  <div class="core-grid" :class="{ one: g.core.length === 1 }">
                    <div v-for="c in g.core" :key="c.m.key" class="core-item">
                      <span class="core-label">
                        {{ c.label }}<span v-if="c.note" class="core-note"> · {{ c.note }}</span>
                      </span>
                      <span class="core-value" :class="{ missing: isMissing(c.m) }">{{ display(c.m) }}</span>
                    </div>
                  </div>
                  <div v-if="g.secondary.length" class="sec-line">
                    <span v-for="(s, si) in g.secondary" :key="s.m.key" class="sec-item">
                      <span v-if="si" class="sec-dot" aria-hidden="true">·</span>
                      <span class="sec-k">{{ s.label }}</span>
                      <span class="sec-v" :class="{ missing: isMissing(s.m) }">{{ s.m.text }}</span>
                    </span>
                  </div>
                  <div v-if="g.disks.length" class="disk-list">
                    <div class="disk-head" aria-hidden="true">
                      <span class="disk-drive"></span>
                      <span class="disk-bar-head"></span>
                      <span class="disk-cap">已用 / 总量</span>
                      <span class="disk-pct">占用</span>
                      <span class="disk-temp">温度</span>
                    </div>
                    <div
                      v-for="d in g.disks"
                      :key="d.key"
                      class="disk-row"
                      :class="{ agg: d.aggregate }"
                    >
                      <span class="disk-drive">{{ d.drive }}</span>
                      <span class="disk-bar" aria-hidden="true"><i :style="{ width: barWidth(d.pct) }"></i></span>
                      <span class="disk-cap">{{ d.used?.text || "—" }} / {{ d.total?.text || "—" }}</span>
                      <span class="disk-pct" :class="{ missing: !d.pct || isMissing(d.pct) }">{{ d.pct?.text || "—" }}</span>
                      <span v-if="!d.aggregate" class="disk-temp">
                        {{ d.temp?.text || "—" }}
                      </span>
                    </div>
                  </div>
                  <div v-if="g.nets.length" class="net-list">
                    <div v-for="n in g.nets" :key="n.key" class="net-row">
                      <div class="net-head">
                        <span class="net-name">{{ n.name }}</span>
                        <span class="net-k">当前速率</span>
                      </div>
                      <div class="net-vals">
                        <span class="net-v">
                          <span class="net-dir">下行</span>
                          <b :class="{ missing: !n.down || isMissing(n.down) }">{{ n.down?.text || "—" }}</b>
                        </span>
                        <span class="net-v">
                          <span class="net-dir">上行</span>
                          <b :class="{ missing: !n.up || isMissing(n.up) }">{{ n.up?.text || "—" }}</b>
                        </span>
                      </div>
                    </div>
                  </div>
                  <button
                    v-if="g.details.length"
                    type="button"
                    class="detail-toggle"
                    :aria-expanded="perfOpen[g.title] ? 'true' : 'false'"
                    :aria-controls="`perf-detail-${ci}-${gi}`"
                    @click="togglePerf(g.title)"
                  >
                    <Icon :icon="perfOpen[g.title] ? 'line-md:chevron-up' : 'line-md:chevron-down'" aria-hidden="true" />
                    <span>详情 · {{ g.details.length }} 项</span>
                  </button>
                  <div v-show="perfOpen[g.title]" :id="`perf-detail-${ci}-${gi}`" class="detail-list">
                    <p v-if="g.detailNote" class="detail-note">{{ g.detailNote }}</p>
                    <div v-for="m in g.details" :key="m.key" class="detail-row">
                      <span class="detail-label">{{ metricLabel(m) }}</span>
                      <span class="detail-value" :class="{ missing: isMissing(m) }">{{ display(m) }}</span>
                    </div>
                  </div>
                </el-card>
              </div>
            </div>
          </div>

          <!-- ============ 外设 ============ -->
          <PeripheralsPage v-show="page === 'peripherals'" />

          <!-- ============ 素材 / 设置 ============ -->
          <MaterialsPage
            v-show="page === 'materials'"
            ref="materialsRef"
            @imported="onMaterialImported"
            @go-home="goPage('home')"
            @apply-theme="applyThemeFromMaterials"
          />
          <SettingsPage v-show="page === 'settings'" />
        </div>
      </main>
    </div>

    <!-- 设备详情：SN / 机型 / 分辨率 / Flash / 可存帧数 / 电压 全部保留，一步可查 -->
    <el-dialog v-model="devDlg" title="设备详情" width="380px" class="dev-dialog">
      <div v-if="info" class="devdlg">
        <div class="dr">
          <span class="dk">SN</span>
          <span class="dv" :title="info.serial || ''">{{ info.serial || "—" }}</span>
        </div>
        <div class="dr">
          <span class="dk">机型</span>
          <span class="dv">{{ info.hw || "—" }}</span>
        </div>
        <div class="dr">
          <span class="dk">分辨率</span>
          <span class="dv">{{ info.width }}×{{ info.height }}</span>
        </div>
        <div class="dr">
          <span class="dk">Flash</span>
          <span class="dv">{{ info.flash_model || info.flash_id }} · {{ info.flash_mbit }} Mbit</span>
        </div>
        <div class="dr">
          <span class="dk">可存帧数</span>
          <span class="dv">{{ info.frames }}（动画区 {{ info.anim_mb }} MB）</span>
        </div>
        <div class="dr">
          <span class="dk">电压</span>
          <span class="dv">{{ info.millivolts }} mV</span>
        </div>
      </div>
      <div v-else class="dev-empty">
        <Icon icon="line-md:monitor-off" aria-hidden="true" />
        <p class="de-title">未发现设备</p>
        <p class="de-note">设备未连接或尚未读取到信息。请插好设备后点「刷新设备」重试。</p>
      </div>
      <p v-if="infoMsg" class="dev-msg" :class="{ bad: infoMsg !== '读取中…' }">{{ infoMsg }}</p>
      <template #footer>
        <div class="dialog-actions">
          <el-button :disabled="busy || ctlBusy" @click="devDlg = false">关闭</el-button>
          <el-button type="primary" :disabled="busy || ctlBusy" @click="refreshDevices">刷新设备</el-button>
        </div>
      </template>
    </el-dialog>

    <!-- 数据源自检：LibreHardwareService 不在跑时引导安装 -->
    <el-dialog
      v-model="srcDlg"
      :title="srcTitle(srcState)"
      width="520px"
      class="src-dialog"
      :close-on-click-modal="false"
    >
      <p class="hint">{{ srcTip(srcState) }}</p>
      <div v-if="lhwsProg?.active" class="prog">
        <div class="prog-head">
          <span class="grow">{{ lhwsProg.message }}</span>
          <span v-if="lhwsSize">{{ lhwsSize }}</span>
          <span v-if="lhwsSpeed">{{ lhwsSpeed }}</span>
        </div>
        <el-progress
          :percentage="lhwsIndet ? 100 : lhwsPct"
          :indeterminate="lhwsIndet"
          :show-text="false"
          :status="lhwsProg.phase === 'failed' ? 'exception' : undefined"
        />
        <div class="prog-head">
          <span class="grow">{{ lhwsAttempt }}</span>
          <span v-if="!lhwsIndet">{{ lhwsPct }}%</span>
        </div>
      </div>
      <p v-if="srcMsg" class="hint">{{ srcMsg }}</p>
      <p class="hint">
        当前状态：{{ srcState?.statusText }}（服务 {{ srcState?.serviceState }} · 数据
        {{ srcState?.publishing ? "已发布" : "未发布" }}）
      </p>
      <p class="hint">
        第三方开源组件 {{ srcState?.version }}（{{ srcState?.license }}）·
        <a @click="openSrcUrl(srcState?.source || '')">来源</a> ·
        <a @click="openSrcUrl(srcState?.licenseUrl || '')">许可</a>
      </p>
      <template #footer>
        <div class="dialog-actions">
          <el-button
            v-if="srcState?.status === 'notInstalled'"
            type="primary"
            :loading="srcBusy"
            @click="installDataSource"
          >
            {{ srcBusy ? "安装中…" : "一键安装" }}
          </el-button>
          <el-button v-else type="primary" :loading="srcBusy" @click="restartDataSource">
            {{ srcBusy ? "处理中…" : "重新启动" }}
          </el-button>
          <el-button :disabled="srcBusy" @click="recheckDataSource">重新检测</el-button>
          <el-button :disabled="srcBusy" @click="srcDlg = false">稍后</el-button>
        </div>
      </template>
    </el-dialog>
  </div>
</template>

<style src="./styles.css"></style>
<style scoped>
/* ============================================================
   首页 / 性能页局部样式（App.vue）
   全局 token 见 styles.css；强调色只用于选中、主操作、链接与焦点。
   状态一律「文字 + 圆点 + 颜色」；卡片标题走中性色（评审 G01）。
   ============================================================ */

/* ---------- 通用：卡片标题 / 头部链接 ---------- */
.card-head {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.card-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.1px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.card-title svg {
  flex: none;
  color: var(--sub);
  font-size: 15px;
}
.card-sub {
  margin-left: auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--sub);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
}
.head-link {
  margin-left: auto;
  flex: none;
  padding: 2px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--accent);
  font-size: 12px;
  cursor: pointer;
}
.head-link:hover {
  text-decoration: underline;
}
/* 按钮内图标与文字统一 4px 间距（EP 默认无 gap） */
:deep(.el-button > span) {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

/* ---------- 首页：顶部设备状态条 ---------- */
.home-page {
  height: 100%;
  min-height: 0;
}
.devbar-card {
  flex: none;
}
.devbar-card :deep(.el-card__body) {
  padding: 7px 12px 8px;
}
.devbar {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.devbar-icon {
  flex: none;
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  color: var(--sub);
}
.devbar-icon svg {
  font-size: 16px;
}
.devbar-sel {
  flex: none;
  width: 148px;
}
.devbar-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex: none;
  color: var(--fg);
  font-size: 12px;
  white-space: nowrap;
}
.devbar .dot,
.run-status .dot {
  width: 7px;
  height: 7px;
  flex: none;
  border-radius: 50%;
  background: var(--sub);
}
.devbar .dot.ok,
.run-status .dot.ok {
  background: var(--ok);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 16%, transparent);
}
.run-status .dot.warn {
  background: var(--warn);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--warn) 16%, transparent);
}
.vsep {
  flex: none;
  width: 1px;
  height: 16px;
  background: var(--line);
}
.devbar-res {
  flex: none;
  color: var(--sub);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.devbar-grow {
  flex: 1;
  min-width: 8px;
}
.devbar-btn {
  height: 26px;
  padding: 0 6px;
  --el-button-bg-color: transparent;
  --el-button-text-color: var(--sub);
  --el-button-hover-bg-color: var(--btn);
  --el-button-hover-text-color: var(--fg);
  --el-button-active-bg-color: var(--btn);
  --el-button-active-text-color: var(--fg);
  color: var(--sub);
}
.devbar-btn :deep(svg) {
  font-size: 14px;
}
.devbar-btn.detail {
  --el-button-text-color: var(--accent);
  --el-button-hover-text-color: var(--accent);
  color: var(--accent);
}
.devbar-hint {
  flex: 0 1 auto;
  min-width: 0;
  max-width: 52%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--sub);
  font-size: 11.5px;
}
.devbar-note.bad {
  color: color-mix(in srgb, var(--danger) 82%, var(--fg));
}

/* ---------- 首页：两列布局 ---------- */
.home-body {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 12px;
}
.home-l,
.home-r {
  flex: 1 1 0;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.home-l .preview-card {
  flex: 1 1 auto;
  min-height: 0;
}
.home-r .themed {
  flex: 0 0 auto;
}
.home-r .display-card {
  flex: 1 1 0;
  min-height: 0;
}
.preview-card,
.themed,
.display-card {
  display: flex;
  flex-direction: column;
}
.preview-card :deep(.el-card__header),
.themed :deep(.el-card__header),
.display-card :deep(.el-card__header) {
  flex: none;
  padding: 7px 12px;
}
.preview-card :deep(.el-card__body),
.display-card :deep(.el-card__body) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.preview-card :deep(.el-card__body) {
  position: relative;
  justify-content: center;
  padding: 8px 12px 36px;
}

/* ---------- 顶部警报条（跨页、可关闭） ---------- */
.alert-bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  padding: 6px 8px 6px 10px;
  border: 1px solid color-mix(in srgb, var(--danger) 38%, var(--line));
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--danger) 9%, var(--panel));
  color: var(--fg);
  font-size: 12px;
  line-height: 1.45;
}
.alert-ic {
  flex: none;
  color: var(--danger);
  font-size: 15px;
}
.alert-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.alert-close {
  flex: none;
  width: 24px;
  height: 24px;
  display: grid;
  place-items: center;
  padding: 0;
  border: none;
  border-radius: var(--r-ctl);
  background: transparent;
  color: var(--sub);
  cursor: pointer;
}
.alert-close:hover {
  background: var(--btn-hover);
  color: var(--fg);
}

/* ---------- 首页：屏幕预览 ---------- */
.run-badge {
  margin-left: auto;
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 20px;
  padding: 0 8px;
  border: 1px solid var(--line);
  border-radius: 999px;
  background: color-mix(in srgb, var(--line) 24%, transparent);
  color: var(--fg);
  font-size: 11px;
  white-space: nowrap;
}
.run-badge .b-dot {
  width: 6px;
  height: 6px;
  flex: none;
  border-radius: 50%;
  background: var(--sub);
}
.run-badge.ok {
  border-color: color-mix(in srgb, var(--ok) 42%, var(--line));
  background: color-mix(in srgb, var(--ok) 13%, transparent);
}
.run-badge.ok .b-dot {
  background: var(--ok);
}
.run-badge.warn {
  border-color: color-mix(in srgb, var(--warn) 45%, var(--line));
  background: color-mix(in srgb, var(--warn) 14%, transparent);
}
.run-badge.warn .b-dot {
  background: var(--warn);
}
.preview {
  /* 固定占用设备真实比例（320×240 / 320×170）：没有画面时也保留同样大小的虚线框，
     运行/停止切换不跳动；尺寸只随设备分辨率变化。 */
  flex: 0 0 auto;
  width: 100%;
  min-height: 0;
  display: grid;
  place-items: center;
  overflow: hidden;
  border: 1px dashed color-mix(in srgb, var(--line) 88%, transparent);
  border-radius: var(--r-ctl);
  background: color-mix(in srgb, var(--bg) 34%, transparent);
}
.pv-canvas {
  width: 100%;
  height: 100%;
  border-radius: 2px;
  image-rendering: pixelated;
}
.ph {
  display: grid;
  place-items: center;
  width: 100%;
  height: 100%;
  padding: 0 18px;
  color: var(--sub);
  font-size: 12px;
  line-height: 1.5;
  text-align: center;
}
.preview-foot {
  position: absolute;
  left: 12px;
  right: 12px;
  bottom: 8px;
  min-width: 0;
  margin: 0;
  padding-top: 6px;
  border-top: 1px solid color-mix(in srgb, var(--line) 60%, transparent);
  display: flex;
  align-items: center;
  gap: 6px;
}
.preview-foot > svg {
  flex: none;
  color: var(--sub);
  font-size: 15px;
}
.pf-name {
  min-width: 0;
  flex: 0 1 auto;
  overflow: hidden;
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pf-meta {
  flex: none;
  color: var(--sub);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.pf-sep {
  flex: none;
  color: var(--sub);
  font-size: 11.5px;
}

/* ---------- 首页：主题与播放 ---------- */
.themed :deep(.el-card__body) {
  padding: 10px 12px 11px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.mode-switch {
  width: 100%;
  display: flex;
  gap: 8px;
}
.mode-switch :deep(.el-radio-button) {
  flex: 1;
}
.mode-switch :deep(.el-radio-button__inner) {
  width: 100%;
  height: 26px;
  padding: 0 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border: 1px solid var(--line);
  border-radius: var(--r-ctl) !important;
  box-shadow: none !important;
  background: transparent;
  color: var(--sub);
  font-size: 12px;
  transition: background 0.12s, border-color 0.12s, color 0.12s;
}
.mode-switch :deep(.el-radio-button__inner:hover) {
  background: color-mix(in srgb, var(--btn) 70%, transparent);
  color: var(--fg);
}
.mode-switch :deep(.el-radio-button.is-active > .el-radio-button__inner) {
  background: color-mix(in srgb, var(--accent) 10%, var(--panel)) !important;
  border-color: color-mix(in srgb, var(--accent) 50%, var(--line)) !important;
  color: color-mix(in srgb, var(--accent) 82%, var(--fg)) !important;
  font-weight: 600;
}
.mode-switch :deep(.el-radio-button.is-active > .el-radio-button__inner:hover) {
  background: color-mix(in srgb, var(--accent) 14%, var(--panel)) !important;
}
.mode-switch :deep(.el-radio-button__original-radio:focus-visible + .el-radio-button__inner) {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
.theme-select {
  width: 100%;
}
.online-state {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px 8px;
  border: 1px dashed color-mix(in srgb, var(--line) 80%, transparent);
  border-radius: var(--r-ctl);
}
.os-title {
  margin: 0;
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
}
.os-note {
  margin: 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.45;
}
.os-link {
  align-self: flex-start;
  margin-top: 2px;
  padding: 1px 2px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: var(--accent);
  font-size: 11.5px;
  cursor: pointer;
}
.os-link:hover {
  text-decoration: underline;
}
.run-status {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  min-height: 26px;
}
.run-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg);
  font-size: 11.5px;
  line-height: 1.35;
  white-space: normal;
  word-break: keep-all;
  overflow-wrap: anywhere;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}
.run-action {
  flex: none;
  height: 26px;
  min-width: 74px;
  padding: 0 8px;
}
/* 停止按钮是危险样式，但浅色下 EP 的 danger 文字对比度只有 ~4.2:1，压深一档到 ≥4.5:1 */
.run-action.el-button--danger {
  --el-button-text-color: color-mix(in srgb, var(--danger) 78%, var(--fg));
  --el-button-hover-text-color: color-mix(in srgb, var(--danger) 90%, var(--fg));
  --el-button-active-text-color: color-mix(in srgb, var(--danger) 90%, var(--fg));
}
.state-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.state-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--sub);
  font-size: 11.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.state-row.bad .state-text {
  color: color-mix(in srgb, var(--danger) 82%, var(--fg));
}
.state-action {
  flex: none;
  height: 22px;
  padding: 0 8px;
}
.run-block-hint {
  margin: 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.45;
}

/* ---------- 首页：显示控制 ---------- */
.ctl-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 28px;
  font-size: 12px;
}
.ctl-label {
  flex: none;
  color: var(--sub);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.ctl-slider {
  flex: 1;
  min-width: 0;
  margin: 0 4px;
}
.ctl-actions {
  display: flex;
  gap: 8px;
}
.ctl-actions :deep(.el-button) {
  flex: 1;
  min-width: 0;
}
.ctl-select {
  margin-left: auto;
  flex: none;
  width: 92px;
}
.ctl-row :deep(.el-switch) {
  margin-left: auto;
}
.display-card :deep(.el-card__body) {
  justify-content: flex-start;
  gap: 8px;
  padding: 8px 12px 10px;
}
.display-card .follow-row {
  margin-top: auto;
}
.ctl-note {
  margin: auto 0 0;
  padding-top: 8px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.4;
}
.ctl-note.bad {
  color: color-mix(in srgb, var(--danger) 82%, var(--fg));
}

/* ---------- 设备详情弹窗 ---------- */
.devdlg {
  display: flex;
  flex-direction: column;
}
.devdlg .dr {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  min-height: 26px;
  border-bottom: 1px dashed color-mix(in srgb, var(--line) 55%, transparent);
  font-size: 12px;
}
.devdlg .dr:last-child {
  border-bottom: none;
}
.devdlg .dk {
  flex: none;
  color: var(--sub);
}
.devdlg .dv {
  min-width: 0;
  overflow: hidden;
  color: var(--fg);
  font-variant-numeric: tabular-nums;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dev-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 10px 0 6px;
  text-align: center;
}
.dev-empty svg {
  color: var(--sub);
  font-size: 28px;
}
.de-title {
  margin: 2px 0 0;
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
}
.de-note {
  margin: 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
}
.dev-msg {
  margin: 8px 0 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.45;
}
.dev-msg.bad {
  color: color-mix(in srgb, var(--danger) 82%, var(--fg));
}

/* ---------- 性能页：状态条 + 核心指标 + 详情 ---------- */
.perf-page {
  min-height: 0;
}
.perf-card {
  min-width: 0;
}
.perf-card :deep(.el-card__header) {
  padding: 7px 12px;
}
.perf-card :deep(.el-card__body) {
  display: flex;
  flex-direction: column;
  gap: 7px;
  padding: 8px 12px 9px;
}
.core-grid {
  display: flex;
  gap: 8px;
  min-width: 0;
}
.core-grid.one .core-item {
  flex: 0 1 auto;
}
.core-item {
  flex: 1 1 0;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.core-label {
  overflow: hidden;
  color: var(--sub);
  font-size: 11.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.core-note {
  color: var(--sub);
}
.core-value {
  overflow: hidden;
  color: var(--fg);
  font-size: 20px;
  font-weight: 600;
  line-height: 1.15;
  letter-spacing: -0.3px;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.core-value.missing {
  color: var(--sub);
  font-weight: 500;
}
.sec-line {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
  color: var(--sub);
  font-size: 11.5px;
}
.sec-item {
  display: inline-flex;
  align-items: baseline;
  gap: 5px;
  min-width: 0;
}
.sec-k {
  color: var(--sub);
}
.sec-v {
  color: var(--fg);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.sec-v.missing {
  color: var(--sub);
  font-weight: 500;
}

.disk-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}
.disk-head {
  display: flex;
  align-items: center;
  gap: 5px;
  height: 15px;
  color: var(--sub);
  font-size: 10.5px;
  line-height: 1;
  white-space: nowrap;
}
.disk-head .disk-cap,
.disk-head .disk-pct,
.disk-head .disk-temp {
  font-weight: 400;
  color: var(--sub);
}
.disk-bar-head {
  flex: 1;
  min-width: 16px;
}
.disk-row {
  display: flex;
  align-items: center;
  gap: 5px;
  height: 23px;
  min-width: 0;
  font-size: 11.5px;
}
.disk-drive {
  width: 22px;
  flex: none;
  overflow: hidden;
  color: var(--fg);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.disk-row.agg .disk-drive {
  width: 30px;
  color: var(--sub);
  font-weight: 400;
}
.disk-bar {
  flex: 1;
  min-width: 16px;
  height: 4px;
  overflow: hidden;
  border-radius: 2px;
  background: color-mix(in srgb, var(--line) 55%, transparent);
}
.disk-bar i {
  display: block;
  height: 100%;
  border-radius: 2px;
  background: color-mix(in srgb, var(--sub) 80%, transparent);
}
.disk-row.agg .disk-bar {
  opacity: 0.7;
}
.disk-cap {
  width: 136px;
  flex: none;
  overflow: hidden;
  color: var(--sub);
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.disk-pct {
  width: 40px;
  flex: none;
  color: var(--fg);
  font-weight: 600;
  text-align: right;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.disk-pct.missing {
  color: var(--sub);
  font-weight: 500;
}
.disk-temp {
  width: 34px;
  flex: none;
  overflow: hidden;
  color: var(--sub);
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.net-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.net-row {
  min-width: 0;
  padding: 3px 0;
  border-bottom: 1px dashed color-mix(in srgb, var(--line) 50%, transparent);
}
.net-row:last-child {
  border-bottom: none;
}
.net-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}
.net-name {
  overflow: hidden;
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.net-k {
  flex: none;
  color: var(--sub);
  font-size: 11px;
}
.net-vals {
  display: flex;
  gap: 16px;
  margin-top: 1px;
  font-size: 11.5px;
}
.net-v {
  display: inline-flex;
  align-items: baseline;
  gap: 5px;
  min-width: 0;
}
.net-dir {
  color: var(--sub);
}
.net-v b {
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.net-v b.missing {
  color: var(--sub);
  font-weight: 500;
}

.detail-toggle {
  align-self: flex-start;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  margin: 0;
  padding: 0 5px;
  border: none;
  border-radius: var(--r-sm);
  background: transparent;
  color: var(--sub);
  font-size: 11.5px;
  cursor: pointer;
}
.detail-toggle:hover {
  background: var(--btn);
  color: var(--fg);
}
.detail-toggle[aria-expanded="true"] {
  color: var(--fg);
}
.detail-toggle svg {
  font-size: 13px;
}
.detail-list {
  display: flex;
  flex-direction: column;
  padding-top: 3px;
  border-top: 1px solid color-mix(in srgb, var(--line) 55%, transparent);
}
.detail-note {
  margin: 4px 0 3px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.45;
}
.detail-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  min-height: 21px;
  font-size: 11.5px;
}
.detail-label {
  min-width: 0;
  overflow: hidden;
  color: var(--sub);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.detail-value {
  flex: none;
  color: var(--fg);
  text-align: right;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.detail-value.missing {
  color: var(--sub);
}
</style>



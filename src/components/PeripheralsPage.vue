<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { Icon } from "@iconify/vue";
import { invoke } from "@tauri-apps/api/core";

// 外设页（Element Plus）：780×540 固定窗口下按「状态条 → 提示条 → 全宽列表 → 折叠帮助」
// 纵向排列。设备列表占满内容宽度；协议 / 排查说明默认收起，只在列表下方留明确入口。
// 数据仍走原命令 peripherals_state / peripherals_refresh / peripherals_rename /
// peripherals_dir / peripherals_open_dir，轮询间隔由后端快照的 pollMs 决定。

/// 后端（peripherals::Snapshot）的 JSON 结构（serde camelCase）
interface Reading {
  vid: string;
  pid: string;
  brand: string;
  brandKey: string;
  model: string;
  name: string;
  /// 同型号内的实例短指纹（4 位十六进制）
  instance: string;
  /// 默认名（品牌+型号）
  defaultName: string;
  /// 用户改过的名字；null = 没改过
  customName: string | null;
  /// live / retrying / sleeping / unavailable / unsupported
  state: string;
  lastSeenMs: number;
  asleepSinceMs: number | null;
  category: string;
  protocol: string;
  transport: string;
  battery: number | null;
  charging: boolean | null;
  online: boolean;
  stale: boolean;
  /// false = 认得品牌，但 brands/<品牌>.json 里没有匹配它端点的档位，电量未知
  supported: boolean;
  note: string | null;
  error: string | null;
  extras: Record<string, number>;
}
interface Snapshot {
  ok: boolean;
  /// 目录的绝对路径（程序自己用，界面不显示）
  dir: string;
  /// 目录的相对标签，如 `<程序目录>\\peripherals`，界面显示这个
  dirLabel: string;
  /// 当前可执行文件名，示例命令用它
  exeName: string;
  pollMs: number;
  devices: Reading[];
  errors: string[];
  updatedMs: number;
}

const snap = ref<Snapshot | null>(null);
const busy = ref(false);
/// 操作结果提示（保存改名 / 读盘失败等）；msgBad 决定用错误色
const msg = ref("");
const msgBad = ref(false);
/// 目录标签（相对路径，不含本机盘符）
const dirLabel = ref("");
/// 可执行文件名（示例命令用）
const exeName = ref("");

const devices = computed(() => snap.value?.devices ?? []);

/// 轮询间隔文案由后端给，不在前端写死秒数。

/// 认得出品牌、但没匹配到协议档位的设备：只影响这些设备的电量，不能压过正常读数。
const unsupportedDevices = computed(() => devices.value.filter((r) => !r.supported));

/// 非阻断提示（后端 errors 里通常是未收录 VID/PID、单个品牌 JSON 解析失败等）。
const partialErrors = computed(() => (snap.value?.ok ? snap.value?.errors ?? [] : []));

/// 紧凑琥珀色提示条：只在“部分设备未支持”时出现。
/// 只影响展示：关闭后本次运行内不再出现；手动「刷新」（或重启程序）会重新出现。
const warnDismissed = ref(false);

/// 小控件“按下即响应 + 350ms 防抖”：
/// Windows 上轻微的指针漂移或掉帧会让 click 不触发，用户就会反复点；
/// 这里在 pointerdown 就执行，并用时间戳吞掉紧随其后的 click。
/// 键盘 Enter/Space 不产生 pointerdown，仍走 click，可正常操作。
const PRESS_LOCK_MS = 350;
/// 每个控件各自计时（而不是全局一个时间戳）：快速点“×”再点“展开”这类不同按钮
/// 不会互相吞掉，只有同一按钮的快速连点才会被防抖。
const pressAt = new Map<string, number>();
function pressOnce(fn: () => void, key = "default") {
  const now = Date.now();
  if (now - (pressAt.get(key) ?? 0) < PRESS_LOCK_MS) return;
  pressAt.set(key, now);
  fn();
}
function onPressDown(fn: () => void, key = "default") {
  return (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    pressOnce(fn, key);
  };
}

let warnTimer: number | undefined;
function clearWarnTimer() {
  if (warnTimer) {
    window.clearTimeout(warnTimer);
    warnTimer = undefined;
  }
}
function dismissWarn() {
  clearWarnTimer();
  warnDismissed.value = true;
}

const partialWarn = computed(() => {
  const n = unsupportedDevices.value.length;
  if (!n && !partialErrors.value.length) return null;
  // 影响范围只用一句话；「已识别 N 台」已经在顶部状态条出现过，这里不重复数字口径。
  const sub = n
    ? `其中 ${n} 台未匹配协议，电量暂不可读；其他设备读数不受影响。`
    : "部分 HID 设备未收录协议，电量暂不可读；已识别设备的读数不受影响。";
  return { title: "部分设备未支持", sub };
});

// 只在本次启动后展示一次：出现后静默自动收起，不再用任何提示打扰用户；
// 手动刷新也不会重新弹出（除非重启应用）。
watch(partialWarn, (v) => {
  if (!v) return;
  if (warnDismissed.value) return;
  clearWarnTimer();
  warnTimer = window.setTimeout(() => {
    warnDismissed.value = true;
    warnTimer = undefined;
  }, 6000);
});


/// 操作提示（保存改名 / 刷新失败等）：几秒后自动消失，避免长期占位
let msgTimer = 0;
function setMsg(text: string, bad = false): void {
  msg.value = text;
  msgBad.value = bad;
  if (msgTimer) window.clearTimeout(msgTimer);
  if (text) msgTimer = window.setTimeout(() => { if (msg.value === text) msg.value = ""; }, 6000);
}

/// 协议库读不出来 = 阻断级错误（列表为空）；后端 errors[0] 带原因。
const fatalError = computed(() => {
  if (!snap.value || snap.value.ok) return "";
  return snap.value.errors[0] || "无法读取外设协议库，请检查 peripherals/ 目录";
});

/// 行唯一键：同型号多台靠 instance 区分（VID/PID 相同）
function keyOf(r: Reading): string {
  return `${r.vid}:${r.pid}:${r.instance || r.name}`;
}

/// 同 VID/PID 出现多台时，行内提示「同型号多台」，避免用户分不清改名改的是哪台
function sameModel(r: Reading): boolean {
  return devices.value.filter((x) => x.vid === r.vid && x.pid === r.pid).length > 1;
}

/// 名称悬停提示：默认名 + VID:PID + 指纹（含自定义名时一并说明）
function nameTitle(r: Reading): string {
  const parts = [`默认名「${r.defaultName}」`, `VID:PID ${r.vid}:${r.pid}`];
  if (r.instance) parts.push(`指纹 ${r.instance}`);
  if (r.customName) parts.push("已自定义名称");
  return parts.join(" · ");
}

/// 行内元信息：品牌 · 指纹（同型号多台时才显示指纹，避免噪音）
function metaText(r: Reading): string {
  const brand = r.brandKey || r.brand || "未知品牌";
  return r.instance ? `${brand} · 指纹 ${r.instance}` : brand;
}

/// 图标分类：从后端 category（中文）推断，仅影响列表左侧的小图标
function deviceKind(r: Reading): "keyboard" | "mouse" | "headset" | "other" {
  const c = `${r.category || ""} ${r.protocol || ""} ${r.name || ""}`;
  if (/键盘|keyboard/i.test(c)) return "keyboard";
  if (/鼠标|mouse/i.test(c)) return "mouse";
  if (/耳机|耳麦|headset|audio/i.test(c)) return "headset";
  return "other";
}

/// 读取状态：只给用户能看懂的五种（已读取 / 休眠中 / 可能休眠·重试中 / 协议未匹配 / 已断开）
function stateInfo(r: Reading): { tone: "ok" | "warn" | "bad" | "idle"; icon: string; text: string } {
  // 被厂商上位机/驱动独占：不是休眠、也不是断开，单独说明，避免用户误判。
  if (r.state === "busy") return { tone: "warn", icon: "line-md:alert-circle", text: "被其他程序占用" };
  if (!r.supported) return { tone: "idle", icon: "line-md:question-circle", text: "协议未匹配" };
  if (!r.online || r.stale) return { tone: "bad", icon: "line-md:close-circle", text: "已断开" };
  switch (r.state) {
    case "live":
      return { tone: "ok", icon: "line-md:confirm-circle", text: "已读取" };
    case "sleeping":
      return { tone: "warn", icon: "line-md:moon", text: "休眠中" };
    case "retrying":
      return { tone: "warn", icon: "line-md:loading-twotone-loop", text: "可能休眠·重试中" };
    case "unavailable":
      return { tone: "bad", icon: "line-md:close-circle", text: "已断开" };
    case "unsupported":
      return { tone: "idle", icon: "line-md:question-circle", text: "协议未匹配" };
    default:
      return { tone: "idle", icon: "line-md:help-circle", text: "未知状态" };
  }
}

/// 状态悬停说明：把“为什么读不到”讲清楚，不只给一个状态词
function rowHint(r: Reading): string {
  const info = stateInfo(r);
  if (r.state === "busy")
    return "设备正被官方上位机/厂商工具独占，暂时读不到电量；关闭对方或稍等会自动恢复。休眠的设备通常还能打开，只是读数超时。";
  if (!r.supported) return `协议未匹配：认得品牌「${r.brand || "未知"}」，但 brands/ 里没有匹配该端点的档位，电量无法读取。`;
  if (!r.online || r.stale) return "设备不在线或读数已过期；重新连接后会自动恢复。";
  if (r.state === "sleeping") return "设备省电休眠，读数暂停；操作一下设备或点「刷新」会尝试唤醒。";
  if (r.state === "retrying") return "协议已匹配但本次读不到，通常是休眠；操作设备或点「刷新」会重试。";
  return `${info.text}${r.protocol ? ` · 协议 ${r.protocol}` : ""}`;
}

function batteryTone(r: Reading): "ok" | "warn" | "bad" | "idle" {
  if (!r.supported || r.battery === null) return "idle";
  if (r.battery <= 10) return "bad";
  if (r.battery <= 25) return "warn";
  return "ok";
}

function batteryText(r: Reading): string {
  if (!r.supported) return "—";
  if (r.battery === null) return "—";
  return `${Math.round(r.battery)}%`;
}

function batteryWidth(r: Reading): number {
  if (r.battery === null || !Number.isFinite(r.battery)) return 0;
  return Math.max(0, Math.min(100, r.battery));
}

function batteryNote(r: Reading): string {
  if (!r.supported) return "协议未匹配，暂不可读";
  if (r.charging) return "充电中";
  if (r.battery === null) return r.state === "sleeping" ? "休眠中，暂不可读" : "暂不可读";
  return "";
}

function batteryTitle(r: Reading): string {
  if (!r.supported) return "协议未匹配，无法读取电量";
  if (r.battery === null) return "当前读不到电量（多为休眠或刚唤醒）";
  const extra = r.charging ? "（充电中）" : "";
  return `电量 ${Math.round(r.battery)}%${extra}`;
}

const helpOpen = ref(false);
const helpRef = ref<HTMLElement | null>(null);
const advancedOpen = ref(false);
const advancedRef = ref<HTMLElement | null>(null);

/// 提示条里的「查看详情」：把两个折叠区都展开，并把高级详情滚进可视区
async function openDetails(): Promise<void> {
  advancedOpen.value = true;
  helpOpen.value = true;
  await nextTick();
  advancedRef.value?.scrollIntoView({ block: "nearest" });
}

/// 展开折叠区后把它滚进可视范围：页面底部展开时，用户能立刻看到内容而不是只有箭头翻转。
async function toggleHelp() {
  helpOpen.value = !helpOpen.value;
  if (!helpOpen.value) return;
  await nextTick();
  helpRef.value?.scrollIntoView({ block: "nearest" });
}

async function toggleAdvanced() {
  advancedOpen.value = !advancedOpen.value;
  if (!advancedOpen.value) return;
  await nextTick();
  advancedRef.value?.scrollIntoView({ block: "nearest" });
}

// ---------------------------------------------------------------- 改名对话框
const renameVisible = ref(false);
const renameTarget = ref<Reading | null>(null);
const draft = ref("");
const renaming = ref(false);
const renameError = ref("");
const renameInput = ref<{ focus: () => void } | null>(null);

function openRename(r: Reading) {
  renameTarget.value = r;
  draft.value = r.name;
  renameError.value = "";
  renameVisible.value = true;
}

/// 关闭动画结束后再清空，避免对话框内容闪一下空白。
function onRenameClosed() {
  renameTarget.value = null;
  draft.value = "";
  renameError.value = "";
}

/// 打开后把焦点放进输入框（键盘用户直接改名）。
function onRenameOpened() {
  renameInput.value?.focus();
}

async function saveRename() {
  const r = renameTarget.value;
  if (!r || renaming.value) return;
  const value = draft.value.trim();
  if (value === r.name) {
    renameVisible.value = false;
    return;
  }
  renaming.value = true;
  renameError.value = "";
  try {
    snap.value = await invoke<Snapshot>("peripherals_rename", {
      vid: r.vid,
      pid: r.pid,
      instance: sameModel(r) ? r.instance : null,
      name: value || null,
    });
    setMsg(value ? "名称已保存到 settings.ini" : "已恢复默认名");
    renameVisible.value = false;
  } catch (e) {
    // 失败不关对话框：用户改完可以再试，错误就显示在输入框下方。
    renameError.value = `保存失败：${e}`;
  } finally {
    renaming.value = false;
  }
}

async function resetName() {
  draft.value = "";
  await saveRename();
}

// ---------------------------------------------------------------- 数据与轮询
async function refresh(force = false) {
  if (busy.value) return;
  busy.value = true;
  try {
    snap.value = force
      ? await invoke<Snapshot>("peripherals_refresh")
      : await invoke<Snapshot>("peripherals_state");
    if (force) {
      setMsg("");
    }
    // 目录标签 / 可执行文件名后端随快照给出，第一次拿到就记下来
    if (!dirLabel.value && snap.value?.dirLabel) dirLabel.value = snap.value.dirLabel;
    if (!exeName.value && snap.value?.exeName) exeName.value = snap.value.exeName;
  } catch (e) {
    setMsg(String(e), true);
  } finally {
    busy.value = false;
  }
}

/// 在资源管理器里打开协议库目录（绝对路径只在这一步交给系统，不进界面文本）。
async function openDir() {
  try {
    await invoke("peripherals_open_dir");
  } catch (e) {
    setMsg(String(e), true);
  }
}

let timer = 0;

onMounted(async () => {
  await refresh();
  // 目录标签单独取一次（命令与旧版保持一致；快照里也有一份，作为兜底）
  const dir = await invoke<string | null>("peripherals_dir").catch(() => null);
  dirLabel.value = dir || snap.value?.dirLabel || "";
  exeName.value = snap.value?.exeName || "";
  // 界面按后端给的轮询间隔的一半取缓存，保证每次都能看到新读数；改名时暂停，避免行内容跳动。
  const ms = Math.max(500, Math.floor((snap.value?.pollMs ?? 3000) / 2));
  timer = window.setInterval(() => {
    if (!renameVisible.value) void refresh();
  }, ms);
});

onUnmounted(() => {
  if (timer) clearInterval(timer);
  clearWarnTimer();
});
</script>

<template>
  <div class="page pd-page">
    <!-- ① 紧凑状态条：已识别 + 刷新、打开目录 -->
    <div class="pd-strip">
      <div class="pd-strip-main">
        <span class="pd-stat">
          <Icon icon="line-md:monitor" class="pd-stat-ic" />
          <span class="pd-stat-k">已识别</span>
          <b class="pd-stat-v">{{ devices.length }} 台</b>
        </span>
        <span class="pd-grow"></span>
        <el-button text :loading="busy" title="重新读取一次（休眠设备会尝试唤醒）" @click="refresh(true)">
          <Icon icon="line-md:backup-restore" /><span>刷新</span>
        </el-button>
        <el-button text :disabled="!dirLabel" title="在资源管理器里打开协议库目录" @click="openDir">
          <Icon icon="line-md:folder" /><span>打开目录</span>
        </el-button>
      </div>
      <p v-if="msg" class="pd-strip-msg" :class="{ bad: msgBad }" :title="msg">
        <Icon :icon="msgBad ? 'line-md:close-circle' : 'line-md:confirm-circle'" />
        <span>{{ msg }}</span>
      </p>
    </div>

    <!-- ② 阻断错误（保留错误级别） / 部分未支持（紧凑琥珀色） -->
    <div v-if="fatalError" class="pd-banner is-error" role="alert">
      <Icon icon="line-md:close-circle" class="pd-banner-ic" />
      <div class="pd-banner-txt">
        <b>协议库读取失败</b>
        <span class="pd-banner-sub">{{ fatalError }}</span>
      </div>
      <button
        type="button"
        class="pd-link"
        @pointerdown="onPressDown(openDetails, 'details')"
        @click.prevent="pressOnce(openDetails, 'details')"
      >查看详情 ›</button>
    </div>
    <div v-else-if="partialWarn && !warnDismissed" class="pd-banner is-warn" role="status">
      <Icon icon="line-md:construction" class="pd-banner-ic" />
      <div class="pd-banner-txt">
        <b>{{ partialWarn.title }}</b>
        <span class="pd-banner-sub">{{ partialWarn.sub }}</span>
      </div>
      <button
        type="button"
        class="pd-link"
        @pointerdown="onPressDown(openDetails, 'details')"
        @click.prevent="pressOnce(openDetails, 'details')"
      >查看详情 ›</button>
      <button
        type="button"
        class="pd-banner-close"
        aria-label="关闭提示"
        title="关闭提示（手动刷新后重新显示）"
        @pointerdown="onPressDown(dismissWarn, 'close')"
        @click.prevent="pressOnce(dismissWarn, 'close')"
      >
        <Icon icon="line-md:close" />
      </button>
    </div>

    <!-- ③ 全宽外设列表 -->
    <el-card shadow="never" class="panel pd-list-card">
      <template #header>
        <div class="panel-head pd-head">
          <span class="panel-title"><Icon icon="line-md:list" /><span>外设列表</span></span>
          <span class="pd-count" :class="{ zero: devices.length === 0 }">{{ devices.length }}</span>
          <span class="pd-grow"></span>
          <span v-if="snap && !snap.ok" class="pd-head-note bad">协议库读取异常</span>
          <span v-else-if="devices.length" class="pd-head-note">双击名称也可改名</span>
        </div>
      </template>

      <div v-if="devices.length" class="pd-table">
        <div class="pd-thead" aria-hidden="true">
          <span>设备</span>
          <span>读取状态</span>
          <span>电量</span>
          <span class="pd-th-act">操作</span>
        </div>
        <div class="pd-rows">
          <div v-for="r in devices" :key="keyOf(r)" class="pd-row">
            <div class="pd-dev">
              <span class="pd-dev-ic" :title="r.category" aria-hidden="true">
                <svg v-if="deviceKind(r) === 'keyboard'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="2.5" y="6.5" width="19" height="11" rx="2" />
                  <path d="M6 10h.01M9 10h.01M12 10h.01M15 10h.01M18 10h.01M6 13.5h.01M9 13.5h.01M15 13.5h.01M18 13.5h.01M10.5 13.5h3" />
                </svg>
                <svg v-else-if="deviceKind(r) === 'mouse'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="7" y="2.5" width="10" height="19" rx="5" />
                  <path d="M12 2.5v6.5" />
                </svg>
                <svg v-else-if="deviceKind(r) === 'headset'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 15v-3a8 8 0 0 1 16 0v3" />
                  <rect x="2.5" y="13.5" width="4" height="7" rx="1.6" />
                  <rect x="17.5" y="13.5" width="4" height="7" rx="1.6" />
                </svg>
                <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="2.5" y="4" width="19" height="13" rx="2" />
                  <path d="M8 21h8M12 17v4" />
                </svg>
              </span>
              <div class="pd-name" :title="nameTitle(r)" @dblclick="openRename(r)">
                <span class="pd-nm">{{ r.name }}</span>
                <span class="pd-meta">{{ metaText(r) }}<template v-if="sameModel(r)"> · 同型号多台</template></span>
              </div>
            </div>

            <span class="pd-state" :class="'tone-' + stateInfo(r).tone" :title="rowHint(r)">
              <Icon :icon="stateInfo(r).icon" class="pd-state-ic" />
              <span>{{ stateInfo(r).text }}</span>
            </span>

            <div class="pd-batt" :class="'tone-' + batteryTone(r)" :title="batteryTitle(r)">
              <div class="pd-batt-row">
                <span class="pd-batt-val">{{ batteryText(r) }}</span>
                <span v-if="r.battery !== null && r.supported" class="pd-bar" aria-hidden="true">
                  <span class="pd-bar-fill" :style="{ width: batteryWidth(r) + '%' }"></span>
                </span>
              </div>
              <span v-if="batteryNote(r)" class="pd-batt-note">{{ batteryNote(r) }}</span>
            </div>

            <el-button
              text
              class="pd-rename"
              :title="`重命名「${r.name}」（仅改显示名称）`"
              :aria-label="`重命名 ${r.name}`"
              @click="openRename(r)"
            >
              <Icon icon="line-md:edit" /><span>重命名</span>
            </el-button>
          </div>
        </div>
      </div>

      <div v-else class="pd-empty">
        <Icon :icon="snap && !snap.ok ? 'line-md:close-circle' : 'line-md:monitor-off'" class="pd-empty-ic" />
        <p class="pd-empty-t">{{ snap && !snap.ok ? "协议库读取失败，暂时无法列出设备" : "没有识别到已收录的外设" }}</p>
        <p class="pd-empty-s">
          插上鼠标 / 键盘 / 耳机；或在 <code>peripherals/index.toml</code> 的 devices 里补一条 VID/PID。
          协议细节见下方「高级：协议与设备详情」。
        </p>
        <el-button v-if="!snap || snap.ok" @click="openDir">
          <Icon icon="line-md:folder" /><span>打开协议目录</span>
        </el-button>
      </div>
    </el-card>

    <!-- ④ 可折叠：排查方法 / 高级协议说明（默认收起，不占列表空间） -->
    <div class="pd-folds">
      <section ref="helpRef" class="pd-fold" :class="{ open: helpOpen }">
        <button
          type="button"
          class="pd-fold-head"
          :aria-expanded="helpOpen"
          aria-controls="pd-help-body"
          @pointerdown="onPressDown(toggleHelp, 'help')"
          @click.prevent="pressOnce(toggleHelp, 'help')"
        >
          <Icon icon="line-md:alert" class="pd-fold-ic" />
          <span class="pd-fold-title">无法读取电量？</span>
          <span class="pd-fold-sub">查看排查方法</span>
          <span class="pd-chev" aria-hidden="true">›</span>
        </button>
        <div v-show="helpOpen" id="pd-help-body" class="pd-fold-body">
          <ul class="pd-tips">
            <li><b>休眠中</b>：设备省电休眠，读数暂停；操作一下设备或用顶部「刷新」读取一次会尝试唤醒，恢复后自动继续。</li>
            <li><b>可能休眠·重试中</b>：协议已匹配但这次读不到，通常是省电休眠；操作设备或点「刷新」会重新读取，下次轮询也会重试。</li>
            <li><b>协议未匹配</b>：认得品牌但缺协议档位，电量无法读取；补 <code>brands/&lt;品牌&gt;.json</code> 后重新读取。</li>
            <li><b>已断开</b>：设备不在线或读数过期；重新连接后自动恢复，不把旧值当实时值。</li>
          </ul>
          <p class="pd-fold-foot">
            协议目录：<code>{{ dirLabel || "（未找到 peripherals 目录）" }}</code>
          </p>
        </div>
      </section>

      <section ref="advancedRef" class="pd-fold" :class="{ open: advancedOpen }">
        <button
          type="button"
          class="pd-fold-head"
          :aria-expanded="advancedOpen"
          aria-controls="pd-adv-body"
          @pointerdown="onPressDown(toggleAdvanced, 'advanced')"
          @click.prevent="pressOnce(toggleAdvanced, 'advanced')"
        >
          <Icon icon="line-md:file-import" class="pd-fold-ic" />
          <span class="pd-fold-title">高级：协议与设备详情</span>
          <span class="pd-fold-sub">VID:PID、协议文件与自检命令</span>
          <span class="pd-chev" aria-hidden="true">›</span>
        </button>
        <div v-show="advancedOpen" id="pd-adv-body" class="pd-fold-body">
          <div v-if="unsupportedDevices.length" class="pd-adv-block">
            <div class="pd-adv-t">未匹配协议的设备</div>
            <ul class="pd-tips">
              <li v-for="r in unsupportedDevices" :key="keyOf(r)">
                <b>{{ r.name }}</b> · VID:PID <code>{{ r.vid }}:{{ r.pid }}</code><template v-if="r.instance"> · 指纹 {{ r.instance }}</template>
              </li>
            </ul>
          </div>

          <div v-if="partialErrors.length || fatalError" class="pd-adv-block">
            <div class="pd-adv-t">后端原始提示</div>
            <p v-for="(e, i) in partialErrors.length ? partialErrors : [fatalError]" :key="i" class="pd-raw">{{ e }}</p>
          </div>

          <div class="pd-adv-block">
            <div class="pd-adv-t">自定义协议（纯 JSON）</div>
            <ol class="pd-steps">
              <li><code>{{ exeName || "jiadian-monitor.exe" }} --peripherals-hid</code> 查端点。</li>
              <li><code>index.toml</code> 加 <code>vid</code>/<code>pid</code> → <code>brand</code>。</li>
              <li><code>brands/&lt;品牌&gt;.json</code> 写读法与电量字节。</li>
              <li><code>--peripherals-selftest</code> 用真实回包自检。</li>
            </ol>
          </div>

          <div class="pd-adv-foot">
            <span class="pd-adv-path">
              目录：<code>{{ dirLabel || "（未找到 peripherals 目录）" }}</code> ·
              本地 API <code>/peripherals</code> 同源
            </span>
            <el-button text :disabled="!dirLabel" @click="openDir">
              <Icon icon="line-md:folder" /><span>打开协议目录</span>
            </el-button>
          </div>
        </div>
      </section>
    </div>

    <!-- ⑤ 重命名弹窗：只改显示名，留空恢复默认 -->
    <el-dialog
      v-model="renameVisible"
      title="重命名设备"
      width="380px"
      class="pd-rename-dialog"
      :close-on-click-modal="false"
      @opened="onRenameOpened"
      @closed="onRenameClosed"
    >
      <div class="pd-field">
        <label class="pd-label" for="pd-rename-input">显示名称</label>
        <el-input
          id="pd-rename-input"
          ref="renameInput"
          v-model="draft"
          autocomplete="off"
          spellcheck="false"
          maxlength="24"
          show-word-limit
          clearable
          placeholder="留空 = 恢复默认名"
          @keyup.enter="saveRename"
        />
        <p class="hint">仅修改显示名称，留空恢复默认；最多 24 个字。</p>
      </div>

      <p v-if="renameTarget" class="pd-dialog-meta" :title="nameTitle(renameTarget)">
        <span>VID:PID <code>{{ renameTarget.vid }}:{{ renameTarget.pid }}</code></span>
        <span class="pd-dot-sep" aria-hidden="true">·</span>
        <span>默认名「{{ renameTarget.defaultName }}」</span>
      </p>

      <p v-if="renameError" class="pd-dialog-err" role="alert">
        <Icon icon="line-md:close-circle" /><span>{{ renameError }}</span>
      </p>

      <template #footer>
        <div class="pd-dialog-actions">
          <el-button @click="renameVisible = false">取消</el-button>
          <el-button :disabled="!renameTarget?.customName || renaming" @click="resetName">恢复默认名</el-button>
          <el-button type="primary" :loading="renaming" @click="saveRename">保存</el-button>
        </div>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
/* 外设页：状态条 / 提示条 / 全宽列表 / 折叠帮助纵向排列。
   颜色全部走主题 token；强调色只出现在主按钮、焦点和链接上。 */
.pd-page {
  min-height: 100%;
  height: auto;
  gap: var(--sp-5);
}

/* ---------- 状态条 ---------- */
.pd-strip {
  flex: none;
  padding: 6px 12px;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--panel) calc(var(--ui-alpha) * 100%), transparent);
}
.pd-strip-main {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  min-height: 28px;
  min-width: 0;
}
.pd-stat {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--sub);
  font-size: 12px;
  white-space: nowrap;
}
.pd-stat-ic {
  flex: none;
  color: var(--sub);
  font-size: 15px;
}
.pd-stat-k {
  color: var(--sub);
}
.pd-stat-v {
  color: var(--fg);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.pd-vline {
  flex: none;
  width: 1px;
  height: 14px;
  background: var(--line);
}
.pd-grow {
  flex: 1;
  min-width: 4px;
}
.pd-strip :deep(.el-button) {
  flex: none;
  color: var(--sub);
}
.pd-strip :deep(.el-button:hover) {
  color: var(--fg);
}
.pd-strip-msg {
  display: flex;
  align-items: center;
  gap: 5px;
  margin: 4px 0 0;
  padding-top: 4px;
  border-top: 1px dashed var(--line);
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
  overflow: hidden;
}
.pd-strip-msg svg {
  flex: none;
  color: var(--ok);
}
.pd-strip-msg span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pd-strip-msg.bad,
.pd-strip-msg.bad svg {
  color: var(--danger);
}

/* ---------- 提示条 ---------- */
.pd-banner {
  flex: none;
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  padding: 8px 12px;
  border: 1px solid;
  border-radius: var(--r-card);
  min-width: 0;
}
.pd-banner.is-warn {
  border-color: color-mix(in srgb, var(--warn) 45%, var(--line));
  background: color-mix(in srgb, var(--warn) 12%, var(--panel));
}
.pd-banner.is-error {
  border-color: color-mix(in srgb, var(--danger) 45%, var(--line));
  background: color-mix(in srgb, var(--danger) 10%, var(--panel));
}
.pd-banner-ic {
  flex: none;
  font-size: 18px;
}
.pd-banner.is-warn .pd-banner-ic {
  color: var(--warn);
}
.pd-banner.is-error .pd-banner-ic {
  color: var(--danger);
}
.pd-banner-txt {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.pd-banner-txt b {
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
}
.pd-banner-sub {
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.45;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pd-link {
  flex: none;
  min-height: 28px;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  padding: 0 4px;
  border: none;
  border-radius: var(--r-ctl);
  background: transparent;
  /* 深色琥珀提示条上 --accent(#619aff) 只有 4.32:1，这里改用更亮的蓝；浅色仍用主题强调色 */
  color: #9ec1ff;
  font-size: 12px;
  cursor: pointer;
}
:root.light .pd-link {
  color: var(--accent);
}
/* 提示条内链接固定用高对比蓝（不依赖 --accent）：深色琥珀底上约 7:1 */
.pd-banner .pd-link {
  color: #b7d0ff;
  font-weight: 600;
}
:root.light .pd-banner .pd-link {
  color: var(--accent);
  font-weight: 600;
}
.pd-link:hover {
  text-decoration: underline;
}
.pd-link:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
/* 提示条关闭按钮：26×26 可聚焦；只隐藏本次展示，不动数据与阻断错误 */
.pd-banner-close {
  flex: none;
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  margin-left: -2px;
  padding: 0;
  border: none;
  border-radius: var(--r-ctl);
  background: transparent;
  color: var(--sub);
  cursor: pointer;
}
.pd-banner-close svg {
  font-size: 14px;
}
.pd-banner-close:hover {
  background: color-mix(in srgb, var(--fg) 8%, transparent);
  color: var(--fg);
}
.pd-banner-close:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

/* ---------- 列表卡片 ---------- */
/* 卡片高度跟随内容，最多 252px：设备少时列表下方直接露出排查入口，
   设备多时只让列表内部滚动，页面其余部分不跟着长。 */
.pd-list-card {
  flex: 0 1 auto;
  min-height: 172px;
  max-height: 252px;
  display: flex;
  flex-direction: column;
  margin: 0;
  min-width: 0;
}
.pd-list-card :deep(.el-card__body) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: 0;
}
.pd-head {
  width: 100%;
}
.pd-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 20px;
  height: 18px;
  padding: 0 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--ok) 14%, transparent);
  color: var(--fg);
  font-size: 11px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.pd-count.zero {
  background: var(--btn);
  color: var(--sub);
}
.pd-head-note {
  color: var(--sub);
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.pd-head-note.bad {
  color: var(--danger);
}

/* 表头 + 行共用一套列宽，保证纵向对齐 */
.pd-table {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.pd-thead,
.pd-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 126px 172px 88px;
  align-items: center;
  gap: var(--sp-3);
  padding: 0 12px;
}
.pd-thead {
  flex: none;
  height: 26px;
  border-bottom: 1px solid var(--line);
  background: color-mix(in srgb, var(--btn) 55%, transparent);
  color: var(--sub);
  font-size: 11.5px;
}
.pd-th-act {
  text-align: right;
}
.pd-rows {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.pd-row {
  min-height: 46px;
  padding-top: 5px;
  padding-bottom: 5px;
  border-bottom: 1px solid var(--line);
}
.pd-row:last-child {
  border-bottom: none;
}
.pd-row:hover {
  background: color-mix(in srgb, var(--btn) 60%, transparent);
}

/* 设备：图标 + 名称 + 副信息 */
.pd-dev {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.pd-dev-ic {
  flex: none;
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: var(--r-ctl);
  background: var(--btn);
  color: var(--sub);
}
.pd-dev-ic svg {
  width: 18px;
  height: 18px;
}
.pd-name {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
  cursor: default;
  user-select: none;
}
.pd-nm {
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
  line-height: 1.25;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pd-meta {
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 状态：文字 + 图标 + 颜色 */
.pd-state {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  color: var(--fg);
  font-size: 12px;
  white-space: nowrap;
}
.pd-state-ic {
  flex: none;
  font-size: 15px;
}
.pd-state.tone-ok .pd-state-ic {
  color: var(--ok);
}
.pd-state.tone-warn .pd-state-ic {
  color: var(--warn);
}
.pd-state.tone-muted .pd-state-ic {
  color: var(--sub);
}
.pd-state.tone-sleep {
  color: var(--sub);
}
.pd-state.tone-sleep .pd-state-ic {
  color: color-mix(in srgb, var(--sub) 72%, var(--accent) 28%);
}
.pd-state.tone-danger .pd-state-ic {
  color: var(--danger);
}

/* 电量：数值 + 电量条（+ 需要时一行解释） */
.pd-batt {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  min-width: 0;
}
.pd-batt-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  max-width: 100%;
}
.pd-batt-val {
  flex: none;
  min-width: 58px;
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.pd-batt.tone-muted .pd-batt-val,
.pd-batt.tone-sleep .pd-batt-val {
  color: var(--sub);
  font-weight: 500;
}
.pd-bar {
  flex: none;
  width: 62px;
  height: 6px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--sub) 22%, transparent);
}
.pd-bar-fill {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: var(--ok);
}
.pd-batt.tone-warn .pd-bar-fill {
  background: var(--warn);
}
.pd-batt.tone-danger .pd-bar-fill {
  background: var(--danger);
}
.pd-batt.tone-muted .pd-bar-fill {
  background: color-mix(in srgb, var(--sub) 55%, transparent);
}
.pd-batt.tone-sleep .pd-bar-fill {
  background: color-mix(in srgb, var(--sub) 70%, var(--accent) 30%);
}
.pd-batt-note {
  max-width: 100%;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.35;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pd-rename {
  justify-self: end;
  flex: none;
  color: var(--sub);
  padding: 0 6px;
}
.pd-rename:hover {
  color: var(--accent);
}
.pd-rename :deep(span) {
  margin-left: 4px;
}

/* 空态 */
.pd-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 14px 24px 18px;
  text-align: center;
}
.pd-empty-ic {
  font-size: 26px;
  color: var(--sub);
}
.pd-empty-t {
  margin: 0;
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
}
.pd-empty-s {
  margin: 0 0 4px;
  max-width: 430px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.55;
}

/* ---------- 折叠区 ---------- */
.pd-folds {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
}
.pd-fold {
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--panel) calc(var(--ui-alpha) * 100%), transparent);
  overflow: hidden;
}
.pd-fold-head {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  width: 100%;
  min-height: 36px;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  padding: 4px 12px;
  border: none;
  background: transparent;
  color: var(--fg);
  text-align: left;
  cursor: pointer;
}
.pd-fold-head:hover {
  background: color-mix(in srgb, var(--btn) 55%, transparent);
}
.pd-fold-head:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.pd-fold-ic {
  flex: none;
  color: var(--sub);
  font-size: 15px;
}
.pd-fold-title {
  flex: none;
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
}
.pd-fold-sub {
  flex: 1;
  min-width: 0;
  color: var(--sub);
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pd-chev {
  flex: none;
  color: var(--sub);
  font-size: 16px;
  line-height: 1;
  transform: rotate(90deg);
  transition: transform 0.12s ease;
}
.pd-fold.open .pd-chev {
  transform: rotate(-90deg);
}
.pd-fold-body {
  padding: 2px 12px 10px 39px;
  border-top: 1px dashed var(--line);
}
.pd-tips,
.pd-steps {
  margin: 6px 0 0;
  padding-left: 16px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.55;
}
.pd-tips li,
.pd-steps li {
  margin-bottom: 2px;
}
.pd-tips b {
  color: var(--fg);
  font-weight: 600;
}
.pd-fold-foot {
  margin: 8px 0 0;
  color: var(--sub);
  font-size: 11.5px;
}
.pd-adv-block + .pd-adv-block {
  margin-top: 8px;
}
.pd-adv-t {
  margin-top: 6px;
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
}
.pd-raw {
  margin: 3px 0 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.pd-adv-foot {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  margin-top: 8px;
  padding-top: 6px;
  border-top: 1px dashed var(--line);
}
.pd-adv-path {
  flex: 1;
  min-width: 0;
  color: var(--sub);
  font-size: 11.5px;
  overflow-wrap: anywhere;
}

/* ---------- 重命名弹窗 ---------- */
.pd-field {
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.pd-label {
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
}
.pd-field .hint {
  margin: 0;
}
.pd-dialog-meta {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
  margin: 10px 0 0;
  padding: 5px 8px;
  border-radius: var(--r-ctl);
  background: var(--btn);
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
}
.pd-dot-sep {
  color: var(--sub);
}
.pd-dialog-err {
  display: flex;
  align-items: flex-start;
  gap: 5px;
  margin: 8px 0 0;
  color: var(--danger);
  font-size: 11.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.pd-dialog-err svg {
  flex: none;
  margin-top: 2px;
}
.pd-dialog-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--sp-3);
}
.pd-dialog-actions :deep(.el-button) {
  margin: 0;
}

/* ---------- 通用内联 ---------- */
code {
  padding: 0 3px;
  border-radius: 4px;
  background: var(--btn);
  color: var(--fg);
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 10.5px;
}
</style>

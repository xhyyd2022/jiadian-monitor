<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { Icon } from "@iconify/vue";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useSettings } from "../composables/useSettings";
import AppearanceCard from "./settings/AppearanceCard.vue";
import {
  prog,
  pct,
  indeterminate,
  sizeText,
  speedText,
  attemptText,
  watchProgress,
  unwatchProgress,
} from "../composables/useLhwsProgress";
import { APP_LICENSE, APP_LICENSE_URL, NOTICES_FILE, OSS_GROUPS } from "../data/third-party";

/// <details> 原生折叠的“连点保护”：快速连点会「开→关」抵消，看起来像第一次没响应。
/// 350ms 内的重复点击直接取消默认行为，保留第一次点击的结果（仍可用键盘 Enter/Space 操作）。
let lastDetailAt = 0;
function guardDetail(e: MouseEvent): void {
  const now = Date.now();
  if (now - lastDetailAt < 350) {
    e.preventDefault();
    return;
  }
  lastDetailAt = now;
}

// ---- 设置项（真值在 exe 同级的 settings.ini，见 useSettings.ts）----
const {
  autostart,
  silentStart,
  closeToTray,
  devtools,
  logging,
  logLevel,
  highFps,
  hybridCompat,
  settingsError,
  settingsSaving,
  settingsReady,
  loadSettings,
  unitMode,
  unitCapacity,
  unitRate,
  unitTemp,
  unitDuration,
  theme,
  appearanceBg,
  appearanceAccent,
  appearanceOpacity,
  toggleAutostart,
  openDevtools,
} = useSettings();

const appVersion = ref("");
/// 「关于」页展示的开源组件数量（不含 LibreHardwareService 与 WebView2）
const ossCount = OSS_GROUPS.reduce((n, g) => n + g.items.length, 0);
/// 开源清单默认折叠：需要时再展开，避免「关于」页一进来就被几十行占满
const ossOpen = ref(false);

interface Opt {
  value: string;
  label: string;
}

const logLevelOptions: Opt[] = [
  { value: "info", label: "全部" },
  { value: "warn", label: "警告及以上" },
  { value: "error", label: "仅错误" },
];
const unitModeOptions: Opt[] = [
  { value: "auto", label: "全自动" },
  { value: "manual", label: "手动指定" },
];
const capacityOptions: Opt[] = [
  { value: "", label: "跟随素材" },
  { value: "B", label: "B" },
  { value: "KB", label: "KB" },
  { value: "MB", label: "MB" },
  { value: "GB", label: "GB" },
  { value: "TB", label: "TB" },
];
const rateOptions: Opt[] = [
  { value: "", label: "跟随素材" },
  { value: "B/s", label: "B/s" },
  { value: "KB/s", label: "KB/s" },
  { value: "MB/s", label: "MB/s" },
  { value: "GB/s", label: "GB/s" },
];
const tempOptions: Opt[] = [
  { value: "", label: "跟随素材" },
  { value: "℃", label: "℃" },
  { value: "℉", label: "℉" },
];
const durationOptions: Opt[] = [
  { value: "", label: "跟随素材" },
  { value: "秒", label: "秒" },
  { value: "分", label: "分" },
  { value: "小时", label: "小时" },
  { value: "天", label: "天" },
];

function setLogLevel(v: string | number): void {
  logLevel.value = String(v) as "info" | "warn" | "error";
}
function setUnitMode(v: string | number): void {
  unitMode.value = String(v) as "auto" | "manual";
}
function setUnitCapacity(v: string | number): void {
  unitCapacity.value = String(v);
}
function setUnitRate(v: string | number): void {
  unitRate.value = String(v);
}
function setUnitTemp(v: string | number): void {
  unitTemp.value = String(v);
}
function setUnitDuration(v: string | number): void {
  unitDuration.value = String(v);
}

// ---- 稳定分类入口 ----
// S01：五类一眼可达，类内纵向设置行，不再靠双列长滚动找外观 / 数据源。
// 使用 aria tab 模式 + 左右方向键，Tab 顺序与视觉阅读顺序一致。
type CatId = "general" | "appearance" | "perf" | "logs" | "about";
interface Category {
  id: CatId;
  label: string;
}
const categories: Category[] = [
  { id: "general", label: "通用" },
  { id: "appearance", label: "显示与外观" },
  { id: "perf", label: "性能与数据源" },
  { id: "logs", label: "日志与单位" },
  { id: "about", label: "关于" },
];
const catIds = categories.map((c) => c.id);
const cat = ref<CatId>("general");

function focusCat(id: CatId): void {
  document.getElementById(`set-tab-${id}`)?.focus();
}
function onTabKey(e: KeyboardEvent, index: number): void {
  const n = catIds.length;
  let next = index;
  if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (index + 1) % n;
  else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (index - 1 + n) % n;
  else if (e.key === "Home") next = 0;
  else if (e.key === "End") next = n - 1;
  else return;
  e.preventDefault();
  const id = catIds[next];
  cat.value = id;
  focusCat(id);
}

// ---- 自动保存状态（S02）----
// 状态只来自 useSettings 的真实 ref，不伪造「已保存」；失败时说明当前生效值可能仍是上次保存值。
const retrying = ref(false);
type SaveState = "reading" | "saving" | "saved" | "failed" | "readFailed";
const saveState = computed<SaveState>(() => {
  if (settingsSaving.value) return "saving";
  if (settingsError.value) return settingsReady.value ? "failed" : "readFailed";
  return settingsReady.value ? "saved" : "reading";
});
const saveText = computed(() => {
  switch (saveState.value) {
    case "saving":
      return "保存中…";
    case "saved":
      return "已保存";
    case "failed":
      return "保存失败";
    case "readFailed":
      return "读取失败";
    default:
      return "正在读取…";
  }
});

/// 完整快照：只在“失败后重试写入”时用，字段与 useSettings 的 Settings 一一对应。
function settingsPatch() {
  return {
    theme: theme.value,
    devtools: devtools.value,
    closeToTray: closeToTray.value,
    silentStart: silentStart.value,
    logging: logging.value,
    logLevel: logLevel.value,
    highFps: highFps.value,
    hybridCompat: hybridCompat.value,
    unitMode: unitMode.value,
    unitCapacity: unitCapacity.value,
    unitRate: unitRate.value,
    unitTemp: unitTemp.value,
    unitDuration: unitDuration.value,
    appearanceBg: appearanceBg.value,
    appearanceAccent: appearanceAccent.value,
    appearanceOpacity: appearanceOpacity.value,
  };
}
async function retrySave(): Promise<void> {
  if (retrying.value || settingsSaving.value || !settingsReady.value) return;
  retrying.value = true;
  try {
    await invoke("settings_set", { patch: settingsPatch() });
    // 只有真实写入成功才清掉错误；失败继续保留原因。
    settingsError.value = "";
  } catch (e) {
    settingsError.value = `保存设置失败，未确认的更改已回滚：${e}`;
  } finally {
    retrying.value = false;
  }
}

// ---- 数据源：LibreHardwareService（第三方组件，MPL-2.0）----
interface LhwsState {
  version: string;
  installed: boolean;
  running: boolean;
  /** SCM 里的真实服务状态：notInstalled / stopped / startPending / running / stopPending / paused / unknown */
  serviceState: string;
  /** 共享内存是否存在（= 真在发布数据） */
  publishing: boolean;
  /** 合并态，界面直接显示：notInstalled / stopped / starting / running / unknown */
  status: string;
  /** 上面的中文文案 */
  statusText: string;
  source: string;
  license: string;
  licenseUrl: string;
  msi: string | null;
}
const lhws = ref<LhwsState | null>(null);
const lhwsBusy = ref(false);
const lhwsMsg = ref("");

/// 状态色：绿（正常）· 黄（装好了但没在跑，可一键修）· 灰（未安装 / 未知）
const lhwsTagType = computed<"success" | "warning" | "info">(() => {
  const s = lhws.value;
  if (!s) return "info";
  if (s.status === "running") return "success";
  if (s.status === "notInstalled") return "info";
  return "warning";
});

/// 只有确认“未安装”才给一键安装，检测中不冒充未安装。
const needInstall = computed(() => lhws.value?.installed === false);
/// 装好了但没在正常发布数据 = 需要恢复动作（此时才把“重新启动”提为主操作）。
const needRecover = computed(() => {
  const s = lhws.value;
  return !!s && s.installed && s.status !== "running";
});
const lhwsMsgBad = computed(() => /失败|错误|异常|不可用/.test(lhwsMsg.value));

/// 状态下方的一句解释（点明"装好了但没在跑"和"没装"的区别）
function stateHint(s: LhwsState | null): string {
  if (!s) return "正在检测服务状态…";
  switch (s.status) {
    case "running":
      return "服务运行正常，正在发布温度 / 功耗 / 风扇等指标。";
    case "starting":
      return "服务进程已启动，但还没发布数据（初始化通常 5~7 秒），稍等或重新检测。";
    case "stopped":
      return "服务已安装，但进程没在运行 —— 需要时点「重新启动」拉起（会弹一次 UAC）。";
    case "notInstalled":
      return "未安装。装好后才有温度 / 功耗 / 风扇等指标（网络与磁盘容量不受影响）。";
    default:
      return "拿不到服务状态，可点「重新检测」再试。";
  }
}

async function refreshLhws() {
  try {
    lhws.value = await invoke<LhwsState>("lhws_state");
    lhwsMsg.value = "";
  } catch (e) {
    lhwsMsg.value = String(e);
  }
}

async function installLhws() {
  lhwsBusy.value = true;
  lhwsMsg.value = "";
  watchProgress();
  try {
    lhws.value = await invoke<LhwsState>("lhws_install");
    lhwsMsg.value = lhws.value.running ? "" : "安装已完成，但服务还没在发布数据 —— 可点「重新启动」。";
  } catch (e) {
    lhwsMsg.value = `安装失败：${e}`;
  } finally {
    unwatchProgress();
    lhwsBusy.value = false;
  }
}

/// 重新启动服务（停 → 起 → 等数据就绪）。失败也要刷新状态，让用户看到真实情况。
async function restartLhws() {
  lhwsBusy.value = true;
  lhwsMsg.value = "正在重新启动服务（会弹一次 UAC，请确认）…";
  try {
    lhws.value = await invoke<LhwsState>("lhws_restart");
    lhwsMsg.value = "";
  } catch (e) {
    lhwsMsg.value = String(e);
  } finally {
    await refreshLhws();
    lhwsBusy.value = false;
  }
}

const uninstallOpen = ref(false);

function askUninstall() {
  uninstallOpen.value = true;
}

async function uninstallLhws() {
  lhwsBusy.value = true;
  lhwsMsg.value = "正在卸载（会弹一次 UAC）…";
  try {
    lhws.value = await invoke<LhwsState>("lhws_uninstall");
    lhwsMsg.value = "已卸载";
  } catch (e) {
    lhwsMsg.value = `卸载失败：${e}`;
  } finally {
    await refreshLhws();
    lhwsBusy.value = false;
  }
}

/// 确认卸载：先收弹窗再执行，避免按钮连点
function confirmUninstall() {
  uninstallOpen.value = false;
  void uninstallLhws();
}

const openUrl = (url: string) => invoke("open_url", { url }).catch(() => {});

onMounted(() => {
  refreshLhws();
  getVersion()
    .then((v) => {
      appVersion.value = v;
    })
    .catch(() => {
      /* 非 Tauri 环境读不到版本号，界面显示 — 即可 */
    });
});

// 切走页面时把进度轮询停掉：安装还在后台继续，但没必要在看不见的页面上每 300ms 打一次
onUnmounted(() => {
  unwatchProgress();
});
</script>

<template>
  <div class="settings">
    <!-- 稳定分类入口 + 常驻保存状态（S01/S02）：头部不随内容滚走 -->
    <div class="settings-head">
      <div class="cat-tabs" role="tablist" aria-label="设置分类">
        <button
          v-for="(c, i) in categories"
          :id="`set-tab-${c.id}`"
          :key="c.id"
          type="button"
          role="tab"
          class="cat-tab"
          :class="{ on: cat === c.id }"
          :aria-selected="cat === c.id"
          :aria-controls="`set-pane-${c.id}`"
          :tabindex="cat === c.id ? 0 : -1"
          @click="cat = c.id"
          @keydown="onTabKey($event, i)"
        >
          {{ c.label }}
        </button>
      </div>
      <div class="save-status" role="status" aria-live="polite" :class="`is-${saveState}`">
        <span
          v-if="saveState === 'saving' || saveState === 'reading'"
          class="save-spinner"
          aria-hidden="true"
        ></span>
        <Icon v-else-if="saveState === 'saved'" icon="line-md:confirm-circle" class="ss-ic" />
        <Icon v-else icon="line-md:close-circle" class="ss-ic" />
        <span class="ss-text">{{ saveText }}</span>
      </div>
    </div>

    <!-- 失败：原因 + 当前生效值说明 + 重试；成功状态绝不伪造 -->
    <div v-if="settingsError" class="save-error" role="alert">
      <Icon icon="line-md:close-circle" class="se-ic" />
      <div class="se-body">
        <span class="se-msg">{{ settingsError }}</span>
        <span class="se-note">
          {{
            settingsReady
              ? "当前生效值可能仍是上次保存的值；可重试写入当前设置。"
              : "设置尚未读取，修改暂不会保存；请先重新读取。"
          }}
        </span>
      </div>
      <el-button v-if="!settingsReady" size="small" @click="loadSettings">重新读取</el-button>
      <el-button v-else size="small" :loading="retrying" @click="retrySave">重试保存</el-button>
    </div>

    <!-- ================= 通用 ================= -->
    <section
      v-show="cat === 'general'"
      id="set-pane-general"
      class="cat-pane"
      role="tabpanel"
      aria-labelledby="set-tab-general"
      tabindex="0"
    >
      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:cog" /><span>启动与行为</span>
            </span>
          </div>
        </template>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">开机自启动</div>
            <p class="row-desc">登录 Windows 后自动启动佳点监控。</p>
          </div>
          <div class="row-ctl">
            <el-switch v-model="autostart" aria-label="开机自启动" @change="toggleAutostart" />
          </div>
        </div>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">静默启动</div>
            <p class="row-desc">启动时不显示窗口，直接收进系统托盘。</p>
            <details class="row-details">
              <summary @click="guardDetail">
                <span class="det-closed">展开详情</span>
                <span class="det-open">收起详情</span>
              </summary>
              <div class="row-detail-body">
                <p>开启后程序启动即最小化到托盘，不弹主窗口；适合配合开机自启动后台接管设备。左键托盘图标恢复窗口，右键退出。下一次启动生效。</p>
              </div>
            </details>
          </div>
          <div class="row-ctl">
            <el-switch v-model="silentStart" aria-label="静默启动" />
          </div>
        </div>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">关闭时收进系统托盘</div>
            <p class="row-desc">关闭窗口时收进托盘继续运行，关掉则直接退出。</p>
            <details class="row-details">
              <summary @click="guardDetail">
                <span class="det-closed">展开详情</span>
                <span class="det-open">收起详情</span>
              </summary>
              <div class="row-detail-body">
                <p>托盘图标：左键恢复窗口，右键退出。退出会先停止当前下发并释放设备会话。</p>
              </div>
            </details>
          </div>
          <div class="row-ctl">
            <el-switch v-model="closeToTray" aria-label="关闭时收进系统托盘" />
          </div>
        </div>
      </el-card>

      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:folder" /><span>设置存储</span>
            </span>
          </div>
        </template>
        <div class="set-row">
          <div class="row-main">
            <div class="row-title">自动保存</div>
            <p class="row-desc">修改后立即写入 exe 同级的 settings.ini（原子替换），无需手动保存。</p>
          </div>
        </div>
        <div class="set-row">
          <div class="row-main">
            <div class="row-title">手工编辑与生效</div>
            <p class="row-desc">settings.ini 可直接编辑，程序启动时读取；静默启动下次启动生效，其余即时生效。</p>
          </div>
        </div>
      </el-card>
    </section>

    <!-- ================= 显示与外观 ================= -->
    <section
      v-show="cat === 'appearance'"
      id="set-pane-appearance"
      class="cat-pane"
      role="tabpanel"
      aria-labelledby="set-tab-appearance"
      tabindex="0"
    >
      <AppearanceCard />
    </section>

    <!-- ================= 性能与数据源 ================= -->
    <section
      v-show="cat === 'perf'"
      id="set-pane-perf"
      class="cat-pane"
      role="tabpanel"
      aria-labelledby="set-tab-perf"
      tabindex="0"
    >
      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:gauge" /><span>性能</span>
            </span>
          </div>
        </template>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">高刷模式</div>
            <p class="row-desc">开：面板与预览约 30fps；关：约 10fps，更省 CPU。指标数值本身每秒刷新一次。</p>
            <details class="row-details">
              <summary @click="guardDetail">
                <span class="det-closed">展开详情</span>
                <span class="det-open">收起详情</span>
              </summary>
              <div class="row-detail-body">
                <p>抓屏节奏与最终下发节奏保持一致，改完立即生效。高刷更适合动画类主题，普通模式对观感影响很小。</p>
              </div>
            </details>
          </div>
          <div class="row-ctl">
            <el-switch v-model="highFps" aria-label="高刷模式" />
          </div>
        </div>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">CPU 占用率自算（PDH）</div>
            <p class="row-desc">关：用数据源给的 CPU 总占用率；开：改由 PDH 自算，支持时输出 P / E 核分组。</p>
            <details class="row-details">
              <summary @click="guardDetail">
                <span class="det-closed">展开详情</span>
                <span class="det-open">收起详情</span>
              </summary>
              <div class="row-detail-body">
                <p>
                  关：使用 LibreHardwareService（LHS）的 CPU 总占用率，不输出 P/E 分组。
                  开：改用 PDH 自算总占用率，并在支持的大小核机型上输出 P 核 / E 核分组。
                  切换后须等待后台确认及新来源采样预热；未就绪或分组不可用时显示「—」，不沿用旧值或补成 0。
                </p>
              </div>
            </details>
          </div>
          <div class="row-ctl">
            <el-switch
              v-model="hybridCompat"
              aria-label="CPU 占用率自算"
              :disabled="settingsSaving || !settingsReady"
            />
          </div>
        </div>
      </el-card>

      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:cloud-alt-download" /><span>数据源</span>
            </span>
            <span class="head-sub">LibreHardwareService</span>
            <el-tag class="head-tag" :type="lhwsTagType" round size="small" effect="plain">
              {{ lhws ? lhws.statusText : "检测中…" }}
            </el-tag>
          </div>
        </template>

        <p class="src-hint">{{ stateHint(lhws) }}</p>

        <!-- 下载 / 安装进度：只在有任务时出现。来源、第几次尝试、速度都写出来，
             用户能判断"是在下"还是"在换源重试"。 -->
        <div v-if="prog?.active" class="lhws-progress">
          <div class="prog-line">
            <span class="prog-msg">{{ prog.message }}</span>
            <span v-if="sizeText" class="prog-meta">{{ sizeText }}</span>
            <span v-if="speedText" class="prog-meta">{{ speedText }}</span>
          </div>
          <el-progress
            :percentage="pct"
            :indeterminate="indeterminate"
            :show-text="false"
            :stroke-width="6"
            :status="prog?.phase === 'failed' ? 'exception' : undefined"
          />
          <div class="prog-line">
            <span class="prog-msg">{{ attemptText }}</span>
            <span v-if="!indeterminate" class="prog-meta">{{ pct }}%</span>
          </div>
        </div>

        <!-- 按钮层级：正常运行时「重新启动 / 重新检测」同为次要；异常才突出恢复；卸载始终危险样式 -->
        <div class="set-actions">
          <template v-if="needInstall">
            <el-button type="primary" :loading="lhwsBusy" @click="installLhws">
              {{ lhwsBusy ? "处理中…" : "一键安装" }}
            </el-button>
            <el-button :disabled="lhwsBusy" @click="refreshLhws">重新检测</el-button>
          </template>
          <template v-else>
            <el-button
              :type="needRecover ? 'primary' : 'default'"
              :loading="lhwsBusy"
              @click="restartLhws"
            >
              {{ lhwsBusy ? "处理中…" : "重新启动" }}
            </el-button>
            <el-button :disabled="lhwsBusy" @click="refreshLhws">重新检测</el-button>
            <el-button
              v-if="lhws?.installed"
              type="danger"
              plain
              :disabled="lhwsBusy"
              @click="askUninstall"
            >
              卸载
            </el-button>
          </template>
        </div>

        <p v-if="lhws?.installed" class="src-impact">
          卸载会移除温度 / 功耗 / 风扇等指标（网络与磁盘容量不受影响）；执行前会先弹确认框，可取消。
        </p>

        <!-- 状态消息常驻占位，避免反复出现/消失时卡片高度跳动 -->
        <p class="src-msg keep" :class="{ bad: lhwsMsgBad }">{{ lhwsMsg }}</p>

        <details class="row-details">
          <summary @click="guardDetail">
            <span class="det-closed">安装与影响说明</span>
            <span class="det-open">收起安装与影响说明</span>
          </summary>
          <div class="row-detail-body">
            <p>安装 / 重新启动 / 卸载都会弹一次 UAC；本程序不修改该组件的任何文件。</p>
            <p>服务未运行时无法读取温度 / 功耗 / 风扇；网络速率与磁盘容量来自系统接口，不受影响。</p>
          </div>
        </details>
      </el-card>
    </section>

    <!-- ================= 日志与单位 ================= -->
    <section
      v-show="cat === 'logs'"
      id="set-pane-logs"
      class="cat-pane"
      role="tabpanel"
      aria-labelledby="set-tab-logs"
      tabindex="0"
    >
      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:list" /><span>运行日志</span>
            </span>
          </div>
        </template>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">输出运行日志</div>
            <p class="row-desc">写入 exe 同级 log/jiadian.log，排查卡顿 / 接管失败 / 断联时用。</p>
            <details class="row-details">
              <summary @click="guardDetail">
                <span class="det-closed">展开详情</span>
                <span class="det-open">收起详情</span>
              </summary>
              <div class="row-detail-body">
                <p>单文件上限 1 MB，超过轮转成 .log.1。关闭后不再写日志，可省掉这部分磁盘写入与内存开销。</p>
              </div>
            </details>
          </div>
          <div class="row-ctl">
            <el-switch v-model="logging" aria-label="输出运行日志" />
          </div>
        </div>

        <!-- 级别只在开启日志时才有意义：关掉时整行隐藏，避免「选了却不生效」 -->
        <div v-if="logging" class="set-row">
          <div class="row-main">
            <div class="row-title">日志级别</div>
            <p class="row-desc">记录哪些级别的日志；这是一个“门槛”，不是分类。</p>
          </div>
          <div class="row-ctl">
            <el-select
              class="set-select w-lg"
              :model-value="logLevel"
              aria-label="日志级别"
              @change="setLogLevel"
            >
              <el-option v-for="o in logLevelOptions" :key="o.label" :label="o.label" :value="o.value" />
            </el-select>
          </div>
        </div>
      </el-card>

      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:hash" /><span>单位设置</span>
            </span>
          </div>
        </template>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">显示单位</div>
            <p class="row-desc">全自动按素材 data-unit 显示；手动则四族统一用下面选定的单位。</p>
          </div>
          <div class="row-ctl">
            <el-select
              class="set-select w-md"
              :model-value="unitMode"
              aria-label="显示单位"
              @change="setUnitMode"
            >
              <el-option v-for="o in unitModeOptions" :key="o.label" :label="o.label" :value="o.value" />
            </el-select>
          </div>
        </div>

        <template v-if="unitMode === 'manual'">
          <div class="set-row">
            <div class="row-main">
              <div class="row-title">容量</div>
              <p class="row-desc">磁盘、显存、内存容量。</p>
            </div>
            <div class="row-ctl">
              <el-select
                class="set-select w-md"
                :model-value="unitCapacity"
                aria-label="容量单位"
                @change="setUnitCapacity"
              >
                <el-option v-for="o in capacityOptions" :key="o.label" :label="o.label" :value="o.value" />
              </el-select>
            </div>
          </div>
          <div class="set-row">
            <div class="row-main">
              <div class="row-title">速率</div>
              <p class="row-desc">网络与磁盘读写速率。</p>
            </div>
            <div class="row-ctl">
              <el-select
                class="set-select w-md"
                :model-value="unitRate"
                aria-label="速率单位"
                @change="setUnitRate"
              >
                <el-option v-for="o in rateOptions" :key="o.label" :label="o.label" :value="o.value" />
              </el-select>
            </div>
          </div>
          <div class="set-row">
            <div class="row-main">
              <div class="row-title">温度</div>
              <p class="row-desc">CPU / GPU / 磁盘温度。</p>
            </div>
            <div class="row-ctl">
              <el-select
                class="set-select w-sm"
                :model-value="unitTemp"
                aria-label="温度单位"
                @change="setUnitTemp"
              >
                <el-option v-for="o in tempOptions" :key="o.label" :label="o.label" :value="o.value" />
              </el-select>
            </div>
          </div>
          <div class="set-row">
            <div class="row-main">
              <div class="row-title">时长</div>
              <p class="row-desc">开机时长等时间类指标。</p>
            </div>
            <div class="row-ctl">
              <el-select
                class="set-select w-md"
                :model-value="unitDuration"
                aria-label="时长单位"
                @change="setUnitDuration"
              >
                <el-option v-for="o in durationOptions" :key="o.label" :label="o.label" :value="o.value" />
              </el-select>
            </div>
          </div>
        </template>

        <p class="cat-note">功耗固定 W；「跟随素材」= 沿用素材里写的该族单位。</p>
      </el-card>
    </section>

    <!-- ================= 关于 ================= -->
    <section
      v-show="cat === 'about'"
      id="set-pane-about"
      class="cat-pane"
      role="tabpanel"
      aria-labelledby="set-tab-about"
      tabindex="0"
    >
      <el-card class="set-card" shadow="never">
        <template #header>
          <div class="panel-head">
            <span class="panel-title">
              <Icon icon="line-md:computer" /><span>开发者与关于</span>
            </span>
          </div>
        </template>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">启用 DevTools</div>
            <p class="row-desc">允许打开开发者工具，仅在排查界面问题时使用。</p>
          </div>
          <div class="row-ctl">
            <el-switch v-model="devtools" aria-label="启用 DevTools" />
          </div>
        </div>

        <div class="set-row">
          <div class="row-main">
            <div class="row-title">打开主窗口 DevTools</div>
            <p class="row-desc">需先启用 DevTools；关闭窗口后再次打开需重新点击。</p>
          </div>
          <div class="row-ctl">
            <el-button :disabled="!devtools" @click="openDevtools">打开</el-button>
          </div>
        </div>

        <div class="about-block">
          <div class="about-line">
            <span class="about-name">佳点监控</span>
            <span v-if="appVersion" class="about-ver">v{{ appVersion }}</span>
          </div>
          <p class="row-desc">
            数据源组件 LibreHardwareService {{ lhws?.version || "—" }}（{{ lhws?.license || "MPL-2.0" }}）
            <span class="about-links">
              · <a @click="openUrl(lhws?.source || '')">来源</a>
              · <a @click="openUrl(lhws?.licenseUrl || '')">许可</a>
            </span>
          </p>
          <p class="row-desc">
            本程序许可：<b>{{ APP_LICENSE }}</b>
            <span class="about-links">· <a @click="openUrl(APP_LICENSE_URL)">许可全文</a></span>
          </p>
        </div>

        <div class="about-oss">
          <button
            type="button"
            class="oss-toggle"
            :aria-expanded="ossOpen"
            @click="ossOpen = !ossOpen"
          >
            <Icon :icon="ossOpen ? 'line-md:chevron-up' : 'line-md:chevron-down'" class="oss-toggle-ic" />
            <span class="oss-head-title">开源项目与许可</span>
            <span class="oss-head-desc">共 {{ ossCount }} 项直接依赖</span>
            <span class="oss-toggle-text">{{ ossOpen ? "收起" : "展开" }}</span>
          </button>
          <p class="oss-note">
            含 LibreHardwareService（MPL-2.0）与 Microsoft WebView2 Runtime（系统组件，非开源）；
            完整传递依赖与许可证声明见程序目录下的 {{ NOTICES_FILE }}。
          </p>
          <div v-show="ossOpen" class="oss-body">
            <div v-for="g in OSS_GROUPS" :key="g.id" class="oss-group">
            <div class="oss-group-title">
              {{ g.title }}<span class="oss-group-desc">{{ g.desc }}</span>
            </div>
            <ul class="oss-list">
              <li v-for="it in g.items" :key="g.id + ':' + it.name" class="oss-item">
                <span class="oss-name" :title="it.name">{{ it.name }}</span>
                <span class="oss-ver">{{ it.version }}</span>
                <el-tag size="small" effect="plain" class="oss-lic">{{ it.license }}</el-tag>
                <a v-if="it.url" class="oss-link" :title="it.url" @click="openUrl(it.url)">主页</a>
              </li>
              </ul>
            </div>
          </div>
        </div>
      </el-card>
    </section>

    <!-- 卸载确认：用 EP 弹窗替代原生 confirm()，写清影响范围与取消路径 -->
    <el-dialog v-model="uninstallOpen" title="卸载 LibreHardwareService" width="360px" align-center>
      <p class="dlg-text">确定要卸载 LibreHardwareService 吗？</p>
      <p class="dlg-text">卸载后温度 / 功耗 / 风扇等指标将不可用（网络、磁盘容量仍正常）。</p>
      <p class="dlg-text">需要时可重新「一键安装」恢复；点「取消」不会做任何改动。</p>
      <template #footer>
        <el-button @click="uninstallOpen = false">取消</el-button>
        <el-button type="danger" @click="confirmUninstall">确定卸载</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
/* 整个设置页：固定高度，头部（分类 + 保存状态）不动，仅分类内容区滚动。
   Element Plus 的少量 CSS 变量在这里对齐 app 的浅/深色主题（变量本身来自 styles.css）。 */
.settings {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  --el-border-radius-base: var(--r-ctl);
  --el-text-color-primary: var(--fg);
  --el-text-color-regular: var(--fg);
  --el-text-color-secondary: var(--sub);
  --el-text-color-placeholder: var(--sub);
  --el-border-color: var(--line);
  --el-border-color-light: var(--line);
  --el-border-color-lighter: var(--line);
  --el-fill-color-blank: var(--btn);
  --el-bg-color-overlay: var(--panel);
}

/* ---- 分类入口 + 保存状态 ---- */
.settings-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
  border-bottom: 1px solid var(--line);
}
.cat-tabs {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  overflow: hidden;
}
.cat-tab {
  position: relative;
  flex: none;
  min-height: 32px;
  margin-right: 18px;
  padding: 0 1px;
  border: none;
  background: transparent;
  color: var(--sub);
  font-size: 12.5px;
  line-height: 1.2;
  white-space: nowrap;
  cursor: pointer;
}
.cat-tab:last-child {
  margin-right: 0;
}
.cat-tab:hover {
  color: var(--fg);
}
.cat-tab.on {
  color: var(--accent);
  font-weight: 600;
}
.cat-tab.on::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  bottom: -1px;
  height: 2px;
  border-radius: 2px;
  background: var(--accent);
}

.save-status {
  flex: none;
  display: flex;
  align-items: center;
  gap: 5px;
  min-height: 20px;
  max-width: 200px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.4;
}
.save-status.is-saved .ss-ic {
  color: var(--ok);
}
.save-status.is-failed .ss-ic,
.save-status.is-readFailed .ss-ic {
  color: var(--danger);
}
.save-status.is-failed .ss-text,
.save-status.is-readFailed .ss-text {
  color: var(--danger);
}
.save-status.is-saving .ss-text {
  color: var(--accent);
}
.ss-ic {
  flex: none;
  font-size: 13px;
}
.ss-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.save-spinner {
  flex: none;
  width: 10px;
  height: 10px;
  border: 1.5px solid var(--line);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: set-spin 0.8s linear infinite;
}
@keyframes set-spin {
  to {
    transform: rotate(360deg);
  }
}

/* ---- 保存失败条 ---- */
.save-error {
  flex: none;
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 6px 8px;
  border: 1px solid color-mix(in srgb, var(--danger) 42%, transparent);
  border-radius: var(--r-ctl);
  background: color-mix(in srgb, var(--danger) 10%, transparent);
  font-size: 11.5px;
  line-height: 1.5;
}
.se-ic {
  flex: none;
  margin-top: 1px;
  color: var(--danger);
  font-size: 14px;
}
.se-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.se-msg {
  color: var(--danger);
  overflow-wrap: anywhere;
}
.se-note {
  color: var(--sub);
}

/* ---- 分类内容：唯一滚动区 ---- */
.cat-pane {
  flex: 1;
  min-height: 0;
  overflow: auto;
  overflow-x: hidden;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 1px 2px 4px 0;
}
.cat-pane:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.cat-note {
  margin: 0;
  padding: 0 2px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
}

/* ---- 子卡片 ---- */
.set-card {
  --el-card-border-color: var(--line);
  --el-card-bg-color: var(--panel);
  flex: none;
  overflow: hidden;
}
.set-card :deep(.el-card__header) {
  padding: 8px 12px;
  border-bottom: 1px solid var(--line);
}
.set-card :deep(.el-card__body) {
  padding: 2px 12px 8px;
}
.head-sub {
  margin-left: 2px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.head-tag {
  margin-left: auto;
  flex: none;
}

/* ---- 纵向设置行：一项一句用途 ---- */
.set-row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 7px 0;
}
.set-row + .set-row {
  border-top: 1px solid var(--line);
}
.row-main {
  flex: 1;
  min-width: 0;
}
.row-title {
  color: var(--fg);
  font-size: 12px;
  font-weight: 500;
  line-height: 1.4;
}
.row-desc {
  margin: 1px 0 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.row-ctl {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 26px;
}
.row-ctl :deep(.el-button) {
  margin-left: 0;
}
/* 点击目标 ≥ 26px：EP 开关默认 24 高，补足可点击高度（视觉尺寸不变） */
.row-ctl :deep(.el-switch) {
  min-height: 26px;
}
.set-select {
  flex: none;
}
.set-select.w-lg {
  width: 140px;
}
.set-select.w-md {
  width: 120px;
}
.set-select.w-sm {
  width: 96px;
}

/* ---- 就地折叠的详情 ---- */
.row-details {
  margin-top: 2px;
}
.row-details > summary {
  list-style: none;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 26px;
  margin-left: -2px;
  padding: 0 2px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.4;
  cursor: pointer;
  user-select: none;
}
.row-details > summary::-webkit-details-marker {
  display: none;
}
.row-details > summary::before {
  content: "";
  width: 5px;
  height: 5px;
  margin-left: 1px;
  border-right: 1.5px solid currentColor;
  border-bottom: 1.5px solid currentColor;
  transform: rotate(-45deg);
  transition: transform 0.12s ease;
}
.row-details[open] > summary::before {
  transform: rotate(45deg);
}
.row-details > summary:hover {
  color: var(--fg);
}
.row-details > summary:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
  border-radius: 3px;
}
.det-open {
  display: none;
}
.row-details[open] .det-open {
  display: inline;
}
.row-details[open] .det-closed {
  display: none;
}
.row-detail-body {
  margin: 2px 0 2px;
  padding: 5px 0 5px 8px;
  border-left: 2px solid var(--line);
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.65;
  overflow-wrap: anywhere;
}
.row-detail-body p {
  margin: 0;
}
.row-detail-body p + p {
  margin-top: 4px;
}
.row-detail-body a {
  color: var(--accent);
  cursor: pointer;
}

/* ---- 数据源 ---- */
.src-hint {
  margin: 4px 0 0;
  color: var(--fg);
  font-size: 12px;
  line-height: 1.5;
}
.src-impact {
  margin: 6px 0 0;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
}
.src-msg {
  margin: 4px 0 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.src-msg.keep {
  min-height: 18px;
}
.src-msg.bad {
  color: var(--danger);
}
.set-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin: 10px 0 0;
}
/* 卸载按钮保持危险语义，但浅色下 EP 默认 plain 底色对比度只有 4.35:1；改为面板底色。 */
.set-actions :deep(.el-button--danger.is-plain) {
  --el-button-text-color: var(--danger);
  --el-button-bg-color: var(--panel);
  --el-button-border-color: color-mix(in srgb, var(--danger) 45%, var(--line));
  --el-button-hover-text-color: var(--danger);
  --el-button-hover-bg-color: color-mix(in srgb, var(--danger) 10%, var(--panel));
  --el-button-hover-border-color: var(--danger);
  --el-button-active-text-color: var(--danger);
  --el-button-active-bg-color: color-mix(in srgb, var(--danger) 16%, var(--panel));
  --el-button-active-border-color: var(--danger);
}
.lhws-progress {
  margin: 8px 0 0;
}
.prog-line {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 4px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
}
.prog-line:last-child {
  margin: 4px 0 0;
}
.prog-msg {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.prog-meta {
  flex: none;
  font-variant-numeric: tabular-nums;
}
.lhws-progress :deep(.el-progress-bar__outer) {
  background-color: var(--btn);
}
.lhws-progress :deep(.el-progress-bar__inner) {
  border-radius: 999px;
}

/* ---- 关于 ---- */
.about-block {
  margin-top: 4px;
  padding-top: 10px;
  border-top: 1px solid var(--line);
}
.about-line {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 3px;
  font-size: 12px;
  line-height: 1.5;
  color: var(--sub);
}
.about-name {
  color: var(--fg);
  font-weight: 600;
}
.about-ver {
  color: var(--sub);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
}
.about-links {
  color: var(--sub);
}
.about-links a {
  color: var(--accent);
  cursor: pointer;
}
.about-oss {
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--line);
}
.oss-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  min-height: 28px;
  padding: 0 4px;
  border: none;
  border-radius: var(--r-ctl);
  background: transparent;
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
  text-align: left;
  cursor: pointer;
}
.oss-toggle:hover {
  background: var(--btn);
}
.oss-toggle:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}
.oss-toggle-ic {
  flex: none;
  color: var(--sub);
  font-size: 14px;
}
.oss-toggle-text {
  margin-left: auto;
  color: var(--accent);
  font-size: 11px;
  font-weight: 500;
}
.oss-note {
  margin: 2px 0 0 20px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
}
.oss-body {
  margin: 6px 0 0 20px;
}
.oss-head-title {
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
}
.oss-head-desc {
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
}
.oss-group {
  margin-bottom: 8px;
}
.oss-group-title {
  color: var(--fg);
  font-size: 11.5px;
  font-weight: 600;
  margin-bottom: 2px;
}
.oss-group-desc {
  margin-left: 6px;
  color: var(--sub);
  font-weight: 400;
  font-size: 10.5px;
}
.oss-list {
  margin: 0;
  padding: 0;
  list-style: none;
}
.oss-item {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 22px;
  padding: 0 2px;
  border-radius: var(--r-ctl);
}
.oss-item:hover {
  background: var(--btn);
}
.oss-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg);
  font-size: 11.5px;
}
.oss-ver {
  flex: none;
  color: var(--sub);
  font-size: 10.5px;
  font-variant-numeric: tabular-nums;
}
.oss-lic {
  flex: none;
  height: 18px;
  padding: 0 5px;
  font-size: 10px;
}
.oss-link {
  flex: none;
  color: var(--accent);
  font-size: 11px;
  cursor: pointer;
}
.oss-link:hover {
  text-decoration: underline;
}

/* ---- 卸载确认弹窗 ---- */
.dlg-text {
  margin: 0 0 6px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--fg);
}
.dlg-text:last-child {
  margin-bottom: 0;
}
</style>

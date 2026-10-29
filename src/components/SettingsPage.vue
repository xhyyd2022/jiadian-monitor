<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { Icon } from "@iconify/vue";
import { invoke } from "@tauri-apps/api/core";
import { useSettings } from "../composables/useSettings";
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
  toggleAutostart,
  openDevtools,
} = useSettings();

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

/// 状态色：ok=绿（正常）· warn=黄（装好了但没在跑，可一键修）· off=灰（未安装）
function stateClass(s: LhwsState | null): string {
  if (!s) return "off";
  if (s.status === "running") return "ok";
  if (s.status === "notInstalled") return "off";
  return "warn";
}

/// 状态下方的一句解释（点明"装好了但没在跑"和"没装"的区别）
function stateHint(s: LhwsState | null): string {
  if (!s) return "";
  switch (s.status) {
    case "running":
      return "服务在跑并在发布数据。";
    case "starting":
      return "服务进程已启动，但还没发布数据（初始化通常 5~7 秒）。稍等，或点「重新启动」。";
    case "stopped":
      return "服务已安装，但进程没在运行 —— 点「重新启动」即可拉起（会弹一次 UAC）。";
    case "notInstalled":
      return "未安装。装好后才有温度 / 功耗 / 风扇等指标（网络与磁盘容量不受影响）。";
    default:
      return "拿不到服务状态，可点「重新检测」。";
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

async function uninstallLhws() {
  if (!confirm("确定要卸载 LibreHardwareService 吗？\n\n卸载后温度 / 功耗 / 风扇等指标将不可用（网络、磁盘容量仍正常）。")) {
    return;
  }
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

const openUrl = (url: string) => invoke("open_url", { url }).catch(() => {});

onMounted(() => {
  refreshLhws();
});

// 切走页面时把进度轮询停掉：安装还在后台继续，但没必要在看不见的页面上每 300ms 打一次
onUnmounted(() => {
  unwatchProgress();
});
</script>

<template>
  <div>
    <!-- 保存状态常驻一行：v-if 会让每次改设置时整页上下跳一下 -->
    <div class="savebar" role="status" aria-live="polite">
      <span v-if="settingsSaving" class="hint">正在保存设置…</span>
      <span v-else-if="settingsError" class="err">{{ settingsError }}</span>
    </div>
    <button v-if="!settingsReady && settingsError" @click="loadSettings">重新读取设置</button>
    <div class="card">
      <h2><Icon icon="line-md:play" />启动</h2>
      <div class="row">
        <span class="lbl">开机自启动</span>
        <label class="sw">
          <input type="checkbox" v-model="autostart" @change="toggleAutostart" />
        </label>
      </div>
      <div class="row">
        <span class="lbl">静默启动</span>
        <label class="sw">
          <input type="checkbox" v-model="silentStart" />
        </label>
      </div>
      <div class="row">
        <span class="lbl">关闭时收进系统托盘</span>
        <label class="sw"><input type="checkbox" v-model="closeToTray" /></label>
      </div>
      <p class="hint">左键恢复窗口，右键退出；关掉则关闭即退出。</p>
      <p class="hint">静默启动：启动时不显示窗口，直接收进托盘（左键托盘图标恢复）。适合配合开机自启动，开机后后台自动接管设备、不弹窗。</p>
      <p class="hint">开机自启动写入当前用户注册表，免管理员；其余设置都存在 exe 同级的 <code>settings.ini</code>（可直接手工编辑，程序启动时读取）。</p>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:gauge" />性能</h2>
      <div class="row">
        <span class="lbl">高刷模式</span>
        <label class="sw">
          <input type="checkbox" v-model="highFps" />
        </label>
      </div>
      <p class="hint">
        开 = 面板与预览 ~30fps（动画更顺滑，CPU / 带宽更高）；
        关 = 普通 ~10fps（省资源，指标本来就每秒才刷新一次，看数值不受影响）。
        抓屏节奏与最终下发节奏保持一致，改完立即生效。
      </p>
      <div class="row">
        <span class="lbl">CPU 占用率自算（PDH）</span>
        <label class="sw">
          <input type="checkbox" v-model="hybridCompat" :disabled="settingsSaving || !settingsReady" />
        </label>
      </div>
      <p class="hint">
        关：使用 LibreHardwareService（LHS）的 CPU 总占用率，不输出 P/E 分组。
        开：改用 PDH 自算总占用率，并在支持的大小核机型上输出 P 核 / E 核分组。
        切换后须等待后台确认及新来源采样预热；未就绪或分组不可用时显示「—」，不沿用旧值或补成 0。
      </p>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:list" />运行日志</h2>
      <div class="row">
        <span class="lbl">输出运行日志</span>
        <label class="sw">
          <input type="checkbox" v-model="logging" />
        </label>
      </div>
      <!-- 级别只在开启日志时才有意义：关掉时整行隐藏，避免出现「选了却不生效」的选项 -->
      <div v-if="logging" class="row">
        <span class="lbl">日志级别</span>
        <select v-model="logLevel" class="lv">
          <option value="info">全部</option>
          <option value="warn">警告及以上</option>
          <option value="error">仅错误</option>
        </select>
      </div>
      <p class="hint">
        开启时把运行日志写进 <code>log/jiadian.log</code>（单文件上限 1 MB，超过轮转成
        <code>.log.1</code>）。三个级别是「门槛」不是分类：全部 = 普通/警告/错误都记、
        警告及以上 = 只记警告与错误、仅错误 = 只记错误。排查面板切换卡顿 / 接管失败 /
        长时间断联时用；关闭后不再写日志，省掉这部分磁盘写入与内存开销。
      </p>
    </div>

    <div class="card unit">
      <h2><Icon icon="line-md:cog" />单位设置</h2>
      <div class="row">
        <span class="lbl">显示单位</span>
        <select v-model="unitMode">
          <option value="auto">全自动</option>
          <option value="manual">手动指定</option>
        </select>
      </div>
      <template v-if="unitMode === 'manual'">
        <div class="row">
          <span class="lbl">容量</span>
          <select v-model="unitCapacity">
            <option value="">跟随素材</option>
            <option>B</option><option>KB</option><option>MB</option><option>GB</option><option>TB</option>
          </select>
        </div>
        <div class="row">
          <span class="lbl">速率</span>
          <select v-model="unitRate">
            <option value="">跟随素材</option>
            <option>B/s</option><option>KB/s</option><option>MB/s</option><option>GB/s</option>
          </select>
        </div>
        <div class="row">
          <span class="lbl">温度</span>
          <select v-model="unitTemp">
            <option value="">跟随素材</option>
            <option>℃</option><option>℉</option>
          </select>
        </div>
        <div class="row">
          <span class="lbl">时长</span>
          <select v-model="unitDuration">
            <option value="">跟随素材</option>
            <option>秒</option><option>分</option><option>小时</option><option>天</option>
          </select>
        </div>
      </template>
      <p class="hint">
        全自动：素材写 <code>data-unit</code> 就按它显示。手动指定：容量 / 速率 / 温度 / 时长
        四族统一用这里选定的单位（覆盖素材），改完立刻生效。功耗固定 W。
      </p>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:cloud-alt-download" />数据源</h2>
      <div class="row">
        <span class="lbl">LibreHardwareService</span>
        <span class="dot" :class="stateClass(lhws)"></span>
        <span class="val">{{ lhws ? lhws.statusText : "检测中…" }}</span>
      </div>
      <!-- 下载 / 安装进度：只在有任务时出现。来源、第几次尝试、速度都写出来，
           用户能判断"是在下"还是"在换源重试"。 -->
      <div v-if="prog?.active" class="prog">
        <div class="prog-head">
          <span class="grow">{{ prog.message }}</span>
          <span v-if="sizeText">{{ sizeText }}</span>
          <span v-if="speedText">{{ speedText }}</span>
        </div>
        <div class="prog-track">
          <div
            class="prog-bar"
            :class="{ indet: indeterminate, fail: prog.phase === 'failed' }"
            :style="indeterminate ? undefined : { width: pct + '%' }"
          ></div>
        </div>
        <div class="prog-head">
          <span class="grow">{{ attemptText }}</span>
          <span v-if="!indeterminate">{{ pct }}%</span>
        </div>
      </div>
      <div class="btns" :class="{ two: !lhws?.installed }">
        <button v-if="!lhws?.installed" class="primary" @click="installLhws" :disabled="lhwsBusy">
          {{ lhwsBusy ? "处理中…" : "一键安装" }}
        </button>
        <button v-else @click="restartLhws" :disabled="lhwsBusy">
          {{ lhwsBusy ? "处理中…" : "重新启动" }}
        </button>
        <button @click="refreshLhws" :disabled="lhwsBusy">重新检测</button>
        <button v-if="lhws?.installed" class="danger" @click="uninstallLhws" :disabled="lhwsBusy">卸载</button>
      </div>
      <!-- 两行状态常驻（空内容也占位）：重新检测时状态文案会反复出现/消失，
           卡片高度一跳动整页就跟着动 -->
      <p class="hint keep">{{ lhws && !lhwsBusy ? stateHint(lhws) : "" }}</p>
      <p class="hint keep">{{ lhwsMsg }}</p>
      <p class="hint">
        第三方开源组件 {{ lhws?.version }}（{{ lhws?.license }}）·
        <a @click="openUrl(lhws?.source || '')">来源</a> ·
        <a @click="openUrl(lhws?.licenseUrl || '')">许可</a>
      </p>
      <p class="hint">安装 / 重新启动 / 卸载都会弹一次 UAC；不修改该组件的任何文件。</p>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:construction" />开发者工具</h2>
      <div class="row">
        <span class="lbl">启用 DevTools</span>
        <label class="sw"><input type="checkbox" v-model="devtools" /></label>
      </div>
      <div class="row">
        <span class="lbl">打开主窗口 DevTools</span>
        <button @click="openDevtools" :disabled="!devtools">打开</button>
      </div>
    </div>
  </div>
</template>

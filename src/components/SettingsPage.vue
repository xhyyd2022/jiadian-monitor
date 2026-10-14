<script setup lang="ts">
import { onMounted, ref } from "vue";
import { Icon } from "@iconify/vue";
import { invoke } from "@tauri-apps/api/core";
import { useSettings } from "../composables/useSettings";

const {
  autostart,
  silentStart,
  closeToTray,
  devtools,
  unitMode,
  unitCapacity,
  unitRate,
  unitTemp,
  unitDuration,
  toggleAutostart,
  loadSilentStart,
  toggleSilentStart,
  openDevtools,
} = useSettings();

// ---- 数据源：LibreHardwareService（第三方组件，MPL-2.0）----
interface LhwsState {
  version: string;
  installed: boolean;
  running: boolean;
  source: string;
  license: string;
  licenseUrl: string;
  msi: string | null;
}
const lhws = ref<LhwsState | null>(null);
const lhwsBusy = ref(false);
const lhwsMsg = ref("");

function stateText(s: LhwsState | null): string {
  if (!s) return "检测中…";
  if (s.running) return "运行中";
  if (s.installed) return "已安装，未运行";
  return "未安装";
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
  lhwsMsg.value = "正在下载并安装（会弹一次 UAC）…";
  try {
    lhws.value = await invoke<LhwsState>("lhws_install");
    lhwsMsg.value = "安装完成";
  } catch (e) {
    lhwsMsg.value = `安装失败：${e}`;
  } finally {
    lhwsBusy.value = false;
  }
}

const openUrl = (url: string) => invoke("open_url", { url }).catch(() => {});

onMounted(() => {
  refreshLhws();
  loadSilentStart();
});
</script>

<template>
  <div>
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
          <input type="checkbox" v-model="silentStart" @change="toggleSilentStart" />
        </label>
      </div>
      <div class="row">
        <span class="lbl">关闭时收进系统托盘</span>
        <label class="sw"><input type="checkbox" v-model="closeToTray" /></label>
      </div>
      <p class="hint">左键恢复窗口，右键退出；关掉则关闭即退出。</p>
      <p class="hint">静默启动：启动时不显示窗口，直接收进托盘（左键托盘图标恢复）。适合配合开机自启动，开机后后台自动接管设备、不弹窗。</p>
      <p class="hint">开机自启动写入当前用户注册表，免管理员。</p>
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
        <span class="val">{{ stateText(lhws) }}</span>
      </div>
      <div class="btns two">
        <button class="primary" @click="installLhws" :disabled="lhwsBusy || !!lhws?.running">
          {{ lhwsBusy ? "安装中…" : "一键安装" }}
        </button>
        <button @click="refreshLhws" :disabled="lhwsBusy">重新检测</button>
      </div>
      <p v-if="lhwsMsg" class="hint">{{ lhwsMsg }}</p>
      <p class="hint">
        第三方开源组件 {{ lhws?.version }}（{{ lhws?.license }}）·
        <a @click="openUrl(lhws?.source || '')">来源</a> ·
        <a @click="openUrl(lhws?.licenseUrl || '')">许可</a>
      </p>
      <p class="hint">安装时系统会弹一次 UAC；不修改该组件的任何文件。</p>
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

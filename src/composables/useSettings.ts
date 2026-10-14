import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

// 单例：所有调用者共享同一份设置状态（模块级 ref）
const theme = ref<"dark" | "light">("dark");
const devtools = ref(false);
const autostart = ref(false);
/// 静默启动：启动时不显示窗口，直接收进托盘。**真值在 exe 同级的 `silent.flag` 文件**，
/// 因为窗口显示与否要在后端 setup() 里决定（那时前端还没跑）。
const silentStart = ref(false);
const closeToTray = ref(true);

// ---- 单位设置 ----
/// 显示单位模式：`auto` 全自动（按素材 data-unit）/ `manual` 手动指定（按下面四族）
const unitMode = ref<"auto" | "manual">("auto");
/// 各族目标单位；空串 = 该族跟随素材。族名与后端 / 文档一致。
/// **功耗不在这里**：电脑功耗固定 W，不做 kW，没有可切换的余地。
const unitCapacity = ref(""); // 容量：B KB MB GB TB
const unitRate = ref(""); // 速率：B/s KB/s MB/s GB/s
const unitTemp = ref(""); // 温度：℃ ℉
const unitDuration = ref(""); // 时长：秒 分 小时 天

const SETTINGS_KEY = "jiaidian.settings";

function loadSettings() {
  try {
    const s = JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}");
    if (s.theme === "light" || s.theme === "dark") theme.value = s.theme;
    if (typeof s.devtools === "boolean") devtools.value = s.devtools;
    if (typeof s.closeToTray === "boolean") closeToTray.value = s.closeToTray;
    if (s.unitMode === "manual" || s.unitMode === "auto") unitMode.value = s.unitMode;
    if (typeof s.unitCapacity === "string") unitCapacity.value = s.unitCapacity;
    if (typeof s.unitRate === "string") unitRate.value = s.unitRate;
    if (typeof s.unitTemp === "string") unitTemp.value = s.unitTemp;
    if (typeof s.unitDuration === "string") unitDuration.value = s.unitDuration;
  } catch {
    /* 忽略损坏的设置 */
  }
  syncUnits();
}

/// 把单位设置推给后端（面板注入脚本每秒拉 `GET /units`，所以改完立刻生效）。
function syncUnits() {
  invoke("set_unit_config", {
    mode: unitMode.value,
    units: {
      capacity: unitCapacity.value,
      rate: unitRate.value,
      temperature: unitTemp.value,
      duration: unitDuration.value,
    },
  }).catch(() => {});
}

// 持久化 + 同步窗口主题（原生标题栏跟随深浅）
watch([theme, devtools, closeToTray], () => {
  persist();
  invoke("set_window_theme", { dark: theme.value === "dark" }).catch(() => {});
});

function persist() {
  localStorage.setItem(
    SETTINGS_KEY,
    JSON.stringify({
      theme: theme.value,
      devtools: devtools.value,
      closeToTray: closeToTray.value,
      unitMode: unitMode.value,
      unitCapacity: unitCapacity.value,
      unitRate: unitRate.value,
      unitTemp: unitTemp.value,
      unitDuration: unitDuration.value,
    }),
  );
}

// 单位设置：改动即生效
watch(
  [unitMode, unitCapacity, unitRate, unitTemp, unitDuration],
  () => {
    persist();
    syncUnits();
  },
);

async function loadAutostart() {
  try {
    autostart.value = await invoke<boolean>("autostart_get");
  } catch {
    /* 忽略 */
  }
}

async function toggleAutostart() {
  try {
    await invoke<string>("autostart_set", { value: autostart.value });
  } catch {
    autostart.value = !autostart.value;
  }
}

async function loadSilentStart() {
  try {
    silentStart.value = await invoke<boolean>("silent_start_get");
  } catch {
    /* 忽略 */
  }
}

async function toggleSilentStart() {
  try {
    await invoke<string>("set_silent_start", { on: silentStart.value });
  } catch {
    silentStart.value = !silentStart.value; // 写失败则回滚开关
  }
}

async function openDevtools() {
  if (!devtools.value) return;
  try {
    await invoke("open_devtools");
  } catch {
    /* 忽略 */
  }
}

export function useSettings() {
  return {
    theme,
    devtools,
    autostart,
    silentStart,
    closeToTray,
    unitMode,
    unitCapacity,
    unitRate,
    unitTemp,
    unitDuration,
    loadSettings,
    loadAutostart,
    toggleAutostart,
    loadSilentStart,
    toggleSilentStart,
    openDevtools,
  };
}

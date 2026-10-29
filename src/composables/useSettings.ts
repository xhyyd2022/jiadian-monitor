import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

// 单例：所有调用者共享同一份设置状态（模块级 ref）。
//
// **真值在 exe 同级的 `settings.ini`**（后端唯一持有）：程序启动时读进内存，
// 前端起来后 `settings_get` 拿一份、改任何一项都整体 `settings_set` 写回（原子替换文件）。
// 前端这些 ref 只是投影 —— 之所以不放 localStorage：静默启动 / 日志开关必须在前端
// 跑起来之前就知道，只有文件能在那一刻读到。
// 唯一的例外是「开机自启动」：它的真值在注册表，走 `autostart_get` / `autostart_set`。
const theme = ref<"dark" | "light">("dark");
const devtools = ref(false);
const autostart = ref(false);
/// 静默启动：启动时不显示窗口，直接收进托盘。**下次启动生效**。
const silentStart = ref(false);
const closeToTray = ref(true);
/// 运行日志输出：关掉后不再写 `log/jiadian.log`，省掉这部分磁盘 I/O。
const logging = ref(true);
/// 日志级别门槛：`info` 全部 / `warn` 只记警告与错误 / `error` 只记错误。
/// **只在 `logging` 开启时才有意义**（关掉日志时界面不显示这一项）。
const logLevel = ref<"info" | "warn" | "error">("info");
/// 高刷模式：开 = 面板 / 预览 ~30fps；关 = 普通 ~10fps（省 CPU）。
const highFps = ref(false);
/// CPU 占用率来源：关闭用 LHS，开启用 PDH 自算，并在可用时输出 P/E 分组。
/// 自算预热或分组不可用时保持缺值，不用旧来源或 0 冒充。
const hybridCompat = ref(false);
const settingsError = ref("");
const settingsSaving = ref(false);
const settingsReady = ref(false);

// ---- 单位设置 ----
/// 显示单位模式：`auto` 全自动（按素材 data-unit）/ `manual` 手动指定（按下面四族）
const unitMode = ref<"auto" | "manual">("auto");
/// 各族目标单位；空串 = 该族跟随素材。**功耗不在这里**（固定 W，没有可切换的余地）。
const unitCapacity = ref(""); // 容量：B KB MB GB TB
const unitRate = ref(""); // 速率：B/s KB/s MB/s GB/s
const unitTemp = ref(""); // 温度：℃ ℉
const unitDuration = ref(""); // 时长：秒 分 小时 天

/// `settings.ini` 里那份设置（后端 `settings_get` 的返回结构）。
interface Settings {
  silentStart: boolean;
  logging: boolean;
  logLevel: "info" | "warn" | "error";
  highFps: boolean;
  hybridCompat: boolean;
  theme: "dark" | "light";
  closeToTray: boolean;
  devtools: boolean;
  unitMode: "auto" | "manual";
  unitCapacity: string;
  unitRate: string;
  unitTemp: string;
  unitDuration: string;
}

const settingRefs = {
  theme, devtools, closeToTray, silentStart, logging, logLevel, highFps, hybridCompat,
  unitMode, unitCapacity, unitRate, unitTemp, unitDuration,
};
const settingKeys = Object.keys(settingRefs) as (keyof Settings)[];
const revisions = Object.fromEntries(settingKeys.map((key) => [key, 0])) as Record<keyof Settings, number>;
let applying = false;
let confirmed: Settings | null = null;
let loadPromise: Promise<void> | null = null;
const saves: { snapshot: Settings; revisions: Record<keyof Settings, number> }[] = [];

function snapshot(): Settings {
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
  };
}

function applySettings(s: Partial<Settings>) {
  applying = true;
  try {
    if (s.theme === "light" || s.theme === "dark") theme.value = s.theme;
    if (typeof s.devtools === "boolean") devtools.value = s.devtools;
    if (typeof s.closeToTray === "boolean") closeToTray.value = s.closeToTray;
    if (typeof s.silentStart === "boolean") silentStart.value = s.silentStart;
    if (typeof s.logging === "boolean") logging.value = s.logging;
    if (s.logLevel === "info" || s.logLevel === "warn" || s.logLevel === "error") {
      logLevel.value = s.logLevel;
    }
    if (typeof s.highFps === "boolean") highFps.value = s.highFps;
    if (typeof s.hybridCompat === "boolean") hybridCompat.value = s.hybridCompat;
    if (s.unitMode === "manual" || s.unitMode === "auto") unitMode.value = s.unitMode;
    if (typeof s.unitCapacity === "string") unitCapacity.value = s.unitCapacity;
    if (typeof s.unitRate === "string") unitRate.value = s.unitRate;
    if (typeof s.unitTemp === "string") unitTemp.value = s.unitTemp;
    if (typeof s.unitDuration === "string") unitDuration.value = s.unitDuration;
  } finally {
    applying = false;
  }
}

/// 同步 watch 配合 applying 闸门，初始化和失败回滚都不会被排队成用户保存。
function loadSettings(): Promise<void> {
  if (settingsReady.value) return Promise.resolve();
  if (loadPromise) return loadPromise;
  loadPromise = invoke<Settings>("settings_get").then((s) => {
    applySettings(s);
    confirmed = snapshot();
    settingsReady.value = true;
    settingsError.value = "";
  }).catch((e) => {
    settingsError.value = `读取设置失败，暂不保存更改：${e}`;
  }).finally(() => {
    loadPromise = null;
  });
  return loadPromise;
}

/// 每次用户修改捕获整份快照，所有 settings_set 按修改顺序逐一等待后台确认。
async function drainSaves() {
  if (settingsSaving.value) return;
  settingsSaving.value = true;
  try {
    while (saves.length) {
      const save = saves.shift()!;
      try {
        await invoke("settings_set", { patch: save.snapshot });
        confirmed = { ...save.snapshot };
      } catch (e) {
        settingsError.value = `保存设置失败，未确认的更改已回滚：${e}`;
        const rollback = snapshot();
        const accepted = confirmed!;
        for (const key of settingKeys) {
          // 同字段后来又编辑过就保留；后续整份快照中继承的失败值则同步回滚。
          if (revisions[key] === save.revisions[key]) {
            Object.assign(rollback, { [key]: accepted[key] });
          }
          for (const pending of saves) {
            if (pending.revisions[key] === save.revisions[key]) {
              Object.assign(pending.snapshot, { [key]: accepted[key] });
            }
          }
        }
        applySettings(rollback);
      }
    }
  } finally {
    settingsSaving.value = false;
  }
}

watch(snapshot, (next, previous) => {
  if (applying || !settingsReady.value) return;
  for (const key of settingKeys) {
    if (next[key] !== previous[key]) revisions[key]++;
  }
  settingsError.value = "";
  saves.push({ snapshot: next, revisions: { ...revisions } });
  void drainSaves();
}, { flush: "sync" });

// 原生主题也跟随初始化和失败回滚，但不触发设置回写。
watch(theme, () => {
  invoke("set_window_theme", { dark: theme.value === "dark" }).catch(() => {});
}, { flush: "sync" });

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
    logging,
    logLevel,
    highFps,
    hybridCompat,
    settingsError,
    settingsSaving,
    settingsReady,
    unitMode,
    unitCapacity,
    unitRate,
    unitTemp,
    unitDuration,
    loadSettings,
    loadAutostart,
    toggleAutostart,
    openDevtools,
  };
}

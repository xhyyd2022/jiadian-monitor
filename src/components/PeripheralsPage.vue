<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { Icon } from "@iconify/vue";
import { invoke } from "@tauri-apps/api/core";

/// 后端（peripherals::Snapshot）的 JSON 结构
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
  /// 目录的相对标签，如 `<程序目录>\peripherals`，界面显示这个
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
const msg = ref("");
/// 目录标签（相对路径，不含本机盘符）
const dirLabel = ref("");
/// 可执行文件名（示例命令用）
const exeName = ref("");

const devices = computed(() => snap.value?.devices ?? []);

/// 轮询间隔文案由后端给，不在前端写死秒数。
const pollText = computed(() => {
  const ms = snap.value?.pollMs ?? 0;
  if (ms <= 0) return "";
  const s = ms / 1000;
  return `${Number.isInteger(s) ? s : s.toFixed(1)} 秒`;
});

/// 电量配色：>=50 绿、>=20 黄、其余红；未知灰。
function levelClass(r: Reading): string {
  if (r.battery === null) return "off";
  if (r.battery >= 50) return "ok";
  if (r.battery >= 20) return "warn";
  return "bad";
}

/// 电量文案：读不到就说「—」，不猜。休眠时保留上一次读数（界面上另有「休眠中」标注）。
function levelText(r: Reading): string {
  if (r.state === "unsupported") return "未支持";
  if (r.battery === null) return "—";
  return `${r.battery}%`;
}

/// 状态点：正常绿；重试黄；休眠/未收录灰；从没读到过红。
function dotClass(r: Reading): string {
  switch (r.state) {
    case "live":
      return "ok";
    case "retrying":
      return "warn";
    case "unsupported":
    case "sleeping":
      return "off";
    default:
      return r.error ? "bad" : "off";
  }
}

/// 多久以前（只给提示文案用，跟着 1.5 秒的轮询自然刷新）。
function ago(ms: number): string {
  if (!ms) return "未知";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return `${s} 秒前`;
  if (s < 3600) return `${Math.round(s / 60)} 分钟前`;
  return `${Math.round(s / 3600)} 小时前`;
}

/// 一行状态说明：休眠/失败/未支持优先，其次充电。
function rowHint(r: Reading): string {
  if (r.state === "sleeping") return `休眠中：连续几次读不到，上次读数在${ago(r.lastSeenMs)}`;
  if (r.state === "retrying") return `这次没读到（${r.error ?? "未知原因"}），正在重试`;
  if (r.state === "unsupported") return r.error ?? "认得品牌，但协议里没有匹配这个端点的档位";
  if (r.state === "unavailable") return r.error ?? "还没读到过电量";
  if (r.charging === true) return "充电中";
  if (r.charging === false) return "放电中";
  return "";
}

/// 同型号是否出现了多台：多台才需要把名字锁到实例，单台按型号存（换 USB 口不丢名）。
function sameModel(r: Reading): boolean {
  return devices.value.filter((d) => d.vid === r.vid && d.pid === r.pid).length > 1;
}

const editing = ref("");
const draft = ref("");

function keyOf(r: Reading): string {
  return `${r.vid}:${r.pid}@${r.instance}`;
}

function startEdit(r: Reading) {
  editing.value = keyOf(r);
  draft.value = r.name;
}

function cancelEdit() {
  editing.value = "";
  draft.value = "";
}

async function saveEdit(r: Reading) {
  if (editing.value !== keyOf(r)) return;
  const value = draft.value.trim();
  if (value === r.name) {
    cancelEdit();
    return;
  }
  try {
    snap.value = await invoke<Snapshot>("peripherals_rename", {
      vid: r.vid,
      pid: r.pid,
      instance: sameModel(r) ? r.instance : null,
      name: value || null,
    });
    msg.value = value ? "名称已保存到 settings.ini" : "已恢复默认名";
    cancelEdit();
  } catch (e) {
    msg.value = String(e);
  }
}

async function refresh(force = false) {
  if (busy.value) return;
  busy.value = true;
  try {
    snap.value = force
      ? await invoke<Snapshot>("peripherals_refresh")
      : await invoke<Snapshot>("peripherals_state");
    if (force) msg.value = "";
  } catch (e) {
    msg.value = String(e);
  } finally {
    busy.value = false;
  }
}

/// 在资源管理器里打开协议库目录（绝对路径只在这一步交给系统，不进界面文本）。
async function openDir() {
  try {
    await invoke("peripherals_open_dir");
  } catch (e) {
    msg.value = String(e);
  }
}

let timer = 0;

onMounted(async () => {
  await refresh();
  dirLabel.value = (await invoke<string | null>("peripherals_dir").catch(() => null)) ?? "";
  // 可执行文件名从后端拿，界面上不写死。
  exeName.value = snap.value?.exeName ?? "";
  // 界面按后端给的轮询间隔的一半取缓存，保证每次都能看到新读数。
  const ms = Math.max(500, Math.floor((snap.value?.pollMs ?? 3000) / 2));
  timer = window.setInterval(() => refresh(), ms);
});

onUnmounted(() => {
  if (timer) clearInterval(timer);
});
</script>

<template>
  <div>
    <div class="card">
      <h2>
        <Icon icon="line-md:monitor" />
        <span class="ttl">外设</span>
        <span
          class="dot"
          :class="{ ok: devices.length > 0 }"
          :title="devices.length ? '已识别到外设' : '未识别到外设'"
        ></span>
        <button class="mini" :disabled="busy" @click="refresh(true)">
          <Icon icon="line-md:backup-restore" />刷新
        </button>
      </h2>

      <p class="hint keep">
        {{
          msg ||
          (devices.length
            ? `电量每 ${pollText} 读一次；点行尾的笔或双击名称可改显示名（存进 settings.ini，本地 API 同步生效）。`
            : "没有识别到已收录的外设。插上鼠标 / 键盘 / 耳机，或在 peripherals/index.json 里补一条 PID。")
        }}
      </p>

      <div v-for="r in devices" :key="keyOf(r)" class="row">
        <span class="dot" :class="dotClass(r)" :title="rowHint(r)"></span>
        <template v-if="editing === keyOf(r)">
          <input
            v-model="draft"
            class="rename"
            maxlength="24"
            autofocus
            placeholder="留空 = 恢复默认名"
            @keydown.enter.prevent="saveEdit(r)"
            @keydown.esc.prevent="cancelEdit"
            @blur="saveEdit(r)"
          />
          <button
            class="mini icon"
            title="保存（点空白处也会保存）"
            aria-label="保存"
            @mousedown.prevent
            @click="saveEdit(r)"
          >
            <Icon icon="line-md:confirm-circle" />
          </button>
        </template>
        <template v-else>
          <span class="lbl" :title="rowHint(r)" @dblclick="startEdit(r)">
            {{ r.name }}<span class="inst">·{{ r.instance }}</span>
            <span v-if="r.state === 'sleeping'" class="tag">休眠中 · {{ ago(r.lastSeenMs) }}</span>
          </span>
          <span class="val" :class="levelClass(r)" :title="rowHint(r)">{{ levelText(r) }}</span>
          <button class="mini icon" title="改显示名（存进 settings.ini）" aria-label="改名" @click="startEdit(r)">
            <Icon icon="line-md:edit" />
          </button>
        </template>
      </div>

      <p v-if="snap && snap.errors.length" class="hint err">
        {{ snap.errors.join("；") }}
      </p>
    </div>

    <div class="card">
      <h2>
        <Icon icon="line-md:file-import" />
        <span class="ttl">兼容配置</span>
        <button class="mini" :disabled="!dirLabel" @click="openDir">
          <Icon icon="line-md:folder" />打开目录
        </button>
      </h2>
      <p class="hint">
        所有外设协议都是纯 JSON，改完存盘即可生效（点「刷新」立刻重读），不用重新编译。 目录：
        <code>{{ dirLabel || "（未找到 peripherals 目录）" }}</code>
        —— 相对程序所在文件夹，装在哪儿都一样。
      </p>
      <ol class="steps">
        <li>
          跑 <code>{{ exeName }} --peripherals-hid</code> 看这台机器上的 HID 端点与 VID:PID。
        </li>
        <li>在 <code>index.json</code> 的 <code>devices</code> 里加一条 <code>vid</code>/<code>pid</code> → <code>brand</code>。</li>
        <li>在 <code>brands/&lt;品牌&gt;.json</code> 里写这个品牌的读法（发什么、怎么认回包、电量在第几字节）。</li>
        <li>填 <code>sample</code> 用真实回包自检：<code>--peripherals-selftest</code>。</li>
      </ol>
      <p class="hint">
        完整的字段说明见同目录 <code>README.md</code>；本地 API <code>/peripherals</code> 也能拿到同一份数据。
      </p>
    </div>
  </div>
</template>

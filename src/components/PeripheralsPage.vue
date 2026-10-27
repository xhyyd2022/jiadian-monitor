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
  dir: string | null;
  pollMs: number;
  devices: Reading[];
  errors: string[];
  updatedMs: number;
}

const snap = ref<Snapshot | null>(null);
const busy = ref(false);
const msg = ref("");
const dir = ref("");

const devices = computed(() => snap.value?.devices ?? []);

/// 电量配色：>=50 绿、>=20 黄、其余红；未知灰。
function levelClass(r: Reading): string {
  if (r.battery === null) return "off";
  if (r.battery >= 50) return "ok";
  if (r.battery >= 20) return "warn";
  return "bad";
}

/// 电量文案：读不到就说「—」，不猜。
function levelText(r: Reading): string {
  if (!r.supported) return "未支持";
  if (r.battery === null) return "—";
  return `${r.battery}%`;
}

/// 状态点：真读到=绿；退避缓存=黄；读失败=红；认得品牌但没协议=灰。
function dotClass(r: Reading): string {
  if (r.error) return "bad";
  if (!r.online) return "off";
  if (r.stale) return "warn";
  return "ok";
}

/// 一行状态说明：错误优先，其次充电/退避。
function rowHint(r: Reading): string {
  if (r.error) return r.error;
  if (r.stale) return "设备在省电，显示的是上一次读数";
  if (r.charging === true) return "充电中";
  if (r.charging === false) return "放电中";
  return "";
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

let timer = 0;

onMounted(async () => {
  await refresh();
  dir.value = (await invoke<string | null>("peripherals_dir").catch(() => null)) ?? "";
  // 后端每秒最多轮询一台设备（默认 3 秒一轮），界面 2 秒取一次缓存足够。
  timer = window.setInterval(() => refresh(), 2000);
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
            ? "电量每 3 秒读一次；协议来自 peripherals/brands 下的 JSON，按 index.json 里的 PID 绑定品牌。"
            : "没有识别到已收录的外设。插上鼠标 / 键盘 / 耳机，或在内置的 peripherals/index.json 里补一条 PID。")
        }}
      </p>

      <div v-for="(r, i) in devices" :key="`${r.vid}:${r.pid}:${i}`" class="row">
        <span class="dot" :class="dotClass(r)" :title="rowHint(r)"></span>
        <span class="lbl" :title="rowHint(r)">{{ r.name }}</span>
        <span class="val" :class="levelClass(r)">{{ levelText(r) }}</span>
      </div>

      <p v-if="snap && snap.errors.length" class="hint err">
        {{ snap.errors.join("；") }}
      </p>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:file-import" />兼容配置</h2>
      <p class="hint">
        所有外设协议都是纯 JSON，改完存盘即可生效（点「刷新」立刻重读），不用重新编译。
        目录：<code>{{ dir || "（未找到 peripherals 目录）" }}</code>
      </p>
      <ol class="steps">
        <li>跑 <code>jiaidian-monitor.exe --peripherals-hid</code> 看这台机器上的 HID 端点与 VID:PID。</li>
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

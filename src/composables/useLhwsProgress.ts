// 下载 / 安装进度（LibreHardwareService 一键安装用）。
//
// 为什么单独抽出来：设置页的「数据源」卡片和首页的引导弹窗都要显示同一份进度，
// 两边各拉一次就会互相抢；这里做成**单例 + 引用计数**，谁先开始谁拉，谁最后结束谁停。
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export interface LhwsProg {
  /** 有安装 / 下载任务在进行 */
  active: boolean;
  /** preparing / downloading / verifying / installing / done / failed */
  phase: string;
  /** 当前下载源的标签（"GitHub 官方" / "gh-proxy.com" …） */
  source: string;
  /** 第几次尝试 */
  attempt: number;
  /** 共可能尝试几次（换源也算一次） */
  totalAttempts: number;
  got: number;
  total: number;
  percent: number;
  /** 字节/秒 */
  speed: number;
  message: string;
}

export const prog = ref<LhwsProg | null>(null);

let timer: number | null = null;
let users = 0;

async function tick() {
  try {
    prog.value = await invoke<LhwsProg>("lhws_progress");
  } catch {
    /* 单次拿不到就等下一轮，不要把界面刷成空白 */
  }
}

/** 开始轮询（幂等：重复调用只加引用计数）。 */
export function watchProgress() {
  users += 1;
  if (timer !== null) return;
  void tick();
  timer = window.setInterval(() => void tick(), 300);
}

/** 结束轮询（最后一个使用者走时才真的停）。 */
export function unwatchProgress() {
  users = Math.max(0, users - 1);
  if (users > 0 || timer === null) return;
  window.clearInterval(timer);
  timer = null;
}

/** 不管引用计数，强制停（组件卸载时兜底）。 */
export function stopProgress() {
  users = 0;
  if (timer !== null) {
    window.clearInterval(timer);
    timer = null;
  }
}

/// 进度条宽度（%）。总大小未知时返回 0，由界面切成"不确定态"。
export const pct = computed(() => {
  const p = prog.value;
  if (!p || p.active === false || p.total <= 0) return 0;
  return Math.max(0, Math.min(100, p.percent));
});

/// 总大小未知（服务器没给 Content-Length，或还在握手）→ 画不确定态动画
export const indeterminate = computed(() => {
  const p = prog.value;
  return !!p && p.active && p.total <= 0;
});

/// 大小文案：`23.4 / 57.0 MB`
export const sizeText = computed(() => {
  const p = prog.value;
  if (!p) return "";
  const mb = (n: number) => (n / 1024 / 1024).toFixed(1);
  if (p.total > 0) return `${mb(p.got)} / ${mb(p.total)} MB`;
  return p.got > 0 ? `${mb(p.got)} MB` : "";
});

/// 速度文案：`2.3 MB/s`
export const speedText = computed(() => {
  const s = prog.value?.speed ?? 0;
  if (s <= 0) return "";
  if (s >= 1024 * 1024) return `${(s / 1024 / 1024).toFixed(1)} MB/s`;
  return `${Math.max(1, Math.round(s / 1024))} KB/s`;
});

/// 第几次尝试文案：`尝试 2/8 · gh-proxy.com`
export const attemptText = computed(() => {
  const p = prog.value;
  if (!p || p.attempt <= 0) return "";
  const src = p.source ? ` · ${p.source}` : "";
  return `尝试 ${p.attempt}/${p.totalAttempts}${src}`;
});

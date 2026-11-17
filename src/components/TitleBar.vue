<script setup lang="ts">
import { onUnmounted } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import { Icon } from "@iconify/vue";
import appIcon from "../assets/app-icon.png";

const appWindow = getCurrentWindow();
defineEmits<{ (e: "close"): void }>();

/**
 * 触控/手写笔拖动窗口（自行移动，不依赖 startDragging）。
 *
 * 背景：Tauri 的 drag.js 只监听 mousedown，WebView2 吃掉触摸后系统不会把触摸转成
 * 鼠标消息，`startDragging()` 在触摸屏上时灵时不灵（tauri#13762）。所以这里自己算位移。
 *
 * 关键：PointerEvent 的 clientX/Y 是相对窗口的，窗口一移动 client 就反向变化，
 * 直接用 client 位移会形成反馈 → 窗口来回抖。这里做补偿：
 *   手指位移 = client 位移 + 上次事件后窗口实际移动量 / scale
 * 只有手指真实位移进入 acc；窗口自身移动的部分被抵消，再配合 1 物理像素死区和
 * requestAnimationFrame 节流，静止按住时不会抖。
 * 鼠标仍走 data-tauri-drag-region="deep" 的原生拖拽。
 */
let pointerId = -1;
let active = false;
let pending = false;
let scale = 1;
let origin = { x: 0, y: 0 }; // 起始窗口位置（物理像素）
let lastApplied = { x: 0, y: 0 }; // 最近一次 setPosition 的目标
let atLastEvent = { x: 0, y: 0 }; // 上一次 pointermove 时的窗口位置
let acc = { x: 0, y: 0 }; // 累计手指位移（CSS px）
let lastClient = { x: 0, y: 0 };
let raf = 0;

function stopDrag(): void {
  active = false;
  pending = false;
  pointerId = -1;
  if (raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
}

function applyMove(): void {
  raf = 0;
  if (!active) return;
  const x = Math.round(origin.x + acc.x * scale);
  const y = Math.round(origin.y + acc.y * scale);
  // 物理像素死区：静止触摸时的亚像素噪声不会让窗口来回写
  if (Math.abs(x - lastApplied.x) < 1 && Math.abs(y - lastApplied.y) < 1) return;
  lastApplied = { x, y };
  void appWindow.setPosition(new PhysicalPosition(x, y)).catch(() => stopDrag());
}

function scheduleApply(): void {
  if (!raf) raf = requestAnimationFrame(applyMove);
}

async function onPointerDown(e: PointerEvent): Promise<void> {
  if (e.pointerType === "mouse") return; // 鼠标：原生 drag-region
  const target = e.target as HTMLElement | null;
  if (target?.closest("button, a, input, textarea, select, [contenteditable]")) return;
  if (!e.isPrimary) return;
  e.preventDefault();
  pointerId = e.pointerId;
  lastClient = { x: e.clientX, y: e.clientY };
  acc = { x: 0, y: 0 };
  pending = true;
  try {
    const [pos, sf] = await Promise.all([appWindow.outerPosition(), appWindow.scaleFactor()]);
    if (!pending || pointerId !== e.pointerId) return; // 期间已抬手
    origin = { x: pos.x, y: pos.y };
    lastApplied = { ...origin };
    atLastEvent = { ...origin };
    scale = sf || 1;
    active = true;
    pending = false;
    try {
      (e.currentTarget as HTMLElement | null)?.setPointerCapture?.(e.pointerId);
    } catch {
      /* 某些合成事件下不支持捕获，忽略 */
    }
  } catch {
    // 权限/平台不支持时退回系统拖拽
    pending = false;
    void appWindow.startDragging().catch(() => {});
  }
}

function onPointerMove(e: PointerEvent): void {
  if (!active || e.pointerId !== pointerId) return;
  e.preventDefault();
  const dx = e.clientX - lastClient.x;
  const dy = e.clientY - lastClient.y;
  lastClient = { x: e.clientX, y: e.clientY };
  // 上次事件之后窗口自己移动了多少（CSS px）：从 client 位移里扣掉，避免反馈抖动
  const winDx = (lastApplied.x - atLastEvent.x) / scale;
  const winDy = (lastApplied.y - atLastEvent.y) / scale;
  atLastEvent = { ...lastApplied };
  acc.x += dx + winDx;
  acc.y += dy + winDy;
  scheduleApply();
}

function onPointerEnd(e: PointerEvent): void {
  if (e.pointerId === pointerId) stopDrag();
}

onUnmounted(stopDrag);
</script>

<template>
  <header
    class="titlebar"
    data-tauri-drag-region="deep"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerEnd"
    @pointercancel="onPointerEnd"
    @lostpointercapture="onPointerEnd"
  >
    <div class="tb-brand">
      <span class="tb-logo"><img :src="appIcon" alt="" /></span>
      <span class="tb-name">佳点监控</span>
      <span class="tb-ver">V1PRO / V2</span>
    </div>
    <div class="tb-btns">
      <button class="tb-btn" type="button" aria-label="最小化窗口" title="最小化" @click="appWindow.minimize()">
        <Icon icon="line-md:minus" />
      </button>
      <button class="tb-btn tb-close" type="button" aria-label="关闭窗口" title="关闭" @click="$emit('close')">
        <Icon icon="line-md:close" />
      </button>
    </div>
  </header>
</template>

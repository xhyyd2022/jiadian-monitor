<script setup lang="ts">
import { computed, ref } from "vue";
import { Icon } from "@iconify/vue";
import type { ColorPickerInstance } from "element-plus";
import { useSettings } from "../../composables/useSettings";
import { useAppearance } from "../../composables/useAppearance";

const { theme, appearanceBg, appearanceAccent, appearanceOpacity } = useSettings();
const { bgUrl, bgError, bgBusy, pickBackground, clearBackground, setAccent } = useAppearance();

interface Preset {
  name: string;
  color: string;
}
const presets: Preset[] = [
  { name: "天蓝", color: "#38bdf8" },
  { name: "靛蓝", color: "#6366f1" },
  { name: "薄荷", color: "#10b981" },
  { name: "琥珀", color: "#f59e0b" },
  { name: "珊瑚", color: "#f97316" },
  { name: "玫红", color: "#ec4899" },
  { name: "紫罗兰", color: "#8b5cf6" },
];
const presetColors = presets.map((p) => p.color);

/// 跟随主题时的实际强调色（仅用于展示“当前颜色”，真值仍是空串 = 默认）。
const DEFAULT_ACCENT: Record<"dark" | "light", string> = {
  dark: "#619aff",
  light: "#2563eb",
};
const defaultAccent = computed(() => DEFAULT_ACCENT[theme.value]);
const currentAccent = computed(() => (appearanceAccent.value || defaultAccent.value).toLowerCase());
const isDefault = computed(() => !appearanceAccent.value);
const isActive = (color: string): boolean =>
  appearanceAccent.value.toLowerCase() === color.toLowerCase();

/// 打开系统色板（自定义入口与色块都走同一个选择器，不额外做“清除”式控件）。
const pickerRef = ref<ColorPickerInstance>();
function openPicker(): void {
  pickerRef.value?.show();
}

const bgName = computed(() => appearanceBg.value || "未选择（使用纯色背景）");
const hasBg = computed(() => !!bgUrl.value);
</script>

<template>
  <el-card shadow="never" class="appearance-card">
    <template #header>
      <div class="panel-head">
        <span class="panel-title">
          <Icon icon="line-md:light-dark" /><span>界面外观</span>
        </span>
      </div>
    </template>

    <!-- 主题 -->
    <div class="ap-row">
      <span class="ap-label">主题</span>
      <div class="ap-content">
        <el-segmented
          v-model="theme"
          aria-label="界面主题"
          :options="[
            { label: '深色', value: 'dark' },
            { label: '浅色', value: 'light' },
          ]"
          size="small"
          class="seg"
        />
      </div>
    </div>

    <!-- 卡片不透明度：数值越高越实；文字始终不透明（A01） -->
    <div class="ap-row">
      <span class="ap-label">卡片不透明度</span>
      <div class="ap-content">
        <div class="op-line">
          <el-slider
            v-model="appearanceOpacity"
            :min="60"
            :max="100"
            :step="1"
            :show-tooltip="false"
            class="ap-slider"
            aria-label="卡片不透明度"
          />
          <span class="ap-value">{{ appearanceOpacity }}%</span>
        </div>
        <p class="ap-hint">越高越实，60% 最透、100% 最实；文字始终不透明。</p>
        <p v-if="hasBg && appearanceOpacity < 85" class="ap-warn">
          <Icon icon="line-md:alert" class="ap-warn-ic" />
          <span>背景图较花或卡片较透时文字可读性会下降；可调高不透明度或换一张更简洁的背景。</span>
        </p>
      </div>
    </div>

    <!-- 强调色：默认选项 + 预设色块 + 当前色/十六进制/自定义 + 恢复默认（A02） -->
    <div class="ap-row">
      <span class="ap-label" title="强调色只用于选中态、主按钮与链接">强调色</span>
      <div class="ap-content">
        <div class="swatches">
          <button
            type="button"
            class="swatch default"
            :class="{ on: isDefault }"
            :aria-pressed="isDefault"
            title="默认强调色（跟随主题）"
            @click="setAccent('')"
          >
            <Icon icon="line-md:light-dark" class="def-ic" />
            <span>默认</span>
            <span v-if="isDefault" class="sw-check"><Icon icon="line-md:confirm-circle" /></span>
          </button>
          <button
            v-for="p in presets"
            :key="p.color"
            type="button"
            class="swatch"
            :class="{ on: isActive(p.color) }"
            :style="{ background: p.color }"
            :aria-pressed="isActive(p.color)"
            :title="p.name + ' ' + p.color"
            :aria-label="'强调色：' + p.name"
            @click="setAccent(p.color)"
          >
            <span v-if="isActive(p.color)" class="sw-check"><Icon icon="line-md:confirm-circle" /></span>
          </button>
        </div>

        <div class="custom-color">
          <el-color-picker
            ref="pickerRef"
            :model-value="currentAccent"
            :predefine="presetColors"
            :clearable="false"
            size="small"
            title="自定义强调色"
            aria-label="自定义强调色"
            @change="setAccent"
          />
          <span class="hex">当前 {{ currentAccent.toUpperCase() }}</span>
          <span class="dot-sep">·</span>
          <button type="button" class="link-btn" @click="openPicker">自定义…</button>
          <span class="custom-gap"></span>
          <button
            type="button"
            class="link-btn"
            :disabled="isDefault"
            title="恢复默认强调色（跟随主题）"
            @click="setAccent('')"
          >
            恢复默认
          </button>
        </div>
      </div>
    </div>

    <!-- 背景：导入 / 清除 + 文件名；预览独立成行，尺寸够大才能判断可读性 -->
    <div class="ap-row">
      <span class="ap-label">背景图片</span>
      <div class="ap-content bg-content">
        <div class="bg-meta">
          <span class="bg-name" :title="bgName">{{ bgName }}</span>
          <span class="bg-hint">PNG / JPG / WebP，单张 ≤ 8 MB</span>
        </div>
        <div class="bg-actions">
          <el-button size="small" :loading="bgBusy" @click="pickBackground">
            {{ appearanceBg ? "更换图片" : "导入图片" }}
          </el-button>
          <el-button size="small" text :disabled="!hasBg && !appearanceBg" @click="clearBackground">
            清除
          </el-button>
        </div>
      </div>
    </div>
    <div class="ap-row bg-preview-row">
      <span class="ap-label">背景预览</span>
      <div class="ap-content">
        <div class="bg-preview" :class="{ empty: !hasBg }">
          <img v-if="hasBg" :src="bgUrl" alt="当前背景预览" />
          <div v-else class="bg-empty">
            <Icon icon="line-md:image" />
            <span>未选择背景图片（当前使用纯色背景）</span>
          </div>
        </div>
      </div>
    </div>
    <p v-if="bgError" class="ap-error bg-error">{{ bgError }}</p>

    <!-- 实时预览：真实文本 / 输入 / 按钮 / 警告，叠在背景与卡片表面之上判断可读性（A03） -->
    <div class="ap-preview">
      <div class="pv-label">
        <span>实时预览</span>
        <span class="pv-note">文字 / 输入 / 按钮 / 警告样例</span>
      </div>
      <div class="pv-stage">
        <div v-if="hasBg" class="pv-bg" :style="{ backgroundImage: `url(${bgUrl})` }"></div>
        <div class="pv-card">
          <div class="pv-head">
            <span class="pv-title">系统状态</span>
            <el-tag type="success" size="small" effect="plain">已连接</el-tag>
          </div>
          <p class="pv-line">CPU 占用 38% · 温度 52 ℃ · 风扇 1,360 RPM</p>
          <div class="pv-controls">
            <el-button type="primary" size="small">主按钮</el-button>
            <el-button size="small">次要按钮</el-button>
            <el-input class="pv-input" size="small" placeholder="搜索主题…" aria-label="搜索主题（样例输入框）" readonly />
          </div>
          <div class="pv-warn">
            <Icon icon="line-md:alert" class="pv-warn-ic" />
            <span>警告样例：数据源未运行，温度与风扇暂不可用。</span>
          </div>
        </div>
      </div>
    </div>
  </el-card>
</template>

<style scoped>
.appearance-card {
  min-width: 0;
  flex: none;
}
.appearance-card :deep(.el-card__body) {
  padding: 2px 12px 6px;
}

/* ---- 左侧标签 + 右侧内容的一行设置 ---- */
.ap-row {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 6px 0;
}
.ap-row + .ap-row {
  border-top: 1px solid var(--line);
}
.ap-label {
  flex: none;
  width: 84px;
  padding-top: 4px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.4;
}
.ap-content {
  flex: 1;
  min-width: 0;
}
.ap-hint {
  margin: 2px 0 0;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.ap-warn {
  display: flex;
  align-items: flex-start;
  gap: 5px;
  margin: 4px 0 0;
  padding: 3px 7px;
  border: 1px solid color-mix(in srgb, var(--warn) 45%, transparent);
  border-radius: var(--r-ctl);
  background: color-mix(in srgb, var(--warn) 12%, transparent);
  color: var(--fg);
  font-size: 11px;
  line-height: 1.45;
}
.ap-warn-ic {
  flex: none;
  margin-top: 1px;
  color: var(--warn);
  font-size: 12px;
}
.ap-error {
  margin: 0 0 2px;
  color: var(--danger);
  font-size: 11.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.bg-error {
  padding-left: 96px;
}

/* ---- 主题 ---- */
.seg {
  max-width: 168px;
  font-size: 12px;
  --el-segmented-item-selected-bg-color: var(--accent);
  --el-segmented-item-selected-color: var(--on-accent);
  --el-segmented-item-selected-hover-bg-color: var(--accent);
  --el-segmented-item-hover-bg-color: var(--btn-hover);
  --el-segmented-item-hover-color: var(--fg);
}

/* ---- 不透明度 ---- */
.op-line {
  display: flex;
  align-items: center;
  gap: 10px;
}
.ap-slider {
  flex: 1;
  min-width: 0;
  margin: 0 2px;
}
.ap-value {
  flex: none;
  width: 38px;
  text-align: right;
  color: var(--fg);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
}

/* ---- 强调色 ---- */
.swatches {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}
.swatch {
  position: relative;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 1px solid var(--line);
  border-radius: 50%;
  background: transparent;
  color: var(--fg);
  cursor: pointer;
}
.swatch:hover {
  transform: scale(1.06);
}
.swatch:active {
  transform: scale(0.96);
}
.swatch.on {
  box-shadow: 0 0 0 2px var(--panel), 0 0 0 3px var(--fg);
}
.swatch.default {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  width: auto;
  min-width: 26px;
  padding: 0 7px 0 6px;
  border-radius: 999px;
  background: var(--btn);
  font-size: 11px;
}
.swatch.default.on {
  color: var(--accent);
  border-color: var(--accent);
}
.def-ic {
  font-size: 13px;
}
.sw-check {
  position: absolute;
  right: -3px;
  bottom: -3px;
  width: 13px;
  height: 13px;
  display: grid;
  place-items: center;
  border: 1px solid var(--line);
  border-radius: 50%;
  background: var(--panel);
  color: var(--fg);
  font-size: 9px;
}
.swatch.default .sw-check {
  position: static;
  margin-left: 1px;
  border: none;
  background: transparent;
  color: var(--accent);
}

.custom-color {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-top: 6px;
  min-width: 0;
}
.custom-color :deep(.el-color-picker__trigger) {
  width: 26px;
  height: 26px;
  padding: 2px;
  border-radius: var(--r-ctl);
}
.custom-color :deep(.el-color-picker__icon) {
  display: none;
}
.hex {
  color: var(--fg);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.2px;
}
.dot-sep {
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.4;
}
.custom-gap {
  flex: 1;
  min-width: 4px;
}
.link-btn {
  min-height: 26px;
  padding: 0 4px;
  border: none;
  background: transparent;
  color: var(--accent);
  font-size: 11.5px;
  line-height: 1.4;
  white-space: nowrap;
  cursor: pointer;
}
.link-btn:hover {
  text-decoration: underline;
}
.link-btn:disabled {
  color: var(--sub);
  text-decoration: none;
  cursor: default;
}

/* ---- 背景 ---- */
.bg-content {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
/* 背景预览独立成行：约占内容宽度整行、200px 高，远大于原来的 60×34 缩略图 */
.bg-preview-row .ap-content {
  width: 100%;
}
.bg-preview {
  width: 100%;
  height: 200px;
  display: grid;
  place-items: center;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: var(--btn);
  color: var(--sub);
}
.bg-preview img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.bg-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  color: var(--sub);
  font-size: 11.5px;
}
.bg-empty :deep(svg) {
  font-size: 26px;
}
.bg-meta {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.bg-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg);
  font-size: 11.5px;
  line-height: 1.4;
}
.bg-hint {
  color: var(--sub);
  font-size: 10.5px;
  line-height: 1.4;
}
.bg-actions {
  flex: none;
  display: flex;
  gap: 6px;
  margin-left: auto;
}

/* ---- 实时预览 ---- */
.ap-preview {
  margin-top: 0;
  padding-top: 6px;
  border-top: 1px solid var(--line);
}
.pv-label {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 4px;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.4;
}
.pv-note {
  color: var(--sub);
  font-size: 10.5px;
}
.pv-stage {
  position: relative;
  overflow: hidden;
  padding: 8px;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: var(--bg);
}
.pv-bg {
  position: absolute;
  inset: 0;
  background-size: cover;
  background-position: center;
}
.pv-bg::after {
  content: "";
  position: absolute;
  inset: 0;
  background: color-mix(in srgb, var(--bg) calc(var(--app-bg-veil, 0.18) * 100%), transparent);
}
.pv-card {
  position: relative;
  padding: 8px 10px;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--panel) calc(var(--ui-alpha) * 100%), transparent);
  backdrop-filter: blur(8px);
  color: var(--fg);
}
.pv-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.pv-title {
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
  line-height: 1.4;
}
.pv-head :deep(.el-tag) {
  margin-left: auto;
}
.pv-line {
  margin: 3px 0 0;
  color: var(--fg);
  font-size: 12px;
  line-height: 1.5;
  font-variant-numeric: tabular-nums;
}
.pv-controls {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 5px;
}
.pv-controls :deep(.el-button) {
  margin-left: 0;
}
.pv-input {
  width: 140px;
  flex: none;
}
.pv-warn {
  display: flex;
  align-items: flex-start;
  gap: 5px;
  margin-top: 5px;
  padding: 2px 6px;
  border: 1px solid color-mix(in srgb, var(--warn) 45%, transparent);
  border-radius: var(--r-ctl);
  background: color-mix(in srgb, var(--warn) 14%, transparent);
  color: var(--fg);
  font-size: 10.5px;
  line-height: 1.45;
}
.pv-warn-ic {
  flex: none;
  margin-top: 1px;
  color: var(--warn);
  font-size: 12px;
}
</style>

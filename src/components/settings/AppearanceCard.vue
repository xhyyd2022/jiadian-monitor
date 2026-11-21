<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { Icon } from "@iconify/vue";
import { ElMessage } from "element-plus";
import { useSettings } from "../../composables/useSettings";
import {
  MATERIAL_ALGORITHMS,
  useAppearance,
  type AlgorithmId,
  type ColorMode,
  type Palette,
} from "../../composables/useAppearance";

const { theme } = useSettings();
const {
  config,
  currentPalette,
  seedColor,
  themeError,
  themeSaving,
  imageUrl,
  imageName,
  imageError,
  imageBusy,
  hasImage,
  setSeed,
  setMode,
  setAlgorithm,
  setColor,
  resetMode,
  resetAll,
  pickBackground,
  clearBackground,
  saveAsTheme,
  saveNow,
  setImageFit,
  setImagePosition,
} = useAppearance();

type Tab = "colors" | "background" | "file";
const tab = ref<Tab>("colors");
const savingAs = ref(false);
const editingName = ref(false);
const nameDraft = ref("");
const nameInput = ref<{ focus: () => void } | null>(null);

function beginEditName(): void {
  nameDraft.value = config.name || "自定义主题";
  editingName.value = true;
  void nextTick(() => nameInput.value?.focus());
}

function cancelName(): void {
  editingName.value = false;
}

function onNameKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    cancelName();
  } else if (e.key === "Enter") {
    e.preventDefault();
    void commitName();
  }
}

async function commitName(): Promise<void> {
  if (!editingName.value) return;
  editingName.value = false;
  const name = nameDraft.value.trim() || "自定义主题";
  if (name === config.name) return;
  config.name = name;
  await saveNow();
}

async function onSaveAsTheme(): Promise<void> {
  savingAs.value = true;
  themeError.value = "";
  try {
    const path = await saveAsTheme();
    if (path) ElMessage.success(`已保存主题到 ${path}`);
  } catch (e) {
    themeError.value = `保存主题失败：${e}`;
  } finally {
    savingAs.value = false;
  }
}

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

const colorFields: { key: keyof Palette; label: string; hint: string }[] = [
  { key: "bg", label: "窗口背景", hint: "最底层背景色" },
  { key: "side", label: "侧栏", hint: "标题栏 / 侧栏" },
  { key: "panel", label: "卡片", hint: "卡片与弹窗表面" },
  { key: "line", label: "边框", hint: "分隔线与控件边框" },
  { key: "fg", label: "主文字", hint: "标题与正文" },
  { key: "sub", label: "次文字", hint: "说明与辅助信息" },
  { key: "accent", label: "强调色", hint: "主按钮 / 选中 / 链接" },
  { key: "button", label: "次按钮", hint: "普通按钮背景" },
  { key: "buttonHover", label: "次按钮悬停", hint: "普通按钮 hover" },
  { key: "danger", label: "危险", hint: "删除 / 错误" },
  { key: "ok", label: "成功", hint: "完成 / 已连接" },
  { key: "warn", label: "警告", hint: "警告 / 注意" },
];

const fitOptions = [
  { value: "cover", label: "填充裁切" },
  { value: "contain", label: "完整显示" },
  { value: "fill", label: "拉伸铺满" },
  { value: "tile", label: "平铺" },
];

const algorithmHint = computed(
  () => MATERIAL_ALGORITHMS.find((a) => a.id === config.algorithm)?.hint || "低饱和，耐看",
);

const packChips = computed(() => [
  { label: "窗口", value: currentPalette.value.bg },
  { label: "侧栏", value: currentPalette.value.side },
  { label: "卡片", value: currentPalette.value.panel },
  { label: "边框", value: currentPalette.value.line },
  { label: "主文字", value: currentPalette.value.fg },
  { label: "次文字", value: currentPalette.value.sub },
  { label: "强调色", value: currentPalette.value.accent },
  { label: "次按钮", value: currentPalette.value.button },
  { label: "次按钮悬停", value: currentPalette.value.buttonHover },
  { label: "危险", value: currentPalette.value.danger },
  { label: "成功", value: currentPalette.value.ok },
  { label: "警告", value: currentPalette.value.warn },
]);
const algorithmLabel = computed(
  () => MATERIAL_ALGORITHMS.find((a) => a.id === config.algorithm)?.label || "柔和",
);
const fitLabel = computed(
  () => fitOptions.find((o) => o.value === config.imageFit)?.label || "填充裁切",
);
const packMeta = computed(() => [
  { label: "主题名", value: config.name || "自定义主题" },
  { label: "配色", value: config.mode === "advanced" ? "高级自定义" : `标准 · ${algorithmLabel.value}` },
  { label: "背景", value: hasImage.value ? config.image : "纯色" },
  { label: "显示", value: hasImage.value ? `${fitLabel.value} · ${config.imageX}% / ${config.imageY}%` : "—" },
]);
const fitHints: Record<string, string> = {
  cover: "铺满预览，超出部分裁切；可微调位置。",
  contain: "完整显示图片，可能留白；可微调位置。",
  fill: "拉伸铺满，不保持比例。",
  tile: "按原尺寸平铺。",
};
const fitHint = computed(() => fitHints[config.imageFit] || fitHints.cover);
function chooseMode(value: string | number | boolean | undefined): void {
  setMode(String(value) as ColorMode);
}

function chooseAlgorithm(value: string | number | boolean | undefined): void {
  setAlgorithm(String(value) as AlgorithmId);
}

function setFit(value: string | number | boolean | Record<string, unknown> | undefined): void {
  setImageFit(String(value) as typeof config.imageFit);
}

const previewStyle = computed(() => {
  if (!hasImage.value) return {};
  const size =
    config.imageFit === "contain"
      ? "contain"
      : config.imageFit === "fill"
        ? "100% 100%"
        : config.imageFit === "tile"
          ? "auto"
          : "cover";
  return {
    backgroundImage: `url("${imageUrl.value}")`,
    backgroundSize: size,
    backgroundPosition: `${config.imageX}% ${config.imageY}%`,
    backgroundRepeat: config.imageFit === "tile" ? "repeat" : "no-repeat",
  };
});

/// 手动改色后给一个可读性提醒；不阻止用户，但别让文字悄悄看不清。
function luminance(hex: string): number {
  const n = Number.parseInt(hex.slice(1), 16);
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
}
const contrast = computed(() => {
  const a = luminance(currentPalette.value.fg || "#ffffff");
  const b = luminance(currentPalette.value.bg || "#11151c");
  const hi = Math.max(a, b);
  const lo = Math.min(a, b);
  return (hi + 0.05) / (lo + 0.05);
});
const contrastWarn = computed(() => contrast.value < 4.5);
</script>

<template>
  <el-card shadow="never" class="appearance-card">
    <!-- 主题子选项：配色 / 背景 / 主题文件 -->
    <div class="theme-sub">
      <el-segmented
        v-model="tab"
        aria-label="主题子选项"
        :options="[
          { label: '配色', value: 'colors' },
          { label: '背景', value: 'background' },
          { label: '主题文件', value: 'file' },
        ]"
        size="small"
        class="sub-seg"
      />
      <span v-if="themeSaving" class="theme-status saving">
        <span class="save-dot"></span>
        保存中…
      </span>
    </div>

    <template v-if="tab === 'colors'">
      <div class="ap-row">
        <span class="ap-label">模式</span>
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
          <span class="ap-note">深色、浅色各一套，切换即时生效。</span>
        </div>
      </div>

      <div class="ap-row">
        <span class="ap-label">配色方式</span>
        <div class="ap-content">
          <el-segmented
            :model-value="config.mode"
            aria-label="配色方式"
            :options="[
              { label: '标准', value: 'standard' },
              { label: '高级', value: 'advanced' },
            ]"
            size="small"
            class="seg mode-seg"
            @change="chooseMode"
          />
        </div>
      </div>

      <template v-if="config.mode === 'standard'">
        <div class="ap-row">
          <span class="ap-label">算法</span>
          <div class="ap-content algo-line">
            <el-select
              :model-value="config.algorithm"
              size="small"
              class="algo-select"
              aria-label="取色算法"
              @change="chooseAlgorithm"
            >
              <el-option
                v-for="a in MATERIAL_ALGORITHMS"
                :key="a.id"
                :label="a.label"
                :value="a.id"
              />
            </el-select>
            <span class="ap-note">{{ algorithmHint }}</span>
          </div>
        </div>

        <div class="ap-row">
          <span class="ap-label">种子色</span>
          <div class="ap-content">
            <div class="seed-line">
              <el-color-picker
                :model-value="seedColor"
                :predefine="presetColors"
                :clearable="false"
                size="small"
                class="seed-picker"
                :style="{ '--seed-color': seedColor }"
                title="选种子色，自动生成深 / 浅两套配色"
                aria-label="主题种子色"
                @change="setSeed"
              />
              <div class="seed-swatches">
                <button
                  v-for="p in presets"
                  :key="p.color"
                  type="button"
                  class="seed-dot"
                  :class="{ on: seedColor.toLowerCase() === p.color.toLowerCase() }"
                  :style="{ background: p.color }"
                  :title="p.name + ' ' + p.color"
                  :aria-label="'主题色：' + p.name"
                  @click="setSeed(p.color)"
                >
                  <Icon
                    v-if="seedColor.toLowerCase() === p.color.toLowerCase()"
                    icon="line-md:confirm"
                  />
                </button>
              </div>
              <span class="seed-hex">{{ seedColor.toUpperCase() }}</span>
            </div>
            <p class="ap-hint">选色即生成；要逐项调整请切到高级。</p>
          </div>
        </div>
      </template>

      <div v-else class="ap-row">
        <span class="ap-label">整体颜色</span>
        <div class="ap-content">
          <div class="palette-head">
            <span v-if="contrastWarn" class="contrast-warn">
              <Icon icon="line-md:alert" />
              对比度 {{ contrast.toFixed(1) }}:1，偏低
            </span>
            <button type="button" class="link-btn reset-mode" @click="resetMode">
              恢复本模式
            </button>
            <button type="button" class="link-btn" title="深色、浅色都恢复默认，并回到标准模式" @click="resetAll">
              全部恢复
            </button>
          </div>
          <div class="color-grid">
            <label v-for="f in colorFields" :key="f.key" class="color-cell" :title="f.hint">
              <span class="cc-label">{{ f.label }}</span>
              <el-color-picker
                :model-value="currentPalette[f.key]"
                :clearable="false"
                size="small"
                :aria-label="f.label"
                @change="(v: string | null) => setColor(f.key, v)"
              />
              <span class="cc-hex">{{ currentPalette[f.key] || "默认" }}</span>
            </label>
          </div>
        </div>
      </div>

      <div class="ap-row opacity-row">
        <span class="ap-label" title="调整卡片背景透明度，文字不受影响">不透明度</span>
        <div class="ap-content">
          <div class="op-line">
            <el-slider
              v-model="config.opacity"
              :min="60"
              :max="100"
              :step="1"
              :show-tooltip="false"
              class="ap-slider"
              aria-label="卡片不透明度"
            />
            <span class="ap-value">{{ config.opacity }}%</span>
          </div>
        </div>
      </div>
    </template>

    <template v-else-if="tab === 'background'">
      <div class="ap-row">
        <span class="ap-label">背景图片</span>
        <div class="ap-content bg-content">
          <div class="bg-meta">
            <span class="bg-name" :title="imageName">{{ imageName }}</span>
            <span class="bg-hint">PNG / JPG / WebP，≤ 8 MB，与 theme.toml 同目录</span>
          </div>
          <div class="bg-actions">
            <el-button size="small" :loading="imageBusy" @click="pickBackground">
              {{ config.image ? "更换" : "导入" }}
            </el-button>
            <el-button size="small" :disabled="!hasImage && !config.image" @click="clearBackground">
              清除
            </el-button>
          </div>
        </div>
      </div>
      <p v-if="imageError" class="ap-error">{{ imageError }}</p>

      <div class="ap-row">
        <span class="ap-label">显示方式</span>
        <div class="ap-content bg-fit-line">
          <el-select
            :model-value="config.imageFit"
            size="small"
            class="fit-select"
            aria-label="背景图缩放模式"
            :disabled="!hasImage"
            @change="setFit"
          >
            <el-option v-for="o in fitOptions" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
          <span class="ap-note">{{ fitHint }}</span>
        </div>
      </div>

      <div class="ap-row">
        <span class="ap-label">位置微调</span>
        <div class="ap-content pos-grid">
          <div class="pos-line">
            <span class="pos-name">水平</span>
            <el-slider
              :model-value="config.imageX"
              :min="0"
              :max="100"
              :step="1"
              :show-tooltip="false"
              :disabled="!hasImage"
              class="ap-slider"
              aria-label="背景图水平位置"
              @input="(v: number) => setImagePosition('x', v)"
            />
            <span class="ap-value">{{ config.imageX }}%</span>
          </div>
          <div class="pos-line">
            <span class="pos-name">垂直</span>
            <el-slider
              :model-value="config.imageY"
              :min="0"
              :max="100"
              :step="1"
              :show-tooltip="false"
              :disabled="!hasImage"
              class="ap-slider"
              aria-label="背景图垂直位置"
              @input="(v: number) => setImagePosition('y', v)"
            />
            <span class="ap-value">{{ config.imageY }}%</span>
          </div>
        </div>
      </div>

      <div class="ap-row bg-preview-row">
        <span class="ap-label">背景预览</span>
        <div class="ap-content">
          <div
            class="bg-preview"
            :class="{ empty: !hasImage }"
            :style="previewStyle"
            role="img"
            :aria-label="hasImage ? '背景图预览' : '未选择背景图'"
          >
            <div v-if="!hasImage" class="bg-empty">
              <Icon icon="line-md:image" class="bg-empty-icon" />
              <span>未选择背景图</span>
              <small>使用当前模式的窗口背景色</small>
            </div>
          </div>
        </div>
      </div>
    </template>

    <template v-else>
      <div class="ap-row">
        <span class="ap-label">主题</span>
        <div class="ap-content theme-line">
          <div class="theme-name-wrap">
            <el-input
              v-if="editingName"
              ref="nameInput"
              v-model="nameDraft"
              size="small"
              maxlength="32"
              class="theme-name-input"
              @keydown="onNameKey"
              @blur="commitName"
            />
            <span
              v-if="!editingName"
              class="theme-name"
              title="双击修改主题名（只改 TOML 内的标题，不改文件夹）"
              @dblclick="beginEditName"
            >
              {{ config.name || "自定义主题" }}
            </span>
            <button
              v-if="!editingName"
              type="button"
              class="name-edit"
              title="修改主题名（只改 TOML 内的标题，不改文件夹）"
              aria-label="修改主题名"
              @click="beginEditName"
            >
              <Icon icon="line-md:edit" />
            </button>
          </div>
          <el-button size="small" :loading="savingAs" @click="onSaveAsTheme">
            保存为主题
          </el-button>
        </div>
        <p v-if="themeError" class="ap-error">{{ themeError }}</p>
      </div>
      <div class="file-pack" aria-label="主题包内容">
        <div class="pack-body">
          <div class="pack-col">
            <div class="pack-title">主题包内容</div>
            <div class="pack-tree">
              <div class="pack-line root">Theme/</div>
              <div class="pack-line">
                theme.toml
                <span>配色 · 缩放 · 位置 · 不透明度</span>
              </div>
              <div class="pack-line">
                {{ config.image || "background.png" }}
                <span>{{ hasImage ? "当前背景图" : "可选背景图" }}</span>
              </div>
            </div>
            <div class="pack-meta">
              <div v-for="m in packMeta" :key="m.label">
                <span>{{ m.label }}</span><b>{{ m.value }}</b>
              </div>
            </div>
          </div>
          <div class="pack-col">
            <div class="pack-title">TOML 中的配色</div>
            <div class="pack-swatches">
              <div v-for="c in packChips" :key="c.label" class="pack-swatch">
                <i class="pack-dot" :style="{ background: c.value }"></i>
                <span class="pack-name">{{ c.label }}</span>
                <code>{{ c.value }}</code>
              </div>
            </div>
          </div>
        </div>
        <p class="pack-foot">改 theme.toml 后重启生效；整个 Theme 文件夹可直接备份或分享。</p>
      </div>
    </template>

    <!-- 实时预览：只在配色页展示，背景/主题文件页避免重复预览 -->
    <div v-if="tab === 'colors'" class="ap-preview">
      <div class="pv-label">
        <span>实时预览</span>
        <span class="pv-note">文字、输入、按钮、警告</span>
      </div>
      <div class="pv-stage">
        <div class="pv-bg"></div>
        <div class="pv-card">
          <div class="pv-head">
            <span class="pv-title">系统状态</span>
            <el-tag type="success" size="small" effect="plain">已连接</el-tag>
          </div>
          <p class="pv-line">CPU 占用 38% · 温度 52 ℃ · 风扇 1,360 RPM</p>
          <div class="pv-controls">
            <el-button type="primary" size="small">主按钮</el-button>
            <el-button size="small">次要按钮</el-button>
            <el-input
              class="pv-input"
              size="small"
              placeholder="搜索主题…"
              aria-label="搜索主题（样例输入框）"
              readonly
            />
          </div>
          <div class="pv-warn">
            <Icon icon="line-md:alert" class="pv-warn-ic" />
            <span>警告：数据源未运行，温度与风扇暂不可用。</span>
          </div>
        </div>
      </div>
    </div>
  </el-card>
</template>

<style scoped>
.appearance-card {
  min-width: 0;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.appearance-card :deep(.el-card__body) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: 6px 12px 6px;
}
.panel-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.theme-status {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--sub);
  font-size: 11px;
}
.save-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--accent);
  animation: pulse 1s ease-in-out infinite;
}
@keyframes pulse {
  50% {
    opacity: 0.3;
  }
}
.theme-sub {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 0 0 6px;
  border-bottom: 1px solid var(--line);
}
.sub-seg {
  max-width: 230px;
  font-size: 11.5px;
  --el-segmented-item-selected-bg-color: var(--accent);
  --el-segmented-item-selected-color: var(--on-accent);
  --el-segmented-item-selected-hover-bg-color: var(--accent);
  --el-segmented-item-hover-bg-color: var(--btn-hover);
  --el-segmented-item-hover-color: var(--fg);
}
.ap-row {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 5px 0;
}
.ap-row + .ap-row {
  border-top: 1px solid var(--line);
}
.ap-label {
  flex: none;
  width: 72px;
  padding-top: 4px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.4;
}
.ap-content {
  flex: 1;
  min-width: 0;
}
.ap-content :deep(.el-button) {
  min-width: 84px;
  justify-content: center;
}
.ap-content :deep(.el-button + .el-button) {
  margin-left: 0;
}
.algo-line {
  display: flex;
  align-items: center;
  gap: 10px;
}
.algo-select {
  width: 104px;
  flex: none;
}
.mode-seg {
  max-width: 154px;
}
.ap-hint {
  margin: 3px 0 0;
  color: var(--sub);
  font-size: 11px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.ap-note {
  color: var(--sub);
  font-size: 10.5px;
  line-height: 1.4;
}
.ap-error {
  margin: 0 0 2px;
  color: var(--danger);
  font-size: 11.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.seg {
  max-width: 168px;
  font-size: 12px;
  --el-segmented-item-selected-bg-color: var(--accent);
  --el-segmented-item-selected-color: var(--on-accent);
  --el-segmented-item-selected-hover-bg-color: var(--accent);
  --el-segmented-item-hover-bg-color: var(--btn-hover);
  --el-segmented-item-hover-color: var(--fg);
}
.seed-line {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.seed-swatches {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.seed-dot {
  position: relative;
  width: 22px;
  height: 22px;
  padding: 0;
  border: 1px solid var(--line);
  border-radius: 50%;
  color: #fff;
  cursor: pointer;
}
.seed-dot :deep(svg) {
  position: absolute;
  inset: 0;
  margin: auto;
  font-size: 13px;
}
.seed-dot.on {
  box-shadow: 0 0 0 2px var(--panel), 0 0 0 3px var(--fg);
}
.seed-hex {
  margin-left: auto;
  color: var(--fg);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}
.palette-head {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 20px;
  color: var(--sub);
  font-size: 11px;
}
.palette-head b {
  color: var(--fg);
  font-weight: 600;
}
.contrast-warn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--warn);
  font-size: 10.5px;
}
.reset-mode {
  margin-left: auto;
}
.color-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 1px 10px;
  margin-top: 2px;
}
.color-cell {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  height: 24px;
}
.cc-label {
  flex: none;
  width: 64px;
  color: var(--sub);
  font-size: 11px;
  white-space: nowrap;
}
.color-cell :deep(.el-color-picker__trigger) {
  width: 24px;
  height: 24px;
  padding: 0;
  border: 1px solid color-mix(in srgb, var(--fg) 22%, transparent);
  border-radius: 5px;
  background: transparent;
}
.color-cell :deep(.el-color-picker__trigger:hover) {
  border-color: var(--accent);
}
.color-cell :deep(.el-color-picker__color) {
  border: 0;
  border-radius: 4px;
}
.color-cell :deep(.el-color-picker__color-inner) {
  border-radius: 4px;
}
.color-cell :deep(.el-color-picker__icon) {
  display: none;
}
.seed-line :deep(.el-color-picker__trigger) {
  width: 24px;
  height: 24px;
  padding: 0;
  border: 1px solid color-mix(in srgb, var(--fg) 24%, transparent);
  border-radius: 50%;
  background: var(--seed-color, var(--accent));
  box-shadow: inset 0 0 0 1px color-mix(in srgb, #fff 18%, transparent);
  transition: transform 0.12s ease, box-shadow 0.12s ease;
}
.seed-line :deep(.el-color-picker__trigger::before) {
  content: "+";
  color: #fff;
  font-size: 14px;
  font-weight: 600;
  line-height: 1;
  text-shadow: 0 0 2px rgba(0, 0, 0, 0.55);
}
.seed-line :deep(.el-color-picker__trigger:hover) {
  transform: scale(1.08);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, #fff 28%, transparent), 0 0 0 2px color-mix(in srgb, var(--accent) 30%, transparent);
}
.seed-line :deep(.el-color-picker__color),
.seed-line :deep(.el-color-picker__icon) {
  display: none;
}
.cc-hex {
  min-width: 0;
  color: var(--fg);
  font-size: 10.5px;
  font-variant-numeric: tabular-nums;
  overflow: hidden;
  text-overflow: ellipsis;
}
.link-btn {
  min-height: 24px;
  padding: 0 3px;
  border: none;
  background: transparent;
  color: var(--accent);
  font-size: 11px;
  line-height: 1.4;
  white-space: nowrap;
  cursor: pointer;
}
.link-btn:hover {
  text-decoration: underline;
}
.bg-content {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
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
.bg-fit-line {
  display: flex;
  align-items: center;
  gap: 10px;
}
.fit-select {
  width: 116px;
  flex: none;
}
.pos-grid {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.pos-line {
  display: flex;
  align-items: center;
  gap: 8px;
}
.pos-name {
  flex: none;
  width: 30px;
  color: var(--sub);
  font-size: 11px;
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
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}
.bg-preview-row {
  flex: 1;
  min-height: 0;
  align-items: stretch;
}
.bg-preview-row .ap-content {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-height: 0;
}
.bg-preview {
  flex: 0 0 auto;
  width: auto;
  height: 100%;
  aspect-ratio: 780 / 540;
  max-width: 100%;
  min-height: 0;
  display: grid;
  place-items: center;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background-color: var(--btn);
}
.bg-preview.empty {
  background-color: var(--bg);
  color: var(--sub);
}
.bg-empty-icon {
  font-size: 24px;
  opacity: 0.55;
}
.bg-empty small {
  color: var(--sub);
  font-size: 10.5px;
}
.bg-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  width: 100%;
  height: 100%;
  padding: 10px;
  box-sizing: border-box;
  color: var(--sub);
  font-size: 11.5px;
}
.theme-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
}
.theme-line .el-button {
  min-width: 84px;
}
.theme-name-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.name-edit {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: var(--sub);
  cursor: pointer;
  transition: color 140ms ease, background 140ms ease;
}
.name-edit:hover {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, transparent);
}
.name-edit .iconify {
  width: 14px;
  height: 14px;
}
.theme-name {
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
  cursor: text;
}
.theme-name:hover {
  color: var(--accent);
}
.theme-name-input {
  width: 220px;
}
.opacity-row {
  flex: 0 0 auto;
}
.opacity-row .ap-content {
  max-width: 420px;
}
.op-line {
  display: flex;
  align-items: center;
  gap: 10px;
}
.op-line :deep(.el-slider) {
  flex: 1;
  margin: 0;
}
.file-pack {
  flex: 1;
  min-height: 0;
  margin: 8px 0 2px;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--btn) 42%, transparent);
}
.pack-body {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 18px;
}
.pack-col {
  min-width: 0;
  display: flex;
  flex-direction: column;
  justify-content: flex-start;
  padding-top: 2px;
}
.pack-title {
  margin-bottom: 8px;
  color: var(--fg);
  font-size: 11.5px;
  font-weight: 600;
}
.pack-tree {
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--fg);
  font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
  font-size: 11.5px;
}
.pack-line {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding-left: 16px;
  min-width: 0;
}
.pack-line.root {
  padding-left: 0;
  color: var(--accent);
  font-weight: 600;
}
.pack-line span {
  color: var(--sub);
  font-family: inherit;
  font-size: 10.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pack-meta {
  margin-top: auto;
  padding-top: 8px;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 4px 12px;
  border-top: 1px solid var(--line);
}
.pack-meta div {
  display: flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  color: var(--sub);
  font-size: 10.5px;
}
.pack-meta span {
  flex: none;
}
.pack-meta b {
  min-width: 0;
  overflow: hidden;
  color: var(--fg);
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pack-swatches {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 6px 12px;
  align-content: start;
}
.pack-swatch {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  font-size: 10.5px;
  color: var(--sub);
}
.pack-dot {
  width: 18px;
  height: 18px;
  flex: none;
  border: 1px solid var(--line);
  border-radius: 5px;
}
.pack-name {
  flex: none;
}
.pack-swatch code {
  margin-left: auto;
  color: var(--sub);
  font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
  font-size: 9.5px;
  overflow: hidden;
  text-overflow: ellipsis;
}
.pack-foot {
  margin: 10px 0 0;
  padding-top: 8px;
  border-top: 1px solid var(--line);
  color: var(--sub);
  font-size: 10.5px;
}
.ap-preview {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  margin-top: 6px;
  padding-top: 7px;
  border-top: 1px solid var(--line);
}
.pv-label {
  flex: none;
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 4px;
  color: var(--sub);
  font-size: 11px;
}
.pv-note {
  color: var(--sub);
  font-size: 10.5px;
}
.pv-stage {
  position: relative;
  flex: 1;
  min-height: 96px;
  display: flex;
  align-items: center;
  overflow: hidden;
  padding: 8px;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: var(--bg);
}
.pv-bg {
  position: absolute;
  inset: 0;
  background-image: var(--app-bg-image);
  background-size: var(--app-bg-size, cover);
  background-position: var(--app-bg-position, center);
  background-repeat: var(--app-bg-repeat, no-repeat);
}
.pv-bg::after {
  content: "";
  position: absolute;
  inset: 0;
  background: color-mix(in srgb, var(--bg) calc(var(--app-bg-veil, 0.18) * 100%), transparent);
}
.pv-card {
  position: relative;
  width: 100%;
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
  width: 96px;
  min-width: 96px;
  padding: 0 8px;
  margin-left: 0;
  justify-content: center;
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
@media (prefers-reduced-motion: reduce) {
  .save-dot {
    animation: none;
  }
}
</style>

<script setup lang="ts">
import { getCurrentInstance, nextTick, onMounted, ref, watch } from "vue";
import { Icon } from "@iconify/vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ElMessage } from "element-plus";
import GUIDE from "./material_prompt.md?raw";
import OnlinePage from "./OnlinePage.vue";
import { useOnline } from "../composables/useOnline";
import { paneIn } from "../motion";

/// 在线 AI 站点（要换站点就改这一行）
const AI_URL = "https://chat.deepseek.com/";

// ---------------- 单层 Tab ----------------
// 「本地素材 / 在线素材 / 生成指引」在同一层，页面层级只有「素材 → 当前标签」两级；
// App 顶部面包屑通过 `crumb` 事件拿到当前标签，点面包屑时调用 `goTab` 切回来。
type Tab = "local" | "online" | "guide";
const tab = ref<Tab>("local");

const emit = defineEmits<{
  /** 当前标签变化时回传层级（App 面包屑显示「素材 / 本地素材」这类两级，不再有三层） */
  crumb: [main: string, sub: string];
  /** 素材页里导入成功后回传主题名，App 刷新首页主题下拉用（可选监听） */
  imported: [name: string];
  /** 去首页应用：App 若监听（@go-home）就切回首页；未监听时界面只显示文字路径，不放空按钮 */
  goHome: [];
  /** 选中某个已安装主题并回首页（App 若监听 @apply-theme，就用它做行内「应用」按钮） */
  applyTheme: [name: string];
}>();

/// App 是否接入了“去首页 / 应用主题”事件（由 App.vue 的 @go-home、@apply-theme 决定；
/// 没接就不渲染按钮，避免出现点了没反应的假入口）。
const parentProps = (getCurrentInstance()?.vnode.props ?? {}) as Record<string, unknown>;
const canGoHome = typeof parentProps.onGoHome === "function";
const canApply = typeof parentProps.onApplyTheme === "function";

function tabLabel(t: Tab): string {
  return t === "local" ? "本地素材" : t === "online" ? "在线素材" : "生成指引";
}

function emitCrumb(): void {
  emit("crumb", tabLabel(tab.value), "");
}

/// App 点面包屑时调用：兼容新写法（本地素材 / 在线素材 / 生成指引）与旧写法（素材管理 + 二级）。
function goTab(main: string, sub?: string): void {
  const name = main || "";
  if (name === "生成指引") tab.value = "guide";
  else if (name === "在线素材") tab.value = "online";
  else if (name === "本地素材") tab.value = "local";
  else if (sub === "在线素材") tab.value = "online";
  else if (sub === "本地素材") tab.value = "local";
}
defineExpose({ goTab });

// ---------------- 本地素材 ----------------
interface ThemeVariant {
  tag: string;
  page: string;
  width: number | null;
  height: number | null;
}
interface Theme {
  name: string;
  page: string;
  variants: ThemeVariant[];
}

const { refresh: refreshOnline, openDir: openOnlineDir } = useOnline();

const assetsDir = ref("");
const themes = ref<Theme[]>([]);
const themesLoaded = ref(false);
const reloading = ref(false);
const importing = ref(false);
const copied = ref(false);

async function loadThemes(): Promise<void> {
  try {
    themes.value = await invoke<Theme[]>("list_themes");
  } catch {
    themes.value = [];
  } finally {
    themesLoaded.value = true;
  }
}

async function refreshThemes(): Promise<void> {
  if (reloading.value) return;
  reloading.value = true;
  try {
    await loadThemes();
  } finally {
    reloading.value = false;
  }
}

onMounted(() => {
  // 首帧就把面包屑从旧的「素材管理 / 本地素材」改成两级，避免默认状态残留三层。
  emitCrumb();
  void (async () => {
    try {
      assetsDir.value = (await invoke<string | null>("assets_dir")) ?? "";
    } catch {
      assetsDir.value = "";
    }
    await loadThemes();
  })();
});

function variantLabel(v: ThemeVariant): string {
  if (v.tag === "v1p") return "V1P 320×170";
  if (v.tag === "v2") return "V2 320×240";
  if (v.width && v.height) return `${v.width}×${v.height}`;
  return "任意尺寸";
}

/// 行内副信息：已安装 + 尺寸摘要（真实 list_themes 数据，不推测“运行中”）。
function themeSummary(t: Theme): string {
  const sizes = t.variants.map(variantLabel).join(" / ");
  return `已安装 · ${sizes || "尺寸未声明"} · ${t.page}`;
}

/// 导入素材：选择 HTML → 后端整理成 `assets/主题名/`（图片一起复制）
async function importMaterial(): Promise<void> {
  if (importing.value) return;
  let picked: string | string[] | null = null;
  try {
    picked = await open({
      multiple: false,
      directory: false,
      title: "选择素材 HTML",
      filters: [{ name: "HTML 面板", extensions: ["html", "htm"] }],
      defaultPath: assetsDir.value || undefined,
    });
  } catch (e) {
    ElMessage.error(`打开文件选择失败：${e}`);
    return;
  }
  const src = typeof picked === "string" ? picked : "";
  if (!src) return;
  importing.value = true;
  try {
    const t = await invoke<Theme>("import_panel", { src });
    ElMessage.success(`已导入「${t.name}」：回首页选择后会自动下发`);
    emit("imported", t.name);
    await loadThemes();
  } catch (e) {
    ElMessage.error(`导入失败：${e}`);
  } finally {
    importing.value = false;
  }
}

/// 资源管理器里定位 `assets/`（没有目录时提示先导入）
async function openAssetsDir(): Promise<void> {
  if (!assetsDir.value) {
    ElMessage.warning("还没有素材目录：先在首页导入一个素材，或手动创建 assets/");
    return;
  }
  try {
    await revealItemInDir(assetsDir.value);
  } catch (e) {
    ElMessage.error(`打开素材目录失败：${e}`);
  }
}

/// 在资源管理器里定位单个已安装主题的目录（“定位素材”路径）。
async function revealTheme(t: Theme): Promise<void> {
  if (!assetsDir.value) {
    ElMessage.warning("还没有素材目录：先导入一个素材");
    return;
  }
  const base = assetsDir.value.replace(/[\\/]+$/, "");
  try {
    await revealItemInDir(`${base}\\${t.name}`);
  } catch (e) {
    ElMessage.error(`定位素材失败：${e}`);
  }
}

/// 打开 `online/` 目录（快捷操作卡用；在线 Tab 自己也有入口）
async function openOnlineFolder(): Promise<void> {
  try {
    await openOnlineDir();
  } catch (e) {
    ElMessage.error(`打开在线素材目录失败：${e}`);
  }
}

/// 复制完整生成提示词（`material_prompt.md`）
async function copyPrompt(): Promise<void> {
  try {
    await navigator.clipboard.writeText(GUIDE);
    copied.value = true;
    window.setTimeout(() => (copied.value = false), 1500);
  } catch {
    ElMessage.error("复制失败：剪贴板不可用");
  }
}

/// 一键打开在线 AI 站点
async function openAi(): Promise<void> {
  try {
    await invoke("open_url", { url: AI_URL });
  } catch (e) {
    ElMessage.error(`打开在线 AI 失败：${e}`);
  }
}

/// 去首页应用（App 监听 @go-home 时可用；没接就退化为文字路径）
function goHome(): void {
  emit("goHome");
}

/// 列表下方的“文件结构与使用步骤”入口：切到生成指引标签
function openGuide(): void {
  tab.value = "guide";
}

/// 行内「应用」：选中该主题并跳回首页（由 App 的 @apply-theme 完成选择+切页）
function applyTheme(t: Theme): void {
  emit("applyTheme", t.name);
}

// Tab 变化：回传面包屑；切到在线素材时重扫 `online/`（手改 TOML 后马上可见）
watch(tab, async () => {
  emitCrumb();
  if (tab.value === "online") void refreshOnline();
  await nextTick();
  paneIn(document.getElementById(`pane-${tab.value}`));
});
</script>

<template>
  <div class="materials-page">
    <el-tabs v-model="tab" class="flat-tabs">
      <!-- ============ 本地素材：已安装清单 ============ -->
      <el-tab-pane label="本地素材" name="local">
        <el-card shadow="never" class="panel local-card">
          <template #header>
            <div class="panel-head">
              <span class="panel-title"><Icon icon="line-md:folder" /><span>已安装素材</span></span>
              <span class="count">{{ themes.length }}</span>
              <span class="grow"></span>
              <div class="head-actions">
                <el-button type="primary" :loading="importing" @click="importMaterial">
                  <Icon icon="line-md:file-import" /><span>导入素材</span>
                </el-button>
                <el-button @click="openAssetsDir">
                  <Icon icon="line-md:folder" /><span>打开目录</span>
                </el-button>
                <el-button :loading="reloading" title="重新扫描 assets/ 目录" @click="refreshThemes">
                  <Icon icon="line-md:backup-restore" /><span>刷新</span>
                </el-button>
                <el-button v-if="canGoHome" title="回首页，选择主题即自动下发" @click="goHome">
                  <Icon icon="line-md:home" /><span>去首页应用</span>
                </el-button>
              </div>
            </div>
          </template>

          <p class="local-hint">
            主题 = <code>assets/</code> 下的文件夹，主页面是 <code>index.html</code>；导入后回首页选择即自动下发，切换主题会自动切换。
          </p>

          <div v-if="themes.length" class="theme-table">
            <div class="theme-thead" aria-hidden="true">
              <span>名称与位置</span>
              <span>适用尺寸</span>
              <span class="th-act">操作</span>
            </div>
            <div class="theme-rows">
              <div v-for="t in themes" :key="t.name" class="theme-row">
                <div class="theme-main" :title="themeSummary(t)">
                  <span class="theme-name">{{ t.name }}</span>
                </div>
                <div class="theme-sizes">
                  <el-tag
                    v-for="v in t.variants"
                    :key="v.page"
                    size="small"
                    effect="plain"
                    :type="v.tag === 'v1p' ? 'success' : v.tag === 'v2' ? 'warning' : 'info'"
                  >
                    {{ variantLabel(v) }}
                  </el-tag>
                </div>
                <div class="theme-acts">
                  <el-button
                    v-if="canApply"
                    text
                    class="act-apply"
                    :title="`选中「${t.name}」并回首页应用`"
                    :aria-label="`应用素材 ${t.name}`"
                    @click="applyTheme(t)"
                  >
                    <Icon icon="line-md:play" /><span>应用</span>
                  </el-button>
                  <el-button
                    text
                    class="act-locate"
                    :title="`在资源管理器中定位「${t.name}」`"
                    :aria-label="`定位素材 ${t.name}`"
                    @click="revealTheme(t)"
                  >
                    <Icon icon="line-md:folder" />
                  </el-button>
                </div>
              </div>
            </div>
          </div>

          <div v-else class="local-empty">
            <Icon icon="line-md:image" class="local-empty-ic" />
            <p class="local-empty-t">{{ themesLoaded ? "还没有本地素材" : "正在读取素材目录…" }}</p>
            <p v-if="themesLoaded" class="local-empty-s">
              导入一个 HTML 面板：程序会把它整理成 <code>assets/主题名/</code>，图片一起复制。
            </p>
            <el-button v-if="themesLoaded" type="primary" :loading="importing" @click="importMaterial">
              <Icon icon="line-md:file-import" /><span>导入素材</span>
            </el-button>
          </div>
        </el-card>

        <button type="button" class="fold-bar" @click="openGuide">
          <Icon icon="line-md:list" class="fold-ic" />
          <span class="fold-title">文件结构与使用步骤</span>
          <span class="fold-sub">在「生成指引」里查看与复制提示词</span>
          <span class="fold-chev" aria-hidden="true">›</span>
        </button>
      </el-tab-pane>

      <!-- ============ 在线素材（内嵌 OnlinePage） ============ -->
      <el-tab-pane label="在线素材" name="online">
        <OnlinePage />
      </el-tab-pane>

      <!-- ============ 生成指引：提示词 + 尺寸/文件结构 + 使用步骤 ============ -->
      <el-tab-pane label="生成指引" name="guide">
        <div class="pane-scroll guide-scroll">
          <el-card shadow="never" class="panel guide-main">
            <template #header>
              <div class="panel-head">
                <span class="panel-title"><Icon icon="line-md:lightbulb" /><span>素材生成提示词</span></span>
                <div class="head-actions">
                  <el-button @click="copyPrompt">{{ copied ? "已复制" : "复制提示词" }}</el-button>
                  <el-button @click="openAi">
                    <Icon icon="line-md:cloud-alt-download" /><span>在线 AI</span>
                  </el-button>
                </div>
              </div>
            </template>
            <p class="hint">
              整段粘给 AI，按下方「尺寸与文件结构」放好文件；回首页选择主题即自动下发，切换主题会自动切换，运行中点「停止」；未连接设备时先刷新设备。
            </p>
            <pre class="prompt-box tall" tabindex="0" aria-label="素材生成提示词">{{ GUIDE }}</pre>
          </el-card>

          <div class="guide-grid">
            <el-card shadow="never" class="panel">
              <template #header>
                <div class="panel-head">
                  <span class="panel-title"><Icon icon="line-md:monitor" /><span>尺寸与文件结构</span></span>
                </div>
              </template>
              <p class="hint">
                主题 = <code>assets/</code> 下文件夹，主页面 <code>index.html</code>，图片同层、相对引用。
              </p>
              <pre class="tree" tabindex="0" aria-label="素材目录结构">assets/
└─ 主题名/
   ├─ index.html      主页面（图片同层）
   ├─ v1p/index.html  320×170
   └─ v2/index.html   320×240</pre>
              <div class="spec">
                <div class="spec-head">
                  <el-tag size="small" effect="plain" type="success">V1P</el-tag>
                  <b>320×170</b>
                  <span class="mini">V1PRO 小屏 · assets/主题名/v1p/index.html</span>
                </div>
                <div class="spec-head">
                  <el-tag size="small" effect="plain" type="warning">V2</el-tag>
                  <b>320×240</b>
                  <span class="mini">V2 屏 · assets/主题名/v2/index.html</span>
                </div>
              </div>
              <p class="hint">
                也兼容把 <code>index.html</code> 放主题根目录：程序按页面里声明的 <code>width</code> / <code>height</code>
                自动归类，识别不出时归为「任意尺寸」。
              </p>
              <p class="hint" :title="assetsDir || 'assets/'">
                素材目录：<code class="path">{{ assetsDir || "assets/" }}</code>
              </p>
            </el-card>

            <el-card shadow="never" class="panel">
              <template #header>
                <div class="panel-head">
                  <span class="panel-title"><Icon icon="line-md:list" /><span>使用步骤与快捷操作</span></span>
                </div>
              </template>
              <ol class="steps">
                <li>复制上面的提示词粘给 AI，说明要显示的指标。</li>
                <li>HTML 与图片放进 <code>assets/主题名/</code>，主页面 <code>index.html</code>。</li>
                <li>单尺寸主题只建一个子目录：<code>v1p/</code> 或 <code>v2/</code>。</li>
                <li>回首页选择主题即自动下发；切换主题会自动切换；运行中点「停止」；未连接设备时先刷新设备。</li>
              </ol>
              <div class="quick">
                <el-button @click="openAssetsDir">
                  <Icon icon="line-md:folder" /><span>素材目录</span>
                </el-button>
                <el-button @click="openOnlineFolder">
                  <Icon icon="line-md:cloud-alt-download" /><span>在线素材目录</span>
                </el-button>
              </div>
              <p class="hint">
                提示词包含数据槽约定、配色要求与交付检查清单；生成的 HTML 必须实现
                <code>__setSlot</code>，数据槽名从标准占位符里选。
              </p>
            </el-card>
          </div>
        </div>
      </el-tab-pane>
    </el-tabs>
  </div>
</template>

<style scoped>
.materials-page {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.materials-page :deep(.el-button) {
  gap: 5px;
}
/* ---------- 单层 Tab ---------- */
.flat-tabs {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.flat-tabs :deep(.el-tabs__header) {
  flex: none;
  margin: 0 0 8px;
}
.flat-tabs :deep(.el-tabs__nav-wrap::after) {
  height: 1px;
  background: var(--line);
}
.flat-tabs :deep(.el-tabs__item) {
  height: 30px;
  line-height: 30px;
  padding: 0 16px;
  color: var(--sub);
  font-size: 12.5px;
  transition: color 140ms ease;
}
.flat-tabs :deep(.el-tabs__item.is-active) {
  color: var(--accent);
  font-weight: 600;
}
.flat-tabs :deep(.el-tabs__active-bar) {
  height: 2px;
  border-radius: 2px;
  background: var(--accent);
  transition:
    transform 180ms cubic-bezier(0.2, 0.7, 0.2, 1),
    width 180ms cubic-bezier(0.2, 0.7, 0.2, 1);
}
.flat-tabs :deep(.el-tabs__content) {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}
.flat-tabs :deep(.el-tab-pane) {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.grow {
  flex: 1;
  min-width: 4px;
}
.head-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: var(--sp-3);
}

/* ---------- 本地素材 ---------- */
.local-card {
  flex: 0 1 auto;
  min-height: 200px;
  display: flex;
  flex-direction: column;
  margin: 0;
}
.local-card :deep(.el-card__body) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding-top: 8px;
}
.count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 20px;
  height: 18px;
  padding: 0 6px;
  border-radius: 999px;
  background: var(--btn);
  color: var(--sub);
  font-size: 11px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.local-hint {
  flex: none;
  margin: 0 0 6px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
}
.theme-table {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--line);
  border-radius: var(--r-ctl);
  overflow: hidden;
}
.theme-thead,
.theme-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 190px 92px;
  align-items: center;
  gap: var(--sp-3);
  padding: 0 10px;
}
.theme-thead {
  flex: none;
  height: 26px;
  border-bottom: 1px solid var(--line);
  background: color-mix(in srgb, var(--btn) 60%, transparent);
  color: var(--sub);
  font-size: 11.5px;
}
.th-act {
  text-align: right;
}
.theme-rows {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.theme-row {
  min-height: 44px;
  padding-top: 5px;
  padding-bottom: 5px;
  border-bottom: 1px solid var(--line);
}
.theme-row:last-child {
  border-bottom: none;
}
.theme-row:hover {
  background: color-mix(in srgb, var(--btn) 55%, transparent);
}
.theme-main {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.theme-name {
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
  line-height: 1.25;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.theme-sub {
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.theme-sizes {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  overflow: hidden;
}
.theme-sizes :deep(.el-tag) {
  flex: none;
}
.theme-acts {
  justify-self: end;
  display: flex;
  align-items: center;
  gap: 2px;
}
.theme-acts :deep(.el-button) {
  margin: 0;
  height: 26px;
  min-width: 26px;
  padding: 0 5px;
}
.act-apply {
  color: var(--accent);
}
.act-locate {
  color: var(--sub);
}
.act-locate:hover {
  color: var(--fg);
}
.local-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 18px 24px;
  text-align: center;
}
.local-empty-ic {
  font-size: 26px;
  color: var(--sub);
}
.local-empty-t {
  margin: 0;
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
}
.local-empty-s {
  margin: 0 0 4px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.55;
}
/* 列表下方的折叠入口：把文件结构/步骤收进“生成指引”，不再占首屏 */
.fold-bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  width: 100%;
  min-height: 34px;
  margin-top: var(--sp-2);
  padding: 4px 12px;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--panel) calc(var(--ui-alpha) * 100%), transparent);
  color: var(--fg);
  text-align: left;
  cursor: pointer;
}
.fold-bar:hover {
  background: color-mix(in srgb, var(--btn) 55%, transparent);
}
.fold-bar:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
.fold-ic {
  flex: none;
  color: var(--sub);
  font-size: 15px;
}
.fold-title {
  flex: none;
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
}
.fold-sub {
  flex: 1;
  min-width: 0;
  color: var(--sub);
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.fold-chev {
  flex: none;
  color: var(--sub);
  font-size: 16px;
  line-height: 1;
}

/* ---------- 生成指引 ---------- */
.pane-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding-right: 2px;
  padding-bottom: 2px;
}
.guide-scroll {
  display: flex;
  flex-direction: column;
  gap: var(--sp-4);
}
.guide-main {
  flex: none;
  margin: 0;
}
.guide-grid {
  flex: none;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--sp-4);
  align-items: start;
}
.guide-grid :deep(.el-card) {
  margin: 0;
}
.guide-grid :deep(.el-card__body) {
  padding: 7px 10px 9px;
}
.prompt-box {
  margin: 0;
  padding: 6px 8px;
  overflow: auto;
  border: 1px solid var(--line);
  border-radius: var(--r-ctl);
  background: var(--btn);
  color: var(--fg);
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 10.5px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}
.prompt-box.tall {
  max-height: 150px;
}
.tree {
  margin: 6px 0 0;
  padding: 5px 8px;
  overflow-x: auto;
  border: 1px solid var(--line);
  border-radius: var(--r-ctl);
  background: var(--btn);
  color: var(--sub);
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 10.5px;
  line-height: 1.45;
  white-space: pre;
}
.spec {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-top: 6px;
}
.spec-head {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--fg);
  font-size: 12px;
  min-width: 0;
}
.spec-head b {
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.mini {
  color: var(--sub);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.steps {
  margin: 0;
  padding-left: 16px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.55;
}
.steps li + li {
  margin-top: 2px;
}
.quick {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--sp-2);
  margin: 8px 0 2px;
}
.quick :deep(.el-button) {
  width: 100%;
  margin: 0;
}
code {
  padding: 0 3px;
  border-radius: 4px;
  background: var(--btn);
  color: var(--fg);
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 10.5px;
}
code.path {
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}
</style>

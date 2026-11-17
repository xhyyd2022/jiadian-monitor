<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { Icon } from "@iconify/vue";
import { ElButton, ElCard, ElForm, ElFormItem, ElInput, ElTag } from "element-plus";
import { useOnline, type OnlineSource } from "../composables/useOnline";

// 在线素材：内嵌在「素材 → 在线素材」单层标签里。
// 列表态只显示工具栏 + 列表（或空态）+ 折叠使用说明；编辑态整页切换成表单，
// 表单操作区固定在卡片底部，滚动字段时「取消 / 保存」始终可达。
// 逻辑与原实现一致：列表来自 useOnline 的共享状态，保存/删除后首页下拉同步刷新。
const { sources, loading, loaded, error, dirLabel, refresh, loadDir, save, remove, openDir } =
  useOnline();

type View = "list" | "edit";
const view = ref<View>("list");
/// 正在编辑的 slug；null = 新建
const editId = ref<string | null>(null);
const title = ref("");
const description = ref("");
const v1p = ref("");
const v2 = ref("");

/// 字段级校验（就地显示在对应输入框下）
const titleError = ref("");
const descError = ref("");
const v1pError = ref("");
const v2Error = ref("");
/// 组级校验：V1P / V2 全空
const urlGroupError = ref("");

/// 是否点过「保存」：只有点过之后才显示整表校验摘要，避免一进来就报错。
const saveAttempted = ref(false);
/// 整表校验摘要（字段下另有具体红字；这里保证滚动时也能看到“为什么没保存”）
const formSummary = computed(() => {
  if (!saveAttempted.value) return "";
  const missing: string[] = [];
  if (titleError.value) missing.push("标题");
  if (urlGroupError.value || v1pError.value || v2Error.value) missing.push("页面地址");
  if (descError.value) missing.push("描述");
  if (!missing.length) return "";
  return `无法保存：请先补全${missing.join("、")}。`;
});
/// 表单底部常驻提示（保存失败 / 删除确认）
const formMsg = ref("");
const formBad = ref(false);
const saving = ref(false);
const deleting = ref(false);
/// 删除两段式：第一次点只把文案变成「确认删除」，第二次才真删
const confirmDelete = ref(false);
/// 列表页常驻的一行提示（操作结果）
const listMsg = ref("");
const listBad = ref(false);
let listTimer: number | undefined;
/// 折叠的「使用说明」（默认收起，不压过主操作）
const helpOpen = ref(false);
/// 小控件“按下即响应 + 350ms 防抖”（与其它页面一致）：
/// 轻微指针漂移/掉帧时 click 可能不触发，用户在 pointerdown 就得到响应；
/// 紧随其后的 click 被时间戳吞掉，键盘 Enter/Space 仍走 click。
const PRESS_LOCK_MS = 350;
let lastPressAt = 0;
function pressOnce(fn: () => void) {
  const now = Date.now();
  if (now - lastPressAt < PRESS_LOCK_MS) return;
  lastPressAt = now;
  fn();
}
function onPressDown(fn: () => void) {
  return (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    pressOnce(fn);
  };
}

const titleInput = ref<{ focus: () => void } | null>(null);
const v1pInput = ref<{ focus: () => void } | null>(null);
const v2Input = ref<{ focus: () => void } | null>(null);

const busy = computed(() => saving.value || deleting.value);
/// el-form 的 model（字段本身用 ref 绑定，这里只给表单一个响应式模型）
const formModel = computed(() => ({
  title: title.value,
  description: description.value,
  url_v1p: v1p.value,
  url_v2: v2.value,
}));

function flash(msg: string, bad = false): void {
  listMsg.value = msg;
  listBad.value = bad;
  clearTimeout(listTimer);
  listTimer = window.setTimeout(() => {
    listMsg.value = "";
    listBad.value = false;
  }, bad ? 6000 : 2200);
}

function clearFieldErrors(): void {
  titleError.value = "";
  descError.value = "";
  v1pError.value = "";
  v2Error.value = "";
  urlGroupError.value = "";
}

function clearForm(): void {
  title.value = "";
  description.value = "";
  v1p.value = "";
  v2.value = "";
  formMsg.value = "";
  formBad.value = false;
  confirmDelete.value = false;
  saveAttempted.value = false;
  clearFieldErrors();
}

function startNew(): void {
  editId.value = null;
  clearForm();
  view.value = "edit";
  // 等表单渲染出来再聚焦标题，键盘用户可以直接输入
  void nextTick(() => titleInput.value?.focus());
}

function startEdit(src: OnlineSource): void {
  editId.value = src.id;
  title.value = src.title;
  description.value = src.description;
  v1p.value = src.url_v1p || "";
  v2.value = src.url_v2 || "";
  formMsg.value = "";
  formBad.value = false;
  confirmDelete.value = false;
  saveAttempted.value = false;
  clearFieldErrors();
  view.value = "edit";
}

function cancelEdit(): void {
  if (busy.value) return;
  view.value = "list";
  editId.value = null;
  formMsg.value = "";
  formBad.value = false;
  confirmDelete.value = false;
  saveAttempted.value = false;
  clearFieldErrors();
}

/// 与后端同规则的本地预校验（只做即时提示，最终仍以后端校验为准）：
/// https:// 任意站点；http:// 仅限本机/局域网地址；不允许带账号密码。
/// 本机/局域网地址判定：localhost、.local、127.x、10.x、172.16-31.x、192.168.x、169.254.x、
/// ::1、fc00::/7、fe80::/10。与后端 `online::is_private_host` 保持一致。
function isPrivateHost(hostname: string): boolean {
  const h = hostname.replace(/^\[|\]$/g, "").toLowerCase();
  if (h === "localhost" || h.endsWith(".local")) return true;
  if (h === "::1") return true;
  const v4 = h.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    if (a === 127 || a === 10) return true;
    if (a === 172 && b >= 16 && b <= 31) return true;
    if (a === 192 && b === 168) return true;
    if (a === 169 && b === 254) return true;
    return false;
  }
  if (h.startsWith("fc") || h.startsWith("fd") || h.startsWith("fe8") || h.startsWith("fe9") || h.startsWith("fea") || h.startsWith("feb")) return true;
  return false;
}

function urlIssue(value: string): string {
  const raw = value.trim();
  if (!raw) return "";
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return "格式不对（例：https://example.com/panel/）";
  }
  if (url.username || url.password) return "不能带账号密码";
  if (url.protocol === "https:") return "";
  if (url.protocol === "http:" && isPrivateHost(url.hostname)) return "";
  return "http:// 仅支持本机或局域网地址（如 http://192.168.1.10:3002/）";
}

/// 标题 / 描述的单字段校验：失焦时立刻反馈，不只等点保存。
function checkTitle(): void {
  const trimmed = title.value.trim();
  if (!trimmed) titleError.value = "请填写标题";
  else if (trimmed.length > 40) titleError.value = "标题最多 40 个字";
  else titleError.value = "";
}
function checkDescription(): void {
  if (description.value.length > 2000) descError.value = "描述最多 2000 个字";
  else descError.value = "";
}
function checkUrl(which: "v1p" | "v2"): void {
  const target = which === "v1p" ? v1p : v2;
  const issue = urlIssue(target.value);
  if (which === "v1p") v1pError.value = issue;
  else v2Error.value = issue;
}
/// 输入时清掉对应字段错误；任一尺寸有内容就撤销“至少填一个”的组级提示。
function onUrlInput(which: "v1p" | "v2"): void {
  if (which === "v1p") v1pError.value = "";
  else v2Error.value = "";
  urlGroupError.value = "";
}

/// 保存前统一校验：全空 → 组级提示 + 聚焦 V1P；无效 URL → 对应字段报错并聚焦。
function validateForSave(): boolean {
  saveAttempted.value = true;
  clearFieldErrors();
  checkTitle();
  checkDescription();
  const a = v1p.value.trim();
  const b = v2.value.trim();
  if (!a && !b) {
    urlGroupError.value = "至少填写一种尺寸的页面地址（V1P 或 V2）";
  } else {
    if (a) v1pError.value = urlIssue(a);
    if (b) v2Error.value = urlIssue(b);
  }
  const ok = !titleError.value && !descError.value && !urlGroupError.value && !v1pError.value && !v2Error.value;
  if (titleError.value) titleInput.value?.focus();
  else if (urlGroupError.value || v1pError.value) v1pInput.value?.focus();
  else if (v2Error.value) v2Input.value?.focus();
  return ok;
}

async function saveForm(): Promise<void> {
  if (busy.value) return;
  if (!validateForSave()) return;
  saving.value = true;
  formMsg.value = "正在保存…";
  formBad.value = false;
  try {
    await save({
      id: editId.value,
      title: title.value.trim(),
      description: description.value,
      urlV1p: v1p.value.trim(),
      urlV2: v2.value.trim(),
    });
    // 保存成功：composable 已刷新共享列表 → 这里回列表即可（首页在线下拉也同步更新）
    view.value = "list";
    editId.value = null;
    formMsg.value = "";
    flash("已保存");
  } catch (e) {
    formMsg.value = `保存失败：${e}`;
    formBad.value = true;
  } finally {
    saving.value = false;
  }
}

async function deleteItem(): Promise<void> {
  if (!editId.value || busy.value) return;
  if (!confirmDelete.value) {
    confirmDelete.value = true;
    formMsg.value = "再点一次「删除」确认；删除后无法恢复。";
    formBad.value = true;
    return;
  }
  deleting.value = true;
  formMsg.value = "正在删除…";
  formBad.value = false;
  try {
    await remove(editId.value);
    view.value = "list";
    editId.value = null;
    confirmDelete.value = false;
    flash("已删除");
  } catch (e) {
    formMsg.value = `删除失败：${e}`;
    formBad.value = true;
  } finally {
    deleting.value = false;
  }
}

/// 手动重扫 `online/`（手改 TOML 后靠它）
async function reload(): Promise<void> {
  listMsg.value = "";
  listBad.value = false;
  await refresh();
}

async function openFolder(): Promise<void> {
  try {
    await openDir();
  } catch (e) {
    flash(`打开目录失败：${e}`, true);
  }
}

/// 描述在列表里只显示第一行（多行描述把行高撑乱）
function firstLine(value: string): string {
  const line = value.split(/\r?\n/)[0]?.trim() || "";
  return line || "（无描述）";
}

onMounted(() => {
  // 首次进入（组件一开始是隐藏的）先把 `online/` 扫一遍；已加载过就只补目录标签
  if (loaded.value) void loadDir();
  else void refresh();
});
onUnmounted(() => clearTimeout(listTimer));
</script>

<template>
  <div class="online-materials">
    <!-- ============ 列表态 ============ -->
    <template v-if="view === 'list'">
      <el-card shadow="never" class="panel online-card">
        <template #header>
          <div class="panel-head">
            <span class="panel-title"><Icon icon="line-md:cloud-alt-download" /><span>已添加素材</span></span>
            <span class="count">{{ loading ? "…" : sources.length }}</span>
            <span class="grow"></span>
            <div class="head-actions">
              <el-button v-if="sources.length" type="primary" @click="startNew">
                <Icon icon="line-md:edit" /><span>新建在线素材</span>
              </el-button>
              <el-button :disabled="loading" @click="reload">
                <Icon icon="line-md:backup-restore" /><span>刷新</span>
              </el-button>
              <el-button @click="openFolder">
                <Icon icon="line-md:folder" /><span>打开目录</span>
              </el-button>
            </div>
          </div>
        </template>

        <div class="online-body">
          <template v-if="sources.length">
            <p class="list-hint">
              在线素材 = 一个网页地址；V1P 320×170 / V2 320×240 各一个 URL，至少填一种尺寸。
              素材目录：<code>{{ dirLabel || "online/" }}</code>
            </p>
            <p v-if="listMsg || error" class="msg" :class="{ bad: listBad || !!error, ok: !listBad && !error && !!listMsg }">
              {{ listMsg || error }}
            </p>
            <div class="online-list">
              <button
                v-for="s in sources"
                :key="s.id"
                type="button"
                class="online-item"
                :title="`点击编辑 ${s.id}.toml`"
                @click="startEdit(s)"
              >
                <span class="oi-body">
                  <span class="oi-title">{{ s.title }}</span>
                  <span class="oi-desc">{{ firstLine(s.description) }}</span>
                </span>
                <span class="oi-tags">
                  <el-tag size="small" :type="s.url_v1p ? 'success' : 'info'" effect="plain">
                    <Icon :icon="s.url_v1p ? 'line-md:confirm-circle' : 'line-md:minus'" class="oi-tag-ic" />
                    {{ s.url_v1p ? "V1P 已配置" : "V1P 未配置" }}
                  </el-tag>
                  <el-tag size="small" :type="s.url_v2 ? 'success' : 'info'" effect="plain">
                    <Icon :icon="s.url_v2 ? 'line-md:confirm-circle' : 'line-md:minus'" class="oi-tag-ic" />
                    {{ s.url_v2 ? "V2 已配置" : "V2 未配置" }}
                  </el-tag>
                </span>
                <span class="oi-edit"><Icon icon="line-md:edit" /><span>编辑</span></span>
              </button>
            </div>
          </template>

          <div v-else class="online-empty">
            <Icon :icon="error ? 'line-md:close-circle' : 'line-md:cloud-alt-download'" class="online-empty-ic" />
            <p class="online-empty-t">{{ error ? "读取在线素材失败" : "还没有在线素材" }}</p>
            <p class="online-empty-s">
              {{ error || "在线素材是一个网页地址：V1P 320×170 / V2 320×240 各一个，至少填一种尺寸。" }}
            </p>
            <el-button v-if="!error" type="primary" @click="startNew">
              <Icon icon="line-md:edit" /><span>新建在线素材</span>
            </el-button>
            <el-button v-else :disabled="loading" @click="reload">
              <Icon icon="line-md:backup-restore" /><span>重新读取</span>
            </el-button>
          </div>
        </div>
      </el-card>

      <!-- 使用说明：默认收起，不压过列表与主操作 -->
      <section class="help-fold" :class="{ open: helpOpen }">
        <button
          type="button"
          class="help-head"
          :aria-expanded="helpOpen"
          aria-controls="online-help-body"
          @pointerdown="onPressDown(() => (helpOpen = !helpOpen))"
          @click.prevent="pressOnce(() => (helpOpen = !helpOpen))"
        >
          <Icon icon="line-md:list" class="help-ic" />
          <span class="help-title">使用说明</span>
          <span class="help-sub">在线页面如何被首页使用、缺失尺寸与数据来源</span>
          <span class="chev" aria-hidden="true">›</span>
        </button>
        <div v-show="helpOpen" id="online-help-body" class="help-body">
          <ol class="notes">
            <li>在线素材 = 一个在线页面的地址；V1P（320×170）与 V2（320×240）各一个 URL，可只填一个。</li>
            <li>回「首页」，把主题切换卡切到「在线素材」，选择素材即自动下发；切换素材会自动切换；运行中点「停止」；未连接设备时先刷新设备。</li>
            <li>在线页面需要自己从 <code>http://127.0.0.1:8766</code> 取数据（程序不注入取数脚本）。</li>
            <li>在线页面是远程代码，只添加你信任的地址；不要填带账号密码的 URL。</li>
            <li>保存 / 删除后列表与首页下拉会自动刷新；手改 <code>online/*.toml</code> 后点「刷新」重扫。</li>
          </ol>
        </div>
      </section>
    </template>

    <!-- ============ 编辑态：整页表单，操作区固定在底部 ============ -->
    <el-card v-else shadow="never" class="panel form-card">
      <template #header>
        <div class="panel-head">
          <el-button text class="back-btn" title="返回在线素材列表" @click="cancelEdit">
            <Icon icon="line-md:rotate-90" /><span>返回列表</span>
          </el-button>
          <span class="panel-title">
            <Icon :icon="editId ? 'line-md:edit' : 'line-md:cloud-alt-download'" />
            <span>{{ editId ? "编辑在线素材" : "新建在线素材" }}</span>
          </span>
          <el-tag v-if="editId" size="small" effect="plain" type="info">{{ editId }}</el-tag>
        </div>
      </template>

      <div class="form-body">
        <p v-if="formSummary" class="form-alert" role="alert">
          <Icon icon="line-md:close-circle" /><span>{{ formSummary }}</span>
        </p>
        <el-form :model="formModel" label-position="top" class="online-form" @submit.prevent>
          <el-form-item :error="titleError">
            <template #label>
              <span class="flabel">标题<span class="req">*</span><span class="cnt">{{ title.trim().length }}/40</span></span>
            </template>
            <el-input
              ref="titleInput"
              v-model="title"
              aria-label="素材标题"
              autocomplete="off"
              maxlength="40"
              placeholder="例如：天气面板"
              :disabled="busy"
              @input="titleError = ''"
              @blur="checkTitle"
            />
          </el-form-item>

          <el-form-item :error="descError">
            <template #label>
              <span class="flabel">描述<span class="cnt">{{ description.length }}/2000</span></span>
            </template>
            <el-input
              v-model="description"
              type="textarea"
              aria-label="素材描述"
              autocomplete="off"
              :rows="2"
              maxlength="2000"
              placeholder="可留空，仅用于界面展示"
              :disabled="busy"
              @input="descError = ''"
              @blur="checkDescription"
            />
          </el-form-item>

          <div class="url-group">
            <div class="group-head">
              <span class="group-title">页面地址：至少填写一种尺寸</span>
            </div>
            <p class="group-hint">只填一种尺寸也可以保存；留空表示该尺寸没有可用页面。</p>

            <div class="form-grid">
              <el-form-item :error="v1pError">
                <template #label>
                  <span class="flabel">V1P（320×170）</span>
                </template>
                <el-input
                  ref="v1pInput"
                  v-model="v1p"
                  aria-label="V1P 页面地址"
                  autocomplete="off"
                  spellcheck="false"
                  placeholder="https://example.com/panel/v1p/"
                  :disabled="busy"
                  @input="onUrlInput('v1p')"
                  @blur="checkUrl('v1p')"
                />
              </el-form-item>

              <el-form-item :error="v2Error">
                <template #label>
                  <span class="flabel">V2（320×240）</span>
                </template>
                <el-input
                  ref="v2Input"
                  v-model="v2"
                  aria-label="V2 页面地址"
                  autocomplete="off"
                  spellcheck="false"
                  placeholder="https://example.com/panel/v2/"
                  :disabled="busy"
                  @input="onUrlInput('v2')"
                  @blur="checkUrl('v2')"
                />
              </el-form-item>
            </div>

            <p v-if="urlGroupError" class="group-err" role="alert">
              <Icon icon="line-md:close-circle" /><span>{{ urlGroupError }}</span>
            </p>
            <p class="hint">
              只接受 <code>https://</code>（任意站点）或 <code>http://</code> 的本机/局域网地址
              （如 <code>http://192.168.1.10:3002/</code>）；不要带账号密码。
              在线页面需自行从 <code>http://127.0.0.1:8766</code> 取数据。
            </p>
          </div>
        </el-form>
      </div>

      <div class="form-foot">
        <p v-if="formMsg" class="msg" :class="{ bad: formBad }">{{ formMsg }}</p>
        <div class="foot-actions">
          <el-button
            v-if="editId"
            type="danger"
            plain
            :loading="deleting"
            :disabled="busy"
            @click="deleteItem"
          >
            {{ confirmDelete ? "确认删除" : "删除" }}
          </el-button>
          <span class="grow"></span>
          <el-button :disabled="busy" @click="cancelEdit">取消</el-button>
          <el-button type="primary" :loading="saving" :disabled="busy" @click="saveForm">保存</el-button>
        </div>
      </div>
    </el-card>
  </div>
</template>

<style scoped>
.online-materials {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--sp-3);
}
.online-materials :deep(.el-button) {
  gap: 5px;
}
.grow {
  flex: 1;
  min-width: 4px;
}
.head-actions {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
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

/* ---------- 列表 ---------- */
.online-card {
  flex: 0 1 auto;
  min-height: 200px;
  display: flex;
  flex-direction: column;
  margin: 0;
}
.online-card :deep(.el-card__body) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding-top: 8px;
}
.online-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.list-hint {
  flex: none;
  margin: 0 0 4px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.5;
}
.msg {
  flex: none;
  min-height: 16px;
  margin: 2px 0;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.45;
  overflow-wrap: anywhere;
}
.msg.bad {
  color: var(--danger);
}
.msg.ok {
  color: var(--ok);
}
.online-list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
  padding: 2px 2px 2px 0;
}
.online-item {
  flex: none;
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  width: 100%;
  min-height: 44px;
  padding: 5px 10px;
  border: 1px solid var(--line);
  border-radius: var(--r-ctl);
  background: transparent;
  color: var(--fg);
  text-align: left;
  cursor: pointer;
  transition: border-color 0.12s ease, background 0.12s ease;
}
.online-item:hover {
  border-color: color-mix(in srgb, var(--accent) 55%, var(--line));
  background: color-mix(in srgb, var(--btn) 55%, transparent);
}
.online-item:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
.oi-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.oi-title {
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.oi-desc {
  color: var(--sub);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.oi-tags {
  flex: none;
  display: flex;
  align-items: center;
  gap: 4px;
}
.oi-tags :deep(.el-tag) {
  display: inline-flex;
  align-items: center;
  gap: 3px;
}
.oi-tag-ic {
  font-size: 11px;
}
.oi-edit {
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--sub);
  font-size: 11.5px;
}
.online-item:hover .oi-edit {
  color: var(--accent);
}

/* 空态：居中标题 + 一句说明 + 主按钮 */
.online-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 16px 24px;
  text-align: center;
}
.online-empty-ic {
  font-size: 28px;
  color: var(--sub);
}
.online-empty-t {
  margin: 0;
  color: var(--fg);
  font-size: 13px;
  font-weight: 600;
}
.online-empty-s {
  margin: 0 0 4px;
  max-width: 420px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.55;
}

/* ---------- 使用说明折叠 ---------- */
.help-fold {
  flex: none;
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: color-mix(in srgb, var(--panel) calc(var(--ui-alpha) * 100%), transparent);
  overflow: hidden;
}
.help-head {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  width: 100%;
  min-height: 36px;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  padding: 4px 12px;
  border: none;
  background: transparent;
  color: var(--fg);
  text-align: left;
  cursor: pointer;
}
.help-head:hover {
  background: color-mix(in srgb, var(--btn) 55%, transparent);
}
.help-head:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.help-ic {
  flex: none;
  color: var(--sub);
  font-size: 15px;
}
.help-title {
  flex: none;
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
}
.help-sub {
  flex: 1;
  min-width: 0;
  color: var(--sub);
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chev {
  flex: none;
  color: var(--sub);
  font-size: 16px;
  line-height: 1;
  transform: rotate(90deg);
  transition: transform 0.12s ease;
}
.help-fold.open .chev {
  transform: rotate(-90deg);
}
.help-body {
  padding: 2px 12px 10px 39px;
  border-top: 1px dashed var(--line);
}
.notes {
  margin: 6px 0 0;
  padding-left: 16px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.55;
}
.notes li + li {
  margin-top: 2px;
}

/* ---------- 编辑表单 ---------- */
.form-card {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  margin: 0;
}
.form-card :deep(.el-card__body) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: 0;
}
.back-btn {
  color: var(--sub);
  padding: 0 4px;
}
.back-btn:hover {
  color: var(--fg);
}
.form-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  /* 底部多留出固定操作区的高度：滚到底时最后一段说明不会被「取消 / 保存」压住 */
  padding: 8px 12px 58px;
}
.form-alert {
  display: flex;
  align-items: center;
  gap: 5px;
  margin: 0 0 8px;
  padding: 5px 8px;
  border: 1px solid color-mix(in srgb, var(--danger) 40%, var(--line));
  border-radius: var(--r-ctl);
  background: color-mix(in srgb, var(--danger) 9%, var(--panel));
  color: var(--fg);
  font-size: 11.5px;
  line-height: 1.45;
}
.form-alert svg {
  flex: none;
  color: var(--danger);
}
.online-form :deep(.el-form-item) {
  margin-bottom: 8px;
}
/* EP 默认把错误文字 absolute 定位到输入框下方，会和下一字段标签重叠；
   这里改成参与布局的块级错误行，item 自动长高。 */
.online-form :deep(.el-form-item__error) {
  position: static;
  padding-top: 2px;
  line-height: 1.4;
  font-size: 11px;
}
.online-form :deep(.el-form-item__label) {
  height: auto;
  line-height: 16px;
  padding: 0 0 3px;
  color: var(--sub);
  font-size: 11.5px;
}
.flabel {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
}
.req {
  color: var(--danger);
}
.cnt {
  margin-left: auto;
  color: var(--sub);
  font-size: 10.5px;
}
.form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0 10px;
}
.url-group {
  margin: 2px 0 4px;
  padding: 8px 10px 2px;
  border: 1px solid var(--line);
  border-radius: var(--r-ctl);
  background: color-mix(in srgb, var(--btn) 45%, transparent);
}
.group-head {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
  min-width: 0;
}
.group-title {
  color: var(--fg);
  font-size: 12.5px;
  font-weight: 600;
}
.group-req {
  margin-left: auto;
  color: var(--sub);
  font-size: 11px;
  white-space: nowrap;
}
.group-hint {
  margin: 3px 0 4px;
  color: var(--sub);
  font-size: 11.5px;
  line-height: 1.45;
}
.group-err {
  display: flex;
  align-items: center;
  gap: 4px;
  margin: 0 0 6px;
  color: var(--danger);
  font-size: 11.5px;
  line-height: 1.45;
}
.form-foot {
  flex: none;
  padding: 6px 12px 10px;
  border-top: 1px solid var(--line);
  background: color-mix(in srgb, var(--panel) calc(var(--ui-alpha) * 100%), transparent);
}
.form-foot .msg {
  margin: 0 0 2px;
}
.foot-actions {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
}
.foot-actions :deep(.el-button) {
  margin: 0;
}

/* ---------- 通用内联 ---------- */
code {
  padding: 0 3px;
  border-radius: 4px;
  background: var(--btn);
  color: var(--fg);
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 10.5px;
}
</style>

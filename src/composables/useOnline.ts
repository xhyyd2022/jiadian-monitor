import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

// 在线素材的共享状态：首页「主题切换」卡的在线下拉（App.vue）与侧栏「在线」页
// （OnlinePage.vue）共用同一份列表 —— 保存/删除后只 refresh 一次，两边同时更新。
//
// **返回字段是 snake_case**：后端 `OnlineSource` 没有 `serde(rename_all)`，序列化出来
// 就是 `url_v1p` / `url_v2`。Tauri 只在**命令参数名**上做 camelCase ↔ snake_case 转换，
// 返回的结构体原样透传，所以这里必须按 snake_case 读。
// （见 app/ONLINE_MODE.md §3）
export interface OnlineSource {
  /** 文件名 slug（= `online/<slug>.toml`），保存/删除都靠它定位 */
  id: string;
  title: string;
  description: string;
  /** V1P（320×170）版 URL；null / 空串 = 该尺寸没有 */
  url_v1p: string | null;
  /** V2（320×240）版 URL；null / 空串 = 该尺寸没有 */
  url_v2: string | null;
}

/// `online_save` 的参数：前端用 camelCase，Tauri 会映射到 Rust 的 snake_case 形参。
export interface OnlineDraft {
  /** null = 新建（后端按标题生成 slug） */
  id: string | null;
  title: string;
  description: string;
  urlV1p: string;
  urlV2: string;
}

const sources = ref<OnlineSource[]>([]);
const loading = ref(false);
/// 是否成功读过一次（App.vue 用它判断在线模式点「确定」前要不要先补一次 online_list）
const loaded = ref(false);
/// 列表级错误（读取失败）：App 的在线下拉与「在线」页都会显示
const error = ref("");
/// `online_dir()` 返回的「相对程序目录」标签，如 `online/`
const dirLabel = ref("");

/// 刷新版本号：并发 refresh 只让最后一次的结果落地，避免旧响应覆盖新列表
let revision = 0;
let dirPromise: Promise<void> | null = null;

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function urlOrNull(value: unknown): string | null {
  const s = text(value).trim();
  return s ? s : null;
}

/// 归一化后端返回：字段缺失按空处理，一条坏数据不让整张列表崩掉。
function normalize(raw: OnlineSource | null | undefined): OnlineSource | null {
  if (!raw || typeof raw !== "object") return null;
  const id = text(raw.id).trim();
  if (!id) return null;
  return {
    id,
    title: text(raw.title).trim() || id,
    description: text(raw.description),
    url_v1p: urlOrNull(raw.url_v1p),
    url_v2: urlOrNull(raw.url_v2),
  };
}

/// 素材目录标签：`online_dir()`，拿不到就让界面退回显示 `online/`
function loadDir(): Promise<void> {
  if (dirLabel.value) return Promise.resolve();
  if (dirPromise) return dirPromise;
  dirPromise = invoke<string>("online_dir")
    .then((dir) => {
      dirLabel.value = text(dir).trim();
    })
    .catch(() => {
      /* 拿不到就显示默认 online/ */
    })
    .finally(() => {
      dirPromise = null;
    });
  return dirPromise;
}

/// 重新扫描 `online/`（进入在线模式 / 打开「在线」页 / 手工刷新时调用）。
/// 失败时保留旧列表，只把原因写进 `error`。
async function refresh(): Promise<boolean> {
  const token = ++revision;
  loading.value = true;
  try {
    const raw = await invoke<OnlineSource[]>("online_list");
    if (token !== revision) return false;
    const next: OnlineSource[] = [];
    for (const item of Array.isArray(raw) ? raw : []) {
      const src = normalize(item);
      if (src) next.push(src);
    }
    sources.value = next;
    loaded.value = true;
    error.value = "";
    // 顺手拿一次「相对程序目录」标签（失败不影响列表）
    void loadDir();
    return true;
  } catch (e) {
    if (token === revision) error.value = `读取在线素材失败：${e}`;
    return false;
  } finally {
    if (token === revision) loading.value = false;
  }
}

/// 保存（新建 id = null / 覆盖已有）。成功后就地刷新共享列表，「首页」在线下拉立即可见。
/// 校验与错误提示交给调用方（OnlinePage）—— 这里只把后端错误原样抛出去。
async function save(draft: OnlineDraft): Promise<void> {
  await invoke("online_save", {
    id: draft.id,
    title: draft.title,
    description: draft.description,
    urlV1p: draft.urlV1p,
    urlV2: draft.urlV2,
  });
  await refresh();
}

/// 删除一条，成功后刷新共享列表。
async function remove(id: string): Promise<void> {
  await invoke("online_delete", { id });
  await refresh();
}

/// 资源管理器打开 `online/` 目录。
async function openDir(): Promise<void> {
  await invoke("online_open_dir");
  void loadDir();
}

/// 取某个尺寸对应的 URL。`tag` 是 `v1p` / `v2`；没探到设备时（`any`）按 V2 优先、
/// V1P 兜底取一个，不让下拉在尺寸未知时直接禁用。
export function urlFor(src: OnlineSource, tag: string): string | null {
  if (tag === "v1p") return src.url_v1p;
  if (tag === "v2") return src.url_v2;
  return src.url_v2 || src.url_v1p;
}

export function useOnline() {
  return {
    sources,
    loading,
    loaded,
    error,
    dirLabel,
    refresh,
    loadDir,
    save,
    remove,
    openDir,
  };
}

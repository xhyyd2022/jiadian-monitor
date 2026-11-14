# 在线素材（Online Mode）设计约定

> 状态：实现中（2026-11-13）。本文件是前后端共同遵守的接口约定；改接口先改这里。
> 背景与浏览器行为调研见对话记录：在线页面自己从 `http://localhost:8766` 取数；
> 数据槽/单位/外设字典见 `MONITOR_METRICS.md` 与 `skills/rgb565-panel-material/`。

## 1. 目标

- 用户手动添加「在线素材」：一条 TOML = 一个在线页面（标题 + 描述 + V1P/V2 两个 URL）。
- 在线素材与本地素材并列：首页「主题切换」卡片顶部两个大按钮「本地素材 / 在线素材」；
  侧栏「设置」上方新增「在线」页管理在线素材。
- 在线页面**自己 fetch** `http://127.0.0.1:8766` 取数据（不注入、不要求 `__setSlot`）。
- 在线页面由 app 的隐藏 WebView2 加载（与本地素材同一渲染管线），按设备尺寸 320×170 / 320×240
  执行 `Emulation.setDeviceMetricsOverride`，之后照常 CDP 抓帧 → RGB565 → 差分 → USB。

## 2. 存储：`online/` 目录

- 位置：**exe 同级 `online/`**；找不到时回退当前工作目录（与 `assets/`、`peripherals/` 同规则）。
  首次需要时自动创建；便携包带上该目录（可含 `README.md` 说明，不含用户配置）。
- 一条在线素材 = 一个独立 TOML：`online/<slug>.toml`；`slug` 为文件名（不含扩展名），
  仅 `[a-z0-9][a-z0-9-]*`，由界面自动生成（标题转写失败时用 `online-<时间戳>`），保证唯一。
- 文件 schema（UTF-8）：

```toml
title = "天气面板"
description = """
多行描述，可留空。
"""

[v1p]
url = "https://example.com/panel/v1p/"

[v2]
url = "https://example.com/panel/v2/"
```

- `title`：必填，1–40 字符（去首尾空白）。
- `description`：可选，多行字符串，最长 2000 字符，仅用于界面展示。
- `[v1p]` / `[v2]`：至少存在一个；每个表里的 `url` 必填。
- `url`：`https://` 开头；为方便本机调试，`http://localhost` / `http://127.0.0.1` 也允许。
  其他 scheme 一律拒绝。URL 不得含账号密码。
- 手改 TOML 支持：打开「在线」页/切换在线模式时重新扫描；解析失败的文件**只记日志并跳过**
  （v1 不在界面弹窗），不影响其他文件；保存走原子写（临时文件 + rename）。
- 排序：按 `title` 的本地化排序；标题重复不报错（靠 slug 区分）。

## 3. 后端接口（Tauri commands）

```rust
struct OnlineSource {
    id: String,            // 文件名 slug
    title: String,
    description: String,
    url_v1p: Option<String>,
    url_v2: Option<String>,
}

#[tauri::command] fn online_list() -> Result<Vec<OnlineSource>, String>;
#[tauri::command] fn online_save(id: Option<String>, title: String, description: String,
                                 url_v1p: String, url_v2: String) -> Result<OnlineSource, String>;
#[tauri::command] fn online_delete(id: String) -> Result<(), String>;
#[tauri::command] fn online_dir() -> Result<String, String>;       // 展示用「相对程序目录」标签
#[tauri::command] fn online_open_dir() -> Result<(), String>;      // 资源管理器打开
```

- 解析/写盘放 `app/src-tauri/src/online.rs`；`toml` 作为直接依赖（源码已在 cargo 缓存，
  离线应可构建；若不行再退化到手写子集解析，但必须先试通）。
- 保存时按设备尺寸无关：两个 URL 都保存成字符串（空串 = 该尺寸没有）。
- 保存/删除后前端重新 `online_list()`。

## 4. 面板加载（Rust 渲染管线）

- 复用现有 `render_start` 命令：`html` 字段允许传 `https://…` / `http://localhost…` 完整 URL；
  以 `http://` / `https://` 开头即视为在线模式。
- 在线模式：
  - `panel_url` 不转 `file://`，直接使用该 URL；追加 `?__api=8766`（已有 query 用 `&`，
    保留 hash），供页面读取本地 API 端口。
  - **不注入** `pull_js` 自取脚本（页面自己 fetch）；`Page.addScriptToEvaluateOnNewDocument`
    与后置 `Runtime.evaluate` 都跳过。
  - CDP target 匹配不能用文件名：用「scheme://host」做 needle（忽略 path/query/hash；
    浏览器会小写化 host、省略默认端口，needle 同步处理）。
  - 页面就绪等待上限比本地素材放宽（如 10s），超时/证书错误/HTTP 错误要把原因回给前端。
  - 仍执行 `Emulation.setDeviceMetricsOverride(width, height)`，与本地素材一致。
- 本地模式行为完全不变（`file://` + `?__api=8766` + 注入脚本）。

## 5. 安全

- 在线页面是**远程代码**，只允许加载用户显式添加的 URL；不做任意 URL 代理/跳转。
- 不给 panel WebView2 增加 Tauri capability（远程 origin 不应能调 Tauri IPC）。
- 8766 控制类接口收紧：`POST /app/show`、`POST /app/quit` 仅在
  「无 Origin 头」或「Origin: null」时执行，其他 Origin 返回 403；数据类
  `/metrics`、`/units`、`/peripherals`、`/themes` 等保持 `Access-Control-Allow-Origin: *`。
  （页面自取只需要数据类接口。）
- 不添加 `--disable-web-security`；不修改 WebView2 既有启动参数（在线页当前依赖
  WebView2 的 LNA 默认关闭 + localhost 混合内容豁免；后续 WebView2 若默认开启 LNA 再评估）。
- 在线 URL 只允许 https / localhost-http；保存时校验，加载前再校验一次。

## 6. 前端

- `Sidebar.vue`：`在线` 放在 `设置` 上方；图标用已注册的 `line-md:cloud-alt-download`（或等价已注册图标）。
- 首页「主题切换」卡片：
  - 顶部两个大按钮 `本地素材` / `在线素材`（等宽、大号，选中态用 accent 底色）；
  - 本地模式：现有下拉 + `确定 / 停止 / 导入素材`；
  - 在线模式：下拉只显示 `title`（缺当前尺寸 URL 的条目加「（缺此尺寸）」并禁用确定）；
    按钮 `确定 / 停止`；提示「在侧栏「在线」页管理」；
  - 卡片内容整体下移/铺满：模式按钮在上，内容区自适应，状态行（正在运行/错误）贴底；
    不要出现明显空白。
- 新增 `OnlinePage.vue`（侧栏「在线」）：
  - 列表：标题 + 当前是否有 v1p/v2 URL；「新建」「刷新」「打开目录」；
  - 编辑器：标题、描述（textarea）、V1P URL、V2 URL；校验提示；保存 / 删除 / 取消；
  - 保存后回到列表并刷新首页在线下拉。
- 共享状态用新 composable（如 `app/src/composables/useOnline.ts`），App.vue 与 OnlinePage.vue 共用。
- 设备尺寸（`v1p`/`v2`）变化时，在线选择自动改用对应 URL；没有则提示缺尺寸。
- 运行中切在线/本地：点「确定」时沿用现有 render session 流程（旧的会停止/替换），失败回显。

## 7. 验收

- `cargo check` / `cargo test --lib`（新增 online 单元测试：解析、校验、save/delete 往返、slug）。
- `npm run build`（vue-tsc + vite）通过。
- 不启动 GUI、不操作真机（交付后由用户构建运行）。
- 截图验收（当前 UI 和谐度）用现有 `tools/gui_shot.py` 或独立 mockup；实现完成后由 Lead 补。
- 打包：`build.ps1` / 便携包把 `online/` 目录带上（内容不放用户配置）；README 增加在线模式说明。

## 8. 本次实现验证（2026-11-13）

- Rust：`cargo test --lib` 全量 **107 passed**（含 online 模块 6 个单测：解析/URL 规则/缺失字段/save-list-delete/坏文件跳过/slug）。
- 前端：`npm run build`（vue-tsc + vite）通过。
- UI 截图（mock IPC 预览）：`outputs/shots/2026-11-13/new-home-local.png`、`new-home-online.png`、`new-online-page.png`。
- 未做：启动 GUI / 真机下发 / 真实 https 页面加载（需用户构建后验证）；编译环境注意：本机中文路径下 ring 的 `/Z7` 会报 D8050，
  用 ASCII `CARGO_TARGET_DIR` 可绕过（不影响项目默认构建脚本）。

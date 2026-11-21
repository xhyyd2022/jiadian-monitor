# 在线素材（Online Mode）设计约定

> 状态：**已实现（2026-11-13），2026-11-18 修订**。本文件是前后端共同遵守的接口约定 + 在线页面的开发要求；改接口先改这里。
> 数据槽/单位/外设字典见 `MONITOR_METRICS.md`；素材怎么做见 `skills/jiadian-monitor-skills/`（`SKILL.md` §二 在线素材契约）。
> 一句话：**本地素材由 app 注入脚本喂数，在线素材由页面自己取数**；其余渲染链路完全相同。

## 0. 两种模式对照

| | 本地素材（local） | 在线素材（online） |
|---|---|---|
| 内容从哪来 | exe 同级 `assets/<主题>/v1p|v2/*.html` | 用户添加的 URL（`online/<slug>.toml`） |
| 页面地址 | `file:///…?__api=8766` | `https://…?__api=8766`（保留原有 query/hash） |
| 谁取数 | app 通过 CDP 注入 `pull_js`，1s 一轮调 `__setSlot` | **页面自己** `fetch http://127.0.0.1:8766/…` |
| `__setSlot` | 必须有（C4） | **不要求**（可选，实现了更好） |
| 视口 | `Emulation.setDeviceMetricsOverride` 320×170 / 320×240 | 同左 |
| 抓帧/差分/USB | 相同 | 相同 |
| 页面来源 | 本地文件（受控） | 远程代码（用户显式添加） |
| 断网 | 不受影响 | 页面打不开 → 报错停止 |

**结论（用户问「哪种最合适托管到云端」）**：在线模式本来就是为「页面托管在云端、数据留在本机」设计的——
页面放静态托管（GitHub Pages / Vercel / OSS / 自建 nginx）拿 https，数据仍由本机 8766 提供。
云端只托管**静态 HTML/JS/CSS**，不要托管任何数据、不要放密钥。局域网自建（`http://192.168.x.x:8080/`）同样支持，适合内网/无外网场景。

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
- `url` 允许范围（`online.rs::validate_url`，保存与加载各校验一次）：
  - `https://`：任意站点（云端托管走这条）；
  - `http://`：**仅限本机与局域网私有地址**——`localhost`、`127.0.0.0/8`、`10.0.0.0/8`、
    `172.16.0.0/12`、`192.168.0.0/16`、`169.254.0.0/16`、`::1`、`fc00::/7`、`fe80::/10`、`*.local`；
    公网 `http://` 一律拒绝（防中间人注入画面）。
  - 其他 scheme 一律拒绝；URL 不得含账号密码（`user:pass@`）、空白与控制字符。
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

- 复用现有 `render_start` 命令：`html` 字段允许传 `https://…` / `http://…` 完整 URL；
  以 `http://` / `https://` 开头即视为在线模式。
- 在线模式（`pipeline.rs`）：
  - `panel_url` 不转 `file://`，直接使用该 URL；追加 `?__api=8766`（已有 query 用 `&`，
    保留 hash；**URL 里已有的 `__api` 先移除再追加**，避免重复参数）。
  - **不注入** `pull_js` 自取脚本（页面自己 fetch）；`Page.addScriptToEvaluateOnNewDocument`
    与后置 `Runtime.evaluate` 都跳过。
  - CDP target 匹配不能用文件名：用「scheme://host」做 needle（忽略 path/query/hash；
    浏览器会小写化 host、省略默认端口，needle 同步处理）。
  - 页面就绪等待上限放宽到 10s（本地 3s）；超时 / 证书错误 / HTTP 错误（`#main-frame-error`、
    `about:neterror`、`chrome-error://`）要把原因回给前端，不要静默重试。
  - 仍执行 `Emulation.setDeviceMetricsOverride(width, height)`，与本地素材一致。
- 本地模式行为完全不变（`file://` + `?__api=8766` + 注入脚本）。

## 5. 安全

- 在线页面是**远程代码**，只允许加载用户显式添加的 URL；不做任意 URL 代理/跳转。
- 不给 panel WebView2 增加 Tauri capability（远程 origin 不应能调 Tauri IPC）。
- 8766 控制类接口收紧：`POST /app/show`、`POST /app/quit` 仅在
  「无 Origin 头」或「Origin: null」时执行，其他 Origin 返回 403；数据类
  `/metrics`、`/units`、`/peripherals`、`/themes` 等保持 `Access-Control-Allow-Origin: *`。
  （页面自取只需要数据类接口。）
- 不添加 `--disable-web-security`；不修改 WebView2 既有启动参数。
- 在线 URL 只允许 https / 本机与局域网私有 http（见 §2）；保存时校验，加载前再校验一次。
- 页面侧要求：不引第三方统计脚本、不放密钥（页面是公开可读的）、`CSP` 若设置必须放行
  `connect-src 'self' http://127.0.0.1:8766`（否则 fetch 被 CSP 拦掉，表现为「画面有、数据不动」）。

### 5.1 已知风险：Local Network Access（原 Private Network Access）

- 现状：在线页（尤其云端 https origin）从「公网 origin」请求 `127.0.0.1:8766`，正是 Chromium
  LNA 管辖的场景；当前可用**依赖 WebView2 尚未默认强制 LNA + localhost 混合内容豁免**。
- 症状（若被强制）：`fetch` 抛 `TypeError: Failed to fetch`，页面上表现为数据一直是 `—`；
  控制台出现 `Permission denied for local network access` 或 preflight 失败。
- 应对顺序（真正被强制时再动，别提前改）：
  1. 在 WebView2 侧注册 `PermissionRequested` 处理器，对 `local network access` 自动放行
     （只对 app 自己加载的 panel origin 放行）；
  2. 仍不行再评估 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 的 feature 开关（会全进程生效，谨慎）；
  3. 长期方案：app 内起一个仅本机回环的 https 反向代理（证书固定），页面改请求它。
- 局域网 http 页面（`http://192.168.x.x`）同样会命中 LNA（private → local），风险一致。

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

## 7. 在线页面怎么写（交给云端托管的那个包）

详细契约见 `skills/jiadian-monitor-skills/`：`SKILL.md` §二「在线素材契约 C9–C12」、
`references/contract.md` §8、`references/online_hosting.md`。摘要：

1. **一个能独立打开的静态页面**（可以多文件、可带图片/字体/CSS，相对路径引用；不要 `file://`、
   不要盘符绝对路径）。托管在 https 或局域网 http，直接填 URL 即可。
2. **自己取数**（app 不注入）：
   ```js
   const port = new URLSearchParams(location.search).get('__api') || '8766';
   const base = 'http://127.0.0.1:' + port;
   // 并发拉 /metrics /units /peripherals，1s 一轮，单请求 4s 超时（AbortController）
   ```
   数值槽传数字字符串、文本槽传字符串；`/metrics` 整轮失败时 CPU 三槽显示 `—`（不是 0）。
3. **视口锁死**：按 320×170 / 320×240 设计，不滚动、不居中、不留白（与本地素材同判据 A1–A3）。
4. **不碰控制接口**：不要调 `/app/show`、`/app/quit`（403），也不要假设页面能调 Tauri IPC。
5. **失败可见**：网络/接口失败显示 `—` 或「离线」，不要空白或假数据；恢复后自动继续。
6. **可选兼容**：实现了 `window.__setSlot` + `data-slot` 的页面，本地模式也能直接跑（一份代码两处用）。
7. **可验证**：`python scripts/check_material.py panel.html --mode online`（A16–A19 + B 层）。

## 8. 验收

- `cargo check` / `cargo test --lib`（online 单元测试：解析、校验、save/delete 往返、slug、`__api` 去重）。
- `npm run build`（vue-tsc + vite）通过。
- 不启动 GUI、不操作真机（交付后由用户构建运行）。
- 截图验收（当前 UI 和谐度）用现有 `tools/gui_shot.py` 或独立 mockup；实现完成后由 Lead 补。
- 打包：`build.ps1` / 便携包把 `online/` 目录带上（内容不放用户配置）；README 增加在线模式说明。

## 9. 本次实现验证（2026-11-13）

- Rust：`cargo test --lib` 全量 **107 passed**（含 online 模块 6 个单测：解析/URL 规则/缺失字段/save-list-delete/坏文件跳过/slug）。
- 前端：`npm run build`（vue-tsc + vite）通过。
- UI 截图（mock IPC 预览）：`outputs/shots/2026-11-13/new-home-local.png`、`new-home-online.png`、`new-online-page.png`。
- 未做：启动 GUI / 真机下发 / 真实 https 页面加载（需用户构建后验证）；编译环境注意：本机中文路径下 ring 的 `/Z7` 会报 D8050，
  用 ASCII `CARGO_TARGET_DIR` 可绕过（不影响项目默认构建脚本）。

## 10. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-13 | 初版：TOML 存储、Tauri commands、`?__api` 追加、跳过注入、10s 就绪、Origin 收紧 |
| 2026-11-18 | 补 §0 本地/在线对照与「云端托管」结论；URL 规则改为「https 任意 + 私有地址 http」（对齐 `online.rs`，`a399df6`）；补 §5.1 LNA 风险与应对；补 §7 在线页面开发要求（指向技能）；`__api` 参数去重 |

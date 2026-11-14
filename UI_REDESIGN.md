# 佳点监控 · UI 重设计规范 v2

> 目标：保留全部功能，**用 Element Plus 彻底重写界面**；统一风格、统一逻辑；
> 固定 780×540 窗口；支持 DPI 缩放；支持用户背景 / 配色 / 透明度。

## 0. 参考资料

- [Material Design 3](https://m3.material.io/)：颜色角色（primary / surface / outline / on-*）、
  4/8px 间距、形状比例、状态层、类型比例。
- [Microsoft Fluent 2](https://fluent2.microsoft.design/) 与
  [Windows Geometry](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/geometry)：
  桌面端 4px 控件圆角、8px 浮层圆角、Mica/Acrylic 材质、Focus 视觉、Segoe UI。
- [Element Plus Design](https://element-plus.org/en-US/guide/design.html)：
  一致性 / 反馈 / 效率 / 可控性；[主题](https://element-plus.org/en-US/guide/theming.html) 与
  [暗黑模式](https://element-plus.org/en-US/guide/dark-mode.html) 全部走 CSS 变量。

## 1. 技术栈

- Vue 3.5 + TypeScript + Vite + Tauri 2。
- **Element Plus 2.x**（全量引入 + 中文 locale + `size="small"`），覆盖 CSS 变量对齐现有浅/深色风格。
- 保留 `@iconify/vue` 的 line-md 图标；原生 select 全部由 Element Plus `el-select` 取代（不再使用手写 SelectBox）。
- 删除旧的手工 `.cols/.home-grid`、原生 select、按钮补丁样式，统一到 EP 组件 + 少量布局 CSS。

## 2. 固定窗口与 DPI

- 主窗口固定 **780×540 逻辑像素**；禁止用户随意缩放破坏布局（resizable=false）。
- 结构：标题栏 35px → **顶部细导航栏 28px** → 内容区（左右 14px / 上下 12px）。
- 内容区：694×451（`780-58-28=694`，`540-35-28-24=453`，按实际渲染微调）。
- DPI：Tauri 逻辑尺寸随系统缩放自动放大物理窗口；Rust 监听 `ScaleFactorChanged`
  重新对齐 780×540 逻辑尺寸，保证 CSS 视口恒定、不重排。

## 3. 顶部细导航（面包屑）

- 每个页面在内容区顶部显示 28px 高的 `el-breadcrumb`，展示当前层级，可点击跳回。
- 层级示例：
  - `首页`
  - `性能`
  - `外设`
  - `素材 / 素材管理 / 本地素材`
  - `素材 / 素材管理 / 在线素材`
  - `素材 / 生成指引`
  - `设置`
- 点击任意层级：切页 / 切 Tab / 切子 Tab；右侧可放当前设备连接状态小圆点。
- 样式：透明底、11.5px 字号、当前层 `--el-text-color-primary`，父级弱化；不使用大标题占高。

## 4. 页面结构（子卡片化）

### 首页
- 左右两列独立堆叠（异形组合）：左列设备 + 显示，右列主题 + 预览；左右列不强制等高/十字对齐，用内容高度自然决定卡片分界。
- 子卡片：设备信息、显示控制、主题切换、预览。

### 性能
- 顶部面包屑后，`el-card` 两列栅格：CPU / GPU / 内存 / 磁盘 / 网络 / 风扇 / 系统；
  每张子卡片内是紧凑指标行；页面可滚动。

### 外设
- 多个子卡片：外设列表（每项一行，操作按钮）、未知/兼容设备说明、刷新/目录操作。

### 素材（在线合并进来）
- 顶部 `el-tabs` 两级：
  1. **素材管理** / **生成指引**（一级 Tab）
  2. 素材管理内再分 **本地素材** / **在线素材**（二级 Tab）
- 素材管理-本地：多个子卡片
  - 「添加素材」：目录、导入按钮、当前主题/尺寸、步骤说明。
  - 「素材文件结构」：目录树/说明。
  - 「提示词」：本地素材生成提示词（只读/复制）。
- 素材管理-在线：多个子卡片
  - 「在线素材列表」：标题、描述、V1P/V2 URL 状态、编辑/删除。
  - 「新增 / 编辑在线素材」：标题、描述、URL 表单；保存/取消。
  - 「使用说明」：在线素材如何被首页使用、缺失尺寸提示。
- 生成指引：多个子卡片
  - 「生成指引」主卡片（长文，内部滚动）。
  - 「尺寸规范」：V1P 320×170 / V2 320×240 卡片。
  - 「快捷操作」：复制提示词、打开素材目录等。
- 侧栏移除「在线」入口；首页在线模式不变，但管理入口指向素材页在线 Tab。

### 设置
- 多个子卡片，两列栅格：
  - 启动与行为（开机自启 / 静默启动 / 关闭到托盘 / 开发者工具）
  - 性能（高帧率 / 兼容模式）
  - 运行日志（开关 / 级别 / 说明）
  - 单位设置（全自动/手动 + 容量/速率/温度/时长）
  - 数据源（LibreHardwareService 状态/安装/重启/卸载）
  - **外观**（新）：主题、用户背景、配色盘、界面透明度
  - 开发者/关于（现有信息）
- 每张子卡片独立边框、标题栏、说明；页面可滚动。

## 5. 外观（新增）

设置 → 外观子卡片：
- **主题**：浅色 / 深色 / 跟随系统。
- **背景**：默认纯色 / 用户导入图片（PNG/JPG/WebP，≤8MB）；
  预览缩略图、清除按钮；背景铺满窗口，卡片覆盖半透明面板。
- **配色盘**：预设色板（蓝、青、绿、琥珀、紫、粉、红）+ `el-color-picker` 自定义；
  生成 9 级 primary 色阶，写入 `--el-color-primary-light-*`，保证按钮/选中态/面包屑一致。
- **界面透明度**：8%–100% 滑杆，作用在卡片/侧栏表面（文字保持不透明以满足对比度）；
  背景图可见但文字对比度 ≥ 4.5:1。
- 持久化到 `settings.ini`；重启恢复。
- 背景图导入与存储由 Rust 命令处理（复制到应用旁 `ui/` 目录，返回本地访问 URL），
  前端只保存相对文件名。

## 6. 统一控件

- `el-button`：small，圆角 6px（Fluent 4px / 可读性折中），高度 26–28；primary/danger/plain 区分。
- `el-select`：small，统一宽高；下拉浮层 EP 自带圆角；不再手写。
- `el-switch` / `el-slider` / `el-progress` / `el-input` / `el-tabs` / `el-card` / `el-breadcrumb`。
- 所有页面 `.page` 统一 padding、gap；子卡片统一 `el-card` 圆角和标题字号。
- 状态色仅用于语义（连接/警告/错误），不偏离 EP 色板太多。

## 7. 验收

- `npm run build`、Rust `cargo test`、静态预览 780×540 截图（六页、浅/深色、外观设置效果）。
- 不改变 Tauri 命令/事件/业务状态；只重写视图层与外观设置。


## 最终实现补充（2026-11-13）

- 窗口：Tauri 配置固定 780×540 逻辑尺寸，`resizable=false` / `maximizable=false`；实际运行 WebView2 `innerWidth/Height = 780/540`，DPR=1.5 时物理 1170×810。
- WebView2 用户数据：放在 exe 同级 `webview-data/`，不写 `%LOCALAPPDATA%`，避免受限环境 0x800700AA，同时保持便携。
- 首页留白：卡片 header 8/12、body 9/12/10；信息行 22px + 虚线分隔；左右列 12px gap；右列主题卡按内容高度，预览吃满剩余高度。
- 外观：背景 PNG/JPG/WebP ≤8MB、预设色 + 自定义、透明度 60–100（默认 94），卡片 `backdrop-filter` 保证背景图上文字可读。
- 截图：实际运行版浅/深各 16 张，位于 `outputs/shots/actual-final/`；自动审计脚本 `app/scripts/ui_audit.py`，运行版 axe 0 violations。

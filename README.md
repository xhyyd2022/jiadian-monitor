# jiaidian-monitor

把**本机实时硬件数据**通过 USB 推送到**佳点（jiaidian）V1P / V2 小屏设备**的 Windows 桌面监控程序。

程序在本机采集 CPU / 内存 / 磁盘 / 网络 / GPU / 温度 / 风扇等数据，注入一块单文件 HTML「面板」，
用内嵌的 WebView2 离屏渲染成画面，做像素差分后按 **LIVE 协议**增量下发给设备 —— 设备屏上就是一块
可以自定义外观的实时监控副屏。

> **许可提示**：本项目采用 **PolyForm Noncommercial License 1.0.0** —— **源码可见、禁止商业使用**。
> 它不是 OSI 认可的开源许可（属于 source-available）。详见 [LICENSE](LICENSE)。

---

## 功能

- **实时推屏**：离屏渲染 → RGB565 → 脏矩形差分 → USB 增量下发，贴近 30fps，无变化不推帧。
- **两种设备自适应**：按设备自报尺寸渲染，V1P = 320×170、V2 = 320×240，素材按尺寸分目录。
- **多设备切换**：设备下拉列表（SN 尾号 + 机型），切换时自动停止旧设备 → 探测尺寸 → 按该尺寸历史主题重连下发。
- **主题素材**：一个主题 = `assets/<主题名>/<尺寸>/index.html`（HTML 与图片同层），可从本地手动导入。
- **显示设置**：亮度 / 旋转 / 跟随息屏，一次接管连发三条并强制整屏重发。
- **数据源**：LibreHardwareService（LHS）共享内存，程序内可一键下载 / 校验（sha256）/ 安装；网络由 Win32 `GetIfTable2` 统一提供。
- **单位两档**：全自动（按素材声明的 `data-unit` 换算）/ 手动（四类单位各选一覆盖）。
- **托盘常驻**：可设置关闭时收进系统托盘、开机静默启动。
- **与官方上位机共存**：上位机（GUI）在运行时，通过其本地 HTTP 的 USB 使用权移交（`/usb/handoff/*`）借用 USB，**不需要退出 GUI**。

## 运行环境

| 项 | 要求 |
| --- | --- |
| 系统 | Windows 10 / 11 x64 |
| WebView2 | Win10/11 一般已内置；缺失时需安装 Evergreen Runtime |
| 设备 | 佳点 V1P（320×170）或 V2（320×240），走 WinUSB |
| 数据源（可选） | LibreHardwareService —— 程序内可一键安装；不装则只显示网络与磁盘容量等 Win32 指标 |

## 本地开发

前置：**Node.js 20+**、**Rust stable（MSVC 工具链）**、**Visual Studio 2022 生成工具（勾选「使用 C++ 的桌面开发」）**。

```powershell
npm install
npm run tauri dev      # 开发模式（Vite 热更新 + 原生窗口）
npm run tauri build    # 生产构建，产物在 src-tauri/target/release/ 与 bundle/
```

构建产物：

- 可执行文件：`src-tauri/target/release/jiaidian-monitor.exe`
- 安装包：`src-tauri/target/release/bundle/`（NSIS `.exe` + WiX `.msi`）

### 便携运行布局

把可执行文件和一个 `assets/` 目录放在一起即可绿色运行：

```
jiaidian-monitor.exe
assets/            主题素材（一个主题 = 一层文件夹，文件夹名即主题名）
log/               运行日志（自动创建）
```

程序按 **exe 同级 → 当前工作目录** 的顺序找 `assets/`。

## 工作原理

```
本机数据采集 (Rust)                       ← LHS 共享内存 / Win32 GetIfTable2
      ↓  注入数据槽 (window.__setSlot)
单文件 HTML 面板（离屏 WebView2 渲染）
      ↓  CDP 抓帧 → RGB565
像素差分（脏矩形）
      ↓  LIVE 协议组帧
WinUSB 增量下发 → 设备屏
```

- 程序自身起一个本地 HTTP 服务（`127.0.0.1:8766`）：`/metrics`（规范单位指标）、`/units`、`/health`。
  面板里的自取脚本每秒拉一次 `/metrics`，按 `data-unit` 换算后喂给 `__setSlot`。
- 与官方上位机共用设备时，通过 `/usb/handoff/start|keepalive|finish` 借用 USB 使用权（租约 120s，程序每 20s 续期）。
- 设备协议见 `src-tauri/src/live_frame.rs`、`device.rs`、`winusb.rs`。

## 目录结构

```
src/                    前端（Vue 3 + TypeScript）
  App.vue               主界面 / 设备与主题切换 / 实时预览
  components/           侧栏、设置页、素材页、标题栏
  composables/          设置项
  line-md-icons.ts      离线打包的 line-md 图标子集（不是 CDN）
src-tauri/              Rust 后端
  src/
    pipeline.rs         实时流管线：抓帧 → 差分 → LIVE 帧 → USB 下发
    render.rs           CDP / 离屏 WebView2 渲染
    device.rs           设备命令（亮度 / 旋转 / 跟随息屏 / 状态）
    winusb.rs           WinUSB 设备枚举与读写
    handoff.rs          向上位机申请 / 续期 / 归还 USB 使用权
    assets.rs           主题素材扫描与导入
    source_lhws.rs      LibreHardwareService 共享内存读取
    lhws_install.rs     LHS 检测 / 下载 / 校验 / 安装
    metrics.rs          指标组装（含 Win32 磁盘 / 网络 / 开机时长）
    units.rs            单位换算
    api.rs              本地 HTTP API（127.0.0.1:8766）
    tray.rs / single.rs 托盘、单实例
  capabilities/         窗口权限
```

## 第三方组件

本项目**静态链接 / 打包**了若干第三方开源组件（Vue、Tauri、Iconify 图标等），并可协助安装
**LibreHardwareService（MPL-2.0）** —— 后者是**独立的第三方软件**，程序仅在用户确认后从其官方
发布页下载、按固定 sha256 校验、再用 msiexec 安装，**不修改其任何文件**。

完整的组件清单、版本与许可证见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)；
MIT / Apache-2.0 / MPL-2.0 许可全文见 [`licenses/`](licenses/)。

## 许可证

[PolyForm Noncommercial License 1.0.0](LICENSE) —— 允许个人、研究、实验等**非商业**用途；
**禁止任何商业使用**。这是 source-available（源码可见）许可，**不是 OSI 认可的开源许可**。

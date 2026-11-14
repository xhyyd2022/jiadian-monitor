# jiadian-monitor

把**本机实时硬件数据**通过 USB 推送到**佳点（jiadian）V1P / V2 小屏设备**的 Windows 桌面监控程序。

程序在本机采集 CPU / 内存 / 磁盘 / 网络 / GPU / 温度 / 风扇等数据，注入一块单文件 HTML「面板」，
用内嵌的 WebView2 离屏渲染成画面，做像素差分后按 **LIVE 协议**增量下发给设备 —— 设备屏上就是一块
可以自定义外观的实时监控副屏。

> **许可提示**：本项目采用 **PolyForm Noncommercial License 1.0.0** —— **源码可见、禁止商业使用**。
> 它不是 OSI 认可的开源许可（属于 source-available）。详见 [LICENSE](LICENSE)。

---

## 功能

- **实时推屏**：离屏渲染 → RGB565 → 脏矩形差分 → USB 增量下发，无变化不推帧；画面长时间静止时自动降频（静止退避）。
- **帧率两档**：高刷约 30fps / 普通约 10fps（默认，更省资源），设置页一键切换、立即生效。抓屏节奏与下发节奏保持一致。
- **两种设备自适应**：按设备自报尺寸渲染，V1P = 320×170、V2 = 320×240，素材按尺寸分目录。
- **多设备切换**：设备下拉列表（SN 尾号 + 机型），切换时自动停止旧设备 → 探测尺寸 → 按该尺寸历史主题重连下发。
- **主题素材**：一个主题 = `assets/<主题名>/<尺寸>/index.html`（HTML 与图片同层），可从本地手动导入。
- **在线素材**：一条在线素材 = `online/<id>.toml`（标题 / 描述 / V1P、V2 两个 https URL）；页面由隐藏 WebView2 加载，自己从本机 `127.0.0.1:8766` 取数，按设备尺寸抓帧下发。首页「主题切换」卡本地/在线两种模式随时切换；侧栏「在线」页管理配置（可手改 TOML）。
- **USB 掉线自愈**：设备拔插或旧句柄写入失败（含 `WinUsb_WritePipe 失败：22`）时，管线自动释放旧链路、按 SN 重新枚举接管并整屏重发，不再停在「运行中但面板无数据」；重连期间状态行显示「正在自动重连…」。「刷新」按钮强制以物理枚举为准，并保留本程序正在接管的设备。
- **显示设置**：亮度 / 旋转 / 跟随息屏，一次接管连发三条并强制整屏重发。
- **数据源**：LibreHardwareService（LHS）共享内存，程序内可一键下载 / 校验（sha256）/ 安装；网络由 Win32 `GetIfTable2` 统一提供。
- **单位两档**：全自动（按素材声明的 `data-unit` 换算）/ 手动（四类单位各选一覆盖）。
- **设置集中在一个文件**：exe 同级 `settings.ini`（带注释、可手工编辑），界面改动即原子写回。
- **托盘常驻**：可设置关闭时收进系统托盘、开机静默启动；收托盘 / 最小化时自动停推预览（画面看不到就不传）。
- **与官方上位机共存**：上位机（GUI）在运行时，通过其本地 HTTP 的 USB 使用权移交（`/usb/handoff/*`）借用 USB，**不需要退出 GUI**。
- **外设电量**：侧栏「外设」页显示鼠标 / 键盘 / 耳机的品牌、型号与电量。协议是**纯 JSON 配置**（`peripherals/index.json` 按 PID 绑定品牌，`peripherals/brands/<品牌>.json` 写各品牌的读法），新增设备只改 JSON、不用重新编译。

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
npm run tauri build    # 生产构建，只生成 exe（不生成安装包）
```

构建产物：

- 可执行文件：`src-tauri/target/release/jiadian-monitor.exe`
- 便携包：发版工作流会生成 `jiadian-monitor-v<版本>.zip`（exe + `assets/` + `peripherals/` + 卸载脚本 + 许可证）；本地把 exe 与 `assets/`、`peripherals/` 放一起即可运行

> **本地完整包与云端包的区别**：仓库内 `assets/` 只收录程序图标（`icon.png`），主题素材不随公开仓库分发，
> 因此云端 Release 的 zip **不含主题文件夹**；本地 `scripts/build.ps1` 从部署目录 `bin/assets/` 打包，
> 会把已放入的主题一起带上，并额外包含 `online/` 模板、`LICENSE` 与 `THIRD-PARTY-NOTICES.md`。
> 需要开箱即用的完整主题，请使用本地打出的完整包，或把自己的主题放进 `assets/`。

### 便携运行布局

把可执行文件和 `assets/`、`peripherals/` 目录放在一起即可绿色运行：

```
jiadian-monitor.exe
settings.ini       全部设置（自动生成，可手工编辑）
assets/            主题素材（一个主题 = 一层文件夹，文件夹名即主题名）
peripherals/       外设协议 JSON（index.json + brands/，可直接编辑）
online/            在线素材 TOML（程序自动创建；可手工编辑，界面里也有管理页）
log/               运行日志（自动创建）
```

程序按 **exe 同级 → 当前工作目录** 的顺序找 `assets/`、`peripherals/` 与 `online/`。

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

- 程序自身起一个本地 HTTP 服务（仅回环，默认端口 8766；`127.0.0.1` / `localhost` / `[::1]` 均可）：
  `/metrics`（规范单位指标）、`/units`、`/health` 等，`GET /` 是接口索引。
  面板里的自取脚本每秒拉一次 `/metrics`，按 `data-unit` 换算后喂给 `__setSlot`。
- **在线素材**走同一条渲染/差分/USB 管线，但页面来自用户配置的 https URL，**不注入**自取脚本：
  页面自己 `fetch('http://127.0.0.1:8766/metrics')`（可读 `?__api=8766`）取数。
  只允许 https（本机调试允许 `http://localhost`）；控制类接口 `/app/show`、`/app/quit`
  仅接受 `Origin: null` / 无 Origin 的调用，远程页面无法调用。
- 与官方上位机共用设备时，通过 `/usb/handoff/start|keepalive|finish` 借用 USB 使用权（租约 120s，程序每 20s 续期）。
- 外设电量走 HID（不是 WinUSB）：枚举 HID 端点的 `VID/PID/UsagePage/Usage/报表长度`，按
  `peripherals/index.json` 找到品牌与协议档位，再按该品牌 JSON 发查询帧、解析回包。
  `GET /peripherals` 返回品牌 / 型号 / 电量，`POST /peripherals/refresh` 立即重读配置并轮询一轮。
- 设备协议见 `src-tauri/src/live_frame.rs`、`device.rs`、`winusb.rs`；外设协议见
  `peripherals/README.md`（字段全集 + 示例）与 `src-tauri/src/peripherals/`。

## 目录结构

```
src/                    前端（Vue 3 + TypeScript）
  App.vue               主界面 / 设备与主题切换 / 实时预览
  components/           侧栏、设置页、素材页、在线页、标题栏
  composables/          设置项、在线素材共享状态
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
    api.rs              本机数据 API（仅回环，8766；IPv4 + IPv6 双监听）
    online.rs           在线素材 TOML 扫描 / 校验 / 原子保存 + 远程面板加载
    peripherals/        外设电量：HID 枚举 + JSON 协议引擎（expr / model / hid / mod）
    tray.rs / single.rs 托盘、单实例
  capabilities/         窗口权限
peripherals/            外设协议 JSON 库（index.json + brands/），随包发布、用户可直接改
online-defaults/        在线素材 README 模板（发版时复制到 bin/online/，用户配置不进 ZIP）
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

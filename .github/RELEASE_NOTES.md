<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这三处：package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml（必须一致）。
-->

## v0.1.0 · 首个版本

把**本机实时硬件数据**通过 USB 推送到**佳点（jiaidian）V1P / V2 小屏设备**的 Windows 桌面监控程序。

程序在本机采集 CPU / 内存 / 磁盘 / 网络 / GPU / 温度 / 风扇等数据，注入一块单文件 HTML「面板」，
用内嵌的 WebView2 离屏渲染成画面，做像素差分后按 LIVE 协议增量下发给设备 —— 设备屏上就是一块
可以自定义外观的实时监控副屏。

### 主要功能

- **实时推屏**：离屏渲染 → RGB565 → 脏矩形差分 → USB 增量下发，贴近 30fps，无变化不推帧
- **两种设备自适应**：V1P（320×170）/ V2（320×240），按设备自报尺寸渲染
- **多设备切换**：设备下拉列表（SN 尾号 + 机型），切换时自动重连下发
- **主题素材**：一个主题一个文件夹，可从本地手动导入
- **显示设置**：亮度 / 旋转 / 跟随息屏
- **数据源**：LibreHardwareService 共享内存，程序内可一键下载 / 校验 / 安装；网络由 Win32 提供
- **单位两档**：全自动（按素材声明）/ 手动
- **托盘常驻**：可设置关闭收进托盘、开机静默启动
- **与官方上位机共存**：通过 USB 使用权移交借用设备，**不需要退出上位机**

### 安装

- 安装包：下载 `jiaidian-monitor_*_x64-setup.exe`（NSIS）或 `.msi`，双击安装
- 绿色版：解压 `jiaidian-monitor-portable.zip`，保持 `jiaidian-monitor.exe` 与 `assets/` 同级即可运行

运行环境：Windows 10 / 11 x64；WebView2 一般已内置，缺失时需安装 Evergreen Runtime。

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

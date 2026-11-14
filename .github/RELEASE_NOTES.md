<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  **只写本次版本的改动**：上一版正文不留在这里，历史看 GitHub Release 页面。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.9.6 · 修复「普通权限启动时托盘图标不显示」

### 根因：exe 的完整性级别低于资源管理器，托盘注册被 Windows 拦截

- Windows 给每个进程和文件都带一个完整性级别（Mandatory Integrity Level）。托盘图标需要通过
  `Shell_NotifyIcon` 向资源管理器（explorer.exe，中等完整性）注册；当本程序以**低完整性**运行时，
  这条跨完整性级别的消息会被 Windows 的 UIPI 机制拦截，注册静默失败——任务管理器里进程正常、
  界面正常、日志也写着“系统托盘就绪”，但通知区域就是没有图标。
- 本机排查结果：程序 exe 及其所在目录被标记为 `Low Mandatory Level`，普通双击启动后进程完整性为
  Low（0x1000），而 explorer 是 Medium（0x2000）；同一 exe 用管理员身份启动时进程为 High，
  不受该限制，所以出现了“管理员启动有图标、普通启动没有”的规律。
- 这不是界面代码或 UI 重构导致的；界面、日志、托盘创建调用本身都正常，问题只在完整性级别。

### 修复

- 构建/部署流程会把发行 exe 的完整性级别重置为 Medium（`icacls … /setintegritylevel Medium`），
  避免打包机/自动化环境把标签带到产物上；普通双击即可正常注册托盘图标，无需管理员权限。
- 托盘恢复标准生命周期：创建、显示、左键显示主窗口、右键菜单，不做额外处理。
- 移除了启动时“自动把图标标记为常驻”的行为——托盘图标显示在任务栏还是「其他系统托盘图标」里，
  完全由用户自己决定，程序不再修改这个选择。
- 升级方式：用新 zip 覆盖到原目录即可；`settings.ini` 不在包内，原设置保留。

### 验证

- 普通权限（非管理员）双击启动：进程完整性 Medium，任务栏出现「佳点监控」托盘图标。
- 把图标手动收进「其他系统托盘图标」后，重启程序不会再被拉回任务栏。
- 其余功能与 v0.9.5 一致。

### 许可证

- 本项目使用 PolyForm Noncommercial 1.0.0；第三方组件清单见 `THIRD-PARTY-NOTICES.md`。
- 数据源组件 LibreHardwareService 为外部安装（MPL-2.0），不随本程序分发。

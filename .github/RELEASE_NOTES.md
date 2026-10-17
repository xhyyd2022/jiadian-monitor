<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这四处：package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.6.0 · 新增外设电量显示 + 底部栏自适应布局

基于上游 v0.5.0，新增无线鼠标 / 键盘 / 耳机电量实时显示功能。

### 新增：外设电量显示

- **独立 mouse-battery.exe 服务**：通过 HID 直读接收器协议，支持 Akko 8K Receiver（compx）、Ninjutso Sora V2（ninjutso）等无线外设
- **设置页外设栏目**：可选择要显示的外设（鼠标 / 耳机 / 键盘），自定义显示名称，顺序 = 面板显示顺序
- **面板底部栏**：自动显示外设名称 + 电量百分比，随外设数量自适应居中布局
- **电量阈值颜色**：≤15% 红色警告，16-30% 黄色，正常绿色
- **名称长度限制**：单外设 ≤16 字符，双外设 ≤6 字符（防止标签遮挡）

### 新增：V1 设备隐藏配置

- `settings.ini` 中 `[device] v1_enable=0` 可隐藏佳点 V1（非 V1P）设备，默认关闭

### 新增：独立 GitHub Action

- `.github/workflows/build-mouse-battery.yml`：修改 mouse-battery/ 目录后自动编译并发版，无需重编主程序

### 升级提示

- 便携版替换 `jiaidian-monitor.exe`，并确保 `mouse-battery.exe` 在同目录
- 首次使用在「设置 → 外设」中勾选要显示的外设并保存
- 新增鼠标协议只需修改 `mouse-battery/` 下的 provider 模块并重编 mouse-battery.exe

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

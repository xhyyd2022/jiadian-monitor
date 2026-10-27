<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.8.3 · 界面不再显示本机绝对路径

这一版只修一处：外设页原来直接把协议库的绝对路径印在界面上（形如 `D:\某处\bin\peripherals`），
换台机器装到别的盘就是错的，也等于把某一台机器的位置写死进了界面。现在改成**相对标签**。

### 变更

- **界面上只显示相对标签**：`<程序目录>\peripherals`。程序装在哪个盘、哪个目录，显示都一样。
- **要打开目录就点按钮**：新增「打开目录」按钮，绝对路径只在这一步交给系统资源管理器，
  不再作为文本出现在任何界面上。
- **示例命令不再写死 exe 名**：兼容配置卡里的 `--peripherals-hid` 示例用后端返回的可执行文件名。
- **轮询间隔文案由后端给**：不再在前端写死「每 3 秒」。
- **协议库读取失败的报错只写文件名**：`读不到 index.json：...` / `index.json 解析失败：...`，
  原来的绝对路径会原样出现在界面上。
- **API 新增字段**：`GET /peripherals` 返回的 `dirLabel`（相对标签，界面显示用）与
  `exeName`（当前可执行文件名）；`dir` 仍然返回绝对路径，但仅供程序与排查使用。

### 不变

v0.8.2 的外设电量功能（JSON 协议库、侧栏「外设」页、`/peripherals` API、三条诊断命令）全部保留，
协议 JSON 无需改动。

### 实测

- 自检：`外设协议自检通过：回放 22 条 sample`；
- 两台实机设备读数正常：VGN S99 键盘 100%、雷柏鼠标 56%；
- 外设页 DOM 全文检索已无盘符（`hasDrive: false`），目录显示为 `<程序目录>\peripherals`；
- 页面无滚动裁切（`scrollHeight == clientHeight`）。

### 升级提示

- 便携版直接用新 zip 覆盖到原目录即可；`settings.ini` 不在包内，原有设置保留；
- `peripherals/` 里你改过的文件不会被覆盖（本地构建脚本只补新增/更新）。

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

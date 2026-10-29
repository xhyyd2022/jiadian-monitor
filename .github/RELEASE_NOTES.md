<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.8.7 · 外设诊断：命令行终于能看见结果

这一版不碰推屏主链路，只修「外设电量」这条新通道的可观测性——查不到设备的时候，
得能说清楚是没插、没收录，还是被别的软件攥着。

### 修复：`--peripherals-hid` 在 cmd / PowerShell 里敲完只有提示符

- 根因有两层：release 是 **GUI 子系统**，被 cmd 直接拉起时标准句柄是空的；补救时又用
  `FILE_GENERIC_WRITE` 去开 `CONOUT$`，控制台设备不支持里面的 `FILE_WRITE_ATTRIBUTES`，
  直接返回 `ERROR_INVALID_HANDLE(6)`。`println!` 写失败也不报错，屏幕上就什么都没有。
- 现在：`AttachConsole(父进程)` + `CreateFileW("CONOUT$", GENERIC_READ|GENERIC_WRITE)`，
  拿到句柄后**直接用 `WriteConsoleW` 写 UTF-16**（不依赖 Rust 的 stdout 缓存，也不受代码页影响）。
- 仍然保留文件兜底：同一份内容写进 `log/peripherals-hid.txt`、`log/peripherals-dump.json`、
  `log/peripherals-selftest.txt`。
- 急着用也可以直接重定向：`.\jiadian-monitor.exe --peripherals-hid > hid.txt`。

### 改进：打不开的 HID 端点也会列出来

- 以前枚举时用「读写」方式打开每个 HID 集合，失败就整条跳过——系统独占的鼠标/键盘集合永远不会出现在诊断里，
  用户只看到一张短表，不知道少的是谁。
- 现在：读写失败会降级成**0 权限查询**，照样列出 `VID:PID / UsagePage / Usage / 各报告长度 / 产品名`，
  并多一列「状态」写明原因（例如 `5 = 被独占`）。这类端点发不了 Feature/Output 报告，电量读不到，但能认出是谁。
- 本机实测：端点数从 15 条变成 24 条，多出来的 9 条正是系统独占的鼠标/键盘集合。

### 改进：界面会提示「未收录的 HID 设备」

- `index.json` 里查不到 `VID:PID` 的设备以前是**静默丢弃**，外设页只剩一句「没有识别到已收录的外设」，
  分不清是没插好、没收录，还是被厂商驱动占着。
- 现在这行提示会带上具体的 `VID:PID（产品名）`，照着往 `peripherals/index.json` 补一条即可。

### 新增：PID 命中、VID 没收录时自动试协议

- 以前匹配是「VID 和 PID 都得对上」，换壳/OEM 的同款鼠标（PID 相同、VID 不同）整台消失。
- 现在：PID 有登记但 VID 没登记时，程序把该 PID 下的候选档位**逐个打开真机试**——
  只读档位（Feature / Input）先试，要写 OUT 查询帧的档位后试，谁先读回合法电量就用谁。
- 结论按 `VID:PID` 缓存，不会每 3 秒重开设备；界面上会注明「PID 命中、VID 未收录，档位是自动试出来的」。
- 手动核对用新命令：`jiadian-monitor.exe --peripherals-probe`，逐个候选打印电量或失败原因，
  哪条标「← 可用」就把它对应的 VID 补进 `index.json`。

### 改名：jiaidian → jiadian

- 产品名一直拼错（佳点 = jiā diǎn），这一版统一改成 `jiadian`：exe、Cargo 包名、identifier
  （`com.jiadian.monitor`）、日志文件名、单实例互斥体、界面文案、脚本、工作流、便携包名。
- **自启项自动迁移**：启动时发现旧的 `HKCU\...\Run\jiaidian-monitor` 就按当前 exe 路径改写成
  `jiadian-monitor` 再删旧值，老用户不用手动关开。
- 注意：identifier 变了，WebView2 的界面缓存目录会重建（窗口尺寸/上次主题这类界面状态回到默认），
  设置本身存在 `settings.ini` 里，不受影响。

### 已知

- Ninjutso、黑爵（Ajazz）目前**没有收录**。`brands/pixart.json` 里的 0x83 公版方案已覆盖这两家的常见方案，
  缺的是 `index.json` 的绑定条目——拿 `--peripherals-hid` 的 `VID:PID` 补上即可，不用改协议。

### 升级提示

- 便携版直接用新 zip 覆盖到原目录即可；`settings.ini` 不在包内，原有设置保留。
- `peripherals/` 目录随包更新，自己加的条目请先备份。

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

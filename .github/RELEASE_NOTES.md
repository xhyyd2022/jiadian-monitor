<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.9.0 · 推屏改成会话模型（界面与后端状态对账）+ 设备信息错位修复

这一版主要动的是「可靠性」：推屏不再是一条谁都能开的链路，而是一个有编号、能取消、
能对账的会话；出问题时界面能说清楚卡在哪一步，而不是停在「要不要重新点一下」。
原先准备的 0.8.8 没有单独发版，内容已经并入本版。

### 修复：型号 UNKNOWN、分辨率 null、Flash 255 · 0 Mbit

- 现象：程序被强杀/崩溃过一次之后，再启动界面上设备信息全变成 `UNKNOWN` / `null`。
- 根因：设备会在上一次会话结束时残留一行状态（`LIVE,exit`），而旧代码「发一条命令、读下一帧」，
  于是回包整体错位一格 —— 日志实测：`JED=LIVE,exit`、`PONG=JED,684019,…`、`STA=PONG,4490`。
  错位一次就永远错位。
- 现在：按**前缀认领**回包（`JED,` / `PONG,` / `STA,` / `DSP,` / `ROT,` / `VSO,`），
  收到不认识的行就丢掉继续读，并把丢掉的那行记进日志。强杀后重启也能自愈。
- 新增诊断命令 `jiadian-monitor.exe --device-info`：不开界面读一次 JED / PONG / STA 原文，
  用来区分「设备回包问题」和「界面问题」。返回 JSON（含 `hw` / `width` / `height` / `frames`）。

### 修复：面板没数据 —— 8766 被系统保留段占用时现在会自动重试

- 现象：`本机 API 绑定 127.0.0.1:8766 失败：os error 10013`，面板 fetch 全挂。
- 根因：装了 Hyper-V / WSL 的机器上，系统会保留若干动态端口段
  （`netsh int ipv4 show excludedportrange protocol=tcp` 可见），8766 可能正好落在里面，
  绑定直接返回 `WSAEACCES`。
- 现在：绑定失败不再一走了之，每 5 秒重试一次，端口一空出来就自动恢复；日志会写明 10013 的含义
  和恢复办法（管理员 `net stop winnat && net start winnat`，或重启电脑）。

### 重构：推屏改为会话模型，界面只认后端状态

- 后端新增 `render_state` / `render_cancel`：`render_start` 先登记请求再进阻塞池（拿不到准入就直接失败，
  不排队干等），取消按 `requestId` 定向；停止一定先于重启，会话 id 只增不回绕。
- 链路代次贯穿每条 retire / 命令路径：上一轮遗留的线程不能关掉、也不能驱动新会话；
  租约先归还，之后再报 idle —— 顺序反了就会出现「界面以为停了、USB 还被占着」。
- GUI 预览帧带 `JDP1` 头 + 会话号；旧会话的帧、解码到一半的旧位图直接丢弃
  （CLI 与 `--shot` 截图仍是原始 PNG，不受影响）。
- 前端改为单飞轮询（1 秒一次，最多一个在途查询）；**状态没确认就不当作可以启动**，
  绝不在未知状态下抢 USB 句柄；点「停止」后以收到 idle 作为释放确认，不再凭超时就宣告停止。
- 设备被拔掉的清理挪到后端释放之后；扫描与设备读取都带意图/版本守卫，迟到的结果不会写回界面。
- 测试：12 个生命周期用例 + 13 个前端会话用例（`npm test`）。

### 修复：日志轮转不再写坏、不再把满文件越写越大

- 「查长度 → 关闭 → 改名 → 重开 → 写入」全在同一把锁里；rename 失败绝不再往超限文件里追加，
  重开失败也不会删掉归档。
- 单条记录上限 16 KiB，按 UTF-8 字符边界截断并标记「已截断」；换行做转义，
  防止设备回包里的文本伪造出后续日志行。
- 普通日志文件预留一条 panic 记录的空间：刚好写满时也不会丢掉第一次崩溃线索；
  写入失败关闭句柄并冷却 5 秒，不整行重发制造重复。
- panic 钩子走一条不碰全局锁的独立通道（panic 很可能正发生在持锁期间，走原锁会死锁）。
- 新增 14 个隔离回归测试。

### 修复：CDP / USB 的等待都有上限，失败帧强制整屏重发

- CDP：单次 HTTP 2 秒、命令 10 秒；WebSocket 握手、发送、响应循环共用同一个 deadline，
  持续收到事件也不能无限延长等待。超时报文带端口、目标与最近一次失败原因，不再是「未就绪」四个字。
- WinUSB：管道超时 `0ms`（等于禁用超时）直接拒绝打开；短写不算成功帧；传输长度不再被静默截断。
- 设备 ACK 不是 `ok`（`timeout` / `exit` / `err`）就丢弃差分基线，下一帧发整屏——
  避免「命令执行了但回包丢了」之后画面一直缺块。
- 显示状态回包按字段逐段解析：畸形值不再让后面的字段整体错位；非法旋转角在打开任何设备前就被拒绝。
- 新增 11 个渲染 / 3 个 WinUSB 单测。

### 界面

- 去掉「主题切换」卡片里多余的「重试状态查询」按钮：状态本来就有 1 秒自动轮询，按钮走的是同一条
  `render_state` 路径；它还被挤进 3 列网格的第二行，把标题折成两行。提示语同步收敛为「可继续停止」。

### 升级提示

- 便携版直接用新 zip 覆盖到原目录即可；`settings.ini` 不在包内，原有设置保留。
- 自 0.8.7 起 identifier 为 `com.jiadian.monitor`；用过更早 `jiaidian` 版本的用户，自启项会自动迁移。

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

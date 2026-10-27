<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.8.2 · 外设电量（JSON 驱动）+ 侧栏「外设」页 + 本地 API

这一版新增**外设电量**：鼠标 / 键盘 / 耳机的品牌、型号、电量显示在侧栏独立的「外设」页里。
关键点是**协议全部由 JSON 配置驱动** —— 软件启动后先读总索引把 `VID:PID` 映射到品牌，
再读该品牌的 JSON 决定「发什么查询帧、怎么认回包、电量在第几字节」。加一款新设备只需要改 JSON，
不用重新编译、不用发新版。

### 新增：外设协议 JSON 库（`peripherals/`）

随包发布的目录，用户可以直接编辑：

```
peripherals/
  index.json            总索引：PID → 品牌 + 协议档位（十六进制字符串）
  brands/<品牌>.json    每个品牌一个文件，写这个品牌的读法
  README.md             字段全集与示例
```

- **总索引先行**：`index.json` 的 `devices[]` 里一条 `{vid, pid, brand, protocol, model, category}`；
  `pid` 可以写 `"*"` 通配（精确条目优先）。软件先查这张表判断读到的是哪个品牌，再打开对应品牌文件。
- **按品牌分文件**：`brands/razer.json`、`brands/logitech.json`、`brands/compx.json` …… 一个品牌一个文件，
  互不干扰；同一个品牌下可以有多套「协议档位」（`protocols{}`），用 `match` 靠
  `usagePage / usage / 各报表长度 / productString` 区分同品牌的不同代固件。
- **首版收录 15 个品牌、35 条 PID**：雷蛇、罗技、雷柏、瓦尔基里、CompX/VGN/ATK、迈从、宏碁、飞智、
  赛睿、漫步者、PixArt 通用引擎、VAXEE、Endgame Gear、Nordic 通用引擎、通用 HID。
- **可表达的真实差异**（不是「只取第 N 字节」的玩具格式）：
  - `transport` 三态：`feature`（SetFeature + GetFeature）/ `output`（WriteFile + ReadFile）/
    `input`（不发指令，被动挑帧，雷柏那种接收器自己推遥测的用这个）；
  - `pre[]` 前置步骤：先发一条查询拿索引，再把结果写进后面帧的某个偏移（罗技 HID++ 的 Feature 索引就是这么用的）；
  - `fields[]` 独立字段：每个字段可以有自己的 `tx`/`rx`，所以「电量一条命令、充电状态另一条命令」
    （雷蛇）和「从回包 ASCII 里取型号名」（索尼）都不用特例代码；
  - `checksum`、`skip`（丢弃不匹配的帧继续等）、`attempts`/`intervalMs`/`timeoutMs` 重试策略。
- **安全表达式**：电量用表达式描述，如 `b[11] * 100 / 255`、`u16le(4) / 10`、
  `(b[6] & 0x0f) * 5`。支持 `b[i]`、`u16le/u16be/s16le/s8`、`+ - * / %`、`& | ^ ~ << >>`、
  比较、`min/max/round/abs/clamp/if/sum/xor/len`。**不是** eval 任意代码，越界读不到就判该条读数失败，
  不返回 0 冒充。
- **拿不到就不猜**：没有 `charging` 字段的协议，界面显示「未知」而不是默认「放电中」；
  读数超出 `min`/`max` 视为无效；连续 3 次读失败自动退避到 3 倍轮询间隔。

### 新增：侧栏「外设」页

侧栏从 4 项变 5 项，顺序为 **首页 / 性能 / 外设 / 素材 / 设置** —— 外设在「性能」下方、「设置」上方。

- 第一张卡按设备一行：状态点（绿=已读到 / 黄=退避缓存 / 红=读失败 / 灰=未识别）、
  品牌 + 型号、电量百分比（≥50 绿、≥20 黄、其余红、未知灰）；
- 右上角「刷新」立即重读 JSON 并马上轮询一轮；
- 第二张卡「兼容配置」直接给出四步改法，并显示本机 `peripherals/` 实际路径。

### 新增：本地 API

- `GET /peripherals` —— 返回 `{ok, dir, pollMs, devices[], errors[], updatedMs}`，
  每台设备含 `brand / model / name / category / battery / charging / online / stale / protocol`。
- `POST /peripherals/refresh` —— 重新读配置并立刻轮询一轮。
- `GET /` 接口索引同步补上这两条。

### 新增：三条诊断命令

```
jiaidian-monitor.exe --peripherals-hid        # 列本机 HID 端点（VID:PID / UsagePage / Usage / 报表长度 / 产品名）
jiaidian-monitor.exe --peripherals-dump       # 等同于 GET /peripherals，跑一轮后打印
jiaidian-monitor.exe --peripherals-selftest   # 用各品牌 JSON 里的 sample 回包回放自检
```

### 实测

- **VGN S99 键盘（`320F:5055`，64 字节 `FF1C/0092` 端点）已实机验证通过**：发
  `04 <seq> 00 1a` 查询帧，回包 `b[8]` 即电量、`b[9]==1` 表示充电中。自检命令输出
  `外设协议自检通过：回放 22 条 sample`。
- **雷柏鼠标（`24AE:1205`）本版已打通**：它的电量不走查询命令，而是接收器主动推 7 字节遥测帧
  `BB B0 51 <dpiX_lo> <dpiX_hi> <状态> <电量>`，用 `transport: "input"` 被动挑帧即可。
  帧格式取自官方上位机 `RapooDevice.dll` 中 `BaseDevice::RecvSlot` 的 `0xBB` 分支反汇编
  （`b[6]` 送进 `QString::number` 写入属性 `Electricity`，`b[5]` 写入 `BatStatus`）。
  实机读数 `bb b0 51 88 13 01 38` → 电量 `0x38 = 56%`、DPI 5000。
- **认得出品牌但端点对不上时不再整台设备消失**：这种情况设备仍会列出（品牌型号照常显示），
  电量显示「未支持」，`error` 写清已知端点和该去补哪份 JSON。这一版之前雷柏那台就是这样
  从列表里消失的 —— 索引里只有 19 字节的 `telemetry19`，而这台只有 7/10 字节端点。

### 变更：便携包内容

`jiaidian-monitor-v<版本>.zip` 现在含 **exe + `assets/` + `peripherals/` + `uninstall-lhws.bat`**。
`peripherals/` 里的文件是你自己的配置，升级解压不会覆盖你改过的同名文件（本地构建脚本只补新增/更新）。

### 升级提示

- 便携版直接用新 zip 覆盖到原目录即可；`settings.ini` 不在包内，原有设置保留；
- 外设页空的就先把 `peripherals/` 解出来（老版本升级容易漏），点「刷新」；
- 想加新设备：`--peripherals-hid` 拿到端点和 VID:PID → 改 `index.json` → 改/建
  `brands/<品牌>.json` → 用 `--peripherals-selftest` 验证。

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

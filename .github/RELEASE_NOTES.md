<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.8.4 · 外设电量上面板 + 界面样式全自控

这一版让**面板素材也能直接画鼠标/键盘电量**，并把「界面不许用浏览器默认样式」这条写成了
可机器检查的硬性要求（A15），同时补齐 app 自身的控件归一化。

### 新增：外设电量进入面板数据通道

- 面板注入脚本原来只拉 `/metrics`，现在**同时拉 `/peripherals`**：两条独立请求、任一失败互不牵连。
- 新增一套占位符（`data-slot` 写法与硬件指标完全一致）：

  | 占位符 | 含义 |
  |---|---|
  | `mouse1_pct` / `kbd1_pct` / `headset1_pct` … | 电量百分比；读不到时是字符串 `—` |
  | `mouse1_name` / `kbd1_name` … | 设备型号或品牌名 |
  | `mouse1_status` / `kbd1_status` … | `充电中` / `放电中` / `离线` / `未支持` / `未连接` / `—` |
  | `peri_count` | 认到的外设台数 |

- 类别前缀：`Mouse`→`mouse`、`Keyboard`→`kbd`、`Headset`/`Earbuds`→`headset`、`Gamepad`→`gamepad`、
  `Dongle`→`dongle`、其他→`peri`；同类内从 1 起编号。
- **绝不喂 0**：读不到就喂 `—`，设备拔掉后对应槽会被清回 `—`。

### 新增：三套「键鼠电量」内置测试素材

| 主题 | 明暗 | 排版 |
|---|---|---|
| `键鼠电量深色` | 深 | 卡片 + 大百分比 + 细条 |
| `键鼠电量浅色` | 浅 | 右对齐大数字 + 细条 |
| `键鼠电量终端` | 深 | 磷光终端 + 纯色块条 |

- 每套都有 `v1p`（320×170）与 `v2`（320×240）两个尺寸，**只画外设槽**，标题栏标注数据来源，
  用于验证第二条通道（`GET /peripherals`）的端到端下发，不占用硬件指标的位置。
- 三套同时作为技能库参考样板 `p14_peri_dark.html` / `p15_peri_light.html` / `p16_peri_terminal.html`，
  全部通过 `A1–A15` + `B1–B7` 闸门。

### 新增：A15「样式全自控」硬性检查

- 面板素材里出现原生控件（`button` / `input` / `select` / `textarea` / `progress` / `option` /
  `ul` / `ol` / `dl` / `h1`–`h6` / `a` / `table` / `fieldset` / `pre` / `hr`）时，
  **必须在 CSS 里显式接管外观**（`appearance` + `background` + `border` + `font-family` 等），
  否则判 FAIL：深色面板上一个没接管的 `<select>` 展开瞬间是纯白底，在 RGB565 上像闪光灯。
- 推荐直接用 `div` / `span` 自绘 —— 现有 18 套内置主题的原生控件数都是 0。

### 变更：app 自身界面的控件归一化

- 补 `button / input / select / textarea / option` 的 `font-family: inherit` 与 `box-sizing: border-box`。
- 补全局 `a` 样式（主题色 + 无下划线，hover 才显示下划线）—— 设置页「来源 / 许可」两个链接
  原来落在浏览器默认的蓝链上，与深色主题不搭。
- 补 `:focus-visible` 焦点环（2px 主题色 + 2px 偏移），键盘 Tab 时不再依赖系统默认焦点框。
- 补 `p` / `h1`–`h6` / `ul` / `ol` / `img` / `option` / `::selection` 的兜底样式。

### 不变

- 外设协议库（`peripherals/index.json` + `peripherals/brands/*.json`）**没有任何格式变化**，
  v0.8.2 起写好的协议文件照旧能用，不需要改。
- `/peripherals`、`/peripherals/refresh` 接口与三条诊断命令（`--peripherals-hid` /
  `--peripherals-dump` / `--peripherals-selftest`）保持原样。

### 实测

- 自检：`外设协议自检通过：回放 22 条 sample`；
- 实机两台：VGN S99 键盘 100%、雷柏鼠标 56%（`telemetry7` 通道）；
- 三套新素材 × 两尺寸共 6 个文件全部通过 `check_material.py`；
- 面板下发时 `mouse1_pct` / `kbd1_pct` / `mouse1_status` / `kbd1_status` 均按实际读数注入，
  未支持的设备显示 `未支持`。

### 升级提示

- 便携版直接用新 zip 覆盖到原目录即可；`settings.ini` 不在包内，原有设置保留；
- 新主题在 `assets/键鼠电量深色`、`assets/键鼠电量浅色`、`assets/键鼠电量终端`，
  需要的话在「素材」页导入或在 `assets/` 下直接放。

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

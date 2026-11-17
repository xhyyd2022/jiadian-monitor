# 外设协议库（TOML）

鼠标 / 键盘 / 耳机的电量读取规则放在这里，**不用重新编译**：改完文件，在界面「外设」页点一次「刷新」就会重新读取。

> 从 2026-11 起协议库由 JSON 改为 **TOML**：JSON 不支持注释，协议里大量“为什么这么发帧”的说明只能塞进字符串；TOML 可以直接写 `# 注释`，也更好手改。
> 旧版 JSON 不会被读取；覆盖安装后请运行程序目录里的 **`清除旧版协议.cmd`**（见文末迁移指南）。

## 目录结构

```
peripherals/
  index.toml              总索引：VID:PID → 品牌 / 协议档位 / 显示型号
  brands/
    <品牌>.toml           各品牌的收发帧、匹配条件、校验与电量表达式
  README.md               本文件
```

## 一、`index.toml`

```toml
schema = 1

[poll]
intervalMs = 30000          # 轮询间隔；最小 30 秒（电量变化很慢），没人看外设页时还会暂停

[brands.razer]
name = "雷蛇"
file = "brands/razer.toml"

[[devices]]
vid = "1532"
pid = "0084"
brand = "razer"
protocol = "razer19"
model = "Razer Viper"
category = "Mouse"
note = "可选：这一档为什么这么匹配"
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `schema` | 否 | 目前为 `1` |
| `poll.intervalMs` | 否 | 轮询间隔（毫秒，最小 30000）；每台设备错开 0–3 秒，打开读一次就关闭；被官方上位机占用时退避 60 秒 |
| `brands.<key>.name` | 是 | 品牌显示名 |
| `brands.<key>.file` | 是 | 相对本目录的路径，**必须以 `.toml` 结尾** |
| `devices[].vid/pid` | 是 | 4 位十六进制字符串（大小写均可） |
| `devices[].brand` | 是 | 必须是 `brands` 里存在的 key |
| `devices[].protocol` | 是 | 品牌文件 `protocols` 里的档位名 |
| `devices[].model` / `category` / `note` | 否 | 型号、分类、说明；不写则用协议里的信息 |

## 二、`brands/<品牌>.toml`

顶层：`brand`（与 index 的 key 一致）、`name`、`note`、`default`（默认档位名）、`protocols`。

```toml
brand = "logitech"
name = "罗技"
default = "hiddpp"

[protocols.hiddpp]
desc = "HID++ 4.x UnifiedBattery（Feature 0x1004）"
transport = "output"          # output / feature / input

[protocols.hiddpp.match]      # 端点匹配：UsagePage / Usage / 报告长度
usagePage = "FF00"
usage = "0002"
inputLength = 20
outputLength = 20

[[protocols.hiddpp.pre]]      # 发正式帧前的准备帧（可为空，可多条）
desc = "先问出 Feature 索引"
tx = { length = 20, reportId = "11", bytes = { "1" = "ff", "2" = "00", "3" = "18", "4" = "10", "5" = "04" } }
rx = { length = 20, match = [{ offset = 2, equals = "00" }], timeoutMs = 300, attempts = 2 }
take = { offset = 4 }
put = { frame = 0, offset = 2 }

[[protocols.hiddpp.tx]]
desc = "读电量"
length = 20
reportId = "11"
bytes = { "1" = "ff", "2" = "07", "3" = "08", "4" = "00", "5" = "00" }

[protocols.hiddpp.rx]
length = 20
match = [{ offset = 2, equals = "07" }]
timeoutMs = 400
attempts = 3
intervalMs = 40

[protocols.hiddpp.battery]
expr = "b[4]"
min = 1
max = 100
valid = "b[4] <= 100"
```

协议字段与旧版一致（`match`、`transport`、`pre`、`tx`、`rx`、`battery`、`extras` 等）；数组用 `[[protocols.<档位>.pre]]` / `[[...tx]]`，单个对象用 `{ ... }` 或独立 `[表]`。表达式里的 `b[n]`、`sum(...)` 等照旧。

## 三、TOML 速记（从 JSON 迁过来最容易踩的几处）

| JSON | TOML |
| --- | --- |
| `"key": value` | `key = value` |
| `"note": "a\nb"` | `note = "a\nb"`，或 `'''a<换行>b'''` |
| `[{...}, {...}]` | `[[table]]` 连续写两段 |
| `{"a": 1, "b": 2}` | 行内表 `{ a = 1, b = 2 }`，或独立 `[table]` |
| 注释 | `# 行注释`（JSON 不支持，这是改 TOML 的主要原因） |
| 十六进制字节 | 保持字符串：`"1f"`；数字键加引号：`"1" = "ff"` |

`bytes` / `match` 等里的键都是字符串；**纯数字键要加引号**，否则 TOML 会当成整数键。

## 四、迁移指南：JSON → TOML

程序升级后只认 `.toml`。覆盖安装时旧文件不会自动消失，按下面任一种方式迁移：

1. **推荐：运行 `清除旧版协议.cmd`**（与 exe 同级）
   - 删除 `peripherals\index.json` 与 `peripherals\brands\*.json`；
   - 新版包里的 `.toml` 保持不变，重启/刷新后生效。
2. **手动删除**：删掉 `peripherals\index.json` 和 `peripherals\brands\` 下所有 `.json`。
3. **保留自定义设备**：如果你在旧 `index.json` 里加过自己的 VID:PID，先把对应 `[[devices]]` 段抄到新的 `index.toml`，再删 JSON。

> 如果只删了 `brands/*.json` 却留下 `index.json`，界面会提示「检测到旧版 JSON 协议库」；删掉 `index.json` 即可。
> 如果 TOML 写错，界面会显示「index.toml 解析失败：…」并带出行号，按行修即可。

## 五、排查

- 识别不到设备：确认 `index.toml` 里有对应 VID:PID，且 `brand` 指向的品牌文件存在。
- 电量读不到但设备在：看界面状态「休眠中 / 被其他程序占用 / 协议未匹配」——
  - 被官方上位机独占时，程序会退避 12 秒再试，不会强行抢设备；
  - 命令行 `jiadian-monitor.exe --peripherals-hid` 可列出本机全部 HID 端点与打开错误。
- 仓库自检：`python peripherals/_check.py`（需要 Python 3.11+，用 `tomllib` 读取）。

## 六、历史备注

- 2026-11：VGN / ATK / CompX 按品牌拆分，`brands/vgn.toml`、`brands/atk.toml`、`brands/compx.toml` 分别维护；旧 `compx.toml` 里混放的 VGN 设备已迁出。
- 2026-11：协议库由 `*.json` 迁移为 `*.toml`；旧 JSON 不再读取。

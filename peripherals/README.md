# 外设电量协议库（可编辑 JSON）

本目录让**软件认识新的鼠标 / 键盘 / 耳机，而不需要改代码**：只改这里面的 JSON 就行。

```
peripherals/
  index.json          总索引：VID:PID → 属于哪个品牌、用哪套协议、显示什么型号
  brands/<品牌>.json   按品牌分文件：真正「怎么发指令、怎么读电量」的细节
  README.md           本文档
```

软件做的事：

1. 枚举本机 HID 端点，逐个拿到 `VID / PID / UsagePage / Usage / 输入输出长度 / 产品名`；
2. 拿 `VID:PID` 去 **`index.json`** 查——查到就说明这台设备归某个品牌，并拿到协议档位名；
3. 读 `brands/<品牌>.json` 里对应档位，照它发指令、解析回包，得到电量、充电状态、型号名。

> 索引与协议分家的原因：`index.json` 是**唯一**记录「这个 PID 是谁」的地方，改一行就能把设备挂到某个品牌下；
> 品牌文件放真正的报文细节，同品牌多台设备共用，不用重复抄。

改完 JSON **不用重新编译**：程序每次轮询都重新读盘（文件没变就用缓存），在界面「外设」页点刷新即可看到效果。
写得不对也不会崩：某个文件解析失败只在界面上显示一条错误提示。

---

## 一、`index.json`

```json
{
  "schema": 1,
  "poll": { "intervalMs": 3000 },
  "brands": { "razer": { "name": "雷蛇", "file": "brands/razer.json" } },
  "devices": [
    { "vid": "1532", "pid": "0555", "brand": "razer", "protocol": "standard90", "model": "旋风黑鲨 V2 Pro", "category": "Headset" }
  ]
}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `schema` | 是 | 固定 `1` |
| `poll.intervalMs` | 否 | 全局轮询间隔，默认 `3000` 毫秒。连续读不到时先按正常间隔重试，睡了超过 1 分钟才放慢到 10 秒 |
| `brands.<key>.name` | 是 | 品牌显示名，界面直接显示 |
| `brands.<key>.file` | 是 | 相对本目录的路径，通常 `brands/<key>.json` |
| `devices[].vid` | 是 | 十六进制字符串，不带 `0x`，如 `"1532"` |
| `devices[].pid` | 是 | 同上；写 `"*"` 表示该 VID 下所有 PID（精确匹配优先于通配） |
| `devices[].brand` | 是 | 必须是 `brands` 里存在的 key |
| `devices[].protocol` | 否 | 品牌文件里的档位名；不写用该文件的 `default` |
| `devices[].model` | 否 | 型号显示名；不写用设备自己的产品字符串 |
| `devices[].category` | 否 | `Mouse`/`Keyboard`/`Headset`/`Earbuds`/`Gamepad`/`Dongle`/`Other` |
| `devices[].note` | 否 | 备注，界面显示 |

十六进制统一用字符串，大小写随意，`0x` 前缀会被忽略。

## 二、`brands/<品牌>.json`

```json
{
  "brand": "razer",
  "name": "雷蛇",
  "note": "90 字节私有帧",
  "default": "standard90",
  "protocols": { "standard90": { ... } }
}
```

顶层：`brand`（与 index 的 key 一致）、`name`、`note`、`default`（默认档位名）、`protocols`。

### 2.1 协议档位字段总表

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `desc` | 否 | 人看的说明 |
| `match` | 否 | 选端点，见 2.2 |
| `transport` | 是 | `feature` ｜ `output` ｜ `input`，见 2.5 |
| `pre` | 否 | 前置步骤，见 2.6（HID++ 查 Feature 索引就靠它） |
| `tx` | 视 transport | 要发的帧：一个对象或数组（多帧依次发），见 2.3 |
| `rx` | 否 | 读回包与筛选，见 2.4 |
| `battery` | 是 | 电量，`{expr, round, min, max, valid}` |
| `fields` | 否 | 附加字段（充电、型号名等），见 2.7 |
| `sample` | 强烈建议 | 真实回包回放，见 2.8 |
| `note` | 否 | 坑说明，界面显示 |

### 2.2 `match`：这台设备该不该用这套协议

```json
"match": { "usagePage": "FF02", "usage": "0002", "inputLength": 17, "outputLength": 17, "featureLength": 0, "productLike": "Beast X" }
```

不写的项不限制。**同一个 VID 下有多套端点时必须写 `usagePage` / `inputLength` 隔离**——
`0x3554` 就同时存在 17 字节鼠标端点与 20 字节键盘端点，只按 VID 匹配会读到别人的包。

`featureLength: 0` 是个有用写法：表示「这台设备没有 Feature 端点」，只能走中断。

### 2.3 `tx`：要发出去的帧

```json
"tx": { "length": 91, "reportId": "00", "pad": "00", "bytes": { "2": "1f", "6": "07", "7": "80" }, "checksum": { "put": 89, "expr": "xor(3, 88)" }, "delayMs": 0 }
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `length` | 是 | 总长度，**含**第 0 字节的 `reportId`；其余填 `pad` |
| `reportId` | 否 | 填到第 0 字节，默认 `"00"` |
| `pad` | 否 | 填充字节，默认 `"00"` |
| `bytes` | 否 | `{ "偏移": "字节" }`，偏移是缓冲区下标 |
| `checksum` | 否 | `{ "put": 写入偏移, "expr": "表达式" }` |
| `delayMs` | 否 | 发完这一帧等多久再发下一帧，默认 `0` |

写 `"tx": [帧1, 帧2]` 就是多帧握手，按顺序发。
**注意**：`output` 传输下 `length` 必须与端点 `OutputReportByteLength` 一致，否则 `WriteFile` 直接报错。

### 2.4 `rx`：怎么读、怎么挑帧

| 字段 | 说明 |
| --- | --- |
| `length` | 期望读多少字节；不写用端点的报告长度 |
| `match` | 全部满足才算有效：`[{ "offset": 0, "equals": "02" }]` |
| `skip` | 命中就丢弃继续等（设备忙 / 等待帧）：`[{ "offset": 0, "equals": "01" }]` |
| `timeoutMs` | 单次等待上限，默认 `300` |
| `attempts` | 最多试几次，默认 `3` |
| `intervalMs` | 两次尝试间隔，默认 `40` |

`transport: "input"`（被动监听）时 `match` 就是在数据流里挑电量帧的特征，比如
`[{"offset":0,"equals":"07"}]` 选中 Report ID 为 7 的那一帧。

### 2.5 三种 transport

| transport | 实际动作 |
| --- | --- |
| `feature` | 有 `tx`：`HidD_SetFeature` 写、`HidD_GetFeature` 读；无 `tx`：只 `GetFeature` |
| `output` | 依次 `WriteFile` 写 `tx`，再带超时 `ReadFile` 读输入报告 |
| `input` | 不发任何东西，按 `rx.timeoutMs` 带超时 `ReadFile`，用 `match` 挑帧 |

### 2.6 `pre`：发主指令前要先做的事

数组，每项一步，能把某次回包里的字节**取出来写进后面帧的某个偏移**。典型用途：罗技 HID++ 要先问
「0x1004 这个 Feature 分配到了几号」，后续指令第 2 字节要填这个号。

```json
"pre": [
  {
    "desc": "查 0x1004 的 Feature 索引",
    "tx": { "length": 7, "reportId": "10", "bytes": { "1": "ff", "2": "00", "3": "00", "4": "00", "5": "00", "6": "5a" } },
    "rx": { "length": 7, "match": [{ "offset": 2, "equals": "00" }], "timeoutMs": 300 },
    "take": { "offset": 3 },
    "put": { "frame": 1, "offset": 2 }
  }
]
```

| 字段 | 说明 |
| --- | --- |
| `tx` / `rx` | 这一步自己的收发（`rx` 省略则用协议的 `rx`） |
| `take.offset` | 从回包哪个位置取值 |
| `put.frame` | 写进 `tx` 数组的第几帧（0 起） |
| `put.offset` | 写进该帧的哪个偏移 |

执行顺序：先跑完所有 `pre`，再依次发 `tx` 并读回包。

### 2.7 `fields`：电量以外的字段

数组。每项要么从**主回包**里取（不写 `tx`），要么**自己再发一条指令**（写 `tx`/`rx`，雷蛇查充电就是这么做的）。

```json
"fields": [
  { "name": "charging", "expr": "b[10] == 1",
    "tx": { "length": 91, "reportId": "00", "bytes": { "2": "1f", "6": "02", "7": "07", "8": "84" }, "checksum": { "put": 89, "expr": "xor(3, 88)" } } },
  { "name": "model", "ascii": { "offset": 4, "length": 12 } }
]
```

| 字段 | 说明 |
| --- | --- |
| `name` | 字段名。`charging` 有特殊含义（界面显示充电标记）；`model` 会覆盖型号名；其它名字原样出现在 API 里 |
| `expr` | 数值表达式，见 2.9 |
| `ascii` | `{ offset, length }`：取一段 ASCII 当字符串（型号名） |
| `round`/`min`/`max`/`valid` | 同 `battery` |
| `tx`/`rx` | 这一项自己的收发；给了就独立查一次 |
| `note` | 说明 |

### 2.8 `sample`：回放自检（强烈建议填）

```json
"sample": { "rx": "02 1f 00 ...", "expect": { "battery": 75, "charging": false, "model": "Beast X" } }
```

`rx` 是十六进制字节串（空格可省），`expect` 是**期望**解析结果。
`--peripherals-selftest` 会把这段字节喂给同一套解析逻辑，不符合就报出来。
手里没真机也能靠这一条把协议写对。

### 2.9 表达式（`battery.expr` / `fields[].expr` / `checksum.expr`）

一套很小的安全算式，**不执行任意代码**：

- 数字：`100`、`0x1F`、`2.5`
- 取字节：`b[6]`（第 6 个，0 起）
- 组合：`u16le(6)`＝`b[6] | b[7]<<8`、`u16be(6)`、`s16le(6)`、`s8(6)`
- 运算：`+ - * / %`、`& | ^ ~ << >>`、比较 `== != < <= > >=`、逻辑 `&& || !`、括号
- 函数：`min` `max` `round` `abs` `clamp(x,lo,hi)` `if(c,a,b)` `sum(from,to)` `xor(from,to)` `len()`

> 偏移必须落在实际读到的缓冲区范围内，越界会让该条读数解析失败（不会被当成 0）。

`battery` / `fields[]` 的数值项还支持：

| 字段 | 说明 |
| --- | --- |
| `round` | 默认 `true`，四舍五入成整数 |
| `min` / `max` | 合法区间，默认 `0` / `100`；**落在区间外这条读数判为无效**（避免把 0x00 当 0%） |
| `valid` | 额外有效性条件，如 `"b[2] != 0xff"` |

常用写法：

| 意图 | 表达式 |
| --- | --- |
| 原始值就是百分比 | `b[6]` |
| 255 刻度 | `b[9] * 100 / 255` |
| 每档 5% | `b[5] * 5` |
| 16 位小端再除 10 | `u16le(11) / 10` |
| 异或掩码 | `b[11] ^ 0xFF` |
| 低 4 位档位，`<=5` 算 20% 一档 | `if((b[6] & 0x0F) <= 5, (b[6] & 0x0F) * 20, (b[6] & 0x0F) * 10)` |
| `0x55` 互补校验 | `(0x55 - sum(1, 15)) & 0xFF` |
| 充电判断 | `b[7] == 1` / `(b[6] & 0x80) != 0` |

### 2.10 偏移指哪个字节

**一律指「实际读到的那块缓冲区」的下标**：`b[0]` 通常是 Report ID。
协议文档若写「从 Report ID 之后算」的偏移，要自己 `+1`。
`sample` 是唯一真相来源——写完协议一定填一条真实回包。

---

## 三、加一台新设备的完整流程

1. 拿 `VID:PID`（设备管理器 → 硬件 Id，`HID\VID_1532&PID_0555`），或跑 `--peripherals-hid` 直接看。
2. `index.json` 的 `devices` 加一行指向已有品牌；新品牌则在 `brands` 加一条并在 `brands/` 下建文件。
3. 品牌文件里写协议档位：先抄最接近的旧档位，再改 `match` / `tx` / `rx` / `battery`。
4. 填一条 `sample` 回放。
5. `jiadian-monitor.exe --peripherals-selftest` 看回放是否通过。
6. 打开界面「外设」页看真实读数。

## 四、调试命令

```
jiadian-monitor.exe --peripherals-hid        # 本机物理设备：VID:PID + 短指纹，一行一台（已按 VID:PID + 指纹去重）
jiadian-monitor.exe --peripherals-hid-all    # 逐 HID 集合展开，排查某个集合的 Usage/长度时用
jiadian-monitor.exe --peripherals-dump       # 当前识别到的外设（等同 GET /peripherals，含 state/名字字段）
jiadian-monitor.exe --peripherals-probe      # PID 命中但 VID 未收录时，逐个候选档位真机试一遍
jiadian-monitor.exe --peripherals-selftest   # 回放所有品牌的 sample
```

`--peripherals-hid` 默认**按物理设备聚合**：同一只鼠标的鼠标/键盘/厂商集合合成一行，
端点数列写有几个集合、`In/Out/Feat` 取各集合最大值、`Usage 集合`去重列出。
三个报告长度全是 0 的集合直接过滤掉（既不能读也不能写，只会淹没有用的行）。
两只同型号鼠标会显示成两行，靠「指纹」列区分。

`--peripherals-probe` 是给「同一颗芯片的公版方案」用的：`index.json` 里只登记了
`3554:F503`，你手里是换壳的 `1234:F503`——PID 认得、VID 没收录。它会把这个 PID 下
所有登记过的档位按「只读的先、要写 OUT 帧的后」逐个打开试，谁读回合法电量就标「← 可用」。
测出来之后把 VID 补进 `index.json` 即可，以后就不用再试。

运行时也走同一套逻辑：某个设备的 `VID:PID` 精确匹配不上，但 PID 有登记，程序会自动试一遍，
选中哪个档位写进界面备注（「PID 命中、VID 未收录，档位是自动试出来的」），并按设备缓存结论。

## 五、显示名（存在 settings.ini）与休眠

### 5.1 名字：两层，写在 settings.ini

程序只有一份真值：exe 同级的 `settings.ini`。改完存盘即生效，不用重新编译：

```ini
[peripherals.names]
; vid:pid = 同型号共用；vid:pid@短指纹 = 只给这一只
1532:0555=主用鼠标
1532:0555@3f2a=左手机
```

- 键：`vid:pid`（十六进制，大小写不敏感）+ 可选 `@短指纹`。
- 查名顺序：**实例名 → 型号名 → 默认（品牌 + 型号）**。
- 名字最长 24 个字符（按字符计，中文算 1 个），换行与控制字符会被去掉；
  空值 = 删掉这一条、回落默认名。
- 界面入口：外设页行尾的笔形按钮，或双击名称；Enter 保存、Esc 取消，点空白处也会保存。
  **同型号只有一台时写型号名**（换 USB 口也不丢）；有多台时才锁到实例。

### 5.2 指纹：怎么区分「同型号第二只」

分组键是 `VID:PID` + 短指纹（4 位十六进制），来源按优先级：

1. Windows `ContainerId` —— 同一只鼠标的多个 HID 集合共享它，两只同型号则不同；
2. HID 序列号（`iSerialNumber`，很多设备不报）；
3. HID 路径里的实例段（**换 USB 口会变**，仅前两者都拿不到时兜底）。

对外只暴露 4 位指纹，不打印完整序列号或设备路径。

### 5.3 休眠：有状态，不是「读不到」

| state | 判定 | 界面 | 电量 |
| --- | --- | --- | --- |
| `live` | 最近一次读成功 | 绿点 | 实时 |
| `retrying` | 连续失败 1–2 次 | 黄点 | 上一次读数（暗） |
| `sleeping` | **曾经读到过** + 连续 3 次失败 | 灰点 +「休眠中 · N 分钟前」 | 保留上一次读数 |
| `unavailable` | 从没读到过 | 红点 | — |
| `unsupported` | 认得品牌但档位不匹配 | 灰点 | 未支持 |

- 恢复：任何一次成功读数立刻回 `live`、清掉错误，日志记一条「已恢复（休眠 N 秒）」。
- **断开就消失**：接收器没了 / 设备拔掉，枚举里找不到就立刻从列表移除，不留灰行；
  名字仍在 `settings.ini` 里，插回同一只还是原来的名字。
- 探测节奏：睡着后头 1 分钟按正常间隔（默认 3 秒）探测，随手动一下鼠标就能发现；
  之后放慢到 10 秒一次，省 CPU 但不至于错过唤醒。

### 5.4 API 字段

`GET /peripherals` 的 `devices[]` 新增：

| 字段 | 含义 |
| --- | --- |
| `name` | **生效名**：编辑过就是编辑后的 |
| `defaultName` | 默认名（品牌 + 型号） |
| `customName` | 用户设的名字；`null` = 没改过 |
| `instance` | 4 位实例短指纹 |
| `state` | `live` / `retrying` / `sleeping` / `unavailable` / `unsupported` |
| `lastSeenMs` | 最近一次真正读到的毫秒时间戳（`0` = 从未） |
| `asleepSinceMs` | 判定休眠的时刻；`null` = 没在睡 |

列表顺序：键盘 → 鼠标 → 耳机 → 其他，同类别按显示名。

## 六、已知取舍

- **认得品牌但端点对不上时，设备仍然列出**：`index.json` 里登记了这个 `VID:PID`，但品牌文件里
  没有任何档位的 `match` 配得上它的端点，这时设备照样出现在「外设」页里，品牌型号照常显示，
  电量显示「未支持」，`error` 会列出该设备的全部端点、并指出该去补哪份文件。
  `GET /peripherals` 里对应字段是 `"supported": false`。
  （这条是踩过坑加的：以前这种情况整台设备会从列表里消失，看起来像程序没认出设备。）
- **拿不到就不猜**：固件不回包的（如漫步者 Halo Space 的充电状态），`fields` 里就不要写 `charging`。
  界面显示「未知」，而不是编一个值。
- **同 VID 多端点必须靠 `match` 隔离**，理由见 2.2。
- **轮询要轻**：连续 3 次读不到判定休眠，头 1 分钟仍按正常间隔探测，之后放慢到 10 秒；设备一恢复立刻回到正常节奏。
- 不要在 `bytes` / `put.offset` 里写超过 `length` 的偏移，会被忽略并记一条警告。
- 电量公式是近似的（电压曲线除外）时，在 `note` 里写清楚，别让人以为精确。

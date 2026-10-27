════════════════ 【任务】 ════════════════
你是前端工程师。请生成【一个单文件 HTML】，在一块嵌入式 RGB565 屏上显示一块动态面板。
设备联网，可引用外部 CDN / 图片 / 字体；页面逻辑全写在这一个文件里。

▷ 先思考、**多反问**（很重要）
  宁可多问一轮，也不要猜着做。开工前把下面这些一次问清（用一组选择题，别一句句问）：
   · **要监控哪些传感器？**分辨率？（没给就必须问；传感器**从下面【标准占位符】里挑**，别自造名字）
   · 风格偏好？（深色 / 浅色 / 某种具体风格；没给就问，或给 2–3 个候选让我选）
   · **背景底图走哪种？**（三选一，必须问，见下）
   · 动效偏好？（完全静态 / 轻微呼吸感 / 有扫描线之类）
   · 有没有品牌色或必须避开的颜色？
  信息已足够时，用一句话复述方案，然后直接输出完整代码。
▷ 背景底图：只有三种，**必须先问用户选哪种**，不要自己默认
  ⓐ **纯 CSS**（默认推荐）：用 `linear-gradient` / `radial-gradient` / `repeating-linear-gradient`
     画底纹 —— 不引任何文件，最省事、最可控、最不容易出错。
  ⓑ **本地文件**：图片与 HTML **放在同一个目录**（放进 app 时就是同一个主题文件夹
     `assets/主题名/` 里，仍然同一层，**不要再建子目录**），代码里用**文件名相对引用**，
     例如 `url("bg.png")`。
     ⚠️ 只写**文件名**，不要写盘符、不要写绝对路径、不要写 `..` 往上层跑。
  ⓒ **在线 URL**：直接用完整 URL，例如 `url("https://example.com/bg.png")`（设备联网时可用）。
  选定后**在交付说明里写明用了哪一种**；如果选了 ⓑ 但没有对应文件，先问我，不要自己造路径。
▷ 工作流（四步；第 ④ 步是强制闸门）
  ① 复述理解 → ② **问清上面几件事** → ③ 写代码 → ④ 过「第三部分 · 交付闸门」
  闸门不过就**就地改代码**，改完**整份清单重跑**（不是只重跑失败那一项）。
▷ 交付物
  一个 .html，**其中 `</body>` 前必须带 1.1 那块产物清单块**（下游机器靠它抓取）；
  前面允许**一行**自检结论（例：「自检：契约 4/4，闸门 1–4 全过，0 WARN」）；不要多余解释。
▷ 数据来源（**你只管画，不管取数**）
  上位机已把**本机硬件指标**（`/metrics`）与**外设电量**（`/peripherals`）都算好，经**本地 API**
  在渲染时**自动注入**喂进页面。你只需：实现 `__setSlot`（见 1.3）+ `data-slot` 用【标准占位符】**真名**。
▷ 实现自由度
  「第一部分 契约」不可违反；本文已写明的按本文来；没规定的部分自由发挥，
  以**更精致 / 更清晰 / 更流畅**为准。

════════════════ 【标准占位符（data-slot 只能用这些，别自造）】 ════════════════
数据由上位机经**本地 API** 喂进来（`/metrics` 硬件指标 + `/peripherals` 外设电量）：渲染面板时上位机会
自动注入一段"自取脚本"，每秒对每条 `{key,value}` 调 `__setSlot(key, …)` —— **你不用写 fetch**。
槽名**必须**从下表选（别用 v1 / s1 这类自造名）。
多实例（可能有多个）**从 1 起编号**：gpu1_* / disk1_* / net1_* / fan1；单实例不编号。
**面板只引用你确实要显示的那几个编号**（如只显示第一个磁盘就用 disk1_*）。
**网卡**所有物理网卡都会出（断开的也在，速率为 0）；**风扇**按通道号稳定编号。
  CPU    cpu_name(型号)  cpu_load(%)   cpu_temp(℃)    cpu_power(W)   cpu_fan(RPM)
  GPU    gpu1_name(型号) gpu1_load(%)  gpu1_mem_pct(%) gpu1_mem_used(MB) gpu1_mem_total(MB) gpu1_temp(℃) gpu1_fan(RPM)
  内存   mem_pct(%)    mem_used(GB)   mem_total(GB)
  磁盘   disk1_pct(%)  disk1_used(GB) disk1_total(GB)    disk1_temp(℃)  ＋ 合计 disk_all_pct/used/total
  网络   net1_down(KB/s) net1_up(KB/s) net1_total_down(GB) net1_total_up(GB)
  风扇   fan1(RPM)      系统  sys_uptime(s)
  外设   mouse1_pct(%)  mouse1_name(型号)  mouse1_status(文本)  ＋ kbd1_* / headset1_* / peri_count
▷ **外设电量（鼠标 / 键盘 / 耳机）是第二条数据通道**：来自 `GET http://127.0.0.1:8766/peripherals`，
  注入脚本**同样会自动喂进 `__setSlot`**（你仍然不用写 fetch）。规则：
  · 槽名 = **类别前缀 + 同类别内编号（从 1 起）+ 指标**。前缀固定为
    `mouse`(鼠标) / `kbd`(键盘) / `headset`(耳机/耳塞) / `gamepad`(手柄) / `dongle`(接收器) / `peri`(其他)。
    例：第一只鼠标 `mouse1_pct`、`mouse1_name`、`mouse1_status`；第一把键盘 `kbd1_pct`、`kbd1_name`、`kbd1_status`。
  · `_pct` 的值**可能是字符串 `—`**（没认到 / 没协议 / 设备省电离线）。**必须**照 `cpu_load` 的
    同一条约定处理：非数字就原样显示 `—`，**绝不能 `parseFloat` 后当 0** —— 0% 是「没电了」，
    「—」是「不知道」，两者在电池语义下差得极远。
  · `_status` 是**文本槽**（`充电中` / `放电中` / `离线` / `未支持` / `未连接` / `—`），
    **不要**配 `data-fmt` / `data-warn` / `data-danger` / `data-suffix`；要画充电图标就按文本判等
    （`v === '充电中'`），**不要**从 `_pct` 数值反推充电状态。
  · `_name` 是**文本槽**（型号名，长度不定，可能 `VGN S99` 也可能 `Rapoo Gaming Device`）：
    给足宽度或做省略号，别假设长度。
  · **编号会变**：同一类别内按快照顺序编号，插拔设备会让后面的编号前移。只画一只鼠标就用 `mouse1_*`，
    不要为「稳定 ID」设计（没有这种东西）。
  · `peri_count` 是认到的外设台数（整数）。想画「没有设备时显示占位」可以拿它做条件。
  · 目标设备电池图标 / 环形电量 / 双设备对比这类画面，**只用上面这几个槽**就能画完。
▷ **外设电量是怎么来的（画之前先知道边界，才不会设计出拿不到的画面）**：
  上位机每 3 秒扫一遍本机 HID 端点（鼠标 / 键盘 / 耳机 / 接收器），拿 `VID:PID` 去查
  `peripherals/index.json`（**PID → 品牌的索引**），命中了品牌再读 `peripherals/brands/<品牌>.json`
  里该型号的**协议档位**，按 JSON 描述的收发帧去问一次电量，最后汇总成 `GET /peripherals`。
  也就是说 **数据来源是「配置文件 + 设备协议」，不是系统电量 API** —— 由此产生四条必须接受的事实：
  · **不是所有设备都能读到**：品牌没收录、或收录了但没有匹配该端点长度的协议档位 → `supported=false`，
    `_pct` 给 `—`、`_status` 给 `未支持`。画面上要能容下"未支持"这个状态，别只画百分比。
  · **设备会睡**：无线鼠标静置一段时间就不回帧，这时是 `_status=离线`（有时还带上一轮读数）。
    所以 `_pct` 与 `_status` **要成对显示**，只画一个大数字会让人以为那是实时值。
  · **刷新是 3 秒一轮，不是每帧**：别做"电流表指针抖动"这类高频动效，它没有新数据可跟。
  · **一台设备可能同时占多条槽**（接收器 + 鼠标），编号按快照顺序，别假设 `mouse1` 一定是你那只。
  · 想直接看效果：内置主题里的 `键鼠电量深色` / `键鼠电量浅色` / `键鼠电量终端` 三套就是
  **外设电量专项测试素材**（标题栏都写了 test-only），只用 `mouse1_*` / `kbd1_*` / `peri_count` 画完，
  骨架可以直接抄。
  · 自己做联调时不必等真设备：`__setSlot` 的默认值可以先用 `?mouse1_pct=56&mouse1_status=充电中`
    这类 URL 参数喂（附录脚本末尾就是干这个的），确认排版后再接真数据。
  · 想确认本机到底有哪些设备被认出来：上位机 exe 有 `--peripherals-dump`（打印 `/peripherals` 的
    同一份 JSON）与 `--peripherals-hid`（列出本机全部 HID 端点，含 VID:PID/UsagePage/长度/产品名）。
▷ `cpu_name` / `gpu1_name` 是**文本槽**（精简型号，如 `9700X` / `5070Ti`）：直接当文字用，
  **不要**配 `data-fmt` / `data-warn` / `data-suffix`。名称（型号 / 网卡连接名 / 风扇通道名）
  都由上位机从数据源读，你不用管。
▷ 括号里是**规范单位**；要别的单位就在元素上写 `data-unit`（可选的见下），上位机会换算后再喂进来：
    容量  B / KB / MB / GB / TB      速率  B/s / KB/s / MB/s / GB/s
    温度  ℃ / ℉                      功耗  W
    时长  秒 / 分 / 小时 / 天          （开机时长 sys_uptime；`s / min / h / d` 同义）
  `%` / `RPM` 原样透传，写不写都一样。
▷ 预览界面可**一键复制**占位符名（直接粘贴，别手打，拼错会静默失效）。
▷ 只有**文本槽**名取自上表；**进度条（bar）**的 data-slot 可自取名，用 data-from 指向它跟随的文本槽。

════════════════ 【本提示词的结构】 ════════════════
先看目录，再按段读。每段开头都有一行「说明」讲它管什么、有几条约束。

 第一部分 · 契约（3 小节，**硬性**，违反即返工）
   1.1 文件与尺寸    ▸ 交付形态：单文件 + {W}×{H} 贴左上 + **产物清单块**；工具靠它推断视口
   1.2 文本槽        ▸ 会变的文字怎么标：{{名字}} + data-slot + 格式化 / 阈值属性
   1.3 进度条与注入  ▸ 条怎么跟数据走 + __setSlot 的职责与参考实现
 第二部分 · 实现（3 小节，**自由发挥区**）
   2.1 结构分层      ▸ 建议：三层组织，便于维护
   2.2 视觉与可读性  ▸ 必须满足下限：对齐 / 不重叠 / 字号 / 对比度
   2.3 性能          ▸ 必须满足：唯一杠杆是每帧变化的像素面积
 第三部分 · 交付闸门（**前置条件**，不是事后补救）
   3.1 四道闸门      ▸ 完整性 / 格式规格 / 运行时行为 / 内容与主题（A# B# = 机器编号）
   3.2 判定与后续    ▸ PASS / WARN / FAIL 三态各怎么办 + 自动核对命令
   3.3 高频坑速查    ▸ 6 个真实踩过的坑
 附录 · __setSlot 参考脚本（**可整段删除**，见 1.3）
 末尾 · 交付前速查（硬性约束一页纸）+ 我的画面需求

──────── 第一部分 · 契约（硬性，违反即返工）────────

■ 1.1 文件与尺寸
  说明：交付形态，4 条硬性约束 —— 工具靠它们推断视口与画布，下游机器靠清单块抓取。
  □ 单文件：一个 .html，CSS / JS 都在里面；可引外部 CDN / 字体；
    不得残留本机路径（file:///C:/Users/... 这类自用路径必须去掉）。
  □ 背景底图三选一（**必须先问用户选哪种**，见【任务】段）：
     ⓐ 纯 CSS 渐变 / 图案 —— 不引文件，推荐；
     ⓑ 本地文件 —— 图片与 HTML 同一目录（放进 app 就是同一个主题文件夹 assets/主题名/），
        只写文件名相对路径，如 url("bg.png")；不许盘符、不许绝对路径、不许 `..` 往上层跑；
     ⓒ 在线 URL —— 直接写完整 URL。
  □ 尺寸：样式表**最前面**写一条**同时声明宽高**的规则块：
       html, body { margin:0; padding:0; width:{W}px; height:{H}px; overflow:hidden; display:block; }
    从左上角 (0,0) 铺满整屏，**绝对禁止居中**：不许 place-items:center / margin:auto / 100vw / 100vh。
    坐标一律**整数像素**（12.5px 在 RGB565 上会抖）。
  □ **产物清单块**（下游机器抓取的唯一入口，**必填**）：放在 `</body>` **之前**、
    **`#stage` 之外**，是一块**惰性 JSON**：
       <script type="application/json" id="material-manifest">
       { "schema": "rgb565-panel-material/1",
         "name": "<文件名去掉 .html>",
         "size": [{W}, {H}],
         "background": "css",                     /* css | local | url，与上面选的底图方式一致 */
          "slots": [
            { "name": "cpu_load", "kind": "text", "warn": 75, "danger": 90, "fmt": 0, "unit": "%" },
            { "name": "cpu_bar",  "kind": "bar",  "from": "cpu_load", "max": 100 }
          ] }
       </script>
    规则：**素材里每一个 data-slot 都要列一条**（含只当标签用的槽）；
    kind / from / max / warn / danger / fmt / unit 必须与该元素上的 data-* **完全一致**
    （unit 取 data-suffix 去掉首尾空格；group 写 `"group": true`）。
    ⚠ `type` 必须 `application/json` —— 写成 text/javascript 会被当 JS 执行而报错（闸门 2.10 会判）。
    ⚠ 放进 `#stage` 里是**语义错误**（它是数据不是画面），`#stage *` 这类选择器会把它一并选中；
      放在 `#stage` **之外**、`</body>` 之前。
    ⤵ 别手抄：`python check_material.py 你的.html --emit-manifest` 会打印派生好的清单块，
      或 `python add_manifest.py 你的.html` 直接就地回填（幂等）。清单漏槽 / 字段不符 → 闸门 2.10 判 FAIL。

■ 1.2 文本槽（会变的文字怎么标）
  说明：页面与上位机之间**唯一**的数据通道，全部硬性。
  静态文字（标题 / 单位 / 说明）**直接写死，不加 data-slot**；会变的文字**一律**用
  {{名字}} 占位符 + **同名** data-slot，**名字必须取自【标准占位符】**（如 cpu_load / gpu1_temp）：
       <span data-slot="cpu_load" data-fmt="0" data-suffix="%" data-warn="75" data-danger="90">{{cpu_load}}</span>
  □ data-fmt = 小数位；data-group="1" = 千分位；data-suffix = 单位
    （用了 data-suffix 就**不要再放静态单位元素**，否则出现「63%%」）。
  □ **data-unit = 「我要什么单位」的标识符**：上位机按它换算后再喂值，**四族任选**（功耗固定 W，无可换算）：
      容量  B / KB / MB / GB / TB        速率  B/s / KB/s / MB/s / GB/s
      温度  ℃ / ℉                        功耗  W
      时长  秒 / 分 / 小时 / 天（开机时长用；`s / min / h / d` 同义）
    跨族不换算 —— 给占用率写 GB 会被闸门拦下（A14 会告诉你"这个槽能用哪些"）。
    不写就按规范单位（内存·磁盘=GB、速率=KB/s、温度=℃、风扇=RPM、开机时长=秒）。
    上位机设置页的「单位设置」默认**全自动**（就按你写的 data-unit）；用户也可切**手动指定**，
    按四族统一覆盖（功耗固定 W，不在其中）—— 那是用户侧的事，你只管把 data-unit 写对，但**别把单位写死在别的元素上**。
  □ data-warn / data-danger = 阈值，方向按语义（占用率类"越大越危险"），且 **danger ≥ warn**；
    写反了告警色永不出现，看着像功能坏了。
  □ 上位机发的是**原始数值字符串**，格式化由页面做。
  ⚠ 占位符与 data-slot **拼错一个字母**（{{cpu_load}} ↔ data-slot="mem_pct"）不报任何错，只会**永远不更新**。

■ 1.3 进度条与注入入口
  说明：条怎么跟数据走 + 上位机调用页面的唯一入口，2 条硬性约束。
  □ 进度条：data-from 指向某个文本槽，推该槽时条一起动；必备 4 个属性：
       <div data-slot="cpu_bar" data-kind="bar" data-from="cpu_load" data-max="100"
            data-box="18,88,128,6" style="left:18px;top:88px;width:0;"></div>
    data-box = 条的可用区域 "x,y,w,h"，必须与元素**实际 left/top/width 对得上**、且不越界。
  □ 注入入口 window.__setSlot(name, value) —— 职责 5 条，缺一不可：
     ① 更新**所有** [data-slot=name] 的元素（条走条逻辑、文本走文本逻辑）
     ② 文本按 data-fmt / group / suffix 格式化，非数字原样输出
     ③ 命中阈值切 .warn / .danger；条从 data-from 的源槽继承阈值
     ④ 条宽 = clamp(值 / 上限, 0, 1) × data-box 宽度，取整像素
      ⑤ 先给默认值（不注入也有画面），再用 URL 参数覆盖同名槽，顺序不能反
     ⤵ 真值不用你取：上位机渲染时**注入自取脚本**，每秒 fetch 本地 API 并调 __setSlot。
       你只管把 ①–⑤ 实现对 + data-slot 用真名。检查闸门用 file:// 打开（不注入）→ 走默认值。
     ⤵ 参考实现见文末【附录】。**上下文紧张时可整段删掉附录** —— 上面 5 条已够你自己
       实现，闸门 1.4 / 3.1（机器编号 B1/B2）会校验实现对不对。

──────── 第二部分 · 实现（自由发挥区）────────

■ 2.1 结构分层（建议）
  说明：只影响可维护性，不影响能否跑通。建议三层组织（z-index 递增）：
    #img    静态底图（CSS 渐变或外部图片）
    #render 与数据无关的图形（静态边框 / 网格 + 动画元素）
    #text   文字与数值（所有动态文本槽都放这里）
  □ 会动的元素各自一个 div，**必须** position:absolute + left/top —— 漏写会全堆到左上角。
  □ 动画用纯 CSS @keyframes + infinite，或 JS 定时器 / requestAnimationFrame 都行。

■ 2.2 视觉与可读性（必须满足下限，好看程度自选）
  说明：分"对齐"与"可读"两组；每条后面是**判据**，过不了闸门（3.1）就返工。
  · 数值右对齐     同类数值用 right: 锚定**右边缘**，别用固定 left
                   （否则 8% ↔ 100% 位数一变就参差，看着像"没对齐"）
  · 单位紧跟数值   与数值右边缘间距固定；能合成一个串就用 data-suffix
  · 同行基线对齐   同一行的标签 / 数值 / 单位，top 落在同一条线上
  · 不重叠         文字×文字、文字×装饰条都不许压；**10px 字两行 top 至少差 11px**
  · 字号下限       正文 ≥11px、次要 ≥10px、**中文一律 ≥11px**（汉字笔画密，10px 会粘连）
  · 对比度 ≥6:1    文字色 vs **它底下真实的底色**（不是"深底/浅底"二分）；硬线 4.5:1
                   ⚠ **中灰底最坑**：同一个 #3a4a5a 在白底绰绰有余，在中灰底
                     （如 #aeb8c2 / #b9b3a8）只剩 4.5:1 —— 次级文字必须压得更深
  · 不压装饰       文字别落在斜纹 / 网格 / 条纹带上；**从图案上挪开时记得同步改颜色**
  · tabular-nums   数字等宽，避免跳动时宽度抖动
  · 配色与层级     主色 1 + 中性灰阶 + 告警黄/红各 1；标题 / 主数值 / 次要指标 / 状态色
                   一眼能读懂，别把主次做成一样大。

■ 2.3 性能（必须满足）
  说明：唯一性能杠杆 = **每一帧变化的像素面积**，目标 ≤ 画布的 10%。
  □ 动效只做"小范围 + 低频"：指针 / 光点 / 扫描线 / 进度条 / 数字 / 小图标。
  □ 禁止整屏持续动画：全屏渐变流动、整屏色相旋转、大面积滚动、整屏转场。
  □ 低饱和配色、层次分明；避免 1px 的浅色细线（屏是 RGB565，细线会丢）。
  注：帧率只求**尽量贴近 30**，不锁上限也不设下限 —— 页面不变就不产帧，那是**省带宽**
      而不是缺陷（不要为了凑帧率强加持续动画）。

■ 2.4 样式自控（必须满足；机器编号 A15）
  说明：**屏幕上出现的每一个像素都要由你的 CSS 决定**。浏览器给原生控件的默认外观
        （深色底上刺眼的白底 select 下拉、Windows 灰按钮、蓝紫色下划线链接）不算数。
  □ **首选：别用原生控件**。仪表盘上的按钮 / 进度条 / 开关 / 标签页，用 `div` + `span` 自绘
     （现有 18 套内置主题全部是这么做的：0 个 `button` / `input` / `select` / `a` / `table`）。
     其中 `键鼠电量深色` / `键鼠电量浅色` / `键鼠电量终端` 三套是**外设电量专项测试素材**
     （只画 mouse1_* / kbd1_* / peri_count），要照着改就直接抄它们的骨架。
  □ 如果确实要用原生标签（`button` `input` `select` `textarea` `progress` `option`
     `ul` `ol` `dl` `h1`–`h6` `a` `table` `fieldset` `pre` `hr`），**必须在样式表里显式接管**：
      · `button` / `input` / `select` / `textarea`：`appearance:none`（或 `-webkit-appearance:none`）
        + `background` + `border` + `font-family`（**不要把字体留给 UA**，与 2.2 的字体一致性同一件事）
      · `option`：`background` + `color`（下拉列表由系统画，深色主题最容易被这一条打穿）
      · `ul` / `ol`：`list-style` + `margin` + `padding`；`dl`：`margin` + `padding`
      · `h1`–`h6`：`margin` + `font-size` + `font-weight`（否则字号与层级全按 UA 来）
      · `a`：`color` + `text-decoration`（**绝对不要留默认蓝链**）
      · `table`：`border-collapse`（或 `margin`）+ `padding`（或 `border-spacing`）
      · `fieldset`：`margin` + `padding` + `border`；`pre`：`margin` + `padding` + `font-family`
      · `hr`：`margin` + `border`
  □ 判据很硬：**只要标签出现了、样式里没接管，A15 直接 FAIL**（不是 WARN）。
  □ `div` / `span` / `i` / `b` / `canvas` / `svg` 这些是"骨架"，不参与 A15，照常随意用。
  注：这一条的来源是真实事故 —— 深色面板里放一个没接管的 `<select>`，展开瞬间是纯白，
       在 RGB565 屏上像闪光灯；蓝色下划线链接在霓虹主题里更是彻底出戏。

──────── 第三部分 · 交付闸门（前置条件，不是事后补救）────────

■ 3.1 四道闸门
  说明：交付前逐条过。**任一项 FAIL → 就地改 → 整份清单重跑**，全过才交付。
       括号里 A#/B# 是**下游机器自检的编号**，可一一对应。闸门 1–3 机器可判，闸门 4 只能人眼。

  【闸门 1 · 完整性】文件能不能独立跑起来
   1.1 单文件、无本机路径残留
   1.2 尺寸声明（A1/A2）：同时声明 width+height 的规则块在样式表最前面
   1.3 占位符闭环（A4）：每个 {{名字}} 都有同名 data-slot，反之亦然
   1.4 注入入口（B1）：__setSlot 存在；不注入时页面也有完整默认画面

  【闸门 2 · 格式与规格】契约字段写对了吗（判据见 1.2 / 1.3）
   2.1 命名唯一（A8）       同一 data-slot 不重复；避免重复 id
   2.2 引用有效（A5）       data-from 指向的槽必须存在；__setSlot 默认值也要有对应槽
   2.3 条必备字段（A6）     data-kind="bar" + data-from + data-max + data-box，四个齐全
   2.4 data-box 自洽（A7）  4 个整数、矩形在画布内、且与元素实际 left/top/width 对得上
   2.5 阈值语义             warn / danger 方向正确且 danger ≥ warn
   2.6 单位不重复           用了 data-suffix 就别再放静态单位元素
   2.7 定位与坐标（A9/A10） absolute 必须有 left/top；坐标一律整数
   2.8 相对定位陷阱（A11）  bottom / right 相对**最近的非 static 祖先**算 —— 把 bottom:0 的
                            元素放进 #render（整块画布高）而不是自己的小容器，就会盖住下方内容
   2.9 不越界（A11）        任何元素的内容包围盒都不超出 {W}×{H}（长文本 + 偏大的 left 最常超）
   2.10 产物清单块（A13）    </body> 前有 <script type="application/json" id="material-manifest">；
                            每个 data-slot 都列到、字段与素材实际一致、background 与实际相符
   2.11 样式自控（A15）      页面里出现的每个原生控件（button / input / select / textarea /
                            progress / option / ul / ol / h1–h6 / a / table / fieldset / pre / hr）
                            **都必须在样式表里显式接管外观**，不许吃浏览器默认样式

  【闸门 3 · 运行时行为】真渲染一遍是不是"活的"（判据见 2.2 / 2.3）
   3.1 注入健壮（B2）  每个槽依次注入 0/50/100/999 都不报错；条宽与告警色跟着变，999 不得撑出框
   3.2 无残留（B3）    注入后页面上不得再出现字面的 {{...}}
   3.3 包围盒（B4）    内容包围盒 ≤ 画布
   3.4 不重叠（B5）    文字×文字要**判严**（10px 两行 top 差 <11px 就算叠上）
   3.5 字号（B6）      正文 ≥11px、次要 ≥10px、中文 ≥11px
   3.6 对比度（B7）    ≥6:1（硬线 4.5:1）
   3.7 动态面积        每帧变化 ≤ 画布 10%（机器不判，自己核算）

  【闸门 4 · 内容与主题一致性】机器查不了，必须自己确认
   4.1 主题对得上    说"电脑性能监控"，画面上就得真的是 CPU / 内存 / 磁盘 / GPU / 网络 /
                      温度 / 风扇 / 进程 这类指标 —— 不要画成时钟、天气、日历、股票，
                      或一个"看着像仪表盘"的空壳
   4.2 数据有来源    每个动态槽都对应一个真实指标；不出现"没有数据来源的装饰性数字"
   4.3 中英混用统一  标签"中文 + 英文"双写，大小写风格前后一致
   4.4 多样性与分布  按需求自查明暗分布（如"3 深 + 3 浅 + 2 自由发挥"）；多套之间
                      **风格 / 排版 / 数据内容要真的不同**，不能一个模子只换配色
   4.5 信息层级      主数值与次要指标不要做成一样大；装饰不能盖过数据

■ 3.2 判定与后续处理
  说明：闸门跑完必须落到三态之一，并照做后续动作，不要含糊过去。
  · PASS  全部硬性项通过 → 允许进入下发 / 制作环节
  · WARN  只命中建议项（如 10px 中文、对比度落在 4.5~6:1）→ **可交付，但必须记录在案**、
          下一轮统一处理；**不允许当成 PASS 吞掉**
  · FAIL  命中任一硬性项（契约缺失 / 注入抛错 / 越界 / 重叠 / 对比度 <4.5:1 / 残留占位符）
          → 就地修复 → **重跑整份清单**；连续 2 次仍失败就**停下来问人**，
            不要靠"放宽阈值"让它变绿
  自动核对（下游用同一份清单做机器复核）：
    python check_material.py --all                # 静态 A1–A15 + 无头真渲染 B1–B7
    python check_material.py 你的.html             # 单个文件
    python check_material.py --all --no-render     # 只静态（更快，不启浏览器）
    python check_material.py --all --json          # 机器可读报告（下游程序抓这个）
  闸门 ↔ 机器编号：闸门1 = A1 A2 A4 B1 ｜ 闸门2 = A5–A11 A13 A15 ｜ 闸门3 = B2–B7 ｜ 闸门4 = 无

■ 3.3 高频坑速查
  说明：历次返工里出现频率最高的 7 条，交付前对着代码扫一遍。
  1. 占位符与 data-slot 拼错 → 数值永远不变，且**不报任何错**（最难查）
  2. absolute 漏 left/top → 全堆左上角；bottom:0 相对错了祖先 → 盖住下方文字
  3. 元素写了 left/top 但它的**容器没尺寸** → 整块塌成 0×0，看不见
  4. 把文字从装饰图案上挪开、却**忘了改颜色** → 深底上留深色字，对比度 1.1:1，等于没有
  5. 中灰底上用"看着够深"的次级色 → 实测只有 4.5:1，真机上发虚
  6. 两行 10px 文字 top 只差 8px → 纵向叠 2px，墨迹糊在一起
  7. 图省事写了原生 `<button>` / `<select>` / `<a>` 却没接管样式 → 深色面板里冒出
     Windows 灰按钮 / 白底下拉 / 蓝链，A15 直接 FAIL（见 2.4，首选还是用 div 自绘）
  （1–3 属"能不能跑"，4–7 属"能不能看"，**两类都要过**。）

════════════════ 【交付前速查】 ════════════════
硬性约束一页纸；与上文冲突时以本段为准。
  □ 单文件 + html,body 声明 {W}×{H} + 贴左上、禁居中、整数坐标
  □ </body> 前有产物清单块 <script type="application/json" id="material-manifest">，
     每个 data-slot 都列到、字段与素材一致（用 --emit-manifest / add_manifest.py 派生，别手抄）
  □ 背景底图只用「纯 CSS / 本地同目录相对路径 / 在线 URL」三者之一，**且事先问过用户**
  □ 会变的文字 = {{名字}} + 同名 data-slot；条 = data-kind/from/max/box；页面实现 __setSlot
  □ 单位：默认按规范单位；要别的单位写 data-unit（容量/速率/温度/时长**四族**可选，功耗固定 W），显示用 data-suffix
  □ 数值右对齐、单位紧跟、同行基线对齐、不重叠
  □ 正文 ≥11px / 次要 ≥10px / **中文 ≥11px**；对比度 **≥6:1**（中灰底尤其要压深）
  □ 文字不压装饰图案；从图案上挪开时**同步改颜色**
  □ 每帧变化 ≤ 画布 10%；不做整屏持续动画；避免 1px 浅色细线
  □ **样式自控**：原生控件（button/input/select/option/ul/ol/h1–h6/a/table/pre/hr…）
     要么不用（div 自绘），要么在 CSS 里显式接管外观（见 2.4 / A15）
  □ 外设电量槽：`mouse1_pct` / `kbd1_pct` … 的**值可能是 `—`**，非数字必须原样显示；
     `_status` 是文本槽，充电判断按文本判等，不要从数值反推
  □ **跑完第三部分四道闸门再交付**；FAIL 就地改并重跑整份清单

════════════════ 【我的画面需求】 ════════════════
分辨率：{W} × {H}        （V2 = 320×240；V1PRO = 320×170）
背景底图：☐ 纯 CSS 渐变/图案   ☐ 本地文件（与 HTML 同目录）______   ☐ 在线 URL ______
监控传感器：__________________________________（从【标准占位符】里挑，如 cpu_load / gpu1_temp / mem_pct /
                                              只画外设电量就用 mouse1_pct / kbd1_pct / mouse1_status）
内容：____________________________________________
（示例：深色科技风性能监控面板：顶部标题与状态灯；左侧 CPU / 内存两个卡片，各带百分比数字与
进度条；右下角一个小柱状图（6 根错相位跳动）；底部一条告警行。
动态槽：cpu_load=CPU%、mem_pct=内存%、gpu1_temp=GPU温度、net1_down=网络下行、sys_uptime=开机时长。）

════════════════ 【附录 · __setSlot 参考脚本（可整段删除）】 ════════════════
功能与 1.3 的 5 条职责完全一致。**上下文紧张时直接删掉本附录**，
按 1.3 的职责自己实现即可 —— 闸门 1.4 / 3.1（B1/B2）会校验。

<script>
(function () {
  function tx(el, raw) {                       /* 文本槽：格式化 */
    var v = parseFloat(raw);
    if (isNaN(v)) return String(raw);
    var d = el.getAttribute('data-fmt');
    var s = (d === null) ? String(raw) : v.toFixed(parseInt(d, 10));
    if (el.getAttribute('data-group') === '1') s = s.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return s + (el.getAttribute('data-suffix') || '');
  }
  function th(el) {                            /* 阈值；条从 data-from 的源槽继承 */
    var w = el.getAttribute('data-warn'), g = el.getAttribute('data-danger');
    if (w === null && g === null && el.getAttribute('data-from')) {
      var s = document.querySelector('[data-slot="' + el.getAttribute('data-from') + '"]');
      if (s) { w = s.getAttribute('data-warn'); g = s.getAttribute('data-danger'); }
    }
    return [w === null ? null : parseFloat(w), g === null ? null : parseFloat(g)];
  }
  function cl(el, raw) {                       /* 上色 */
    var v = parseFloat(raw), t = th(el);
    el.classList.toggle('warn',   t[0] !== null && v >= t[0] && !(t[1] !== null && v >= t[1]));
    el.classList.toggle('danger', t[1] !== null && v >= t[1]);
  }
  function bar(el, raw) {                      /* 条：按 data-box 宽度换算 */
    var v = parseFloat(raw), m = parseFloat(el.getAttribute('data-max') || '100');
    var b = (el.getAttribute('data-box') || '0,0,100,4').split(',');
    el.style.width = Math.round(Math.max(0, Math.min(1, (isNaN(v) ? 0 : v) / m)) * parseFloat(b[2])) + 'px';
    cl(el, raw);
  }
  window.__setSlot = function (name, value) {
    var sp = document.querySelectorAll('[data-slot="' + name + '"]');
    for (var i = 0; i < sp.length; i++) {
      if (sp[i].getAttribute('data-kind') === 'bar') { bar(sp[i], value); continue; }
      sp[i].textContent = tx(sp[i], value); cl(sp[i], value);
    }
    var bs = document.querySelectorAll('[data-kind="bar"][data-from="' + name + '"]');
    for (var j = 0; j < bs.length; j++) bar(bs[j], value);
    return true;
  };
  window.__setSlot('cpu_load', '42'); window.__setSlot('mem_pct', '61');   /* 默认值：不注入也有画面 */
  new URLSearchParams(location.search).forEach(function (v, k) { window.__setSlot(k, v); });
})();
</script>
配套 CSS：.warn{color:#fbbf24!important}  .danger{color:#f87171!important}
          .bar{position:absolute;height:6px;border-radius:3px;background:#38bdf8}

════════════════════════════════════════
先复述你的理解、**问清「开工前必问」那几件事**（尤其背景底图走哪种）→ 输出完整 HTML →
**交付前跑完第三部分四道闸门**。
════════════════════════════════════════

# 在线素材（online/）

这个目录放**在线素材的配置**：一条在线素材 = 一个 `.toml` 文件。
文件名（去掉 `.toml`）就是它的 id，界面里显示的是文件里的 `title`。

- 不想要某条在线素材：在程序「在线」页删除，或直接删掉对应的 `.toml`。
- 手改 `.toml` 后回到「在线」页点「刷新」即可重新读取；正在运行的画面不会自动切。
- 这个目录由程序自动创建，放在 `jiadian-monitor.exe` 同级。

## 文件格式

```toml
title = "天气面板"
description = """
可选的说明：给这个在线页面写点备注，只有界面里能看见。
"""

[v1p]
url = "https://example.com/panel/v1p/index.html"

[v2]
url = "https://example.com/panel/v2/index.html"
```

- `title` 必填，1–40 个字符。
- `description` 可选，最多 2000 字符，仅界面展示。
- `[v1p]` / `[v2]` 至少写一个；`v1p` 是 320×170（V1PRO），`v2` 是 320×240（V2）。
  当前设备是哪个尺寸，程序就加载哪个 URL；缺对应尺寸时「确定」按钮会禁用。
- `url` 允许：
  - `https://` 任意网站（**推荐**，云端托管走这条）；
  - `http://` 仅限本机与局域网私有地址：`localhost`、`127.x`、`10.x`、`172.16–31.x`、
    `192.168.x`、`169.254.x`、`::1`、`fc00::/7`、`fe80::/10`、`*.local`。
    公网 `http://` 会被拒绝（防止画面被中途篡改）。
  - 不支持 `file://`、`ftp://` 等；URL 里不能带账号密码。

## 托管到云端（最省事的用法）

在线模式的设计就是「**页面在云端，数据在本机**」：云端只放静态文件，数据仍由本机 8766 提供。

1. 准备一个能独立打开的静态页面（一个 `index.html` 或一个目录，图片/字体/CSS 用相对路径）。
2. 丢到任意静态托管：GitHub Pages / Vercel / Netlify / 对象存储（OSS、COS、S3）/ 自己的 nginx。
   **必须 https**（http 公网地址会被拒）。
3. 把 `https://…/index.html` 填进上面的 `[v1p]` / `[v2]`。
4. 页面里如果设了 CSP，必须放行本机接口，否则会出现「画面有、数据不动」：
   `connect-src 'self' http://127.0.0.1:8766`
5. 云端不要放任何数据、密钥或内网地址——页面是公开可读的。

局域网自建同理：`http://192.168.1.20:8080/panel.html` 直接填，适合内网/无外网环境。

## 在线页面怎么写

- 页面**自己取数**（程序不注入脚本）：
  ```js
  const port = new URLSearchParams(location.search).get('__api') || '8766';
  const base = 'http://127.0.0.1:' + port;
  // 并发拉 /metrics /units /peripherals，约 1s 一轮，单请求 4s 超时（AbortController）
  ```
  **不需要**实现 `__setSlot`；实现了更好（同一份页面在本地模式也能直接跑）。
- 视口按 320×170 / 320×240 设计，不滚动、不居中、不留白；程序用隐藏 WebView2 加载并抓帧。
- 失败要看得见：接口不通就显示 `—` 或「离线」，不要空白、不要假数据；恢复后自动继续。
- 不要调用控制类接口 `/app/show`、`/app/quit`（会 403），也不要假设页面能调程序内部接口。
- 页面会访问本机回环接口，浏览器对 localhost 有豁免，但不要依赖这一点做设计；
  页面里不要放密钥，不要引用不可信的第三方脚本（在线页面属于远程代码）。
- 只添加你信任的网站；程序只负责加载和抓帧，不校验页面内容。
- 交付前可自检：`python skills/jiadian-monitor-skills/scripts/check_material.py 页面.html --mode online`。

指标名、单位、外设数据格式见 `MONITOR_METRICS.md`（占位符字典）与
`skills/jiadian-monitor-skills/`（素材契约、在线托管说明、自检脚本）。
程序侧完整约定见 `app/ONLINE_MODE.md`。

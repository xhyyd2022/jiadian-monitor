<!--
  发版说明（Release 正文）
  每次发版前改这里；正文会原样放进 GitHub Release，末尾自动附上「本次包含的提交」列表。
  版本号改这五处：package.json / package-lock.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock（必须一致）。
-->

## v0.8.1 · 推屏稳定性 + 修复面板无法推屏 + 只发 zip 便携包

这一版集中修掉长时间推屏的五个薄弱点（脏矩形判定、USB 阻塞异步运行时、启动握手、
CDP 断开不自愈、租约注释），修掉「找不到面板页面 → 无法推屏」这个一直潜伏的问题，
并把发布产物统一改成 **zip 便携包**：不再生成 NSIS / MSI 安装包。

### 修复：找不到面板 CDP 页面，`render_start` 超时

现象：启动后日志停在「找不到含 index.html 的页面目标」，`render_start` 等待 20s 后失败，
设备收不到任何画面。解压 v0.7.0 便携包实测同样复现，说明问题出在 wry / WebView2 一侧，
不是某一版的业务代码改动。

根因：Tauri 2 使用的 wry 0.57 在创建 WebView2 环境时**总会**调用
`SetAdditionalBrowserArguments`；一旦显式设置，WebView2 就会忽略
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 环境变量。上一版仍用环境变量传
`--remote-debugging-port=9333`，因此主窗口和离屏面板都没有监听 9333，
`page_ws_url_matching` 自然找不到面板页。

修复：

- 主窗口在 `tauri.conf.json` 的 `app.windows[0].additionalBrowserArgs` 显式设置；
- 离屏面板在 `ensure_panel_webview` 的 `WebviewWindowBuilder` 上设置**完全相同**的
  `additional_browser_args`（两处字符串必须逐字一致，否则同一 user-data-dir 下
  WebView2 会拒绝复用环境）；
- 删除 `main.rs` 里设置环境变量的代码，避免"设了却不生效"的假象。

验证：启动后 `http://127.0.0.1:9333/json/list` 能看到面板页与主页面；日志依次出现
`面板 CDP 就绪`、`USB 接管完成`、`首帧已下发`；`/link` 为 ready。

参数固定为：

```
--remote-debugging-port=9333
--disable-features=CalculateNativeWinOcclusion,msWebOOUI,msPdfOOUI,msSmartScreenProtection
--autoplay-policy=no-user-gesture-required
```

⚠️ **不要加 `--disable-web-security`**：它会让 Chromium 不再发送 `Origin` 头，而 Tauri IPC
强制校验这个头，缺了之后所有 `invoke` 直接失败，界面表现为「点了没反应」。面板要跨源取
本机指标 API，靠的是 API 自己回 `Access-Control-Allow-Origin: *`，与浏览器安全开关无关。

### 修复：画面轻微变化被放大成大量脏矩形

旧版逐字节严格比较 RGB565，动画、抗锯齿、RGB565 量化都会把整块判成脏区，一帧里出现
很多小矩形、USB 发送次数明显偏多。

现在先把两帧解成 RGB888，按 16×8 块统计：

- **单像素尖峰阈值 12**：只有明显亮点才算变化，滤掉量化噪声；
- **块均差阈值 2.0**：整块轻微淡变、渐变仍能检出，不会漏；
- **收紧阈值 2**：合并后只保留真正有变化的像素边界；
- 块被判脏但找不到显著单像素时，退回整块，不丢弃。

仍然最多输出 6 个矩形，首帧 / 强制整屏走全帧。新增 5 个单测覆盖空帧、稀疏噪声、亮点、
整屏小幅变化和单像素边界。

### 修复：USB 读写不再占用异步运行时

推屏管线改到独立的 `render-pipeline` 线程（current-thread Tokio runtime）里跑，
USB 的打开、写入、ACK 读取都不再占用 Tauri 的 async worker —— 以前几次并发就能把
运行时拖垮，所有 `invoke` 一起不返回，界面像死机。

链路锁拆成两层：外层 `LINK` 只负责取 `Arc`，内层锁串行化真正的 USB I/O。
`with_link` 克隆 `Arc` 后立刻放开外层锁；`close_link` / `close_link_if_gen` 先取走句柄、
在锁内立即释放，再出锁归还租约 —— 既避免新旧句柄重叠，也不会让设备信息、亮度、旋转
等命令排在一次 3s 的管道超时后面。

### 修复：`render_start` 返回时 USB 已经接管

旧版命令返回 `started` 只代表管线线程已启动，USB 可能还没打开，前端只能轮询
`link_state.ready`。现在启动线程前先同步申请 USB 接管，命令返回时直接读实际链路状态：

- 接管成功：`started`；
- 只有预览：`started（USB 未接管，仅预览）`。

线程创建失败会把已拿到的链路还回去，不再泄漏租约。前端启动超时从 20s 放宽到 30s，
`waitLinkReady` 保留作后台重连和旧后端兜底。

### 修复：CDP / WebView2 断开后管线自动重连

以前 CDP 连接一断，整条管线就退出，只能手动停止再启动。现在增加监督循环
`run_supervised`：

- 单次等 CDP 响应的上限 5s（`CDP_RECV_TIMEOUT`），页面导航、渲染进程被回收、
  WebView2 卡死时不会再无限等待；
- 断开后先 3s 短探已有页面；找不到就重新导航（8s）；仍不行才关闭并重建隐藏的
  panel WebView2（10s）；
- 重连退避 0.5s → 1s → 2s → 4s，封顶 5s；
- 会话期间**不重新移交 USB**：句柄由监督循环持有，跨会话复用；每轮重新读一次链路状态，
  上一轮因拔线被关掉就自然退回仅预览；
- 用户主动停止、到达运行时长仍然正常结束；只有异常断开才进入重连；
- 解码 / 转换失败不再 `?` 打死整条管线，转成一次重连。

`RunningGuard` 统一负责收尾，异常退出也会复位运行状态和链路状态。

### 修复：租约注释与实际的 120s 对齐

`handoff.rs` 顶部注释原来写租约 90s，实际管线申请的是 **120s**、每约 20s 续期一次。
注释已改成与代码一致，避免后续维护按错误 TTL 估算。

### 变更：发布产物只保留 zip 便携包

- `tauri.conf.json` 关闭 `bundle.active`，普通 `tauri build` 只生成 exe；
- CI 改用 `tauri build --no-bundle`，不再产出 NSIS `.exe` / WiX `.msi`；
- `scripts/build.ps1` 部署后自动打包 `bin/jiaidian-monitor-v<版本>.zip`；
- zip 只含 `jiaidian-monitor.exe`、`assets/`、`uninstall-lhws.bat`，
  不带 `settings.ini`、`log/` 和旧 zip；
- CI artifact 与 GitHub Release 都只上传版本化 zip。

### 升级提示

- 便携版直接替换 `jiaidian-monitor.exe`，或把 zip 解压覆盖到原目录；
- `settings.ini` 会保留原有设置；新解压的 zip 不包含它，首次运行会自动生成；
- 不再提供安装包，卸载辅助脚本仍在 zip 内（`uninstall-lhws.bat`）；
- 若升级后仍无画面，先看日志里有没有 `面板 CDP 就绪`：没有就是 WebView2 参数没生效
  （检查两处 `additionalBrowserArgs` 是否一致、是否被别处覆盖）。

### 许可证

本项目采用 **PolyForm Noncommercial License 1.0.0** —— 源码可见，**禁止商业使用**（非 OSI 开源许可）。
第三方组件声明见 `THIRD-PARTY-NOTICES.md`。

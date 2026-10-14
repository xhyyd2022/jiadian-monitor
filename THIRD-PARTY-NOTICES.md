# 第三方组件声明（Third-Party Notices）

本程序包含或可协助安装以下第三方开源组件。各组件的版权、许可与源码由上游提供。

---

## LibreHardwareService

- **版本**：0.3.4
- **版权**：Copyright (C) 2022 Emerson Pinter
- **许可证**：Mozilla Public License v. 2.0（MPL-2.0）—— 全文见 [`licenses/MPL-2.0.txt`](licenses/MPL-2.0.txt)
- **来源**：https://github.com/epinter/LibreHardwareService
- **源码（以 MPL-2.0 提供）**：https://github.com/epinter/LibreHardwareService
- **下载地址**（本程序"一键安装"使用的官方发布页）：
  https://github.com/epinter/LibreHardwareService/releases/download/v0.3.4/LibreHardwareService.msi
  sha256：`30e7c877e4b7979b174f93ac5a3a2cf13e5209c3d64a34ed5040a013f68f4020`
- **说明**：该组件是**独立的第三方软件**，并非本程序的一部分。本程序仅在用户确认后，
  从上述**官方发布页**下载并按固定 sha256 校验，再用 Windows Installer（msiexec）以管理员权限安装；
  **不修改其任何文件**。可选地，用户可自行把 MSI 放到程序目录，本程序将直接使用本地文件（仍会校验 sha256）。
  该组件的数据读取接口为其写入的共享内存（`Global\LibreHardwareService/json/…`）。
  本程序不对该组件作任何担保，也不代表其作者或与之存在隶属、合作或背书关系。

### 该组件自身依赖的开源库（由上游分发，随其安装包提供）

| 组件 | 许可证 | 来源 |
|---|---|---|
| LibreHardwareMonitor | MPL-2.0 | https://github.com/LibreHardwareMonitor/LibreHardwareMonitor |
| Utf8Json | MIT | https://github.com/neuecc/Utf8Json |
| MessagePack-CSharp | MIT | https://github.com/MessagePack-CSharp/MessagePack-CSharp |

---

## 本项目直接包含的开源组件

以下组件**随本程序分发**（打包进前端资源，或静态链接进可执行文件），一并声明。

### 前端（打包进程序资源）

| 组件 | 版本 | 许可证 | 来源 |
|---|---|---|---|
| Vue | 3.5.43 | MIT | https://github.com/vuejs/core |
| @tauri-apps/api | 2.12.3 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| @tauri-apps/plugin-dialog | 2.8.2 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| @tauri-apps/plugin-opener | 2.7.1 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| @iconify/vue | 5.0.3 | MIT | https://github.com/iconify/iconify |
| @iconify-icons/line-md | 2.0.0 | MIT | https://github.com/iconify/icon-sets |

### 后端（Rust，静态链接进 `jiaidian-monitor.exe`）

| 组件 | 版本 | 许可证 | 来源 |
|---|---|---|---|
| tauri | 2.x | MIT OR Apache-2.0 | https://github.com/tauri-apps/tauri |
| tauri-plugin-dialog | 2.x | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| tauri-plugin-opener | 2.x | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| serde / serde_json | 1.x | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| ureq | 3.4.2 | MIT OR Apache-2.0 | https://github.com/algesten/ureq |
| tokio | 1.53.2 | MIT | https://github.com/tokio-rs/tokio |
| tokio-tungstenite | 0.30.0 | MIT | https://github.com/snapview/tokio-tungstenite |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| png | 0.18.1 | MIT OR Apache-2.0 | https://github.com/image-rs/image-png |
| base64 | 0.23.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |

> 上表为**直接依赖**；它们各自的传递依赖同样静态链接进二进制。完整依赖树与每个 crate 的
> 版本见 [`src-tauri/Cargo.lock`](src-tauri/Cargo.lock)，各 crate 的许可证声明见其自身
> `Cargo.toml` 的 `license` 字段。这些组件均以 **MIT** 或 **Apache-2.0** 授权，全文见
> [`licenses/MIT.txt`](licenses/MIT.txt) 与 [`licenses/Apache-2.0.txt`](licenses/Apache-2.0.txt)。

### 仅构建期使用（不随程序分发）

| 组件 | 版本 | 许可证 |
|---|---|---|
| Vite | 8.3.4 | MIT |
| TypeScript | 6.0.3 | Apache-2.0 |
| vue-tsc | 3.3.13 | MIT |
| @vitejs/plugin-vue | 6.0.10 | MIT |
| @tauri-apps/cli | 2.12.2 | Apache-2.0 OR MIT |

### 图标

界面图标取自 **Iconify** 的 `line-md` 图标集（MIT），已**离线打包**为 `src/line-md-icons.ts`，
运行时不访问任何 CDN。

---

## 说明

- 本程序**读取** LibreHardwareService 产出的数据（共享内存），未复制其源代码，因此不构成其衍生作品；
  上述声明是为了满足 MPL-2.0 §3.2（分发可执行形式时告知源代码获取方式）并尊重上游署名。
- 若本程序未来改为**打包分发**该组件的安装包，本声明与 `licenses/MPL-2.0.txt` 必须随包提供。
- 本程序**静态链接**了上表所列 Rust crates、**打包**了上表所列前端组件；MIT / Apache-2.0 /
  MPL-2.0 许可全文分别见 [`licenses/MIT.txt`](licenses/MIT.txt)、
  [`licenses/Apache-2.0.txt`](licenses/Apache-2.0.txt)、[`licenses/MPL-2.0.txt`](licenses/MPL-2.0.txt)。
- 依赖与编译产物**不入库**，均由本地或 GitHub Actions 现场安装、编译。

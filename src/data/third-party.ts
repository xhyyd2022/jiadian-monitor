// 由 scripts/gen-third-party.mjs 生成，请勿手改；升级依赖后重跑脚本。
export interface OssEntry {
  name: string;
  version: string;
  license: string;
  url: string;
}

export interface OssGroup {
  id: string;
  title: string;
  desc: string;
  items: OssEntry[];
}

export const APP_LICENSE = "PolyForm-Noncommercial-1.0.0";
export const APP_LICENSE_URL = "https://polyformproject.org/licenses/noncommercial/1.0.0";
export const NOTICES_FILE = "THIRD-PARTY-NOTICES.md";

export const OSS_GROUPS: OssGroup[] = [
  {
    "id": "runtime",
    "title": "界面运行时",
    "desc": "随程序一起分发，直接驱动界面与窗口。",
    "items": [
      {
        "name": "vue",
        "version": "3.5.43",
        "license": "MIT",
        "url": "https://vuejs.org/"
      },
      {
        "name": "element-plus",
        "version": "2.14.8",
        "license": "MIT",
        "url": "https://element-plus.org/"
      },
      {
        "name": "@iconify/vue",
        "version": "5.0.3",
        "license": "MIT",
        "url": "https://iconify.design/"
      },
      {
        "name": "@iconify-icons/line-md",
        "version": "2.0.0",
        "license": "MIT",
        "url": "https://iconify.design/icon-sets/line-md/"
      },
      {
        "name": "@material/material-color-utilities",
        "version": "0.4.0",
        "license": "Apache-2.0",
        "url": "https://github.com/material-foundation/material-color-utilities/tree/main/typescript"
      },
      {
        "name": "@tauri-apps/api",
        "version": "2.12.3",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/tauri#readme"
      },
      {
        "name": "@tauri-apps/plugin-dialog",
        "version": "2.8.2",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/tauri-apps/plugins-workspace"
      },
      {
        "name": "@tauri-apps/plugin-opener",
        "version": "2.7.1",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/tauri-apps/plugins-workspace"
      }
    ]
  },
  {
    "id": "framework",
    "title": "桌面框架",
    "desc": "Tauri 框架与 Windows 窗口/WebView 集成层。",
    "items": [
      {
        "name": "tauri",
        "version": "2.12.3",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/tauri"
      },
      {
        "name": "wry",
        "version": "0.57.0",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/wry"
      },
      {
        "name": "tao",
        "version": "0.37.1",
        "license": "Apache-2.0",
        "url": "https://github.com/tauri-apps/tao"
      }
    ]
  },
  {
    "id": "rust",
    "title": "Rust 依赖",
    "desc": "采集、渲染与设备通信用到的库。",
    "items": [
      {
        "name": "tauri-plugin-opener",
        "version": "2.7.1",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/plugins-workspace"
      },
      {
        "name": "tauri-plugin-dialog",
        "version": "2.8.2",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/plugins-workspace"
      },
      {
        "name": "serde",
        "version": "1.0.229",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/serde-rs/serde"
      },
      {
        "name": "serde_json",
        "version": "1.0.154",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/serde-rs/json"
      },
      {
        "name": "toml",
        "version": "1.1.8+spec-1.1.0",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/toml-rs/toml"
      },
      {
        "name": "windows-sys",
        "version": "0.61.2",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/microsoft/windows-rs"
      },
      {
        "name": "ureq",
        "version": "3.4.2",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/algesten/ureq"
      },
      {
        "name": "tokio",
        "version": "1.53.2",
        "license": "MIT",
        "url": "https://github.com/tokio-rs/tokio"
      },
      {
        "name": "tokio-tungstenite",
        "version": "0.30.0",
        "license": "MIT",
        "url": "https://github.com/snapview/tokio-tungstenite"
      },
      {
        "name": "futures-util",
        "version": "0.3.34",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/rust-lang/futures-rs"
      },
      {
        "name": "png",
        "version": "0.18.1",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/image-rs/image-png"
      },
      {
        "name": "base64",
        "version": "0.23.1",
        "license": "MIT OR Apache-2.0",
        "url": "https://github.com/marshallpierce/rust-base64"
      }
    ]
  },
  {
    "id": "build",
    "title": "开发与构建工具",
    "desc": "只在开发/构建/无障碍检查时使用，不随程序分发。",
    "items": [
      {
        "name": "vite",
        "version": "8.3.4",
        "license": "MIT",
        "url": "https://vite.dev"
      },
      {
        "name": "@vitejs/plugin-vue",
        "version": "6.0.10",
        "license": "MIT",
        "url": "https://github.com/vitejs/vite-plugin-vue/tree/main/packages/plugin-vue#readme"
      },
      {
        "name": "typescript",
        "version": "6.0.3",
        "license": "Apache-2.0",
        "url": "https://www.typescriptlang.org/"
      },
      {
        "name": "vue-tsc",
        "version": "3.3.13",
        "license": "MIT",
        "url": "https://github.com/vuejs/language-tools"
      },
      {
        "name": "@tauri-apps/cli",
        "version": "2.12.2",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/tauri#readme"
      },
      {
        "name": "axe-core",
        "version": "4.10.0",
        "license": "MPL-2.0",
        "url": "https://www.deque.com/axe/"
      },
      {
        "name": "tauri-build",
        "version": "2.7.2",
        "license": "Apache-2.0 OR MIT",
        "url": "https://github.com/tauri-apps/tauri"
      }
    ]
  }
];

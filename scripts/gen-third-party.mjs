// 生成「关于」页面的开源依赖清单与 THIRD-PARTY-NOTICES.md。
// 用法（在 app/ 下）：node scripts/gen-third-party.mjs
//
// 数据来源都是本机实际安装/解析到的版本：
//   - npm：package.json 的 dependencies / devDependencies + node_modules/<pkg>/package.json
//   - Rust：cargo metadata（Cargo.lock 解析结果）
// 因此重新 npm install / cargo update 之后重跑本脚本即可同步版本号。
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");

function readJson(p) {
  return JSON.parse(readFileSync(p, "utf8"));
}

function npmEntry(name) {
  const pkgPath = join(appDir, "node_modules", ...name.split("/"), "package.json");
  if (!existsSync(pkgPath)) return { name, version: "—", license: "见包内 LICENSE", url: "" };
  const p = readJson(pkgPath);
  let url = (p.homepage || (p.repository && (p.repository.url || p.repository)) || "").replace(/^git\+/, "").replace(/\.git$/, "");
  if (!url && name === "@iconify-icons/line-md") url = "https://iconify.design/icon-sets/line-md/";
  return { name, version: p.version, license: p.license || "见包内 LICENSE", url };
}

function rustPackages() {
  const out = execFileSync("cargo", ["metadata", "--format-version", "1"], {
    cwd: join(appDir, "src-tauri"),
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return JSON.parse(out).packages;
}

const pkg = readJson(join(appDir, "package.json"));
const depNames = Object.keys(pkg.dependencies || {});
const devNames = Object.keys(pkg.devDependencies || {});

// 运行时（随程序分发）
const runtime = [
  "vue",
  "element-plus",
  "@iconify/vue",
  "@iconify-icons/line-md",
  "@material/material-color-utilities",
  "@tauri-apps/api",
  "@tauri-apps/plugin-dialog",
  "@tauri-apps/plugin-opener",
].map(npmEntry);

// 开发 / 构建工具（不随程序分发）
const build = [
  "vite",
  "@vitejs/plugin-vue",
  "typescript",
  "vue-tsc",
  "@tauri-apps/cli",
  "axe-core",
].map(npmEntry);

// Rust 直接依赖（按 Cargo.toml）
const rustAll = rustPackages();
const rustByName = new Map(rustAll.map((p) => [p.name, p]));
const rustDirect = [
  "tauri",
  "tauri-build",
  "wry",
  "tao",
  "tauri-plugin-opener",
  "tauri-plugin-dialog",
  "serde",
  "serde_json",
  "toml",
  "windows-sys",
  "ureq",
  "tokio",
  "tokio-tungstenite",
  "futures-util",
  "png",
  "base64",
].map((name) => {
  const p = rustByName.get(name);
  return {
    name,
    version: p ? p.version : "—",
    license: (p && p.license) || "见 crate 内 LICENSE",
    url: (p && p.repository) || (p && p.homepage) || "",
  };
});

// 框架和 Rust 库分开展示，读起来更清楚
const frameworkNames = new Set(["tauri", "wry", "tao"]);
const framework = rustDirect.filter((e) => frameworkNames.has(e.name));
const rustLibs = rustDirect.filter((e) => !frameworkNames.has(e.name) && e.name !== "tauri-build");
const tauriBuild = rustDirect.find((e) => e.name === "tauri-build");
if (tauriBuild) build.push(tauriBuild);

const groups = [
  {
    id: "runtime",
    title: "界面运行时",
    desc: "随程序一起分发，直接驱动界面与窗口。",
    items: runtime,
  },
  {
    id: "framework",
    title: "桌面框架",
    desc: "Tauri 框架与 Windows 窗口/WebView 集成层。",
    items: framework,
  },
  {
    id: "rust",
    title: "Rust 依赖",
    desc: "采集、渲染与设备通信用到的库。",
    items: rustLibs,
  },
  {
    id: "build",
    title: "开发与构建工具",
    desc: "只在开发/构建/无障碍检查时使用，不随程序分发。",
    items: build,
  },
];

// ---------- 生成 src/data/third-party.ts ----------
const ts = `// 由 scripts/gen-third-party.mjs 生成，请勿手改；升级依赖后重跑脚本。
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

export const OSS_GROUPS: OssGroup[] = ${JSON.stringify(groups, null, 2)};
`;
mkdirSync(join(appDir, "src", "data"), { recursive: true });
writeFileSync(join(appDir, "src", "data", "third-party.ts"), ts, "utf8");

// ---------- 生成 THIRD-PARTY-NOTICES.md ----------
function table(entries) {
  const esc = (s) => String(s || "").replace(/\|/g, "/");
  const lines = ["| 名称 | 版本 | 许可证 | 主页 / 仓库 |", "| --- | --- | --- | --- |"];
  for (const e of entries) lines.push(`| ${esc(e.name)} | ${esc(e.version)} | ${esc(e.license)} | ${e.url ? esc(e.url) : "—"} |`);
  return lines.join("\n");
}

// npm 完整树：package-lock v3 的 packages 键
const lock = readJson(join(appDir, "package-lock.json"));
let npmAll = [];
for (const [key, meta] of Object.entries(lock.packages || {})) {
  if (!key || !meta || !meta.version) continue;
  const name = key.slice(key.lastIndexOf("node_modules/") + "node_modules/".length);
  if (!name) continue;
  const candidates = [key, `node_modules/${name}`];
  let lic = "—";
  let url = "";
  for (const rel of candidates) {
    const p = join(appDir, rel, "package.json");
    if (existsSync(p)) {
      const j = readJson(p);
      lic = j.license || lic;
      url = (j.homepage || (j.repository && (j.repository.url || j.repository)) || "").replace(/^git\+/, "").replace(/\.git$/, "");
      break;
    }
  }
  npmAll.push({ name, version: meta.version, license: lic, url });
}
const seen = new Set();
const npmAllUnique = npmAll.filter((e) => {
  const k = `${e.name}@${e.version}`;
  if (seen.has(k)) return false;
  seen.add(k);
  return true;
});
npmAllUnique.sort((a, b) => a.name.localeCompare(b.name));

// Rust 完整树：cargo metadata 里的全部外部包
const rustAllEntries = rustAll
  .filter((p) => p.name !== "jiadian-monitor")
  .map((p) => ({ name: p.name, version: p.version, license: p.license || "—", url: p.repository || p.homepage || "" }))
  .sort((a, b) => a.name.localeCompare(b.name));

const notices = `# 第三方开源组件与许可证（THIRD-PARTY NOTICES）

佳点监控（jiadian-monitor）v${pkg.version} 使用下列第三方开源组件。本文件由
\`app/scripts/gen-third-party.mjs\` 依据实际安装/锁定版本生成，升级依赖后请重新生成。

- 本程序自身许可：**${pkg.license}**（源码可见的非商业许可，不是 OSI 开源许可），
  许可全文：https://polyformproject.org/licenses/noncommercial/1.0.0
- 数据源组件 **LibreHardwareService** 由外部安装，许可 **MPL-2.0**，不随本程序分发。
- Windows 上运行的 **Microsoft Edge WebView2 Runtime** 属于系统/微软组件，不是开源项目。

## 直接依赖

### 界面运行时（随程序分发）

${table(runtime)}

### 桌面框架

${table(framework)}

### Rust 依赖

${table(rustLibs)}

### 开发与构建工具（不随程序分发）

${table(build)}

## 完整依赖清单

### npm（共 ${npmAll.length} 个包）

${table(npmAll)}

### Rust / Cargo（共 ${rustAllEntries.length} 个 crate）

${table(rustAllEntries)}

## 许可证说明

- **MIT**：保留版权与许可声明即可商用/修改/再分发；完整文本见各包 \`LICENSE\` 或
  <https://opensource.org/license/mit>。
- **Apache-2.0**：保留版权、许可与 NOTICE（如有），并标明修改；完整文本见
  <https://www.apache.org/licenses/LICENSE-2.0>。
- **MPL-2.0**（LibreHardwareService）：文件级 copyleft，修改其源码需以 MPL-2.0 公开该文件；
  完整文本见 <https://mozilla.org/MPL/2.0/>。
- 标注为 \`A OR B\` 的组件可按任一许可使用；本程序按 **MIT** 使用 \`MIT OR Apache-2.0\` 类组件。
- 各依赖的版权归其各自作者所有；本文件不改变任何第三方许可条款。
`;

writeFileSync(join(appDir, "THIRD-PARTY-NOTICES.md"), notices, "utf8");

console.log(
  `third-party.ts: runtime=${runtime.length} framework=${framework.length} rust=${rustLibs.length} build=${build.length}`,
);
console.log(`THIRD-PARTY-NOTICES.md: npm=${npmAll.length} rust=${rustAllEntries.length}`);

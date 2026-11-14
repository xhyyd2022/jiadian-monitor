import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useSettings } from "./useSettings";

/**
 * 外观：背景图 / 强调色 / 面板不透明度。
 *
 * 真值全部在 settings.ini（由 useSettings 统一读写）；这里只负责：
 * - 把设置投影成 CSS 变量（--accent / --on-accent / --ui-alpha / --app-bg-image）；
 * - 背景图导入、清除、读取字节并转成 Blob URL。
 *
 * 背景图文件由 Rust 复制到 exe 同级 `ui/background.<ext>`，前端只保存文件名；
 * 这样 settings.ini 可读、可迁移，也不会把几 MB 的图片塞进设置文件。
 */
const { appearanceAccent, appearanceBg, appearanceOpacity, settingsReady } = useSettings();

const bgUrl = ref("");
const bgError = ref("");
const bgBusy = ref(false);

function isHex(v: string): boolean {
  return /^#[0-9a-fA-F]{6}$/.test(v);
}

function accentOn(hex: string): string {
  const n = Number.parseInt(hex.slice(1), 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  // 相对亮度（WCAG）：浅色强调色用深色前景，深色强调色用白字。
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const lum = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  // 选取对比度更高的前景色：白字 (1.05/(L+0.05)) 与深色字 ((L+0.05)/0.05) 的交点在 L≈0.179。
  // 之前用 0.45 阈值，琥珀/天蓝这类中间亮度强调色仍配白字，实测只有 2.1–2.6:1。
  return lum >= 0.18 ? "#08111c" : "#ffffff";
}

function applyAppearance(): void {
  const root = document.documentElement;
  const accent = appearanceAccent.value;
  if (isHex(accent)) {
    root.style.setProperty("--accent", accent);
    // --title 现在表示“中性标题色”，不再跟随强调色（评审 G01）
    root.style.setProperty("--on-accent", accentOn(accent));
  } else {
    root.style.removeProperty("--accent");
    root.style.removeProperty("--title");
    root.style.removeProperty("--on-accent");
  }
  root.style.setProperty("--ui-alpha", String(appearanceOpacity.value / 100));
  root.style.setProperty("--app-bg-image", bgUrl.value ? `url("${bgUrl.value}")` : "none");
  root.style.setProperty("--app-bg-veil", bgUrl.value ? "0.22" : "0");
}

async function loadBackground(): Promise<void> {
  const name = appearanceBg.value;
  if (!name) {
    if (bgUrl.value) URL.revokeObjectURL(bgUrl.value);
    bgUrl.value = "";
    applyAppearance();
    return;
  }
  try {
    const bytes = await invoke<number[] | null>("appearance_bg_bytes", { name });
    if (!bytes || !bytes.length) {
      bgError.value = "背景图文件不存在，可重新导入或清除";
      if (bgUrl.value) URL.revokeObjectURL(bgUrl.value);
      bgUrl.value = "";
    } else {
      const ext = name.split(".").pop()?.toLowerCase() || "png";
      const mime = ext === "png" ? "image/png" : ext === "webp" ? "image/webp" : "image/jpeg";
      if (bgUrl.value) URL.revokeObjectURL(bgUrl.value);
      bgUrl.value = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: mime }));
      bgError.value = "";
    }
  } catch (e) {
    bgError.value = `读取背景图失败：${e}`;
  }
  applyAppearance();
}

async function pickBackground(): Promise<void> {
  bgError.value = "";
  bgBusy.value = true;
  try {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (typeof picked !== "string" || !picked) return;
    const name = await invoke<string>("appearance_import_bg", { path: picked });
    appearanceBg.value = name;
    await loadBackground();
  } catch (e) {
    bgError.value = String(e);
  } finally {
    bgBusy.value = false;
  }
}

async function clearBackground(): Promise<void> {
  bgError.value = "";
  try {
    await invoke("appearance_clear_bg");
  } catch {
    /* 文件删不掉不影响把设置清空 */
  }
  appearanceBg.value = "";
  if (bgUrl.value) URL.revokeObjectURL(bgUrl.value);
  bgUrl.value = "";
  applyAppearance();
}

function setAccent(hex: string | null | undefined): void {
  appearanceAccent.value = hex && isHex(hex) ? hex : "";
}

watch([appearanceAccent, appearanceOpacity], applyAppearance, { immediate: true });
watch(appearanceBg, () => void loadBackground(), { immediate: true });
// 设置首次读完后再对一遍，避免默认值覆盖 ini 里的外观。
watch(settingsReady, (ready) => {
  if (ready) {
    applyAppearance();
    void loadBackground();
  }
});

export function useAppearance() {
  return {
    bgUrl,
    bgError,
    bgBusy,
    pickBackground,
    clearBackground,
    setAccent,
    applyAppearance,
  };
}

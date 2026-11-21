import { computed, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { ElMessageBox } from "element-plus";
import { open } from "@tauri-apps/plugin-dialog";
import {
  argbFromHex,
  hexFromArgb,
  Hct,
  MaterialDynamicColors,
  SchemeContent,
  SchemeExpressive,
  SchemeFidelity,
  SchemeFruitSalad,
  SchemeMonochrome,
  SchemeNeutral,
  SchemeRainbow,
  SchemeTonalSpot,
  SchemeVibrant,
  type DynamicScheme,
} from "@material/material-color-utilities";
import { useSettings } from "./useSettings";

export type ColorMode = "standard" | "advanced";

/**
 * M3 自带的几种取色风格；同一颗种子色换算法会得到完全不同的气质。
 * 这些算法直接来自 Material Color Utilities，不是自己拍脑袋的调色。
 */
export const MATERIAL_ALGORITHMS = [
  { id: "tonal", label: "柔和", hint: "低饱和，耐看" },
  { id: "vibrant", label: "鲜艳", hint: "提高色度，强调色更跳" },
  { id: "expressive", label: "表现", hint: "色相偏移，更有性格" },
  { id: "fidelity", label: "还原", hint: "保留种子色本来的颜色" },
  { id: "content", label: "内容", hint: "种子色成为画面里的容器色" },
  { id: "monochrome", label: "单色", hint: "灰阶为主，只保留明暗" },
  { id: "neutral", label: "中性", hint: "近乎灰阶，克制耐看" },
  { id: "rainbow", label: "彩虹", hint: "多色相，适合活泼界面" },
  { id: "fruit", label: "果色", hint: "高饱和、多色相" },
] as const;

export type AlgorithmId = (typeof MATERIAL_ALGORITHMS)[number]["id"];

const ALGORITHM_IDS = MATERIAL_ALGORITHMS.map((a) => a.id);

type SchemeCtor = new (hct: Hct, dark: boolean, contrast: number) => DynamicScheme;

const SCHEMES: Record<AlgorithmId, SchemeCtor> = {
  tonal: SchemeTonalSpot,
  vibrant: SchemeVibrant,
  expressive: SchemeExpressive,
  fidelity: SchemeFidelity,
  content: SchemeContent,
  monochrome: SchemeMonochrome,
  neutral: SchemeNeutral,
  rainbow: SchemeRainbow,
  fruit: SchemeFruitSalad,
};

/**
 * 主题：`Theme/theme.toml` + 同目录的一张背景图。
 *
 * 配色用 Material 3 的 HCT 算法从种子色生成整套深/浅色板（成熟、可访问），
 * 生成后每一项仍可手动覆盖。图片缩放/位置、不透明度也写在同一份 TOML 里，
 * 用户可以直接备份、编辑、分享整个 Theme 文件夹。
 */
export interface Palette {
  bg: string;
  side: string;
  panel: string;
  line: string;
  fg: string;
  sub: string;
  accent: string;
  button: string;
  buttonHover: string;
  danger: string;
  ok: string;
  warn: string;
}

export interface ThemeConfig {
  name: string;
  seed: string;
  mode: ColorMode;
  algorithm: AlgorithmId;
  image: string;
  imageFit: "cover" | "contain" | "fill" | "tile";
  imageX: number;
  imageY: number;
  opacity: number;
  dark: Palette;
  light: Palette;
}

interface ThemeData {
  config: ThemeConfig;
  image: number[] | null;
  dir: string;
  exists: boolean;
}

export const DARK_DEFAULT: Palette = {
  bg: "#11151c",
  side: "#141a22",
  panel: "#1c2430",
  line: "#344155",
  fg: "#f1f5f9",
  sub: "#bac5d3",
  accent: "#619aff",
  button: "#242e3d",
  buttonHover: "#2d3a4b",
  danger: "#f87171",
  ok: "#34d399",
  warn: "#fbbf24",
};

export const LIGHT_DEFAULT: Palette = {
  bg: "#f4f6f9",
  side: "#fbfcfe",
  panel: "#ffffff",
  line: "#dce2ea",
  fg: "#202733",
  sub: "#596579",
  accent: "#2563eb",
  button: "#f1f4f8",
  buttonHover: "#e6ebf2",
  danger: "#dc2626",
  ok: "#059669",
  warn: "#d97706",
};

function defaultConfig(): ThemeConfig {
  return {
    name: "自定义主题",
    seed: "#619aff",
    mode: "standard",
    algorithm: "tonal",
    image: "",
    imageFit: "cover",
    imageX: 50,
    imageY: 50,
    opacity: 94,
    dark: { ...DARK_DEFAULT },
    light: { ...LIGHT_DEFAULT },
  };
}

const { theme, settingsReady } = useSettings();

const config = reactive<ThemeConfig>(defaultConfig());
const themeDir = ref("");
const themeExists = ref(false);
const themeReady = ref(false);
const themeError = ref("");
const themeSaving = ref(false);
const imageUrl = ref("");
const imageError = ref("");
const imageBusy = ref(false);

const currentPalette = computed<Palette>(() =>
  theme.value === "dark" ? config.dark : config.light,
);
const hasImage = computed(() => !!imageUrl.value);
const themeFilePath = computed(() => {
  if (!themeDir.value) return "Theme\\theme.toml";
  return `${themeDir.value.replace(/[\\/]+$/, "")}\\theme.toml`;
});
const imageName = computed(() => config.image || "未选择（使用窗口背景色）");

function isHex(v: string): boolean {
  return /^#[0-9a-fA-F]{6}$/.test(v);
}

function hex(argb: number): string {
  return hexFromArgb(argb).slice(0, 7).toLowerCase();
}

function accentOn(hexColor: string): string {
  const n = Number.parseInt(hexColor.slice(1), 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const lum = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return lum >= 0.18 ? "#08111c" : "#ffffff";
}

const COLOR_VARS: Record<keyof Palette, string> = {
  bg: "--bg",
  side: "--side",
  panel: "--panel",
  line: "--line",
  fg: "--fg",
  sub: "--sub",
  accent: "--accent",
  button: "--btn",
  buttonHover: "--btn-hover",
  danger: "--danger",
  ok: "--ok",
  warn: "--warn",
};

function bgSize(fit: ThemeConfig["imageFit"]): string {
  if (fit === "contain") return "contain";
  if (fit === "fill") return "100% 100%";
  if (fit === "tile") return "auto";
  return "cover";
}

function applyAppearance(): void {
  const root = document.documentElement;
  const p = currentPalette.value;
  for (const [key, cssVar] of Object.entries(COLOR_VARS) as [keyof Palette, string][]) {
    const value = p[key];
    if (isHex(value)) root.style.setProperty(cssVar, value.toLowerCase());
    else root.style.removeProperty(cssVar);
  }
  root.style.setProperty("--on-accent", accentOn(isHex(p.accent) ? p.accent : "#619aff"));
  root.style.setProperty("--ui-alpha", String(config.opacity / 100));
  root.style.setProperty("--app-bg-image", imageUrl.value ? `url("${imageUrl.value}")` : "none");
  root.style.setProperty("--app-bg-size", bgSize(config.imageFit));
  root.style.setProperty("--app-bg-position", `${config.imageX}% ${config.imageY}%`);
  root.style.setProperty("--app-bg-repeat", config.imageFit === "tile" ? "repeat" : "no-repeat");
  root.style.setProperty("--app-bg-veil", imageUrl.value ? "0.22" : "0");
}

function paletteFrom(seed: string, dark: boolean, algorithm: AlgorithmId): Palette {
  const scheme = new SCHEMES[algorithm](Hct.fromInt(argbFromHex(seed)), dark, 0);
  const c = new MaterialDynamicColors();
  const tone = (role: { getArgb(s: DynamicScheme): number }): string => hex(role.getArgb(scheme));
  return {
    // 用 M3 的 surface 分层而不是纯黑+纯灰：层级看得出，又不会太生硬。
    bg: tone(c.background()),
    side: tone(c.surfaceContainerLow()),
    panel: tone(c.surfaceContainer()),
    line: tone(c.outlineVariant()),
    fg: tone(c.onBackground()),
    sub: tone(c.onSurfaceVariant()),
    accent: tone(c.primary()),
    button: tone(c.surfaceContainerHigh()),
    buttonHover: tone(c.surfaceContainerHighest()),
    // 危险/成功/警告保持语义稳定，不跟着种子色偏色。
    danger: tone(c.error()),
    ok: dark ? DARK_DEFAULT.ok : LIGHT_DEFAULT.ok,
    warn: dark ? DARK_DEFAULT.warn : LIGHT_DEFAULT.warn,
  };
}

/** 选种子色即生成整套深/浅色板，不需要再点“确定”。 */
function generateFromSeed(seed: string): void {
  if (!isHex(seed)) return;
  const s = seed.toLowerCase();
  config.seed = s;
  config.mode = "standard";
  Object.assign(config.dark, paletteFrom(s, true, config.algorithm));
  Object.assign(config.light, paletteFrom(s, false, config.algorithm));
  applyAppearance();
}

function setSeed(value: string | null | undefined): void {
  if (value && isHex(value)) generateFromSeed(value);
}

function setMode(value: ColorMode): void {
  if (value === "standard") {
    config.mode = "standard";
    generateFromSeed(config.seed);
    return;
  }
  // 高级模式保留当前颜色，之后所有改动都以手动值为准。
  config.mode = "advanced";
}

function setAlgorithm(value: AlgorithmId): void {
  if (!ALGORITHM_IDS.includes(value)) return;
  config.algorithm = value;
  if (config.mode === "standard") generateFromSeed(config.seed);
  else applyAppearance();
}

const seedColor = computed(() => (isHex(config.seed) ? config.seed : "#619aff"));

function setColor(key: keyof Palette, value: string | null): void {
  const p = currentPalette.value;
  p[key] = value && isHex(value) ? value.toLowerCase() : "";
  applyAppearance();
}

function resetMode(): void {
  if (config.mode === "standard") {
    generateFromSeed(config.seed);
    return;
  }
  const p = currentPalette.value;
  Object.assign(p, theme.value === "dark" ? DARK_DEFAULT : LIGHT_DEFAULT);
  applyAppearance();
}

function resetAll(): void {
  config.seed = "#619aff";
  config.mode = "standard";
  config.algorithm = "tonal";
  Object.assign(config.dark, DARK_DEFAULT);
  Object.assign(config.light, LIGHT_DEFAULT);
  applyAppearance();
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
let saveSeq = 0;
let savedSeq = 0;

async function saveNow(): Promise<void> {
  const seq = ++saveSeq;
  themeSaving.value = true;
  const snapshot = JSON.parse(JSON.stringify(config)) as ThemeConfig;
  try {
    await invoke("theme_save", { config: snapshot });
    if (seq > savedSeq) savedSeq = seq;
    if (seq === saveSeq) themeError.value = "";
  } catch (e) {
    if (seq === saveSeq) themeError.value = `保存主题失败：${e}`;
  } finally {
    if (seq === saveSeq) themeSaving.value = false;
  }
}

function scheduleSave(): void {
  if (!themeReady.value) return;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => void saveNow(), 260);
}

async function loadImage(name: string, bytes?: number[] | null): Promise<void> {
  if (imageUrl.value) URL.revokeObjectURL(imageUrl.value);
  imageUrl.value = "";
  if (!name) {
    imageError.value = "";
    applyAppearance();
    return;
  }
  try {
    const data = bytes ?? (await invoke<number[] | null>("theme_image_bytes", { name }));
    if (!data || !data.length) {
      imageError.value = "背景图文件不存在，可重新导入或清除";
      applyAppearance();
      return;
    }
    const ext = name.split(".").pop()?.toLowerCase() || "png";
    const mime = ext === "png" ? "image/png" : ext === "webp" ? "image/webp" : "image/jpeg";
    imageUrl.value = URL.createObjectURL(new Blob([new Uint8Array(data)], { type: mime }));
    imageError.value = "";
  } catch (e) {
    imageError.value = `读取背景图失败：${e}`;
  }
  applyAppearance();
}

async function applyThemeData(data: ThemeData): Promise<void> {
  themeReady.value = false;
  Object.assign(config, data.config);
  config.dark = { ...data.config.dark };
  config.light = { ...data.config.light };
  themeDir.value = data.dir;
  themeExists.value = data.exists;
  await loadImage(data.config.image, data.image);
  applyAppearance();
  themeReady.value = true;
}

async function loadTheme(): Promise<void> {
  try {
    await applyThemeData(await invoke<ThemeData>("theme_load"));
    themeError.value = "";
  } catch (e) {
    themeError.value = `读取主题失败：${e}`;
    themeReady.value = true;
    applyAppearance();
  }
}

async function pickBackground(): Promise<void> {
  imageError.value = "";
  imageBusy.value = true;
  try {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (typeof picked !== "string" || !picked) return;
    const name = await invoke<string>("theme_import_image", { path: picked });
    config.image = name;
    await loadImage(name);
    await saveNow();
  } catch (e) {
    imageError.value = String(e);
  } finally {
    imageBusy.value = false;
  }
}

async function clearBackground(): Promise<void> {
  imageError.value = "";
  try {
    await invoke("theme_clear_image");
  } catch {
    /* 文件删不掉不影响把配置清空 */
  }
  config.image = "";
  if (imageUrl.value) URL.revokeObjectURL(imageUrl.value);
  imageUrl.value = "";
  applyAppearance();
  await saveNow();
}

/**
 * 把当前主题保存成一份独立主题包：先落盘，再让用户选择目标文件夹。
 * 目标文件夹必须为空（没有 theme.toml），避免覆盖别人的主题。
 */
async function saveAsTheme(): Promise<string | null> {
  await saveNow();
  const picked = await open({
    directory: true,
    multiple: false,
    title: "选择保存主题的位置",
  });
  if (typeof picked !== "string" || !picked) return null;
  try {
    return await invoke<string>("theme_export", { dir: picked, overwrite: false });
  } catch (e) {
    if (!String(e).includes("EXISTS")) throw e;
    try {
      await ElMessageBox.confirm(
        "这个文件夹里已经有一份主题（theme.toml），覆盖后无法撤销。",
        "覆盖已有主题？",
        {
          confirmButtonText: "覆盖",
          cancelButtonText: "取消",
          type: "warning",
        },
      );
    } catch {
      return null;
    }
    return invoke<string>("theme_export", { dir: picked, overwrite: true });
  }
}

async function openThemeDir(): Promise<void> {
  try {
    await invoke("theme_open_dir");
  } catch (e) {
    themeError.value = String(e);
  }
}

function setImageFit(fit: ThemeConfig["imageFit"]): void {
  config.imageFit = fit;
  applyAppearance();
}

function setImagePosition(axis: "x" | "y", value: number): void {
  if (axis === "x") config.imageX = Math.max(0, Math.min(100, Math.round(value)));
  else config.imageY = Math.max(0, Math.min(100, Math.round(value)));
  applyAppearance();
}

watch(theme, applyAppearance);
watch(
  config,
  () => {
    applyAppearance();
    scheduleSave();
  },
  { deep: true },
);
watch(settingsReady, (ready) => {
  if (ready) applyAppearance();
});

void loadTheme();

export function useAppearance() {
  return {
    config,
    currentPalette,
    seedColor,
    themeDir,
    themeFilePath,
    themeExists,
    themeReady,
    themeError,
    themeSaving,
    imageUrl,
    imageName,
    imageError,
    imageBusy,
    hasImage,
    setSeed,
    setMode,
    setAlgorithm,
    setColor,
    resetMode,
    resetAll,
    pickBackground,
    clearBackground,
    saveAsTheme,
    openThemeDir,
    setImageFit,
    setImagePosition,
    applyAppearance,
    saveNow,
  };
}

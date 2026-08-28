/**
 * 外观偏好（主题 / 面板透明度 / 整体缩放）：
 * localStorage 持久化，通过 CSS 变量（.dark 类、--panel-alpha、--ui-scale）即时生效。
 */

/** 主题类型 */
export type Theme = "light" | "dark";

/** 主题持久化键 */
export const THEME_KEY = "zbar-theme";
/** 面板透明度持久化键 */
export const PANEL_ALPHA_KEY = "zbar-panel-alpha";
/** 面板透明度默认值（与 index.css 中 --panel-alpha 初始值一致） */
export const DEFAULT_PANEL_ALPHA = 0.55;
/** 整体缩放（字体大小）持久化键 */
export const UI_SCALE_KEY = "zbar-ui-scale";
/** 整体缩放默认值（与 index.css 中 --ui-scale 初始值一致） */
export const DEFAULT_UI_SCALE = "1";
/** 整体缩放档位表：设置页选项渲染与 loadUiScale 白名单校验共用同一来源 */
export const UI_SCALE_OPTIONS: {
  value: string;
  labelKey:
    | "settings.fontSmall"
    | "settings.fontStandard"
    | "settings.fontLarge"
    | "settings.fontXl";
}[] = [
  { value: "0.9", labelKey: "settings.fontSmall" },
  { value: "1", labelKey: "settings.fontStandard" },
  { value: "1.1", labelKey: "settings.fontLarge" },
  { value: "1.25", labelKey: "settings.fontXl" },
];

/** 读取主题偏好：仅 "dark" 视为暗色，其余（含无值/损坏值）一律亮色 */
export function loadTheme(): Theme {
  try {
    return localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** 应用主题：切换 <html> 的 .dark 类（纯 DOM，不写盘） */
export function applyTheme(t: Theme): void {
  document.documentElement.classList.toggle("dark", t === "dark");
}

/** 持久化主题偏好 */
export function persistTheme(t: Theme): void {
  try {
    localStorage.setItem(THEME_KEY, t);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
  }
}

/** 切换主题并返回新值（供快捷按钮调用） */
export function toggleTheme(current: Theme): Theme {
  const next: Theme = current === "dark" ? "light" : "dark";
  applyTheme(next);
  persistTheme(next);
  return next;
}

/** 读取面板透明度：非数字或超出 [0.2, 1] 回退默认值 */
export function loadPanelAlpha(): number {
  try {
    const v = parseFloat(localStorage.getItem(PANEL_ALPHA_KEY) ?? "");
    if (isNaN(v) || v < 0.2 || v > 1) return DEFAULT_PANEL_ALPHA;
    return v;
  } catch {
    return DEFAULT_PANEL_ALPHA;
  }
}

/** 应用面板透明度：写 --panel-alpha CSS 变量（纯 DOM，不写盘） */
export function applyPanelAlpha(a: number): void {
  document.documentElement.style.setProperty("--panel-alpha", String(a));
}

/** 持久化面板透明度 */
export function persistPanelAlpha(a: number): void {
  try {
    localStorage.setItem(PANEL_ALPHA_KEY, String(a));
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
  }
}

/** 读取整体缩放：不在档位白名单内（含无值/损坏值）回退默认值 */
export function loadUiScale(): string {
  try {
    const v = localStorage.getItem(UI_SCALE_KEY) ?? DEFAULT_UI_SCALE;
    return UI_SCALE_OPTIONS.some((o) => o.value === v) ? v : DEFAULT_UI_SCALE;
  } catch {
    return DEFAULT_UI_SCALE;
  }
}

/** 应用整体缩放：写 --ui-scale CSS 变量（纯 DOM，不写盘） */
export function applyUiScale(scale: string): void {
  document.documentElement.style.setProperty("--ui-scale", scale);
}

/** 持久化整体缩放 */
export function persistUiScale(scale: string): void {
  try {
    localStorage.setItem(UI_SCALE_KEY, scale);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
  }
}

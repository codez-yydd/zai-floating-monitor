/**
 * 外观偏好（主题 / 面板透明度）：
 * localStorage 持久化，通过 CSS 变量（.dark 类、--panel-alpha）即时生效。
 */

/** 主题类型 */
export type Theme = "light" | "dark";

/** 主题持久化键 */
export const THEME_KEY = "zbar-theme";
/** 面板透明度持久化键 */
export const PANEL_ALPHA_KEY = "zbar-panel-alpha";
/** 面板透明度默认值（与 index.css 中 --panel-alpha 初始值一致） */
export const DEFAULT_PANEL_ALPHA = 0.55;

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

/**
 * 语言偏好（zh / en）：localStorage 持久化，风格对齐 appearance.ts（try-catch 静默降级）。
 */

/** 语言类型 */
export type Locale = "zh" | "en";

/** 语言持久化键 */
export const LOCALE_KEY = "zbar-locale";

/** 检测系统语言：以 zh 开头（zh / zh-CN / zh-TW 等）视为中文，其余一律英文 */
export function detectLocale(): Locale {
  try {
    return (navigator.language || "zh").toLowerCase().startsWith("zh")
      ? "zh"
      : "en";
  } catch {
    return "zh";
  }
}

/** 读取语言偏好：仅接受合法值，无值或损坏值返回 null（由调用方回退 detectLocale） */
export function loadLocale(): Locale | null {
  try {
    const v = localStorage.getItem(LOCALE_KEY);
    return v === "zh" || v === "en" ? v : null;
  } catch {
    return null;
  }
}

/** 持久化语言偏好 */
export function persistLocale(l: Locale): void {
  try {
    localStorage.setItem(LOCALE_KEY, l);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等（对齐 appearance.ts） */
  }
}

/** 应用语言：写 <html> 的 lang 属性（纯 DOM，不写盘） */
export function applyLocale(l: Locale): void {
  document.documentElement.lang = l === "zh" ? "zh-CN" : "en";
}

/** 日期/时间格式化所用 BCP-47 区域（toLocaleTimeString / toLocaleString） */
export function dateLocale(l: Locale): "zh-CN" | "en-US" {
  return l === "zh" ? "zh-CN" : "en-US";
}

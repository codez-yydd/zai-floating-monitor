/**
 * i18n 运行时：Provider + useI18n。
 *  - 扁平点路径词典（zh 为基准类型，en 必须键集一致）
 *  - t(key, vars) 支持 {name} 插值
 *  - setLocale 同步 setState + <html lang> + localStorage 持久化，
 *    并广播 zbar://appearance-changed（Session HUD 等独立窗口同步）
 */
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
} from "react";
import type { ReactNode } from "react";
import { emit } from "@tauri-apps/api/event";
import { APPEARANCE_CHANGED_EVENT } from "../appearance";
import {
  applyLocale,
  detectLocale,
  loadLocale,
  persistLocale,
  type Locale,
} from "./locale";
import { zh } from "./zh";
import { en } from "./en";

/** 插值变量 */
export type Vars = Record<string, string | number>;
/** 词典键（zh 的键集即全集） */
export type MessageKey = keyof typeof zh;
/** 翻译函数 */
export type TFn = (key: MessageKey, vars?: Vars) => string;

interface I18nContextValue {
  locale: Locale;
  t: TFn;
  setLocale: (l: Locale) => void;
}

const DICTS: Record<Locale, Record<MessageKey, string>> = { zh, en };

const Ctx = createContext<I18nContextValue | null>(null);

/** {name} 占位符插值：无对应变量时保留原文，避免丢信息 */
function interpolate(template: string, vars?: Vars): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (raw, name: string) =>
    name in vars ? String(vars[name]) : raw
  );
}

export function I18nProvider({ children }: { children: ReactNode }) {
  // 初值：持久化值优先，无值/损坏值回退系统语言检测
  const [locale, setLocaleState] = useState<Locale>(
    () => loadLocale() ?? detectLocale()
  );

  // 语言切换：同步 React 状态 + <html lang> + localStorage，并广播外观
  // 变更事件通知 Session HUD 等独立窗口同步。调用链仅顶栏/设置页的用户
  // 切换（初始化走 applyLocale，不经此处），无启动噪音；重复值也会广播
  // （幂等：HUD 侧重读 localStorage 对比后不重渲染）。emit 失败静默，
  // 不影响主面板本窗已生效的语言
  const setLocale = useCallback((l: Locale) => {
    setLocaleState(l);
    applyLocale(l);
    persistLocale(l);
    emit(APPEARANCE_CHANGED_EVENT).catch(() => {
      /* 广播失败静默：语言在主面板本窗已生效 */
    });
  }, []);

  const t = useCallback(
    (key: MessageKey, vars?: Vars) => interpolate(DICTS[locale][key], vars),
    [locale]
  );

  const value = useMemo(
    () => ({ locale, t, setLocale }),
    [locale, t, setLocale]
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** 读取 i18n 上下文。必须在 <I18nProvider> 内使用。 */
export function useI18n(): I18nContextValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useI18n must be used within an I18nProvider");
  return v;
}

import type { ReactNode } from "react";
import { useState } from "react";
import {
  loadTheme,
  toggleTheme as flipTheme,
  type Theme,
} from "./appearance";
import { useI18n } from "./i18n";

/* ============================================================
 * 全站统一布局组件 — 所有子页面共用同一套视觉语言
 * ============================================================ */

/** 主题快捷切换（主界面 / 子页面顶栏一键切换亮暗色） */
export function ThemeToggle({ className = "toolbar-btn" }: { className?: string }) {
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const { t } = useI18n();

  return (
    <button
      type="button"
      onClick={() => setTheme((t) => flipTheme(t))}
      className={`${className} ${theme === "dark" ? "text-amber-500!" : ""}`}
      title={theme === "dark" ? t("layout.themeLight") : t("layout.themeDark")}
      aria-label={theme === "dark" ? t("layout.themeLight") : t("layout.themeDark")}
    >
      {theme === "dark" ? (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" className="h-3.5 w-3.5" aria-hidden>
          <circle cx="12" cy="12" r="4" />
          <path strokeLinecap="round" d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
        </svg>
      ) : (
        <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5" aria-hidden>
          <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
        </svg>
      )}
    </button>
  );
}

/** 语言快捷切换（主界面 / 子页面顶栏一键切换中英文）。
 *  与设置页的语言胶囊读写同一 Context，切换后全站即时同步。 */
export function LanguageToggle({ className = "toolbar-btn" }: { className?: string }) {
  const { locale, setLocale, t } = useI18n();

  return (
    <button
      type="button"
      onClick={() => setLocale(locale === "zh" ? "en" : "zh")}
      className={`${className} text-[10px]! font-semibold tracking-wide`}
      title={t("common.switchLanguage")}
      aria-label={t("common.switchLanguage")}
    >
      {locale === "zh" ? "EN" : t("layout.langGlyph")}
    </button>
  );
}

/** 页面根容器 */
export function PageShell({ children }: { children: ReactNode }) {
  return <div className="flex flex-col h-full">{children}</div>;
}

/** 子页面顶部栏：← 返回 + 标题 + 右侧操作 */
export function PageHeader({
  title,
  onBack,
  right,
  subtitle,
}: {
  title: string;
  onBack?: () => void;
  right?: ReactNode;
  subtitle?: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8 shrink-0">
      <div className="flex items-center justify-between">
        {onBack ? (
          <button onClick={onBack} className="btn-ghost text-[11px] px-1 -ml-1">
            {t("layout.back")}
          </button>
        ) : (
          <span className="w-10" />
        )}
        <h1 className="text-[13px] font-bold text-slate-900/90 tracking-tight">
          {title}
        </h1>
        <div className="min-w-[2.5rem] flex justify-end items-center gap-0.5">
          <ThemeToggle />
          <LanguageToggle />
          {right}
        </div>
      </div>
      {subtitle && <div className="mt-2">{subtitle}</div>}
    </div>
  );
}

/** 可滚动内容区 */
export function PageBody({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`flex-1 overflow-y-auto px-3 py-2.5 ${className}`}>
      {children}
    </div>
  );
}

/** 底部操作栏 */
export function PageFooter({ children }: { children: ReactNode }) {
  return (
    <div className="px-3 py-1.5 border-t border-slate-900/8 flex items-center justify-between gap-2 shrink-0 text-[10px]">
      {children}
    </div>
  );
}

/** 设置/表单区块卡片 */
export function SettingsCard({
  title,
  action,
  hint,
  children,
}: {
  title: string;
  action?: ReactNode;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="card-base rounded-2xl p-3">
      <div className="flex items-center justify-between mb-2">
        <span className="section-title">{title}</span>
        {action}
      </div>
      {hint && (
        <p className="text-[9px] text-slate-500 leading-relaxed mb-2">{hint}</p>
      )}
      {children}
    </div>
  );
}

/** 内容区块卡片（带可选标题） */
export function SectionCard({
  title,
  action,
  children,
  className = "",
}: {
  title?: string;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`card-base rounded-2xl px-3 py-2.5 ${className}`}>
      {title && (
        <div className="flex items-center justify-between mb-2">
          <span className="section-title">{title}</span>
          {action}
        </div>
      )}
      {children}
    </div>
  );
}

/** 英雄指标卡：大数字 + 副指标（参考 DeepSeek 余额卡） */
export function HeroMetric({
  label,
  value,
  accent = "sky",
  badge,
  footer,
}: {
  label: string;
  value: string;
  accent?: "sky" | "emerald" | "orange" | "violet";
  badge?: ReactNode;
  footer?: ReactNode;
}) {
  const accentMap = {
    sky: "hero-sky",
    emerald: "hero-emerald",
    orange: "hero-orange",
    violet: "hero-violet",
  };
  const valueColor = {
    sky: "text-sky-600",
    emerald: "text-emerald-600",
    orange: "text-orange-600",
    violet: "text-violet-600",
  };
  return (
    <div className={`${accentMap[accent]} rounded-2xl px-3.5 py-3`}>
      <div className="flex items-center justify-between mb-1.5">
        <span className="section-title">{label}</span>
        {badge}
      </div>
      <div className={`num text-[26px] font-bold leading-none tracking-tight ${valueColor[accent]}`}>
        {value}
      </div>
      {footer && (
        <div className="mt-2.5 pt-2 border-t border-current/8 opacity-80">
          {footer}
        </div>
      )}
    </div>
  );
}

/** 胶囊切换组 */
export function PillGroup({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`flex gap-1 p-0.5 rounded-xl bg-slate-900/4 ${className}`}>
      {children}
    </div>
  );
}

export function PillButton({
  active,
  onClick,
  children,
  className = "",
  disabled,
}: {
  active: boolean;
  onClick?: () => void;
  children: ReactNode;
  className?: string;
  disabled?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`flex-1 px-2 py-1 rounded-lg text-[10px] font-medium transition-all duration-150 disabled:opacity-40 ${
        active
          ? "bg-sky-500 text-white shadow-sm"
          : "text-slate-600/70 hover:text-slate-800 hover:bg-slate-900/5"
      } ${className}`}
    >
      {children}
    </button>
  );
}

/** 排序/筛选切换（小尺寸） */
export function SortToggle<T extends string>({
  options,
  value,
  onChange,
  accent = "sky",
}: {
  options: { key: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
  accent?: "sky" | "emerald" | "orange" | "violet";
}) {
  const activeClass = {
    sky: "bg-sky-500/15 text-sky-700 font-medium",
    emerald: "bg-emerald-500/15 text-emerald-700 font-medium",
    orange: "bg-orange-500/15 text-orange-700 font-medium",
    violet: "bg-violet-500/15 text-violet-700 font-medium",
  };
  return (
    <div className="flex gap-0.5 p-0.5 rounded-lg bg-slate-900/4">
      {options.map((opt) => (
        <button
          key={opt.key}
          onClick={() => onChange(opt.key)}
          className={`px-1.5 py-0.5 rounded-md text-[9px] transition-all duration-150 ${
            value === opt.key
              ? activeClass[accent]
              : "text-slate-500 hover:text-slate-700"
          }`}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

/** 按钮变体 */
export function BtnPrimary({
  children,
  onClick,
  disabled,
  className = "",
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`btn-primary ${className}`}
    >
      {children}
    </button>
  );
}

export function BtnSecondary({
  children,
  onClick,
  disabled,
  className = "",
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`btn-secondary ${className}`}
    >
      {children}
    </button>
  );
}

/** 提示横幅 */
export function AlertBanner({
  type = "error",
  children,
}: {
  type?: "error" | "warning" | "info" | "success";
  children: ReactNode;
}) {
  const styles = {
    error: "bg-red-500/12 text-red-700 border-red-500/20",
    warning: "bg-amber-500/12 text-amber-800/90 border-amber-500/20",
    info: "bg-sky-500/10 text-sky-700/90 border-sky-500/20",
    success: "bg-emerald-500/10 text-emerald-700/90 border-emerald-500/20",
  };
  return (
    <div
      className={`mx-0 mb-2 px-2.5 py-1.5 rounded-xl border text-[11px] leading-relaxed ${styles[type]}`}
    >
      {children}
    </div>
  );
}

/** 空状态 */
export function EmptyState({
  title,
  hint,
  action,
}: {
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center px-6 text-center gap-2 py-10">
      <div className="w-10 h-10 rounded-2xl bg-slate-900/5 flex items-center justify-center mb-1">
        <div className="w-4 h-4 rounded-full bg-slate-400/30" />
      </div>
      <div className="text-xs text-slate-700/70 font-medium">{title}</div>
      {hint && (
        <div className="text-[10px] text-slate-500 leading-relaxed whitespace-pre-line max-w-[220px]">
          {hint}
        </div>
      )}
      {action}
    </div>
  );
}

/** 加载占位（text 缺省取词典「加载中…」） */
export function LoadingState({ text }: { text?: string }) {
  const { t } = useI18n();
  return (
    <div className="flex-1 flex items-center justify-center text-xs text-slate-500 py-10">
      {text ?? t("common.loading")}
    </div>
  );
}

/** 表单输入框 */
export function FormInput({
  label,
  children,
}: {
  label?: string;
  children: ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1 text-[10px]">
      {label && <span className="text-slate-600">{label}</span>}
      {children}
    </label>
  );
}

export function InputBox({
  className = "",
  ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...props}
      className={`input-box ${className}`}
    />
  );
}

/** 双栏副指标（英雄卡底部用） */
export function MetricPair({
  left,
  right,
}: {
  left: { label: string; value: string };
  right: { label: string; value: ReactNode };
}) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <div>
        <div className="text-[9px] text-slate-500">{left.label}</div>
        <div className="num text-[13px] font-semibold text-slate-800 mt-0.5">
          {left.value}
        </div>
      </div>
      <div>
        <div className="text-[9px] text-slate-500">{right.label}</div>
        <div className="mt-1">{right.value}</div>
      </div>
    </div>
  );
}

/** 状态徽章 */
export function StatusBadge({
  children,
  color = "emerald",
}: {
  children: ReactNode;
  color?: "emerald" | "sky" | "amber" | "violet";
}) {
  const colors = {
    emerald: "bg-emerald-500/12 text-emerald-700",
    sky: "bg-sky-500/12 text-sky-700",
    amber: "bg-amber-500/12 text-amber-700",
    violet: "bg-violet-500/12 text-violet-700",
  };
  return (
    <span
      className={`px-1.5 py-0.5 rounded-full text-[9px] font-medium ${colors[color]}`}
    >
      {children}
    </span>
  );
}

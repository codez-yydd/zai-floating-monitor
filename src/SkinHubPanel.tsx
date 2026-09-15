/**
 * 皮肤分类页：统计页工具栏「皮肤」入口的落地页，两张入口卡片分别进入
 * 免注入子页（SkinStandalonePanel：会话悬浮窗 + 桌面宠物）与需注入子页
 * （ThemePanel：ZCode 动态壁纸）。
 *
 * 纯导航页：不发起任何后端调用（尤其不做注入状态检测），把扫描 ZCode
 * 安装目录的开销留给用户显式进入需注入子页时才发生。
 */
import type { ReactNode } from "react";
import { PageBody, PageHeader, PageShell } from "./layout";
import { useI18n } from "./i18n";

interface Props {
  onBack: () => void;
  /** 进入免注入子页（会话悬浮窗 + 桌面宠物） */
  onOpenStandalone: () => void;
  /** 进入需注入子页（ZCode 动态壁纸） */
  onOpenInjected: () => void;
}

export function SkinHubPanel({
  onBack,
  onOpenStandalone,
  onOpenInjected,
}: Props) {
  const { t } = useI18n();
  return (
    <PageShell>
      <PageHeader title={t("theme.title")} onBack={onBack} />
      <PageBody className="page-stack">
        <EntryCard
          icon={
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="h-5 w-5"
              aria-hidden
            >
              {/* 独立窗口：窗体 + 标题栏 + 两个窗口控制点 */}
              <rect x="3" y="4" width="18" height="16" rx="2" />
              <path d="M3 9h18" />
              <circle cx="6" cy="6.5" r=".5" fill="currentColor" />
              <circle cx="8.5" cy="6.5" r=".5" fill="currentColor" />
            </svg>
          }
          title={t("theme.sectionStandalone")}
          desc={t("theme.hubStandaloneDesc")}
          onClick={onOpenStandalone}
        />
        <EntryCard
          icon={
            // 调色板：与统计页工具栏「皮肤」按钮同款图形（放大版）
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="h-5 w-5"
              aria-hidden
            >
              <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
              <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
              <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
              <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
              <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z" />
            </svg>
          }
          title={t("theme.sectionInjected")}
          desc={t("theme.hubInjectedDesc")}
          onClick={onOpenInjected}
        />
      </PageBody>
    </PageShell>
  );
}

/** 入口卡：图标 + 标题 + 一行说明 + 右侧箭头（风格对齐设置卡/菜单行） */
function EntryCard({
  icon,
  title,
  desc,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  desc: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="card-base rounded-2xl p-3 w-full text-left flex items-center gap-2.5 hover:border-slate-900/25 transition-colors group cursor-pointer active:scale-[0.98]"
    >
      <span className="shrink-0 w-9 h-9 rounded-lg bg-sky-500/10 text-sky-600 flex items-center justify-center">
        {icon}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-[11px] font-semibold text-slate-900">
          {title}
        </span>
        <span className="block text-[9px] text-slate-500 leading-relaxed mt-0.5">
          {desc}
        </span>
      </span>
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="h-3.5 w-3.5 shrink-0 text-slate-400 group-hover:text-slate-600 transition-colors"
        aria-hidden
      >
        <path d="m9 18 6-6-6-6" />
      </svg>
    </button>
  );
}

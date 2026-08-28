import { useEffect, useMemo, useState } from "react";
import type { Currency, PricingConfig } from "./types";
import type { AgentVisibility } from "./agentVisibility";
import { LanguageToggle, PageShell, ThemeToggle } from "./layout";
import { CompareContent } from "./ComparePanel";
import { ReportContent } from "./ReportPanel";
import { useI18n } from "./i18n";

/** 报表页标签：周额度对比 / 用量报告 */
type ReportsTab = "compare" | "report";

interface Props {
  onBack: () => void;
  pricing: PricingConfig;
  currency: Currency;
  agentVisibility: AgentVisibility;
}

/** 读取记忆的标签，非法值回落「周额度对比」（对齐 StatsPanel 的 loadStatsTab）。 */
function loadReportsTab(): ReportsTab {
  try {
    const saved = localStorage.getItem("zbar-reports-tab");
    if (saved === "compare" || saved === "report") return saved;
  } catch {
    // 存储不可用时使用默认标签，不阻断报表页启动。
  }
  return "compare";
}

/**
 * 报表页 —— 「周额度对比 / 用量报告」合并壳。
 *
 * 顶部统一提供返回与标签切换（含主题/语言快捷切换，承接原各页 PageHeader 的
 * 外壳职责），标签内容分别复用 CompareContent / ReportContent；两个内容组件
 * 的数据拉取、图表与导出逻辑保持独立（见各自文件），壳只负责导航与记忆标签。
 */
export function ReportsPanel({
  onBack,
  pricing,
  currency,
  agentVisibility,
}: Props) {
  const { t } = useI18n();
  const [tab, setTab] = useState<ReportsTab>(() => loadReportsTab());

  // 记忆当前标签（localStorage 异常静默，对齐 cache.ts：记忆仅锦上添花，不影响主流程）
  useEffect(() => {
    try {
      localStorage.setItem("zbar-reports-tab", tab);
    } catch {
      /* 忽略：QuotaExceededError、隐私模式等 */
    }
  }, [tab]);

  // 标签文案复用两页标题词典，语言切换时随 t 重建
  const tabs = useMemo<{ id: ReportsTab; label: string }[]>(
    () => [
      { id: "compare", label: t("compare.title") },
      { id: "report", label: t("report.title") },
    ],
    [t]
  );

  return (
    <PageShell>
      {/* 顶栏：返回 + 标签切换 + 主题/语言，样式沿用统计页 statTabs 的胶囊切换 */}
      <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8 shrink-0">
        <div className="flex items-center justify-between gap-2">
          <button
            onClick={onBack}
            className="btn-ghost text-[11px] px-1 -ml-1 shrink-0"
          >
            {t("layout.back")}
          </button>
          <div className="flex gap-1 p-0.5 rounded-xl bg-slate-900/4">
            {tabs.map((item) => (
              <button
                key={item.id}
                onClick={() => setTab(item.id)}
                type="button"
                aria-pressed={tab === item.id}
                className={`whitespace-nowrap rounded-lg px-2.5 py-1 text-[10px] font-medium transition-all duration-150 ${
                  tab === item.id
                    ? "bg-sky-500/15 text-sky-700 shadow-sm"
                    : "text-slate-600/60 hover:text-slate-800 hover:bg-slate-900/4"
                }`}
              >
                {item.label}
              </button>
            ))}
          </div>
          <div className="flex items-center gap-0.5 shrink-0">
            <ThemeToggle />
            <LanguageToggle />
          </div>
        </div>
      </div>

      {tab === "compare" ? (
        <CompareContent agentVisibility={agentVisibility} />
      ) : (
        <ReportContent
          pricing={pricing}
          currency={currency}
          agentVisibility={agentVisibility}
        />
      )}
    </PageShell>
  );
}

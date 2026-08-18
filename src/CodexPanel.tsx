import type { CodexSnapshot, Currency, PricingConfig, TrendBucket } from "./types";
import { AgentUsagePanel } from "./AgentUsagePanel";
import { useI18n } from "./i18n";

interface Props {
  snapshot: CodexSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率：人民币花费 = 美元 × 汇率（价格只存美元） */
  fxRate: number;
  trendBucket: TrendBucket;
  /** 按模型折算花费用（与 Z.ai 页同款前端自算） */
  pricing: PricingConfig;
}

/** Codex 用量面板：通用 AgentUsagePanel 的 OpenAI 品牌皮肤（emerald）。 */
export function CodexPanel(props: Props) {
  const { t } = useI18n();
  return (
    <AgentUsagePanel
      {...props}
      theme={{
        rowBar: "bg-emerald-500/10",
        sortSelected: "bg-emerald-500/20 text-emerald-700",
        badge: "bg-emerald-500/15 text-emerald-700",
        accent: "emerald",
      }}
      empty={{
        name: "Codex",
        icon: "⌨️",
        title: t("stats.codexNotFound"),
        hint: t("stats.codexNotFoundHint"),
      }}
      cacheRateMode="included"
    />
  );
}

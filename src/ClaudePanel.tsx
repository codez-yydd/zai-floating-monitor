import type {
  AgentQuotaDelta,
  ClaudeSnapshot,
  Currency,
  PricingConfig,
  TrendBucket,
} from "./types";
import { AgentUsagePanel } from "./AgentUsagePanel";
import { useI18n } from "./i18n";

interface Props {
  snapshot: ClaudeSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率：人民币花费 = 美元 × 汇率（价格只存美元） */
  fxRate: number;
  trendBucket: TrendBucket;
  /** 按模型折算花费用（与 Z.ai 页同款前端自算） */
  pricing: PricingConfig;
  agentQuotaDelta?: AgentQuotaDelta;
}

/** Claude 用量面板：通用 AgentUsagePanel 的 Anthropic 品牌皮肤（orange）。 */
export function ClaudePanel(props: Props) {
  const { t } = useI18n();
  return (
    <AgentUsagePanel
      {...props}
      theme={{
        rowBar: "bg-orange-500/10",
        sortSelected: "bg-orange-500/20 text-orange-700",
        badge: "bg-orange-500/15 text-orange-700",
        accent: "orange",
      }}
      empty={{
        name: "Claude",
        icon: "🤖",
        title: t("stats.claudeNotFound"),
        hint: t("stats.claudeNotFoundHint"),
      }}
      cacheRateMode="separate"
    />
  );
}

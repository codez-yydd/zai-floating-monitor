import type { ClaudeSnapshot, Currency, PricingConfig, TrendBucket } from "./types";
import { AgentUsagePanel } from "./AgentUsagePanel";

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
}

/** Claude 用量面板：通用 AgentUsagePanel 的 Anthropic 品牌皮肤（orange）。 */
export function ClaudePanel(props: Props) {
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
        icon: "🤖",
        title: "未检测到 Claude Code",
        hint: "请安装并使用 Anthropic Claude Code 产生本地会话记录\n（~/.claude/projects）后再查看",
      }}
      cacheRateMode="separate"
    />
  );
}

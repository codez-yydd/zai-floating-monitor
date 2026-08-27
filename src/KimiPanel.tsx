import type {
  AgentQuotaDelta,
  Currency,
  KimiSnapshot,
  PricingConfig,
  TrendBucket,
} from "./types";
import { AgentUsagePanel } from "./AgentUsagePanel";
import { SectionCard } from "./layout";
import { useI18n } from "./i18n";

interface Props {
  snapshot: KimiSnapshot | null;
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

/** Kimi 用量面板：通用 AgentUsagePanel 的 indigo 品牌皮肤，
 *  外层附加加油包余额小节（接口带 boosterWallet 时才有值）。 */
export function KimiPanel(props: Props) {
  const { t } = useI18n();
  const { snapshot } = props;
  const booster = snapshot?.rate_limits;
  return (
    <>
      {/* 加油包余额：与下方"额度"卡语义相邻，固定在滚动区上方常显 */}
      {booster?.booster_balance != null && (
        <div className="px-3 pt-2.5 shrink-0">
          <SectionCard title={t("stats.boosterBalance")}>
            <div className="flex items-baseline gap-3">
              <span className="num text-[15px] font-bold text-indigo-700">
                ¥{booster.booster_balance.toFixed(2)}
              </span>
              {booster.booster_monthly_used != null && (
                <span className="num text-[10px] text-slate-500">
                  {t("stats.boosterMonthlyUsed", {
                    amount: booster.booster_monthly_used.toFixed(2),
                  })}
                </span>
              )}
            </div>
          </SectionCard>
        </div>
      )}
      {/* 额度获取失败：统计仍正常展示，仅透出失败原因（与 booster 块同位互斥） */}
      {snapshot && !snapshot.rate_limits && snapshot.rate_limits_error && (
        <div className="px-3 pt-2.5 shrink-0">
          <p
            className="text-[9px] text-amber-700/90 leading-relaxed break-all"
            title={snapshot.rate_limits_error}
          >
            ⚠ {t("quota.quotaFail")}：{snapshot.rate_limits_error}
          </p>
        </div>
      )}
      <AgentUsagePanel
        {...props}
        theme={{
          rowBar: "bg-indigo-500/10",
          sortSelected: "bg-indigo-500/20 text-indigo-700",
          badge: "bg-indigo-500/15 text-indigo-700",
          accent: "indigo",
        }}
        empty={{
          name: "Kimi",
          icon: "🌙",
          title: t("stats.kimiNotFound"),
          hint: t("stats.kimiNotFoundHint"),
        }}
        cacheRateMode="separate"
      />
    </>
  );
}

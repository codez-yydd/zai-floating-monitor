import { useEffect, useState } from "react";
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
import { useDataCache, PROVIDER_QUOTA_STALE_MS } from "./DataCache";
import { CredentialsCard } from "./CredentialsCard";
import { QuotaEntryCard } from "./QuotaEntryCard";
import { useResetDisplay } from "./resetDisplay";

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
      <KimiOtherAccounts />
    </>
  );
}

/**
 * Kimi「其他账号」区：凭证体系 ~/.zbar/credentials/kimi.json 条目
 * （OAuth 网页登录保存的 refresh_token / 手动粘贴的 API Key），每条凭证
 * 按其 region 调对应域名组的 usages 端点展示订阅额度
 * （后端 get_provider_quota("kimi")）。结构与 ClaudeOtherAccounts 同构：
 * - 本地 CLI 登录态不在此展示（上方 AgentUsagePanel 额度卡），避免双查询；
 * - 有凭证时默认展开（凭证卡 + 每账号一张配额卡）；无凭证时收起为单行
 *   入口（保持单账号用户界面基本不变，同时保留添加第一条凭证的入口）；
 * - 刷新时机：挂载补刷一轮 + 凭证增删改事件（DataCache 联动）+ 手动刷新。
 */
function KimiOtherAccounts() {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const { credentials, refreshCredentials, providerQuota, refreshProviderQuota } =
    useDataCache();
  const entries = credentials["kimi"];
  const hasCreds = (entries?.length ?? 0) > 0;
  // 展开态：用户显式操作优先，否则有凭证自动展开、无凭证收起
  const [expandedOverride, setExpandedOverride] = useState<boolean | null>(null);
  const open = expandedOverride ?? hasCreds;
  const cacheEntry = providerQuota["kimi"];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;

  // 凭证列表为按需缓存（无轮询）：首挂载加载一次
  const entriesLoaded = entries !== undefined;
  useEffect(() => {
    if (!entriesLoaded) refreshCredentials("kimi").catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entriesLoaded]);

  // 有凭证但额度缓存缺失或已老化（>120s）→ 立即补刷一轮。只判「无缓存」
  // 不够：冷启动会从 localStorage 恢复上次的 providerQuota 缓存，无缓存
  // 条件恒 false，额度直到下个 120s 轮询都不刷新；老化阈值与轮询同频
  useEffect(() => {
    if (!hasCreds) return;
    const cached = providerQuota["kimi"];
    if (!cached || Date.now() - cached.ts > PROVIDER_QUOTA_STALE_MS) {
      void refreshProviderQuota("kimi");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasCreds]);

  // 倒计时显示用时钟（有窗口带重置时间才有意义，空数据不启定时器）
  const hasResets = quotaEntries.some((e) =>
    e.windows.some((w) => w.resetsAt != null)
  );
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!hasResets) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [hasResets]);

  // 凭证列表未加载完成不渲染（防闪：加载为本地文件读，瞬时完成）
  if (!entriesLoaded) return null;

  return (
    <div className="shrink-0 max-h-[45%] overflow-y-auto overscroll-contain border-t border-slate-900/8 mx-3 pb-2.5">
      {/* 区块头：标题 + 计数 + 手动刷新 + 展开收起 */}
      <div className="flex items-center justify-between gap-2 pt-1.5 pb-1">
        <button
          onClick={() => setExpandedOverride(!open)}
          className="flex items-center gap-1 min-w-0 text-[10px] text-slate-500 hover:text-slate-700 transition-colors"
        >
          <span
            className={`inline-block transition-transform ${open ? "rotate-90" : ""}`}
          >
            ›
          </span>
          <span className="truncate">{t("stats.kimiOtherAccounts")}</span>
          {hasCreds && (
            <span className="num text-[9px] text-slate-500/70 shrink-0">
              {t("credentials.countBadge", { n: entries!.length })}
            </span>
          )}
        </button>
        <div className="flex items-center gap-1 shrink-0">
          {hasCreds && (
            <button
              onClick={() => void refreshProviderQuota("kimi")}
              disabled={refreshing}
              className={`toolbar-btn shrink-0 ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
          )}
        </div>
      </div>

      {open && (
        <>
          <CredentialsCard
            provider="kimi"
            kind="token"
            guideKey="credentials.guide.kimi"
            brand="kimi"
            regionOptions={[
              { value: "cn", label: t("credentials.regionCn") },
              { value: "global", label: t("credentials.regionGlobal") },
            ]}
          />

          {/* 额度查询中提示（有凭证但条目未返回；空 entries 时收敛展示） */}
          {quotaEntries.length === 0 && refreshing && (
            <div className="card-base rounded-2xl px-3 py-2 mt-1.5">
              <p className="text-[10px] text-slate-500 leading-relaxed">
                {t("credentials.quotaRefreshing")}
              </p>
            </div>
          )}

          {/* 每条手动凭证一张配额卡（与 GenericQuotaPanel 共用渲染） */}
          {quotaEntries.map((entry) => (
            <div key={entry.credentialId} className="mt-1.5 first:mt-0">
              <QuotaEntryCard
                entry={entry}
                accent="#4338ca"
                resetDisplay={resetDisplay}
                now={now}
              />
            </div>
          ))}
        </>
      )}
    </div>
  );
}

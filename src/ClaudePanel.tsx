import { useEffect, useState } from "react";
import type {
  AgentQuotaDelta,
  ClaudeSnapshot,
  Currency,
  PricingConfig,
  TrendBucket,
} from "./types";
import { AgentUsagePanel } from "./AgentUsagePanel";
import { useI18n } from "./i18n";
import { useDataCache, PROVIDER_QUOTA_STALE_MS } from "./DataCache";
import { CredentialsCard } from "./CredentialsCard";
import { QuotaEntryCard } from "./QuotaEntryCard";
import { useResetDisplay } from "./resetDisplay";

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

/** Claude 用量面板：通用 AgentUsagePanel 的 Anthropic 品牌皮肤（orange），
 *  底部附带「其他账号」区（手动凭证 kind=token 的多账号订阅额度）。
 *  无手动凭证时该区收起为单行入口，额度/用量主链路与原先完全一致。 */
export function ClaudePanel(props: Props) {
  const { t } = useI18n();
  return (
    <div className="flex-1 min-h-0 flex flex-col">
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
      <ClaudeOtherAccounts />
    </div>
  );
}

/**
 * Claude「其他账号」区：凭证体系 ~/.zbar/credentials/claude.json 的
 * kind=token 条目（sk-ant-oat OAuth access token），每条凭证调同一
 * OAuth usage 端点展示订阅额度（后端 get_provider_quota("claude")）。
 * - 本地登录态不在此展示（上方 AgentUsagePanel 额度卡），避免双查询；
 * - 有凭证时默认展开（凭证卡 + 每账号一张配额卡）；无凭证时收起为单行
 *   入口（保持单账号用户界面基本不变，同时保留添加第一条凭证的入口）；
 * - 刷新时机：挂载补刷一轮 + 凭证增删改事件（DataCache 联动）+ 手动刷新。
 */
function ClaudeOtherAccounts() {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const { credentials, refreshCredentials, providerQuota, refreshProviderQuota } =
    useDataCache();
  const entries = credentials["claude"];
  const hasCreds = (entries?.length ?? 0) > 0;
  // 展开态：用户显式操作优先，否则有凭证自动展开、无凭证收起
  const [expandedOverride, setExpandedOverride] = useState<boolean | null>(null);
  const open = expandedOverride ?? hasCreds;
  const cacheEntry = providerQuota["claude"];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;

  // 凭证列表为按需缓存（无轮询）：首挂载加载一次
  const entriesLoaded = entries !== undefined;
  useEffect(() => {
    if (!entriesLoaded) refreshCredentials("claude").catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entriesLoaded]);

  // 有凭证但额度缓存缺失或已老化（>120s）→ 立即补刷一轮。只判「无缓存」
  // 不够：冷启动会从 localStorage 恢复上次的 providerQuota 缓存，无缓存
  // 条件恒 false，额度直到下个 120s 轮询都不刷新；老化阈值与轮询同频
  useEffect(() => {
    if (!hasCreds) return;
    const cached = providerQuota["claude"];
    if (!cached || Date.now() - cached.ts > PROVIDER_QUOTA_STALE_MS) {
      void refreshProviderQuota("claude");
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
          <span className="truncate">{t("stats.claudeOtherAccounts")}</span>
          {hasCreds && (
            <span className="num text-[9px] text-slate-500/70 shrink-0">
              {t("credentials.countBadge", { n: entries!.length })}
            </span>
          )}
        </button>
        <div className="flex items-center gap-1 shrink-0">
          {hasCreds && (
            <button
              onClick={() => void refreshProviderQuota("claude")}
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
            provider="claude"
            kind="token"
            guideKey="credentials.guide.claude"
            brand="claude"
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
                accent="#d97757"
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

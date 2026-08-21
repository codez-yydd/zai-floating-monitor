import { useEffect, useState } from "react";
import type {
  AgentQuotaDelta,
  Currency,
  PricingConfig,
  Stats,
  TrendBucket,
  TrendPoint,
} from "./types";
import { formatCost, formatCountdownCore, formatPct, formatTokens, formatTps } from "./format";
import { modelCost } from "./merge";
import {
  CurrentModelBar,
  DetailRow,
  Metric,
  ProgressBar,
  SpeedMetricsGrid,
  TrendChart,
  remainingGradient,
  remainingTextColor,
} from "./widgets";
import {
  HeroMetric,
  MetricPair,
  SectionCard,
  SortToggle,
  EmptyState,
  LoadingState,
} from "./layout";
import { useI18n } from "./i18n";

/** 通用快照形状：Codex / Claude 的快照结构一致（stats/trend 同构 +
 *  同款五字段速率限制），TS 结构化类型直接兼容。 */
export interface AgentUsageSnapshot {
  stats: Stats;
  trend: TrendPoint[];
  rate_limits: {
    plan_type: string | null;
    primary_pct: number | null;
    primary_reset_at: number | null;
    secondary_pct: number | null;
    secondary_reset_at: number | null;
  } | null;
}

/** 品牌主题（Tailwind 类名）：Codex 用 emerald、Claude 用 orange。 */
export interface AgentPanelTheme {
  /** 模型排行行底条 */
  rowBar: string;
  /** 排序按钮选中态（保留兼容，SortToggle 用 accent） */
  sortSelected: string;
  /** 套餐徽标 */
  badge: string;
  /** 英雄卡 accent */
  accent: "sky" | "emerald" | "orange" | "violet";
}

/** 无数据空态文案（未安装对应 CLI 时展示）。
 *  title/hint 由品牌皮肤组件查词典传入；name 为品牌名（不进词典，加载文案插值用）。 */
export interface AgentEmptyState {
  name: string;
  icon: string;
  title: string;
  hint: string;
}

interface Props {
  snapshot: AgentUsageSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率：人民币花费 = 美元 × 汇率（价格只存美元） */
  fxRate: number;
  trendBucket: TrendBucket;
  /** 按模型折算花费用（与 Z.ai 页同款前端自算） */
  pricing: PricingConfig;
  theme: AgentPanelTheme;
  empty: AgentEmptyState;
  /** 缓存率口径：input 含缓存读（zcode/Codex/OpenAI 口径）还是三分并列
   *  （Anthropic 口径：input 不含 cache_read/cache_creation，需用输入侧总量
   *  做分母，否则比率会远超 100%） */
  cacheRateMode: "included" | "separate";
  /** 周额度今日增量；仅周窗口显示。 */
  agentQuotaDelta?: AgentQuotaDelta;
}

/** 速率限制一行：标签 + 重置倒计时 + 剩余% 在上，剩余进度条在下
 *  （与汇总页 QuotaMiniRow / Cursor 页 PlanQuotaRow 同款视觉） */
function AgentQuotaRow({
  label,
  usedPct,
  resetAt,
  now,
  delta,
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
  delta?: AgentQuotaDelta;
}) {
  const { t } = useI18n();
  const remain = Math.max(0, 100 - usedPct);
  const showReset = resetAt != null && resetAt > now;
  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-0.5">
        <span className="text-[10px] text-slate-600 truncate">
          {label}
          {showReset && (
            <span className="ml-1 text-[8px] text-slate-400 num">
              ↻ {formatCountdownCore(resetAt - now)}
            </span>
          )}
        </span>
        <span
          className="num text-[10px] font-semibold shrink-0 whitespace-nowrap"
          style={{ color: remainingTextColor(remain) }}
        >
          {t("common.remaining", { pct: Math.round(remain) })}
        </span>
      </div>
      <ProgressBar
        pct={remain / 100}
        height="h-1.5"
        gradient={remainingGradient(remain)}
      />
      {delta && delta.samples >= 2 && delta.pct > 0 && (
        <div className="text-[9px] mt-0.5 num text-slate-700/50">
          {t("quota.todayDelta", { pct: Math.round(delta.pct) })}
        </div>
      )}
    </div>
  );
}

/** 单 CLI Agent 的用量面板（Codex / Claude 共用）：
 *  订阅额度块（无数据不渲染）→ 总花费/Token → 趋势图 → 三指标 →
 *  明细条 → 按模型排行。品牌差异只体现在 theme/empty 两个 props。 */
export function AgentUsagePanel({
  snapshot,
  loading,
  error,
  currency,
  fxRate,
  trendBucket,
  pricing,
  theme,
  empty,
  cacheRateMode,
  agentQuotaDelta,
}: Props) {
  const { t } = useI18n();
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");
  const [sortBy, setSortBy] = useState<"cost" | "token" | "requests">("cost");

  // 重置倒计时需要秒级跳动（与汇总页同款 1s tick）
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);

  // 加载中 & 无数据
  if (loading && !snapshot) {
    return (
      <LoadingState text={t("common.loadingUsage", { name: empty.name })} />
    );
  }

  // 未安装 / 无会话目录 / 其他错误且无数据
  if (!snapshot) {
    return (
      <EmptyState
        title={empty.title}
        hint={empty.hint}
        action={
          error ? (
            <div className="text-[10px] text-red-600/70 mt-1">{error}</div>
          ) : undefined
        }
      />
    );
  }

  const stats = snapshot.stats;
  const rate = snapshot.rate_limits;

  // 总花费：对趋势桶求和（与后端口径一致，双货币各自累加）
  const totalCost = snapshot.trend.reduce(
    (s, p) => s + (currency === "cny" ? p.cost_cny : p.cost_usd),
    0
  );

  // 缓存率：included 口径 input_tokens 已含缓存读（分母=input）；
  // separate 口径（Anthropic）三项并列，分母=输入侧总量
  const inputSideTotal =
    stats.overall.input_tokens +
    stats.overall.cache_read_tokens +
    stats.overall.cache_write_tokens;
  const cacheRate =
    cacheRateMode === "separate"
      ? inputSideTotal > 0
        ? stats.overall.cache_read_tokens / inputSideTotal
        : 0
      : stats.overall.input_tokens > 0
        ? stats.overall.cache_read_tokens / stats.overall.input_tokens
        : 0;

  // 额度块渲染条件：至少一行有百分比数据（中转模式 rate_limits 为 null，不渲染）
  const hasRateRow = rate && (rate.primary_pct != null || rate.secondary_pct != null);

  return (
    <div className="flex-1 overflow-y-auto px-3 py-2.5 page-stack">
      {/* 速率限制 */}
      {hasRateRow && rate && (
        <SectionCard
          title={t("stats.rateLimits")}
          action={
            rate.plan_type ? (
              <span className={`shrink-0 px-1.5 py-0.5 rounded text-[9px] font-medium capitalize ${theme.badge}`}>
                {rate.plan_type}
              </span>
            ) : undefined
          }
        >
          <div className="space-y-2">
            {rate.primary_pct != null && (
              <AgentQuotaRow label={t("common.hour5")} usedPct={rate.primary_pct} resetAt={rate.primary_reset_at} now={now} />
            )}
            {rate.secondary_pct != null && (
              <AgentQuotaRow
                label={t("common.weekly")}
                usedPct={rate.secondary_pct}
                resetAt={rate.secondary_reset_at}
                now={now}
                delta={agentQuotaDelta}
              />
            )}
          </div>
        </SectionCard>
      )}

      <HeroMetric
        label={t("common.totalCost")}
        value={formatCost(totalCost, currency)}
        accent={theme.accent}
        footer={
          <MetricPair
            left={{ label: t("common.totalTokens"), value: formatTokens(stats.overall.total_tokens) }}
            right={{ label: t("common.cacheRate"), value: formatPct(cacheRate) }}
          />
        }
      />

      <CurrentModelBar model={snapshot.stats.current_model} />

      {snapshot.trend.length > 0 && (
        <TrendChart
          points={snapshot.trend}
          bucket={trendBucket}
          currency={currency}
          metric={trendMetric}
          onMetricChange={setTrendMetric}
        />
      )}

      <div className="grid grid-cols-3 gap-1.5">
        <Metric label={t("common.requests")} value={String(stats.overall.requests)} />
        <Metric label={t("common.cacheRate")} value={formatPct(cacheRate)} accent="text-emerald-600" />
        <Metric label={t("common.output")} value={formatTokens(stats.overall.output_tokens)} />
      </div>

      <SpeedMetricsGrid overall={stats.overall} />

      <SectionCard title={t("common.tokenComposition")}>
        <div className="space-y-1.5">
          <DetailRow label={t("common.input")} value={formatTokens(stats.overall.input_tokens)} pct={stats.overall.total_tokens > 0 ? stats.overall.input_tokens / stats.overall.total_tokens : 0} color="bg-sky-500" />
          <DetailRow label={t("common.cache")} value={formatTokens(stats.overall.cache_read_tokens)} pct={cacheRate} color="bg-emerald-500" />
          <DetailRow label={t("common.output")} value={formatTokens(stats.overall.output_tokens)} pct={stats.overall.total_tokens > 0 ? stats.overall.output_tokens / stats.overall.total_tokens : 0} color="bg-violet-500" />
          {stats.overall.reasoning_tokens > 0 && (
            <DetailRow label={t("common.reasoning")} value={formatTokens(stats.overall.reasoning_tokens)} pct={stats.overall.total_tokens > 0 ? stats.overall.reasoning_tokens / stats.overall.total_tokens : 0} color="bg-amber-500" />
          )}
        </div>
      </SectionCard>

      {stats.by_model.length > 0 && (
        <SectionCard
          title={t("common.modelUsage")}
          action={
            <SortToggle
              options={[
                { key: "requests", label: t("common.requests") },
                { key: "token", label: "Token" },
                { key: "cost", label: t("common.cost") },
              ]}
              value={sortBy}
              onChange={setSortBy}
              accent={theme.accent}
            />
          }
        >
          <div className="space-y-1">
            {(() => {
              const rows = stats.by_model.map((m) => {
                const price = pricing.usd[m.model_id];
                const hasPrice = Boolean(price && (price.input > 0 || price.output > 0));
                const costVal = modelCost(m.model_id, m.input_tokens, m.output_tokens, m.cache_read_tokens, pricing, currency, fxRate);
                return { m, hasPrice, costVal, sortVal: sortBy === "cost" ? costVal : sortBy === "token" ? m.total_tokens : m.requests };
              });
              rows.sort((a, b) => b.sortVal - a.sortVal);
              const maxVal = rows.length ? rows[0].sortVal : 0;
              // 任一模型有速度数据才显示速度列（Claude 有、Codex 无，自动隐藏）
              const hasSpeedCol = rows.some((r) => r.m.avg_tps != null);
              return rows.map(({ m, hasPrice, costVal, sortVal }) => {
                const pct = maxVal > 0 ? Math.max(sortVal / maxVal, 0.02) : 0;
                return (
                  <div key={m.provider_id + m.model_id} className="relative rounded-lg hover:bg-slate-900/4 transition-colors py-1.5 px-1.5 overflow-hidden">
                    <div className={`absolute inset-y-0 left-0 rounded-lg pointer-events-none ${theme.rowBar}`} style={{ width: `${pct * 100}%` }} />
                    <div className="relative flex items-center justify-between text-xs min-w-0">
                      <div className="flex items-center gap-1 min-w-0 flex-1">
                        <span className="font-medium text-slate-900/90 truncate text-[11px]">{m.model_id}</span>
                        {!hasPrice && <span className="text-[10px] text-amber-600/90 shrink-0" title={t("common.noPrice")}>⚠</span>}
                      </div>
                      <div className="flex items-center gap-1 num shrink-0 text-[10px]">
                        {/* 请求/Token/花费三列深浅递进（浅→中→深），便于扫视区分；速度列插在 Token 与花费之间 */}
                        <span className="min-w-[1.5rem] text-right text-slate-500/80" title={t("common.requestCount")}>{formatTokens(m.requests)}</span>
                        <span className="min-w-[2rem] text-right text-slate-700" title={t("common.totalTokens")}>{formatTokens(m.total_tokens)}</span>
                        {hasSpeedCol && (
                          <span className="min-w-[1.75rem] text-right text-sky-700/80" title={t("common.avgSpeed")}>
                            {m.avg_tps != null ? `${formatTps(m.avg_tps)}/s` : "—"}
                          </span>
                        )}
                        <span className={`min-w-[2.5rem] text-right font-medium ${hasPrice ? "text-slate-900/90" : "text-slate-500/50"}`}>
                          {hasPrice ? formatCost(costVal, currency) : "—"}
                        </span>
                      </div>
                    </div>
                  </div>
                );
              });
            })()}
          </div>
        </SectionCard>
      )}
    </div>
  );
}

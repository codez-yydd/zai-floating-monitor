import { useState } from "react";
import type {
  CostResult,
  Currency,
  PricingConfig,
  Stats,
  TrendBucket,
  TrendPoint,
} from "./types";
import { formatCost, formatPct, formatTokens, formatTps } from "./format";
import {
  CurrentModelBar,
  DetailRow,
  Metric,
  SpeedMetricsGrid,
  TrendChart,
} from "./widgets";
import {
  HeroMetric,
  MetricPair,
  SectionCard,
  SortToggle,
  StatusBadge,
} from "./layout";
import { useI18n } from "./i18n";
import {
  canonicalModelId,
  hasPositivePrice,
  type FoldedModelStat,
} from "./modelName";

interface Props {
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];
  pricing: PricingConfig;
  currency: Currency;
  trendBucket: TrendBucket;
}

export function ZaiStatsContent({
  stats,
  cost,
  trend,
  pricing,
  currency,
  trendBucket,
}: Props) {
  const { t } = useI18n();
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");
  const [sortBy, setSortBy] = useState<"cost" | "token" | "requests">("cost");

  const totalCost =
    currency === "cny" ? cost?.total_cny ?? 0 : cost?.total_usd ?? 0;
  const perModelCost =
    currency === "cny" ? cost?.per_model_cny : cost?.per_model_usd;

  const cacheRate =
    stats && stats.overall.input_tokens > 0
      ? stats.overall.cache_read_tokens / stats.overall.input_tokens
      : 0;

  if (!stats) return <StatsSkeleton />;

  return (
    <div className="page-stack">
      <HeroMetric
        label={t("common.totalCost")}
        value={formatCost(totalCost, currency)}
        accent="sky"
        badge={
          cacheRate > 0 ? (
            <StatusBadge color="emerald">
              {t("common.cacheHit", { pct: formatPct(cacheRate) })}
            </StatusBadge>
          ) : undefined
        }
        footer={
          <MetricPair
            left={{ label: t("common.totalTokens"), value: formatTokens(stats.overall.total_tokens) }}
            right={{ label: t("common.requestCount"), value: String(stats.overall.requests) }}
          />
        }
      />

      <CurrentModelBar model={stats.current_model} />

      {trend.length > 0 && (
        <TrendChart
          points={trend}
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
          <DetailRow
            label={t("common.input")}
            value={formatTokens(stats.overall.input_tokens)}
            pct={stats.overall.total_tokens > 0 ? stats.overall.input_tokens / stats.overall.total_tokens : 0}
            color="bg-sky-500"
          />
          <DetailRow
            label={t("common.cache")}
            value={formatTokens(stats.overall.cache_read_tokens)}
            pct={cacheRate}
            color="bg-emerald-500"
          />
          <DetailRow
            label={t("common.output")}
            value={formatTokens(stats.overall.output_tokens)}
            pct={stats.overall.total_tokens > 0 ? stats.overall.output_tokens / stats.overall.total_tokens : 0}
            color="bg-violet-500"
          />
          {stats.overall.reasoning_tokens > 0 && (
            <DetailRow
              label={t("common.reasoning")}
              value={formatTokens(stats.overall.reasoning_tokens)}
              pct={stats.overall.total_tokens > 0 ? stats.overall.reasoning_tokens / stats.overall.total_tokens : 0}
              color="bg-amber-500"
            />
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
              accent="sky"
            />
          }
        >
          <ModelRankList
            stats={stats}
            perModelCost={perModelCost}
            pricing={pricing}
            currency={currency}
            sortBy={sortBy}
          />
        </SectionCard>
      )}
    </div>
  );
}

function ModelRankList({
  stats,
  perModelCost,
  pricing,
  currency,
  sortBy,
}: {
  stats: Stats;
  perModelCost: CostResult["per_model_cny"] | CostResult["per_model_usd"] | undefined;
  pricing: PricingConfig;
  currency: Currency;
  sortBy: "cost" | "token" | "requests";
}) {
  const { t } = useI18n();
  // 花费按归一化模型名聚合：per_model 数组里可能仍含大小写/隐藏字符变体的多条目，
  // 折叠后的行（DataCache 已 fold）要用同一归一键才能取到合计花费
  const costById = new Map<string, number>();
  perModelCost?.forEach((x) => {
    const k = canonicalModelId(x.model_id);
    costById.set(k, (costById.get(k) ?? 0) + x.cost);
  });

  const rows = stats.by_model.map((m) => {
    const hasPrice = hasPositivePrice(m.model_id, pricing);
    const costVal = costById.get(canonicalModelId(m.model_id)) ?? 0;
    return {
      m,
      hasPrice,
      costVal,
      sortVal: sortBy === "cost" ? costVal : sortBy === "token" ? m.total_tokens : m.requests,
    };
  });
  rows.sort((a, b) => b.sortVal - a.sortVal);
  const maxVal = rows.length ? rows[0].sortVal : 0;
  // 任一模型有速度数据才显示速度列（整列显示/隐藏，保持列对齐）
  const hasSpeedCol = rows.some((r) => r.m.avg_tps != null);

  return (
    <div className="space-y-1">
      {rows.map(({ m, hasPrice, costVal, sortVal }) => {
        const pct = maxVal > 0 ? Math.max(sortVal / maxVal, 0.02) : 0;
        // 发生折叠时在 tooltip 里列出被合并的原始写法与请求份额，便于追溯
        const variantTitle = (() => {
          const vs = (m as FoldedModelStat).variants;
          return vs && vs.length > 1
            ? vs.map((v) => `${v.model_id} ×${v.requests}`).join(" · ")
            : undefined;
        })();
        return (
          <div
            key={m.provider_id + m.model_id}
            className="relative rounded-lg hover:bg-slate-900/4 transition-colors py-1.5 px-1.5 overflow-hidden"
          >
            <div
              className="absolute inset-y-0 left-0 bg-sky-500/10 rounded-lg pointer-events-none"
              style={{ width: `${pct * 100}%` }}
            />
            <div className="relative flex items-center justify-between text-xs min-w-0">
              <div className="flex items-center gap-1 min-w-0 flex-1">
                <span
                  className="font-medium text-slate-900/90 truncate text-[11px]"
                  title={variantTitle}
                >
                  {m.model_id}
                </span>
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
      })}
    </div>
  );
}

function StatsSkeleton() {
  const barHeights = [38, 55, 42, 70, 48, 62, 35, 58];
  return (
    <div className="page-stack">
      <div className="hero-sky rounded-2xl px-3.5 py-3 animate-pulse">
        <div className="h-2.5 w-12 rounded bg-slate-900/15 mb-2" />
        <div className="h-7 w-28 rounded bg-slate-900/15" />
      </div>
      <div className="card-base rounded-2xl px-3 py-2.5 animate-pulse">
        <div className="h-2.5 w-12 rounded bg-slate-900/15 mb-3" />
        <div className="flex items-end gap-1 h-14">
          {barHeights.map((h, i) => (
            <div key={i} className="flex-1 rounded-t-md bg-slate-900/10" style={{ height: `${h}%` }} />
          ))}
        </div>
      </div>
      <div className="grid grid-cols-3 gap-1.5">
        {[0, 1, 2].map((i) => (
          <div key={i} className="card-base rounded-xl py-2 animate-pulse">
            <div className="h-2 w-8 rounded bg-slate-900/15 mx-auto mb-1.5" />
            <div className="h-3.5 w-10 rounded bg-slate-900/15 mx-auto" />
          </div>
        ))}
      </div>
    </div>
  );
}

import { useState } from "react";
import type {
  CostResult,
  Currency,
  PricingConfig,
  Stats,
  TrendBucket,
  TrendPoint,
} from "./types";
import { formatCost, formatPct, formatTokens } from "./format";
import { DetailRow, Metric, TrendChart } from "./widgets";

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

  if (!stats) {
    // 首屏数据未到时渲染与最终布局同构的骨架（灰条占位），
    // 数据到达后由下方 return 无跳变地替换，避免中间空白。
    return <StatsSkeleton />;
  }

  return (
    <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
      {/* 总览：花费为主，token 次之 */}
      <div className="flex items-end justify-between">
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
            总花费
          </div>
          <div className="num text-[26px] font-bold text-slate-900 leading-none mt-0.5">
            {formatCost(totalCost, currency)}
          </div>
        </div>
        <div className="text-right">
          <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
            总 Token
          </div>
          <div className="num text-[15px] font-semibold text-slate-900/70 leading-none mt-1">
            {formatTokens(stats.overall.total_tokens)}
          </div>
        </div>
      </div>

      {/* 趋势图（粒度跟随所选时间范围） */}
      <TrendChart
        points={trend}
        bucket={trendBucket}
        currency={currency}
        metric={trendMetric}
        onMetricChange={setTrendMetric}
      />

      {/* 三个指标 */}
      <div className="grid grid-cols-3 gap-1.5">
        <Metric label="请求" value={String(stats.overall.requests)} />
        <Metric
          label="缓存率"
          value={formatPct(cacheRate)}
          accent="text-emerald-600"
        />
        <Metric label="输出" value={formatTokens(stats.overall.output_tokens)} />
      </div>

      {/* 明细条 */}
      <div className="space-y-1.5 pt-1">
        <DetailRow
          label="输入"
          value={formatTokens(stats.overall.input_tokens)}
          pct={
            stats.overall.total_tokens > 0
              ? stats.overall.input_tokens / stats.overall.total_tokens
              : 0
          }
          color="bg-sky-500"
        />
        <DetailRow
          label="缓存"
          value={formatTokens(stats.overall.cache_read_tokens)}
          pct={cacheRate}
          color="bg-emerald-500"
        />
        <DetailRow
          label="输出"
          value={formatTokens(stats.overall.output_tokens)}
          pct={
            stats.overall.total_tokens > 0
              ? stats.overall.output_tokens / stats.overall.total_tokens
              : 0
          }
          color="bg-violet-500"
        />
        {stats.overall.reasoning_tokens > 0 && (
          <DetailRow
            label="推理"
            value={formatTokens(stats.overall.reasoning_tokens)}
            pct={
              stats.overall.total_tokens > 0
                ? stats.overall.reasoning_tokens / stats.overall.total_tokens
                : 0
            }
            color="bg-amber-500"
          />
        )}
      </div>

      {/* 按模型分组（排行榜） */}
      <div>
        <div className="flex items-center justify-between mb-1.5 mt-1">
          <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
            按模型
          </span>
          {/* 排序维度切换：花费 / Token / 请求 */}
          <div className="flex gap-0.5 text-[10px]">
            {(["cost", "token", "requests"] as const).map((s) => (
              <button
                key={s}
                onClick={() => setSortBy(s)}
                className={`px-1.5 py-0.5 rounded transition-colors ${
                  sortBy === s
                    ? "bg-sky-500/20 text-sky-700"
                    : "text-slate-700/45 hover:text-slate-900/70"
                }`}
              >
                {s === "cost" ? "花费" : s === "token" ? "Token" : "请求"}
              </button>
            ))}
          </div>
        </div>
        <div className="space-y-1">
          {(() => {
            // per_model_cost 在多设备合并时同一 model_id 可能有多条（本地+远端），
            // 需先按 model_id 聚合求和，再与 by_model 对应，否则花费会被低估。
            const costById = new Map<string, number>();
            perModelCost?.forEach((x) => {
              costById.set(
                x.model_id,
                (costById.get(x.model_id) ?? 0) + x.cost
              );
            });

            // 合并 by_model 与聚合后的 cost，按 sortBy 降序排序
            const rows = stats.by_model.map((m) => {
              const hasPrice = Boolean(
                pricing[currency][m.model_id] &&
                  (pricing[currency][m.model_id].input > 0 ||
                    pricing[currency][m.model_id].output > 0)
              );
              const costVal = costById.get(m.model_id) ?? 0;
              return {
                m,
                hasPrice,
                costVal,
                sortVal:
                  sortBy === "cost"
                    ? costVal
                    : sortBy === "token"
                      ? m.total_tokens
                      : m.requests,
              };
            });
            rows.sort((a, b) => b.sortVal - a.sortVal);

            // 占比条基准：当前维度的最大值（归一化到最大值 100%）
            const maxVal = rows.length ? rows[0].sortVal : 0;

            return rows.map(({ m, hasPrice, costVal, sortVal }) => {
              const pct =
                maxVal > 0 ? Math.max(sortVal / maxVal, 0.02) : 0;
              return (
                <div
                  key={m.provider_id + m.model_id}
                  className="relative rounded-lg hover:bg-slate-900/5 transition-colors py-1.5 px-2 -mx-2 overflow-hidden"
                >
                  {/* 占比条背景：按当前排序维度归一化 */}
                  <div
                    className="absolute inset-y-0 left-0 bg-sky-500/10 rounded-lg pointer-events-none"
                    style={{ width: `${pct * 100}%` }}
                  />
                  <div className="relative flex items-center justify-between text-xs">
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span className="font-medium text-slate-900/90 truncate">
                        {m.model_id}
                      </span>
                      {!hasPrice && (
                        <span
                          className="text-[10px] text-amber-600/90"
                          title="未配置价格"
                        >
                          ⚠
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-2 text-slate-700/60 num shrink-0">
                      <span>{m.requests}</span>
                      <span className="text-slate-700/25">·</span>
                      <span>{formatTokens(m.total_tokens)}</span>
                      <span
                        className={`w-12 text-right ${
                          hasPrice
                            ? "text-slate-900/90"
                            : "text-slate-700/35"
                        }`}
                      >
                        {hasPrice
                          ? formatCost(costVal, currency)
                          : "—"}
                      </span>
                    </div>
                  </div>
                </div>
              );
            });
          })()}
        </div>
      </div>
    </div>
  );
}

/**
 * 首屏骨架屏：与真实数据区（总览/趋势/指标/明细/排行榜）布局同构，
 * 数据到达后可无跳变地替换。仅用 animate-pulse 灰条，无依赖、无副作用。
 */
function StatsSkeleton() {
  // 柱子高度：用固定伪随机模式，视觉上更像真实趋势图
  const barHeights = [38, 55, 42, 70, 48, 62, 35, 58];
  return (
    <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
      {/* 总览行 */}
      <div className="flex items-end justify-between">
        <div className="space-y-1.5">
          <div className="h-2.5 w-10 rounded bg-slate-900/20 animate-pulse" />
          <div className="h-6 w-24 rounded bg-slate-900/20 animate-pulse" />
        </div>
        <div className="space-y-1.5 flex flex-col items-end">
          <div className="h-2.5 w-10 rounded bg-slate-900/20 animate-pulse" />
          <div className="h-4 w-16 rounded bg-slate-900/20 animate-pulse" />
        </div>
      </div>

      {/* 趋势图框 */}
      <div className="rounded-lg bg-white/25 border border-white/30 px-2.5 py-2">
        <div className="h-2.5 w-12 rounded bg-slate-900/20 animate-pulse mb-2" />
        <div className="flex items-end gap-1 h-12">
          {barHeights.map((h, i) => (
            <div
              key={i}
              className="flex-1 rounded-t-sm bg-slate-900/15 animate-pulse"
              style={{ height: `${h}%` }}
            />
          ))}
        </div>
      </div>

      {/* 三个指标 */}
      <div className="grid grid-cols-3 gap-1.5">
        {[0, 1, 2].map((i) => (
          <div
            key={i}
            className="rounded-lg bg-white/25 border border-white/30 py-2 text-center"
          >
            <div className="h-2 w-8 rounded bg-slate-900/20 animate-pulse mx-auto mb-1.5" />
            <div className="h-3.5 w-12 rounded bg-slate-900/20 animate-pulse mx-auto" />
          </div>
        ))}
      </div>

      {/* 明细条 */}
      <div className="space-y-2 pt-1">
        {[0, 1, 2].map((i) => (
          <div key={i} className="flex items-center gap-2">
            <div className="h-2 w-10 rounded bg-slate-900/20 animate-pulse shrink-0" />
            <div className="flex-1 h-1 rounded-full bg-slate-900/15 animate-pulse" />
            <div className="h-2 w-10 rounded bg-slate-900/20 animate-pulse shrink-0" />
          </div>
        ))}
      </div>

      {/* 排行榜 */}
      <div>
        <div className="h-2.5 w-12 rounded bg-slate-900/20 animate-pulse mb-2" />
        <div className="space-y-1.5">
          {[0, 1, 2].map((i) => (
            <div
              key={i}
              className="flex items-center justify-between py-1.5 px-2 -mx-2"
            >
              <div className="h-2.5 w-24 rounded bg-slate-900/20 animate-pulse" />
              <div className="h-2.5 w-20 rounded bg-slate-900/20 animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

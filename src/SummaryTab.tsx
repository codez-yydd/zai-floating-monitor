import type {
  CostResult,
  CursorSnapshot,
  Currency,
  Stats,
  TrendPoint,
} from "./types";
import { formatCost, formatTokens } from "./format";
import { ProgressBar, TrendChart } from "./widgets";
import { useState } from "react";

interface Props {
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];
  cursor: CursorSnapshot | null;
  currency: Currency;
  bucket: "hour" | "day";
  fxRate: number;
}

export function SummaryTab({
  stats,
  cost,
  trend,
  cursor,
  currency,
  bucket,
  fxRate,
}: Props) {
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");

  // z.ai 花费 & token
  const zaiCost =
    currency === "cny" ? cost?.total_cny ?? 0 : cost?.total_usd ?? 0;
  const zaiTokens = stats?.overall.total_tokens ?? 0;

  // Cursor 花费 & token（events 口径）
  const cursorEvents = cursor?.events;
  const cursorCostRaw = cursorEvents?.total_cost_usd ?? 0;
  const cursorCost =
    currency === "cny" ? cursorCostRaw * fxRate : cursorCostRaw;
  const cursorTokens = cursorEvents?.total_tokens ?? 0;

  // 合计
  const totalCost = zaiCost + cursorCost;
  const totalTokens = zaiTokens + cursorTokens;

  // 占比（用于可视化条）
  const zaiCostPct = totalCost > 0 ? zaiCost / totalCost : 0;
  const zaiTokenPct = totalTokens > 0 ? zaiTokens / totalTokens : 0;

  // Cursor 套餐进度
  const plan = cursor?.plan;
  const planPct =
    plan?.total_pct ??
    (plan?.used_cents != null && plan?.limit_cents != null &&
    plan.limit_cents > 0
      ? (plan.used_cents / plan.limit_cents) * 100
      : null);

  // 合并趋势：按 label 对齐 z.ai 趋势 + Cursor daily（仅日桶有意义）
  const cursorDailyMap = new Map<string, { cost: number; tokens: number }>();
  (cursor?.daily ?? []).forEach((d) => {
    cursorDailyMap.set(d.date, {
      cost: currency === "cny" ? d.cost_usd * fxRate : d.cost_usd,
      tokens: d.total_tokens,
    });
  });
  const mergedTrend: TrendPoint[] = trend.map((p) => {
    const c = cursorDailyMap.get(p.label);
    return {
      label: p.label,
      total_tokens: p.total_tokens + (c?.tokens ?? 0),
      requests: p.requests,
      cost_cny: p.cost_cny + (c?.cost ?? 0),
      cost_usd: p.cost_usd + (c?.cost ?? 0),
    };
  });

  return (
    <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
      {/* 合计花费 */}
      <div>
        <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
          合计花费
        </div>
        <div className="num text-[26px] font-bold text-slate-900 leading-none mt-0.5">
          {formatCost(totalCost, currency)}
        </div>
        {/* 花费占比条：z.ai vs Cursor */}
        <div className="flex items-center gap-1.5 mt-2">
          <span className="text-[9px] text-slate-700/45 w-10 shrink-0">ZCode</span>
          <div className="flex-1 h-2 rounded-full overflow-hidden flex bg-slate-900/8">
            <div
              className="h-full bg-sky-400"
              style={{ width: `${zaiCostPct * 100}%` }}
            />
            <div
              className="h-full bg-violet-400"
              style={{ width: `${(1 - zaiCostPct) * 100}%` }}
            />
          </div>
        </div>
        <div className="flex items-center justify-between text-[10px] num mt-1">
          <span className="text-sky-600/80">
            ZCode {formatCost(zaiCost, currency)}
          </span>
          <span className="text-violet-600/80">
            Cursor {formatCost(cursorCost, currency)}
          </span>
        </div>
      </div>

      {/* 合计 Token */}
      <div className="rounded-lg bg-white/25 border border-white/30 px-2.5 py-2">
        <div className="flex items-center justify-between mb-1.5">
          <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
            合计 Token
          </span>
          <span className="num text-[13px] font-semibold text-slate-900/80">
            {formatTokens(totalTokens)}
          </span>
        </div>
        <div className="h-2 rounded-full overflow-hidden flex bg-slate-900/8">
          <div
            className="h-full bg-sky-400"
            style={{ width: `${zaiTokenPct * 100}%` }}
          />
          <div
            className="h-full bg-violet-400"
            style={{ width: `${(1 - zaiTokenPct) * 100}%` }}
          />
        </div>
        <div className="flex items-center justify-between text-[10px] num mt-1">
          <span className="text-sky-600/70">
            ZCode {formatTokens(zaiTokens)}
          </span>
          <span className="text-violet-600/70">
            Cursor {formatTokens(cursorTokens)}
          </span>
        </div>
      </div>

      {/* 双看板：额度进度 */}
      <div className="grid grid-cols-2 gap-1.5">
        {/* ZCode 额度入口提示 */}
        <div className="rounded-lg bg-white/25 border border-white/30 px-2 py-1.5">
          <div className="text-[9px] text-slate-700/45">ZCode 周额度</div>
          <div className="text-[10px] text-slate-700/50 mt-1 leading-tight">
            切换至 ZCode 标签查看 5 小时 / 每周额度详情
          </div>
        </div>
        {/* Cursor 套餐 */}
        <div className="rounded-lg bg-white/25 border border-white/30 px-2 py-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[9px] text-slate-700/45">Cursor 套餐</span>
            {planPct != null && (
              <span
                className={`num text-[10px] font-semibold ${
                  planPct > 90
                    ? "text-rose-600"
                    : planPct > 70
                      ? "text-amber-600"
                      : "text-emerald-600"
                }`}
              >
                {planPct.toFixed(0)}%
              </span>
            )}
          </div>
          {planPct != null ? (
            <ProgressBar
              pct={planPct / 100}
              height="h-1"
              color={
                planPct > 90
                  ? "bg-rose-400"
                  : planPct > 70
                    ? "bg-amber-400"
                    : "bg-emerald-400"
              }
            />
          ) : (
            <div className="text-[10px] text-slate-700/40 mt-1">未登录</div>
          )}
        </div>
      </div>

      {/* 合并趋势图 */}
      {mergedTrend.length > 0 && (
        <TrendChart
          points={mergedTrend}
          bucket={bucket}
          currency={currency}
          metric={trendMetric}
          onMetricChange={setTrendMetric}
        />
      )}

      {/* 明细行 */}
      <div className="space-y-1.5 pt-0.5">
        <SummaryRow
          label="ZCode 花费"
          value={formatCost(zaiCost, currency)}
          pct={totalCost > 0 ? zaiCostPct : 0}
          color="bg-sky-400"
        />
        <SummaryRow
          label="Cursor 花费"
          value={formatCost(cursorCost, currency)}
          pct={totalCost > 0 ? 1 - zaiCostPct : 0}
          color="bg-violet-400"
        />
      </div>
    </div>
  );
}

function SummaryRow({
  label,
  value,
  pct,
  color,
}: {
  label: string;
  value: string;
  pct: number;
  color: string;
}) {
  return (
    <div className="flex items-center gap-2 text-[11px]">
      <span className="text-slate-700/60 w-16 shrink-0">{label}</span>
      <div className="flex-1 h-1 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className={`h-full rounded-full ${color} opacity-70`}
          style={{ width: `${Math.min(pct * 100, 100)}%` }}
        />
      </div>
      <span className="num text-slate-900/85 font-medium w-14 text-right">
        {value}
      </span>
    </div>
  );
}

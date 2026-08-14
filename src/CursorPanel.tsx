import { useState } from "react";
import type {
  CursorSnapshot,
  Currency,
  TrendPoint,
} from "./types";
import { formatCost, formatTokens, formatPct } from "./format";
import {
  Metric,
  ProgressBar,
  TrendChart,
  remainingGradient,
  remainingTextColor,
} from "./widgets";

interface Props {
  snapshot: CursorSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率（events 花费转 CNY 用） */
  fxRate: number;
}

/** 美分 → 美元 */
function centsToUsd(cents: number | null): number | null {
  return cents == null ? null : cents / 100;
}

/** 套餐额度行：标签 + 剩余% 在上，通栏进度条在下（Auto / API 同一套） */
function PlanQuotaRow({
  label,
  hint,
  usedPct,
}: {
  label: string;
  hint?: string;
  usedPct: number;
}) {
  const remain = Math.max(0, 100 - usedPct);
  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-0.5">
        <span className="text-[10px] text-slate-600 truncate">
          {label}
          {hint && (
            <span className="ml-1 text-[8px] text-slate-400">{hint}</span>
          )}
        </span>
        <span
          className="num text-[10px] font-semibold shrink-0 whitespace-nowrap"
          style={{ color: remainingTextColor(remain) }}
        >
          剩 {Math.round(remain)}%
        </span>
      </div>
      <ProgressBar
        pct={remain / 100}
        height="h-1.5"
        gradient={remainingGradient(remain)}
      />
    </div>
  );
}

export function CursorPanel({
  snapshot,
  loading,
  error,
  currency,
  fxRate,
}: Props) {
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");
  const [sortBy, setSortBy] = useState<"cost" | "token" | "requests">("cost");

  // 加载中 & 无数据
  if (loading && !snapshot) {
    return (
      <div className="flex-1 flex items-center justify-center text-xs text-slate-700/40">
        加载 Cursor 用量…
      </div>
    );
  }

  // 未登录 / 错误
  if (!snapshot || !snapshot.logged_in) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center px-6 text-center gap-2">
        <div className="text-2xl opacity-40">🖱️</div>
        <div className="text-xs text-slate-700/60 font-medium">
          未检测到 Cursor 登录
        </div>
        <div className="text-[10px] text-slate-700/40 leading-relaxed">
          请在 Cursor 应用中登录，或在「⚙ 价格设置」中手动粘贴 Cookie
        </div>
        {error && (
          <div className="text-[10px] text-red-600/70 mt-1 leading-relaxed">
            {error}
          </div>
        )}
      </div>
    );
  }

  // 错误但已有数据（events 拉取失败等情况）
  const partialError =
    snapshot.events_error ?? (error && snapshot.logged_in ? error : null);

  const plan = snapshot.plan;
  const onDemand = snapshot.on_demand;
  const events = snapshot.events;

  // 按需用量
  const odUsedUsd = centsToUsd(onDemand?.used_cents ?? null);
  const odLimitUsd = centsToUsd(onDemand?.limit_cents ?? null);

  // events 花费（转当前货币）
  const eventsCost =
    events?.total_cost_usd != null
      ? currency === "cny"
        ? events.total_cost_usd * fxRate
        : events.total_cost_usd
      : 0;

  // Cursor daily → TrendPoint（复用 TrendChart）
  const trendPoints: TrendPoint[] = (snapshot.daily ?? []).map((d) => ({
    label: d.date,
    total_tokens: d.total_tokens,
    requests: d.requests,
    cost_cny: d.cost_usd * fxRate,
    cost_usd: d.cost_usd,
  }));

  return (
    <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
      {/* 账户信息 */}
      <div className="flex items-center justify-between text-[10px]">
        <div className="min-w-0">
          <span className="text-slate-700/55">账户 </span>
          <span className="text-slate-900/80 font-medium truncate">
            {snapshot.account_email || snapshot.account_name || "未知"}
          </span>
        </div>
        {snapshot.membership_type && (
          <span className="shrink-0 px-1.5 py-0.5 rounded bg-violet-500/15 text-violet-700 text-[9px] font-medium capitalize">
            {snapshot.membership_type}
          </span>
        )}
      </div>

      {partialError && (
        <div className="px-2.5 py-1.5 rounded-lg bg-amber-500/15 text-amber-700 text-[10px]">
          {partialError}
        </div>
      )}

      {/* 套餐额度：与 Cursor 客户端一致，只展示 Auto / API（本计费周期，不随时间范围变化） */}
      {plan && (
        <div className="rounded-lg bg-surface/25 border border-surface/30 px-2.5 py-2 space-y-2">
          {plan.auto_pct != null || plan.api_pct != null ? (
            <>
              {plan.auto_pct != null && (
                <PlanQuotaRow label="Auto" usedPct={plan.auto_pct} />
              )}
              {plan.api_pct != null && (
                <PlanQuotaRow label="API" usedPct={plan.api_pct} />
              )}
            </>
          ) : (
            <>
              <div className="text-[10px] text-slate-600">套餐额度</div>
              <div className="text-[10px] text-slate-700/40">暂无额度数据</div>
            </>
          )}
        </div>
      )}

      {/* 按需用量 */}
      {onDemand && onDemand.enabled !== false && (
        <div className="rounded-lg bg-surface/25 border border-surface/30 px-2.5 py-2 space-y-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
              按需用量
            </span>
            <span className="num text-[10px] text-slate-700/60">
              {odUsedUsd != null
                ? formatCost(
                    currency === "cny" ? odUsedUsd * fxRate : odUsedUsd,
                    currency
                  )
                : "—"}
              {odLimitUsd != null
                ? " / " +
                  formatCost(
                    currency === "cny" ? odLimitUsd * fxRate : odLimitUsd,
                    currency
                  )
                : ""}
            </span>
          </div>
          {odLimitUsd != null && odLimitUsd > 0 && odUsedUsd != null && (
            <ProgressBar
              pct={(odLimitUsd - odUsedUsd) / odLimitUsd}
              height="h-1"
              gradient={remainingGradient(
                ((odLimitUsd - odUsedUsd) / odLimitUsd) * 100
              )}
            />
          )}
        </div>
      )}

      {/* 计费周期 */}
      {(snapshot.billing_cycle_start || snapshot.billing_cycle_end) && (
        <div className="flex items-center justify-between text-[10px] text-slate-700/50">
          <span>
            {snapshot.billing_cycle_start
              ? `周期 ${snapshot.billing_cycle_start.slice(0, 10)}`
              : ""}
          </span>
          {snapshot.billing_cycle_end && (
            <span>
              重置 {snapshot.billing_cycle_end.slice(0, 10)}
            </span>
          )}
        </div>
      )}

      {/* events 概览（所选时间范围内的 Token 使用明细） */}
      {events && (
        <>
          <div className="flex items-end justify-between">
            <div>
              <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
                Token 花费
                <span className="ml-1 text-[8px] text-sky-600/50 normal-case">
                  所选时间范围
                </span>
              </div>
              <div className="num text-[26px] font-bold text-slate-900 leading-none mt-0.5">
                {formatCost(eventsCost, currency)}
              </div>
            </div>
            <div className="text-right">
              <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
                总 Token
              </div>
              <div className="num text-[15px] font-semibold text-slate-900/70 leading-none mt-1">
                {formatTokens(events.total_tokens)}
              </div>
            </div>
          </div>

          {/* 趋势图（Cursor events 按日聚合，bucket 始终为 day） */}
          {trendPoints.length > 0 && (
            <TrendChart
              points={trendPoints}
              bucket="day"
              currency={currency}
              metric={trendMetric}
              onMetricChange={setTrendMetric}
            />
          )}

          {/* 三个指标 */}
          <div className="grid grid-cols-3 gap-1.5">
            <Metric label="请求" value={String(events.requests)} />
            <Metric
              label="缓存率"
              value={
                events.input_tokens + events.cache_read_tokens > 0
                  ? formatPct(
                      events.cache_read_tokens /
                        (events.input_tokens + events.cache_read_tokens),
                    )
                  : "0%"
              }
              accent="text-emerald-600"
            />
            <Metric label="输出" value={formatTokens(events.output_tokens)} />
          </div>

          {/* 按模型排行 */}
          {snapshot.by_model.length > 0 && (
            <div>
              <div className="flex items-center justify-between mb-1.5 mt-1">
                <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
                  按模型
                </span>
                <div className="flex gap-0.5 text-[10px]">
                  {(["cost", "token", "requests"] as const).map((s) => (
                    <button
                      key={s}
                      onClick={() => setSortBy(s)}
                      className={`px-1.5 py-0.5 rounded transition-colors ${
                        sortBy === s
                          ? "bg-violet-500/20 text-violet-700"
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
                  const rows = snapshot.by_model.map((m) => ({
                    m,
                    sortVal:
                      sortBy === "cost"
                        ? m.cost_usd
                        : sortBy === "token"
                          ? m.total_tokens
                          : m.requests,
                  }));
                  rows.sort((a, b) => b.sortVal - a.sortVal);
                  const maxVal = rows.length ? rows[0].sortVal : 0;
                  return rows.map(({ m, sortVal }) => {
                    const pct =
                      maxVal > 0 ? Math.max(sortVal / maxVal, 0.02) : 0;
                    return (
                      <div
                        key={m.model}
                        className="relative rounded-lg hover:bg-slate-900/5 transition-colors py-1.5 px-2 -mx-2 overflow-hidden"
                      >
                        <div
                          className="absolute inset-y-0 left-0 bg-violet-500/10 rounded-lg pointer-events-none"
                          style={{ width: `${pct * 100}%` }}
                        />
                        <div className="relative flex items-center justify-between text-xs">
                          <span className="font-medium text-slate-900/90 truncate">
                            {m.model}
                          </span>
                          <div className="flex items-center gap-2 text-slate-700/60 num shrink-0">
                            <span>{m.requests}</span>
                            <span className="text-slate-700/25">·</span>
                            <span>{formatTokens(m.total_tokens)}</span>
                            <span className="w-12 text-right text-slate-900/90">
                              {formatCost(
                                currency === "cny"
                                  ? m.cost_usd * fxRate
                                  : m.cost_usd,
                                currency
                              )}
                            </span>
                          </div>
                        </div>
                      </div>
                    );
                  });
                })()}
              </div>
            </div>
          )}
        </>
      )}

      {/* 无 events 数据但已登录 */}
      {!events && (
        <div className="rounded-lg bg-surface/25 border border-surface/30 px-3 py-6 text-center text-[10px] text-slate-700/40">
          {snapshot.events_error
            ? `Token 明细拉取失败：${snapshot.events_error}`
            : "所选时间范围内暂无 Token 使用明细"}
        </div>
      )}
    </div>
  );
}

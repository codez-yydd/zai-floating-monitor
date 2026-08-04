import { useCallback, useEffect, useState } from "react";
import type {
  CostResult,
  Currency,
  PricingConfig,
  RangePreset,
  Stats,
  TrendBucket,
  TrendPoint,
} from "./types";
import { computeCost, fetchStats, fetchTrend } from "./api";
import { formatCost, formatPct, formatTokens } from "./format";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker, resolveRange } from "./RangePicker";

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  onGoPricing: () => void;
}

export function StatsPanel({ currency, pricing, onGoPricing }: Props) {
  const [preset, setPreset] = useState<RangePreset>("today");
  const [custom, setCustom] = useState(() => {
    const today = new Date().toISOString().slice(0, 10);
    const week = new Date(Date.now() - 6 * 86400000).toISOString().slice(0, 10);
    return { from: week, to: today };
  });

  const [stats, setStats] = useState<Stats | null>(null);
  const [cost, setCost] = useState<CostResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<number>(0);
  // 趋势图：分桶数据（小时/日，跟随所选时间范围）
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  // 趋势图度量切换：花费 / Token
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");

  // preset → 桶粒度：今日/24h 按小时，更长范围按日
  const trendBucket: TrendBucket =
    preset === "today" || preset === "1d" ? "hour" : "day";

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    const [from, to] = resolveRange(preset, custom);
    try {
      const [s, c, t] = await Promise.all([
        fetchStats(from, to),
        computeCost(from, to),
        fetchTrend(from, to, trendBucket),
      ]);
      setStats(s);
      setCost(c);
      setTrend(t);
      setLastUpdate(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [preset, custom, trendBucket]);

  useEffect(() => {
    load();
  }, [load]);

  // 自动刷新：30 秒
  useEffect(() => {
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [load]);

  const totalCost =
    currency === "cny" ? cost?.total_cny ?? 0 : cost?.total_usd ?? 0;
  const perModelCost =
    currency === "cny" ? cost?.per_model_cny : cost?.per_model_usd;

  const cacheRate =
    stats && stats.overall.input_tokens > 0
      ? stats.overall.cache_read_tokens / stats.overall.input_tokens
      : 0;

  return (
    <div className="flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3.5 pt-3 pb-2.5 border-b border-slate-900/10">
        <div className="flex items-center justify-between mb-2.5">
          <h1 className="text-[13px] font-semibold text-slate-900/90">
            ZCode Token
          </h1>
          <button
            onClick={load}
            disabled={loading}
            className="text-slate-700/50 hover:text-slate-900/80 text-xs transition-colors"
            title="刷新"
          >
            ↻
          </button>
        </div>
        <RangePicker
          preset={preset}
          custom={custom}
          onChange={(p, c) => {
            setPreset(p);
            setCustom(c);
          }}
        />
      </div>

      {/* Coding Plan 额度监控 */}
      <QuotaPanel onGoSettings={onGoPricing} />

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      {stats && (
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
            <Metric
              label="输出"
              value={formatTokens(stats.overall.output_tokens)}
            />
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
              color="bg-sky-400"
            />
            <DetailRow
              label="缓存"
              value={formatTokens(stats.overall.cache_read_tokens)}
              pct={cacheRate}
              color="bg-emerald-400"
            />
            <DetailRow
              label="输出"
              value={formatTokens(stats.overall.output_tokens)}
              pct={
                stats.overall.total_tokens > 0
                  ? stats.overall.output_tokens / stats.overall.total_tokens
                  : 0
              }
              color="bg-violet-400"
            />
            {stats.overall.reasoning_tokens > 0 && (
              <DetailRow
                label="推理"
                value={formatTokens(stats.overall.reasoning_tokens)}
                pct={
                  stats.overall.total_tokens > 0
                    ? stats.overall.reasoning_tokens /
                      stats.overall.total_tokens
                    : 0
                }
                color="bg-amber-400"
              />
            )}
          </div>

          {/* 按模型分组 */}
          <div>
            <div className="text-[10px] uppercase tracking-wide text-slate-700/55 mb-1.5 mt-1">
              按模型
            </div>
            <div className="space-y-0.5">
              {stats.by_model.map((m) => {
                const mc = perModelCost?.find(
                  (x) => x.model_id === m.model_id
                );
                const hasPrice = Boolean(
                  pricing[currency][m.model_id] &&
                    (pricing[currency][m.model_id].input > 0 ||
                      pricing[currency][m.model_id].output > 0)
                );
                return (
                  <div
                    key={m.provider_id + m.model_id}
                    className="flex items-center justify-between text-xs py-1.5 px-2 -mx-2 rounded-lg hover:bg-slate-900/5 transition-colors"
                  >
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
                        {hasPrice ? formatCost(mc?.cost ?? 0, currency) : "—"}
                      </span>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}

      {/* 底部 */}
      <div className="px-3.5 py-2 border-t border-slate-900/10 flex items-center justify-between text-[10px] text-slate-700/50">
        <span className="num">
          {lastUpdate
            ? new Date(lastUpdate).toLocaleTimeString("zh-CN", {
                hour: "2-digit",
                minute: "2-digit",
              })
            : ""}
        </span>
        <button
          onClick={onGoPricing}
          className="hover:text-sky-600 transition-colors"
        >
          ⚙ 价格设置
        </button>
      </div>
    </div>
  );
}

function Metric({
  label,
  value,
  accent,
}: {
  label: string;
  value: string;
  accent?: string;
}) {
  return (
    <div className="rounded-lg bg-white/25 border border-white/30 py-2 text-center">
      <div className="text-[10px] text-slate-700/55">{label}</div>
      <div
        className={`num text-[13px] font-semibold mt-0.5 ${
          accent || "text-slate-900/80"
        }`}
      >
        {value}
      </div>
    </div>
  );
}

function DetailRow({
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
      <span className="text-slate-700/60 w-14 shrink-0">{label}</span>
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

/** 趋势图：迷你柱状图 + 最新桶环比 + 花费/Token 切换。
 *  粒度跟随所选时间范围：今日/24h 按小时，更长范围按日。 */
function TrendChart({
  points,
  bucket,
  currency,
  metric,
  onMetricChange,
}: {
  points: TrendPoint[];
  bucket: TrendBucket;
  currency: Currency;
  metric: "cost" | "token";
  onMetricChange: (m: "cost" | "token") => void;
}) {
  // 取每根柱子的数值
  const values = points.map((d) =>
    metric === "cost"
      ? currency === "cny"
        ? d.cost_cny
        : d.cost_usd
      : d.total_tokens
  );
  const maxValue = Math.max(...values, 1); // 至少为 1，避免除 0

  // 最新桶 vs 上一桶 环比（始终按花费比较，更直观）
  const last = points[points.length - 1];
  const prev = points[points.length - 2];
  const lastCost = last
    ? currency === "cny"
      ? last.cost_cny
      : last.cost_usd
    : 0;
  const prevCost = prev
    ? currency === "cny"
      ? prev.cost_cny
      : prev.cost_usd
    : 0;
  let deltaText: string | null = null;
  let deltaUp = false;
  if (last && prev) {
    if (prevCost > 0) {
      const pct = ((lastCost - prevCost) / prevCost) * 100;
      if (Math.abs(pct) < 0.5) {
        deltaText = "持平";
      } else {
        deltaUp = pct > 0;
        deltaText = `${deltaUp ? "↑" : "↓"}${Math.abs(pct).toFixed(0)}%`;
      }
    } else if (lastCost > 0) {
      deltaText = "新增";
    }
  }

  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const isHour = bucket === "hour";
  const n = points.length;
  // 间距：柱子越多越收紧。日桶 30 天/大跨度可达 30+ 根。
  const barGap = n > 20 ? "gap-px" : n > 10 ? "gap-0.5" : "gap-1";
  // 标签步长：目标约显示 6~8 个标签，避免文字重叠。
  // n<=8 全显示；否则向上取整到合适的步长。
  const labelStep =
    n <= 8 ? 1 : Math.max(2, Math.ceil(n / 7));

  return (
    <div className="rounded-lg bg-white/25 border border-white/30 px-2.5 py-2">
      {/* 标题行 */}
      <div className="flex items-center justify-between mb-1.5">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
            趋势
          </span>
          {deltaText && (
            <span
              className={`text-[10px] num ${
                deltaText === "持平" || deltaText === "新增"
                  ? "text-slate-700/50"
                  : deltaUp
                    ? "text-rose-600/90"
                    : "text-emerald-600/90"
              }`}
              title={`最新${isHour ? "小时" : "日"} vs 上一${isHour ? "小时" : "日"}`}
            >
              {deltaText}
            </span>
          )}
        </div>
        {/* 花费/Token 切换 */}
        <div className="flex gap-0.5">
          {(["cost", "token"] as const).map((m) => (
            <button
              key={m}
              onClick={() => onMetricChange(m)}
              className={`px-1.5 rounded text-[9px] transition-colors ${
                metric === m
                  ? "bg-sky-500/80 text-white"
                  : "text-slate-700/45 hover:text-slate-900/70"
              }`}
            >
              {m === "cost" ? "花费" : "Token"}
            </button>
          ))}
        </div>
      </div>

      {/* 柱状图 */}
      <div className={`flex items-end ${barGap} h-12 relative`}>
        {points.map((d, i) => {
          const v = values[i];
          const h = maxValue > 0 ? (v / maxValue) * 100 : 0;
          const isLast = i === points.length - 1;
          const isHover = hoverIdx === i;
          return (
            <div
              key={d.label}
              className="flex-1 h-full flex items-end justify-center relative min-w-0"
              onMouseEnter={() => setHoverIdx(i)}
              onMouseLeave={() => setHoverIdx(null)}
            >
              {/* tooltip */}
              {isHover && (
                <div className="absolute bottom-full mb-1 left-1/2 -translate-x-1/2 z-10 whitespace-nowrap rounded-md bg-slate-900/85 text-white px-1.5 py-1 text-[9px] leading-tight pointer-events-none">
                  <div className="num">{d.label}</div>
                  <div className="num">
                    {formatCost(
                      currency === "cny" ? d.cost_cny : d.cost_usd,
                      currency
                    )}
                  </div>
                  <div className="num opacity-70">
                    {formatTokens(d.total_tokens)}
                  </div>
                </div>
              )}
              <div
                className={`w-full rounded-t-sm transition-all duration-300 ${
                  isLast
                    ? "bg-sky-500/80"
                    : isHover
                      ? "bg-slate-700/70"
                      : "bg-slate-700/35"
                }`}
                style={{
                  height: `${Math.max(h, v > 0 ? 4 : 0)}%`,
                  // 柱子少时限制最大宽度，避免单根过粗
                  maxWidth: n <= 7 ? "14px" : undefined,
                }}
              />
            </div>
          );
        })}
      </div>

      {/* 标签：柱子多时隔几个显示一个，避免文字重叠 */}
      <div className={`flex ${barGap} mt-1`}>
        {points.map((d, i) => {
          // 按 labelStep 隔行；最后一个总是显示
          const showLabel = i === points.length - 1 || i % labelStep === 0;
          const isLast = i === points.length - 1;
          return (
            <span
              key={d.label}
              className={`flex-1 text-center text-[8px] num min-w-0 ${
                isLast
                  ? "text-sky-600/80 font-medium"
                  : "text-slate-700/40"
              } ${showLabel ? "" : "opacity-0"}`}
            >
              {d.label}
            </span>
          );
        })}
      </div>
    </div>
  );
}

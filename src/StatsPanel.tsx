import { useCallback, useEffect, useState } from "react";
import type {
  CostResult,
  Currency,
  PricingConfig,
  RangePreset,
  Stats,
} from "./types";
import { computeCost, fetchStats } from "./api";
import { formatCost, formatPct, formatTokens } from "./format";
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

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    const [from, to] = resolveRange(preset, custom);
    try {
      const [s, c] = await Promise.all([
        fetchStats(from, to),
        computeCost(from, to),
      ]);
      setStats(s);
      setCost(c);
      setLastUpdate(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [preset, custom]);

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
              label="缓存读"
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

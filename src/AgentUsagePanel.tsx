import { useEffect, useState } from "react";
import type { Currency, PricingConfig, Stats, TrendBucket, TrendPoint } from "./types";
import { formatCost, formatCountdown, formatPct, formatTokens } from "./format";
import { modelCost } from "./merge";
import {
  DetailRow,
  Metric,
  ProgressBar,
  TrendChart,
  remainingGradient,
  remainingTextColor,
} from "./widgets";

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
  /** 排序按钮选中态 */
  sortSelected: string;
  /** 套餐徽标 */
  badge: string;
}

/** 无数据空态文案（未安装对应 CLI 时展示） */
export interface AgentEmptyState {
  icon: string;
  title: string;
  hint: string;
}

interface Props {
  snapshot: AgentUsageSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  trendBucket: TrendBucket;
  /** 按模型折算花费用（与 Z.ai 页同款前端自算） */
  pricing: PricingConfig;
  theme: AgentPanelTheme;
  empty: AgentEmptyState;
  /** 缓存率口径：input 含缓存读（zcode/Codex/OpenAI 口径）还是三分并列
   *  （Anthropic 口径：input 不含 cache_read/cache_creation，需用输入侧总量
   *  做分母，否则比率会远超 100%） */
  cacheRateMode: "included" | "separate";
}

/** 速率限制一行：标签 + 重置倒计时 + 剩余% 在上，剩余进度条在下
 *  （与汇总页 QuotaMiniRow / Cursor 页 PlanQuotaRow 同款视觉） */
function AgentQuotaRow({
  label,
  usedPct,
  resetAt,
  now,
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
}) {
  const remain = Math.max(0, 100 - usedPct);
  const showReset = resetAt != null && resetAt > now;
  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-0.5">
        <span className="text-[10px] text-slate-600 truncate">
          {label}
          {showReset && (
            <span className="ml-1 text-[8px] text-slate-400 num">
              ↻ {formatCountdown(resetAt - now, true)}
            </span>
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

/** 单 CLI Agent 的用量面板（Codex / Claude 共用）：
 *  订阅额度块（无数据不渲染）→ 总花费/Token → 趋势图 → 三指标 →
 *  明细条 → 按模型排行。品牌差异只体现在 theme/empty 两个 props。 */
export function AgentUsagePanel({
  snapshot,
  loading,
  error,
  currency,
  trendBucket,
  pricing,
  theme,
  empty,
  cacheRateMode,
}: Props) {
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
      <div className="flex-1 flex items-center justify-center text-xs text-slate-700/40">
        加载{empty.title.replace("未检测到 ", "")}用量…
      </div>
    );
  }

  // 未安装 / 无会话目录 / 其他错误且无数据
  if (!snapshot) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center px-6 text-center gap-2">
        <div className="text-2xl opacity-40">{empty.icon}</div>
        <div className="text-xs text-slate-700/60 font-medium">
          {empty.title}
        </div>
        <div className="text-[10px] text-slate-700/40 leading-relaxed whitespace-pre-line">
          {empty.hint}
        </div>
        {error && (
          <div className="text-[10px] text-red-600/70 mt-1 leading-relaxed">
            {error}
          </div>
        )}
      </div>
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
    <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
      {/* 速率限制：5 小时窗口 + 周窗口（额度来自订阅账号，不随时间范围变化） */}
      {hasRateRow && rate && (
        <div className="rounded-lg bg-white/25 border border-white/30 px-2.5 py-2 space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
              额度
            </span>
            {rate.plan_type && (
              <span
                className={`shrink-0 px-1.5 py-0.5 rounded text-[9px] font-medium capitalize ${theme.badge}`}
              >
                {rate.plan_type}
              </span>
            )}
          </div>
          {rate.primary_pct != null && (
            <AgentQuotaRow
              label="5小时"
              usedPct={rate.primary_pct}
              resetAt={rate.primary_reset_at}
              now={now}
            />
          )}
          {rate.secondary_pct != null && (
            <AgentQuotaRow
              label="本周"
              usedPct={rate.secondary_pct}
              resetAt={rate.secondary_reset_at}
              now={now}
            />
          )}
        </div>
      )}

      {/* 总览：花费为主，token 次之（Z.ai 页同款） */}
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
      {snapshot.trend.length > 0 && (
        <TrendChart
          points={snapshot.trend}
          bucket={trendBucket}
          currency={currency}
          metric={trendMetric}
          onMetricChange={setTrendMetric}
        />
      )}

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

      {/* 按模型排行（花费按价格表前端自算，与 Z.ai 页同款判断） */}
      {stats.by_model.length > 0 && (
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
                      ? theme.sortSelected
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
              const rows = stats.by_model.map((m) => {
                const price = pricing[currency][m.model_id];
                const hasPrice = Boolean(
                  price && (price.input > 0 || price.output > 0)
                );
                const costVal = modelCost(
                  m.model_id,
                  m.input_tokens,
                  m.output_tokens,
                  m.cache_read_tokens,
                  pricing,
                  currency
                );
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
              const maxVal = rows.length ? rows[0].sortVal : 0;

              return rows.map(({ m, hasPrice, costVal, sortVal }) => {
                const pct =
                  maxVal > 0 ? Math.max(sortVal / maxVal, 0.02) : 0;
                return (
                  <div
                    key={m.provider_id + m.model_id}
                    className="relative rounded-lg hover:bg-slate-900/5 transition-colors py-1.5 px-2 -mx-2 overflow-hidden"
                  >
                    <div
                      className={`absolute inset-y-0 left-0 rounded-lg pointer-events-none ${theme.rowBar}`}
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
      )}
    </div>
  );
}

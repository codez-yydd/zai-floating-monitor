import type {
  CostResult,
  CursorSnapshot,
  Currency,
  PricingConfig,
  Stats,
  TrendPoint,
} from "./types";
import { formatCost, formatTokens } from "./format";
import {
  ProgressBar,
  TrendChart,
  remainingGradient,
  remainingTextColor,
} from "./widgets";
import { useDataCache } from "./DataCache";
import { useState } from "react";

interface Props {
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];
  cursor: CursorSnapshot | null;
  currency: Currency;
  bucket: "hour" | "day";
  fxRate: number;
  pricing: PricingConfig;
}

const LEVEL_LABEL: Record<string, string> = {
  lite: "Lite",
  pro: "Pro",
  max: "Max",
  ultra: "Ultra",
};

/** 单个 agent 在汇总页的展示单元。以后加新 agent 往数组里追加一项即可。 */
interface AgentSummary {
  id: string;
  name: string;
  /** 占比条 / 圆点颜色（hex，避免 Tailwind 动态类名失效） */
  color: string;
  nameClass: string;
  badge?: string | null;
  badgeClass: string;
  cost: number;
  tokens: number;
  metrics: { label: string; usedPct: number }[];
  empty?: string;
}

/** 花费占比条：用 gradient 硬切色段，避免 flex/width 舍入露出底色 */
function CompositionBar({ agents }: { agents: AgentSummary[] }) {
  const total = agents.reduce((s, a) => s + a.cost, 0);
  if (total <= 0) {
    return <div className="h-1.5 rounded-full bg-slate-900/8" />;
  }

  let acc = 0;
  const stops: string[] = [];
  for (const a of agents) {
    if (a.cost <= 0) continue;
    const start = (acc / total) * 100;
    acc += a.cost;
    const end = (acc / total) * 100;
    stops.push(`${a.color} ${start}%`, `${a.color} ${end}%`);
  }

  return (
    <div
      className="h-1.5 rounded-full"
      style={{ background: `linear-gradient(90deg, ${stops.join(", ")})` }}
    />
  );
}

/** 汇总页模型排行的一行（ZCode / Cursor 混排） */
interface ModelRankRow {
  key: string;
  name: string;
  source: string;
  color: string;
  barBg: string;
  requests: number;
  tokens: number;
  cost: number;
  hasPrice: boolean;
}

function buildModelRows(
  stats: Stats | null,
  cost: CostResult | null,
  cursor: CursorSnapshot | null,
  pricing: PricingConfig,
  currency: Currency,
  fxRate: number
): ModelRankRow[] {
  const rows: ModelRankRow[] = [];

  const costById = new Map<string, number>();
  const perModel =
    currency === "cny" ? cost?.per_model_cny : cost?.per_model_usd;
  perModel?.forEach((x) => {
    costById.set(x.model_id, (costById.get(x.model_id) ?? 0) + x.cost);
  });

  for (const m of stats?.by_model ?? []) {
    const price = pricing[currency][m.model_id];
    const hasPrice = Boolean(price && (price.input > 0 || price.output > 0));
    rows.push({
      key: `zcode:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "ZCode",
      color: "#0ea5e9",
      barBg: "bg-sky-500/10",
      requests: m.requests,
      tokens: m.total_tokens,
      cost: costById.get(m.model_id) ?? 0,
      hasPrice,
    });
  }

  for (const m of cursor?.by_model ?? []) {
    rows.push({
      key: `cursor:${m.model}`,
      name: m.model,
      source: "Cursor",
      color: "#8b5cf6",
      barBg: "bg-violet-500/10",
      requests: m.requests,
      tokens: m.total_tokens,
      cost: currency === "cny" ? m.cost_usd * fxRate : m.cost_usd,
      hasPrice: true,
    });
  }

  return rows;
}

/** 额度一行：标签 + 剩余进度条 + 剩余% */
function QuotaMiniRow({
  label,
  usedPct,
}: {
  label: string;
  usedPct: number;
}) {
  const remain = Math.max(0, 100 - usedPct);
  return (
    <div className="flex items-center gap-1.5">
      <span className="text-[9px] text-slate-500 w-7 shrink-0">{label}</span>
      <ProgressBar
        pct={remain / 100}
        height="h-1"
        gradient={remainingGradient(remain)}
      />
      <span
        className="num text-[9px] font-medium w-12 text-right shrink-0 whitespace-nowrap"
        style={{ color: remainingTextColor(remain) }}
      >
        剩 {Math.round(remain)}%
      </span>
    </div>
  );
}

export function SummaryTab({
  stats,
  cost,
  trend,
  cursor,
  currency,
  bucket,
  fxRate,
  pricing,
}: Props) {
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");
  const [sortBy, setSortBy] = useState<"cost" | "token" | "requests">("cost");

  // ZCode 额度（与范围无关，读全局缓存，与 QuotaPanel 同源）
  const { quota, quotaError } = useDataCache();
  const hour5Pct = quota?.hour5?.percentage ?? null;
  const weeklyPct = quota?.weekly?.percentage ?? null;
  const levelLabel = quota?.level
    ? LEVEL_LABEL[quota.level] || quota.level
    : null;

  // z.ai 花费 & token
  const zaiCost =
    currency === "cny" ? (cost?.total_cny ?? 0) : (cost?.total_usd ?? 0);
  const zaiTokens = stats?.overall.total_tokens ?? 0;

  // Cursor 花费 & token（events 口径）
  const cursorEvents = cursor?.events;
  const cursorCostRaw = cursorEvents?.total_cost_usd ?? 0;
  const cursorCost =
    currency === "cny" ? cursorCostRaw * fxRate : cursorCostRaw;
  const cursorTokens = cursorEvents?.total_tokens ?? 0;

  // Cursor 套餐进度：只用总量 / Auto / API 百分比。
  // used_cents/limit_cents 是套餐标价口径，不能代表 Auto 额度（Auto 可用量远大于 API 的 $20 限额）。
  const plan = cursor?.plan;

  const zcodeEmpty = quotaError
    ? /未配置|Token/i.test(quotaError)
      ? "未配置 Token"
      : "额度获取失败"
    : "加载中…";

  const zcodeMetrics = [
    ...(weeklyPct != null ? [{ label: "本周", usedPct: weeklyPct }] : []),
    ...(hour5Pct != null ? [{ label: "5h", usedPct: hour5Pct }] : []),
  ];
  const cursorMetrics = [
    ...(plan?.total_pct != null
      ? [{ label: "总量", usedPct: plan.total_pct }]
      : []),
    ...(plan?.auto_pct != null
      ? [{ label: "Auto", usedPct: plan.auto_pct }]
      : []),
    ...(plan?.api_pct != null
      ? [{ label: "API", usedPct: plan.api_pct }]
      : []),
  ];

  // 按 agent 组装。新增来源时在此追加，总览占比条 / 花费表 / 额度列表会一起跟上。
  const agents: AgentSummary[] = [
    {
      id: "zcode",
      name: "ZCode",
      color: "#0ea5e9",
      nameClass: "text-sky-700",
      badge: levelLabel,
      badgeClass: "bg-sky-500/12 text-sky-700",
      cost: zaiCost,
      tokens: zaiTokens,
      metrics: zcodeMetrics,
      empty: zcodeEmpty,
    },
    {
      id: "cursor",
      name: "Cursor",
      color: "#8b5cf6",
      nameClass: "text-violet-700",
      badge: cursor?.membership_type ?? null,
      badgeClass: "bg-violet-500/12 text-violet-700 capitalize",
      cost: cursorCost,
      tokens: cursorTokens,
      metrics: cursorMetrics,
      empty: cursor?.logged_in ? "暂无额度数据" : "未登录",
    },
  ];

  const totalCost = agents.reduce((s, a) => s + a.cost, 0);
  const totalTokens = agents.reduce((s, a) => s + a.tokens, 0);

  // 合并趋势：按 label 对齐 z.ai 趋势 + Cursor daily（仅日桶有意义）
  // 双货币各自计算：usd 用原值，cny = usd × 汇率，分别写入对应字段，
  // 避免同一个换算值同时进 cost_cny/cost_usd 造成非当前货币字段错误
  const cursorDailyMap = new Map<
    string,
    { costCny: number; costUsd: number; tokens: number }
  >();
  (cursor?.daily ?? []).forEach((d) => {
    cursorDailyMap.set(d.date, {
      costCny: d.cost_usd * fxRate,
      costUsd: d.cost_usd,
      tokens: d.total_tokens,
    });
  });
  const mergedTrend: TrendPoint[] = trend.map((p) => {
    const c = cursorDailyMap.get(p.label);
    return {
      label: p.label,
      total_tokens: p.total_tokens + (c?.tokens ?? 0),
      requests: p.requests,
      cost_cny: p.cost_cny + (c?.costCny ?? 0),
      cost_usd: p.cost_usd + (c?.costUsd ?? 0),
    };
  });

  const modelRows = buildModelRows(
    stats,
    cost,
    cursor,
    pricing,
    currency,
    fxRate
  );
  const sortedModels = [...modelRows].sort((a, b) => {
    const av =
      sortBy === "cost" ? a.cost : sortBy === "token" ? a.tokens : a.requests;
    const bv =
      sortBy === "cost" ? b.cost : sortBy === "token" ? b.tokens : b.requests;
    return bv - av;
  });
  const maxModelVal = sortedModels[0]
    ? sortBy === "cost"
      ? sortedModels[0].cost
      : sortBy === "token"
        ? sortedModels[0].tokens
        : sortedModels[0].requests
    : 0;

  return (
    <div className="flex-1 min-h-0 overflow-y-auto px-3.5 pt-3 pb-2 flex flex-col gap-2.5">
      {/* 总览：数字 → 一条占比 → 来源表 */}
      <div className="rounded-xl bg-white/30 border border-white/35 px-3 py-2.5 shrink-0">
        <div className="flex items-end justify-between gap-3">
          <div>
            <div className="text-[10px] text-slate-500">合计花费</div>
            <div className="num text-[26px] font-bold text-slate-900 leading-none mt-1">
              {formatCost(totalCost, currency)}
            </div>
          </div>
          <div className="text-right">
            <div className="text-[10px] text-slate-500">合计 Token</div>
            <div className="num text-[16px] font-semibold text-slate-800 leading-none mt-1">
              {formatTokens(totalTokens)}
            </div>
          </div>
        </div>

        <div className="mt-2.5">
          <CompositionBar agents={agents} />
        </div>

        <div className="mt-2.5 grid grid-cols-[1fr_auto_auto] gap-x-3 gap-y-1 items-center">
          {agents.map((a) => (
            <div key={a.id} className="contents">
              <div className="flex items-center gap-1.5 min-w-0">
                <span
                  className="w-1.5 h-1.5 rounded-full shrink-0"
                  style={{ background: a.color }}
                />
                <span
                  className={`text-[10px] font-medium truncate ${a.nameClass}`}
                >
                  {a.name}
                </span>
              </div>
              <span className="num text-[11px] font-semibold text-slate-800 text-right">
                {formatCost(a.cost, currency)}
              </span>
              <span className="num text-[11px] text-slate-500 text-right">
                {formatTokens(a.tokens)}
              </span>
            </div>
          ))}
        </div>
      </div>

      {/* 额度：每个 agent 一块，纵向排列，方便以后继续加 */}
      <div className="rounded-xl bg-white/30 border border-white/35 shrink-0 divide-y divide-slate-900/8">
        {agents.map((a) => (
          <div key={a.id} className="px-3 py-2">
            <div className="flex items-center justify-between gap-1 mb-1.5">
              <div className="flex items-center gap-1.5 min-w-0">
                <span
                  className="w-1.5 h-1.5 rounded-full shrink-0"
                  style={{ background: a.color }}
                />
                <span className="text-[10px] font-medium text-slate-700 truncate">
                  {a.name}
                </span>
                {a.badge && (
                  <span
                    className={`shrink-0 px-1 py-px rounded text-[8px] font-medium ${a.badgeClass}`}
                  >
                    {a.badge}
                  </span>
                )}
              </div>
            </div>
            {a.metrics.length > 0 ? (
              <div className="space-y-1">
                {a.metrics.map((m) => (
                  <QuotaMiniRow
                    key={m.label}
                    label={m.label}
                    usedPct={m.usedPct}
                  />
                ))}
              </div>
            ) : (
              <div className="text-[10px] text-slate-500">{a.empty}</div>
            )}
          </div>
        ))}
      </div>

      {/* 合并趋势图：下方有模型排行时不再撑满，把空间让给列表 */}
      {mergedTrend.length > 0 && (
        <TrendChart
          points={mergedTrend}
          bucket={bucket}
          currency={currency}
          metric={trendMetric}
          onMetricChange={setTrendMetric}
          fill={sortedModels.length === 0}
        />
      )}

      {/* 按模型：高度封顶，超出在列表内滚动，避免把面板撑高 */}
      {sortedModels.length > 0 && (
        <div className="shrink-0">
          <div className="flex items-center justify-between mb-1.5">
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
                      ? "bg-slate-900/10 text-slate-800"
                      : "text-slate-700/45 hover:text-slate-900/70"
                  }`}
                >
                  {s === "cost" ? "花费" : s === "token" ? "Token" : "请求"}
                </button>
              ))}
            </div>
          </div>
          <div className="max-h-40 overflow-y-auto overflow-x-hidden overscroll-contain">
            {sortedModels.map((m) => {
              const sortVal =
                sortBy === "cost"
                  ? m.cost
                  : sortBy === "token"
                    ? m.tokens
                    : m.requests;
              const pct =
                maxModelVal > 0 ? Math.max(sortVal / maxModelVal, 0.02) : 0;
              return (
                <div
                  key={m.key}
                  className="relative rounded-lg hover:bg-slate-900/5 transition-colors py-1.5 overflow-hidden min-w-0"
                >
                  <div
                    className={`absolute inset-y-0 left-0 ${m.barBg} rounded-lg pointer-events-none`}
                    style={{ width: `${pct * 100}%` }}
                  />
                  <div className="relative flex items-center gap-1.5 text-xs min-w-0">
                    <div className="flex items-center gap-1.5 min-w-0 flex-1">
                      <span
                        className="w-1.5 h-1.5 rounded-full shrink-0"
                        style={{ background: m.color }}
                        title={m.source}
                      />
                      <span
                        className="font-medium text-slate-900/90 truncate"
                        title={`${m.source} · ${m.name}`}
                      >
                        {m.name}
                      </span>
                      {!m.hasPrice && (
                        <span
                          className="text-[10px] text-amber-600/90 shrink-0"
                          title="未配置价格"
                        >
                          ⚠
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-1.5 text-slate-700/60 num shrink-0 whitespace-nowrap">
                      <span>{m.requests}</span>
                      <span className="text-slate-700/25">·</span>
                      <span>{formatTokens(m.tokens)}</span>
                      <span
                        className={`min-w-[2.75rem] text-right ${
                          m.hasPrice
                            ? "text-slate-900/90"
                            : "text-slate-700/35"
                        }`}
                      >
                        {m.hasPrice ? formatCost(m.cost, currency) : "—"}
                      </span>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

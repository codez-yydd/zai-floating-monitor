import type {
  AgentQuotaDelta,
  ClaudeSnapshot,
  CodexSnapshot,
  CostResult,
  CursorSnapshot,
  Currency,
  OverallStat,
  PricingConfig,
  Stats,
  TrendPoint,
} from "./types";
import { formatCost, formatCountdownCore, formatTokens } from "./format";
import { modelCost } from "./merge";
import {
  ProgressBar,
  TrendChart,
  remainingGradient,
  remainingTextColor,
} from "./widgets";
import { useDataCache } from "./DataCache";
import { useEffect, useState } from "react";
import { useI18n } from "./i18n";
import type { AgentId, AgentVisibility } from "./agentVisibility";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  HeroMetric,
  MetricPair,
  SectionCard,
  SortToggle,
  StatusBadge,
} from "./layout";

interface Props {
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];
  codex: CodexSnapshot | null;
  claude: ClaudeSnapshot | null;
  cursor: CursorSnapshot | null;
  currency: Currency;
  bucket: "hour" | "day";
  fxRate: number;
  pricing: PricingConfig;
  agentVisibility: AgentVisibility;
}

const LEVEL_LABEL: Record<string, string> = {
  lite: "Lite",
  pro: "Pro",
  max: "Max",
  ultra: "Ultra",
};

/** 缓存命中率：input 含 cache_read 口径（ZCode / Codex） */
function cacheHitPctIncluded(
  overall: Pick<OverallStat, "input_tokens" | "cache_read_tokens"> | null | undefined
): number | null {
  if (!overall || overall.input_tokens <= 0) return null;
  return (overall.cache_read_tokens / overall.input_tokens) * 100;
}

/** 缓存命中率：Anthropic separate 口径（Claude） */
function cacheHitPctSeparate(overall: OverallStat | null | undefined): number | null {
  if (!overall) return null;
  const inputSideTotal =
    overall.input_tokens +
    overall.cache_read_tokens +
    overall.cache_write_tokens;
  if (inputSideTotal <= 0) return null;
  return (overall.cache_read_tokens / inputSideTotal) * 100;
}

/** 缓存命中率：Cursor events 口径 */
function cacheHitPctCursor(
  events: { input_tokens: number; cache_read_tokens: number } | null | undefined
): number | null {
  if (!events) return null;
  const total = events.input_tokens + events.cache_read_tokens;
  if (total <= 0) return null;
  return (events.cache_read_tokens / total) * 100;
}

/** 单个 agent 在汇总页的展示单元。以后加新 agent 往数组里追加一项即可。 */
interface AgentSummary {
  id: AgentId;
  name: string;
  /** 占比条 / 圆点颜色（hex，避免 Tailwind 动态类名失效） */
  color: string;
  /** 卡片浅色背景 tint */
  tintBg: string;
  nameClass: string;
  badge?: string | null;
  badgeClass: string;
  cost: number;
  tokens: number;
  /** 缓存命中率（有 cache_read 数据时展示） */
  cacheHitPct?: number | null;
  metrics: {
    label: string;
    usedPct: number;
    resetAt?: number | null;
    delta?: AgentQuotaDelta;
  }[];
  empty?: string;
  /** 套餐级重置时间（Cursor 计费周期结束），显示在标题行 */
  cycleResetAt?: number | null;
  cycleResetDate?: string | null;
}

/** Agent 花费卡片：品牌图标 + Token 大数字 + 花费进度条 */
function AgentCostCard({
  agent,
  currency,
  costPct,
}: {
  agent: AgentSummary;
  currency: Currency;
  costPct: number;
}) {
  const { t } = useI18n();
  const brandMap: Record<AgentId, BrandIconName | null> = {
    zai: "zai",
    codex: "codex",
    claude: "claude",
    cursor: "cursor",
  };
  const brand = brandMap[agent.id];

  return (
    <div
      className="rounded-xl px-2.5 py-2 border shrink-0"
      style={{
        background: agent.tintBg,
        borderColor: `${agent.color}22`,
      }}
    >
      <div className="flex items-start gap-2">
        {/* 左侧：图标 + 名称 + Token */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5 mb-1">
            {brand && (
              <span style={{ color: agent.color }}>
                <BrandIcon brand={brand} className="h-3.5 w-3.5 shrink-0" />
              </span>
            )}
            <span className={`text-[11px] font-semibold ${agent.nameClass}`}>
              {agent.name}
            </span>
            {agent.badge && (
              <span
                className={`shrink-0 px-1 py-px rounded text-[8px] font-medium ${agent.badgeClass}`}
              >
                {agent.badge}
              </span>
            )}
          </div>
          <div className="num text-[15px] font-bold text-slate-900 leading-none">
            {formatTokens(agent.tokens)}
          </div>
          {agent.cacheHitPct != null && agent.cacheHitPct > 0 && (
            <div
              className="text-[9px] font-medium mt-0.5"
              style={{ color: agent.color }}
            >
              {t("common.cacheHit", { pct: `${Math.round(agent.cacheHitPct)}%` })}
            </div>
          )}
        </div>
        {/* 右侧：花费 + 占比条 */}
        <div className="w-[72px] shrink-0 pt-0.5">
          <div className="num text-[12px] font-semibold text-slate-800 text-right mb-1">
            {formatCost(agent.cost, currency)}
          </div>
          <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
            <div
              className="h-full rounded-full transition-all duration-500"
              style={{
                width: `${Math.max(costPct * 100, agent.cost > 0 ? 4 : 0)}%`,
                background: `linear-gradient(90deg, ${agent.color}cc, ${agent.color})`,
              }}
            />
          </div>
        </div>
      </div>
    </div>
  );
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
  codex: CodexSnapshot | null,
  claude: ClaudeSnapshot | null,
  cursor: CursorSnapshot | null,
  pricing: PricingConfig,
  currency: Currency,
  fxRate: number,
  agentVisibility: AgentVisibility
): ModelRankRow[] {
  const rows: ModelRankRow[] = [];

  const costById = new Map<string, number>();
  const perModel =
    currency === "cny" ? cost?.per_model_cny : cost?.per_model_usd;
  perModel?.forEach((x) => {
    costById.set(x.model_id, (costById.get(x.model_id) ?? 0) + x.cost);
  });

  for (const m of agentVisibility.zai ? stats?.by_model ?? [] : []) {
    const price = pricing.usd[m.model_id];
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

  // Codex：后端无按模型花费命令，前端按价格表自算（与 zcode 行同款口径）
  for (const m of agentVisibility.codex ? codex?.stats.by_model ?? [] : []) {
    const price = pricing.usd[m.model_id];
    const hasPrice = Boolean(price && (price.input > 0 || price.output > 0));
    rows.push({
      key: `codex:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "Codex",
      color: "#10a37f",
      barBg: "bg-emerald-500/10",
      requests: m.requests,
      tokens: m.total_tokens,
      cost: modelCost(
        m.model_id,
        m.input_tokens,
        m.output_tokens,
        m.cache_read_tokens,
        pricing,
        currency,
        fxRate
      ),
      hasPrice,
    });
  }

  // Claude：与 Codex 行同款（Anthropic 品牌橙）
  for (const m of agentVisibility.claude ? claude?.stats.by_model ?? [] : []) {
    const price = pricing.usd[m.model_id];
    const hasPrice = Boolean(price && (price.input > 0 || price.output > 0));
    rows.push({
      key: `claude:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "Claude",
      color: "#d97757",
      barBg: "bg-orange-500/10",
      requests: m.requests,
      tokens: m.total_tokens,
      cost: modelCost(
        m.model_id,
        m.input_tokens,
        m.output_tokens,
        m.cache_read_tokens,
        pricing,
        currency,
        fxRate
      ),
      hasPrice,
    });
  }

  for (const m of agentVisibility.cursor ? cursor?.by_model ?? [] : []) {
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

/** 额度一行：标签 + 重置倒计时 + 剩余% 在上，进度条在下 */
function QuotaMiniRow({
  label,
  usedPct,
  resetAt,
  now,
  delta,
}: {
  label: string;
  usedPct: number;
  resetAt?: number | null;
  now: number;
  delta?: AgentQuotaDelta;
}) {
  const { t } = useI18n();
  const remain = Math.max(0, 100 - usedPct);
  const showReset = resetAt != null && resetAt > now;
  return (
    <div>
      <div className="flex items-center gap-1 mb-0.5">
        <span className="text-[9px] text-slate-500 w-10 shrink-0 whitespace-nowrap">{label}</span>
        <span className="num text-[8px] text-slate-400 flex-1 text-right truncate min-w-0">
          {showReset ? `↻ ${formatCountdownCore(resetAt - now)}` : ""}
        </span>
        <span
          className="num text-[9px] font-medium w-12 text-right shrink-0 whitespace-nowrap"
          style={{ color: remainingTextColor(remain) }}
        >
          {t("common.remaining", { pct: Math.round(remain) })}
        </span>
      </div>
      <ProgressBar
        pct={remain / 100}
        height="h-1"
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

function cycleEndMs(iso: string | null | undefined): number | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isFinite(t) ? t : null;
}

export function SummaryTab({
  stats,
  cost,
  trend,
  codex,
  claude,
  cursor,
  currency,
  bucket,
  fxRate,
  pricing,
  agentVisibility,
}: Props) {
  const { t } = useI18n();
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");
  const [sortBy, setSortBy] = useState<"cost" | "token" | "requests">("cost");
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);

  // ZCode 额度（与范围无关，读全局缓存，与 QuotaPanel 同源）
  const {
    quota,
    quotaError,
    todayDelta,
    codexError,
    claudeError,
    agentQuotaDeltas,
  } = useDataCache();
  const hour5 = quota?.hour5 ?? null;
  const weekly = quota?.weekly ?? null;
  const mcp = quota?.mcp ?? null;
  const levelLabel = quota?.level
    ? LEVEL_LABEL[quota.level] || quota.level
    : null;
  const zcodeWeeklyDelta: AgentQuotaDelta | undefined = todayDelta
    ? { pct: todayDelta[0], samples: todayDelta[1] }
    : undefined;

  // z.ai 花费 & token
  const zaiCost =
    currency === "cny" ? (cost?.total_cny ?? 0) : (cost?.total_usd ?? 0);
  const zaiTokens = stats?.overall.total_tokens ?? 0;

  // Codex 花费 & token：花费对趋势桶求和（与后端口径一致）
  const codexRate = codex?.rate_limits ?? null;
  const codexCostRaw = (codex?.trend ?? []).reduce(
    (s, p) => s + (currency === "cny" ? p.cost_cny : p.cost_usd),
    0
  );
  const codexTokens = codex?.stats.overall.total_tokens ?? 0;

  // Claude 花费 & token（与 Codex 同款口径）
  const claudeRate = claude?.rate_limits ?? null;
  const claudeCostRaw = (claude?.trend ?? []).reduce(
    (s, p) => s + (currency === "cny" ? p.cost_cny : p.cost_usd),
    0
  );
  const claudeTokens = claude?.stats.overall.total_tokens ?? 0;

  // Cursor 花费 & token（events 口径）
  const cursorEvents = cursor?.events;
  const cursorCostRaw = cursorEvents?.total_cost_usd ?? 0;
  const cursorCost =
    currency === "cny" ? cursorCostRaw * fxRate : cursorCostRaw;
  const cursorTokens = cursorEvents?.total_tokens ?? 0;

  // Cursor 套餐进度：与客户端一致，只展示 Auto / API。
  // used_cents/limit_cents 是套餐标价口径，不能代表 Auto 额度。
  const plan = cursor?.plan;

  // 注意：正则匹配 Rust 后端返回的中文错误串（如「未配置 Token」），仅做布尔分支，不能翻译
  const zcodeEmpty = quotaError
    ? /未配置|Token/i.test(quotaError)
      ? t("summary.noToken")
      : t("summary.quotaFailed")
    : t("common.loading");

  const zcodeMetrics = [
    ...(hour5 != null
      ? [
          {
            label: t("common.hour5"),
            usedPct: hour5.percentage,
            resetAt: hour5.nextResetTime,
          },
        ]
      : []),
    ...(weekly != null
      ? [
          {
            label: t("common.weekly"),
            usedPct: weekly.percentage,
            resetAt: weekly.nextResetTime,
            delta: zcodeWeeklyDelta,
          },
        ]
      : []),
    ...(mcp != null
      ? [
          {
            label: "MCP",
            usedPct: mcp.percentage,
            resetAt: mcp.nextResetTime,
          },
        ]
      : []),
  ];
  const cursorMetrics = [
    ...(plan?.auto_pct != null
      ? [{ label: "Auto", usedPct: plan.auto_pct, delta: agentQuotaDeltas.cursor?.cursor_auto }]
      : []),
    ...(plan?.api_pct != null
      ? [{ label: "API", usedPct: plan.api_pct, delta: agentQuotaDeltas.cursor?.cursor_api }]
      : []),
  ];

  // Codex 额度：本机会话解析（API 中转模式无 rate_limits → 空 metrics 走文案）
  const codexMetrics =
    codexRate &&
    (codexRate.primary_pct != null || codexRate.secondary_pct != null)
      ? [
          ...(codexRate.primary_pct != null
            ? [
                {
                  label: t("common.hour5"),
                  usedPct: codexRate.primary_pct,
                  resetAt: codexRate.primary_reset_at,
                },
              ]
            : []),
          ...(codexRate.secondary_pct != null
            ? [
                {
                  label: t("common.weekly"),
                  usedPct: codexRate.secondary_pct,
                  resetAt: codexRate.secondary_reset_at,
                  delta: agentQuotaDeltas.codex?.weekly,
                },
              ]
            : []),
        ]
      : [];
  // 注意：正则匹配 Rust 后端返回的中文错误串（未找到/未安装/会话目录），仅做布尔分支，不能翻译
  const codexEmpty = codexError
    ? /未找到|未安装|会话目录/i.test(codexError)
      ? t("stats.codexNotFound")
      : t("summary.loadFailed")
    : t("summary.noData");

  // Claude 额度：仅订阅登录机器上有实时值（中转模式 rate_limits 为 null → 空 metrics 走文案）
  const claudeMetrics =
    claudeRate &&
    (claudeRate.primary_pct != null || claudeRate.secondary_pct != null)
      ? [
          ...(claudeRate.primary_pct != null
            ? [
                {
                  label: t("common.hour5"),
                  usedPct: claudeRate.primary_pct,
                  resetAt: claudeRate.primary_reset_at,
                },
              ]
            : []),
          ...(claudeRate.secondary_pct != null
            ? [
                {
                  label: t("common.weekly"),
                  usedPct: claudeRate.secondary_pct,
                  resetAt: claudeRate.secondary_reset_at,
                  delta: agentQuotaDeltas.claude?.weekly,
                },
              ]
            : []),
        ]
      : [];
  // 注意：正则匹配 Rust 后端返回的中文错误串（未找到/未安装/会话目录），仅做布尔分支，不能翻译
  const claudeEmpty = claudeError
    ? /未找到|未安装|会话目录/i.test(claudeError)
      ? t("stats.claudeNotFound")
      : t("summary.loadFailed")
    : t("summary.noData");

  // 各 Agent 缓存命中率（口径与各详情页一致）
  const zaiCacheHitPct = cacheHitPctIncluded(stats?.overall);
  const codexCacheHitPct = cacheHitPctIncluded(codex?.stats.overall);
  const claudeCacheHitPct = cacheHitPctSeparate(claude?.stats.overall);
  const cursorCacheHitPct = cacheHitPctCursor(cursorEvents);

  // 按 agent 组装。新增来源时在此追加，总览占比条 / 花费表 / 额度列表会一起跟上。
  const allAgents: AgentSummary[] = [
    {
      id: "zai",
      name: "ZCode",
      color: "#0ea5e9",
      tintBg: "rgba(14, 165, 233, 0.07)",
      nameClass: "text-sky-700",
      badge: levelLabel,
      badgeClass: "bg-sky-500/12 text-sky-700",
      cost: zaiCost,
      tokens: zaiTokens,
      cacheHitPct: zaiCacheHitPct,
      metrics: zcodeMetrics,
      empty: zcodeEmpty,
    },
    {
      id: "codex",
      name: "Codex",
      color: "#10a37f",
      tintBg: "rgba(16, 163, 127, 0.07)",
      nameClass: "text-emerald-700",
      badge: codexRate?.plan_type ?? null,
      badgeClass: "bg-emerald-500/12 text-emerald-700",
      cost: codexCostRaw,
      tokens: codexTokens,
      cacheHitPct: codexCacheHitPct,
      metrics: codexMetrics,
      empty: codexEmpty,
    },
    {
      id: "claude",
      name: "Claude",
      color: "#d97757",
      tintBg: "rgba(217, 119, 87, 0.07)",
      nameClass: "text-orange-700",
      badge: claudeRate?.plan_type ?? null,
      badgeClass: "bg-orange-500/12 text-orange-700 capitalize",
      cost: claudeCostRaw,
      tokens: claudeTokens,
      cacheHitPct: claudeCacheHitPct,
      metrics: claudeMetrics,
      empty: claudeEmpty,
    },
    {
      id: "cursor",
      name: "Cursor",
      color: "#8b5cf6",
      tintBg: "rgba(139, 92, 246, 0.07)",
      nameClass: "text-violet-700",
      badge: cursor?.membership_type ?? null,
      badgeClass: "bg-violet-500/12 text-violet-700 capitalize",
      cost: cursorCost,
      tokens: cursorTokens,
      cacheHitPct: cursorCacheHitPct,
      metrics: cursorMetrics,
      empty: cursor?.logged_in ? t("summary.noQuotaData") : t("summary.notLoggedIn"),
      cycleResetAt: cycleEndMs(cursor?.billing_cycle_end),
      cycleResetDate: cursor?.billing_cycle_end
        ? cursor.billing_cycle_end.slice(0, 10)
        : null,
    },
  ];

  const agents = allAgents.filter((agent) => agentVisibility[agent.id]);
  const totalCost = agents.reduce((s, a) => s + a.cost, 0);
  const totalTokens = agents.reduce((s, a) => s + a.tokens, 0);

  // 合并趋势：只加入开启的 Agent，并按多个来源的 label 建立并集。
  // 这样即使关闭 Z.ai，只剩 Codex/Claude/Cursor 时汇总趋势仍然可用。
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
  const zcodeTrendMap = new Map<string, TrendPoint>();
  trend.forEach((p) => zcodeTrendMap.set(p.label, p));
  // Codex / Claude 趋势桶自带双货币花费，按 label 索引后直接相加（与 z.ai 桶格式一致）
  const codexTrendMap = new Map<string, TrendPoint>();
  (codex?.trend ?? []).forEach((p) => codexTrendMap.set(p.label, p));
  const claudeTrendMap = new Map<string, TrendPoint>();
  (claude?.trend ?? []).forEach((p) => claudeTrendMap.set(p.label, p));

  const trendLabels: string[] = [];
  const trendLabelSet = new Set<string>();
  const addTrendLabel = (label: string) => {
    if (!trendLabelSet.has(label)) {
      trendLabelSet.add(label);
      trendLabels.push(label);
    }
  };
  if (agentVisibility.zai) trend.forEach((p) => addTrendLabel(p.label));
  if (agentVisibility.codex)
    codexTrendMap.forEach((_point, label) => addTrendLabel(label));
  if (agentVisibility.claude)
    claudeTrendMap.forEach((_point, label) => addTrendLabel(label));
  if (agentVisibility.cursor)
    cursorDailyMap.forEach((_point, label) => addTrendLabel(label));

  const mergedTrend: TrendPoint[] = trendLabels.map((label) => {
    const p =
      zcodeTrendMap.get(label) ??
      ({
        label,
        total_tokens: 0,
        requests: 0,
        cost_cny: 0,
        cost_usd: 0,
      } satisfies TrendPoint);
    const c = agentVisibility.cursor ? cursorDailyMap.get(label) : undefined;
    const x = agentVisibility.codex ? codexTrendMap.get(label) : undefined;
    const a = agentVisibility.claude ? claudeTrendMap.get(label) : undefined;
    return {
      label: p.label,
      total_tokens:
        (agentVisibility.zai ? p.total_tokens : 0) +
        (x?.total_tokens ?? 0) +
        (a?.total_tokens ?? 0) +
        (c?.tokens ?? 0),
      requests:
        (agentVisibility.zai ? p.requests : 0) +
        (x?.requests ?? 0) +
        (a?.requests ?? 0),
      cost_cny:
        (agentVisibility.zai ? p.cost_cny : 0) +
        (x?.cost_cny ?? 0) +
        (a?.cost_cny ?? 0) +
        (c?.costCny ?? 0),
      cost_usd:
        (agentVisibility.zai ? p.cost_usd : 0) +
        (x?.cost_usd ?? 0) +
        (a?.cost_usd ?? 0) +
        (c?.costUsd ?? 0),
    };
  });

  const modelRows = buildModelRows(
    stats,
    cost,
    codex,
    claude,
    cursor,
    pricing,
    currency,
    fxRate,
    agentVisibility
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
    <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2.5 page-stack">
      <HeroMetric
        label={t("summary.totalCost")}
        value={formatCost(totalCost, currency)}
        accent="sky"
        badge={
          <StatusBadge color="emerald">
            {t("summary.sources", { count: agents.length })}
          </StatusBadge>
        }
        footer={
          <MetricPair
            left={{ label: t("summary.totalTokens"), value: formatTokens(totalTokens) }}
            right={{ label: t("summary.costDist"), value: <CompositionBar agents={agents} /> }}
          />
        }
      />

      {agents.length > 0 && (
        <div className="flex flex-col gap-1.5 shrink-0">
          {agents.map((a) => (
            <AgentCostCard
              key={a.id}
              agent={a}
              currency={currency}
              costPct={totalCost > 0 ? a.cost / totalCost : 0}
            />
          ))}
        </div>
      )}

      {agents.some((a) => a.metrics.length > 0 || a.empty) && (
        <SectionCard title={t("summary.quotaMonitor")}>
          <div className="divide-y divide-slate-900/6 -mx-1">
            {agents.map((a) => (
              <div key={a.id} className="px-1 py-2 first:pt-0 last:pb-0">
                <div className="flex items-center justify-between gap-1 mb-1.5">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: a.color }} />
                    <span className="text-[10px] font-semibold text-slate-700 truncate">{a.name}</span>
                    {a.badge && (
                      <span className={`shrink-0 px-1 py-px rounded text-[8px] font-medium ${a.badgeClass}`}>{a.badge}</span>
                    )}
                  </div>
                  {a.cycleResetAt != null && a.cycleResetAt > now ? (
                    <span className="num text-[8px] text-slate-400 shrink-0 whitespace-nowrap">
                      ↻ {formatCountdownCore(a.cycleResetAt - now)}
                    </span>
                  ) : a.cycleResetDate ? (
                    <span className="num text-[8px] text-slate-400 shrink-0 whitespace-nowrap">
                      {t("summary.resetDate", { date: a.cycleResetDate })}
                    </span>
                  ) : null}
                </div>
                {a.metrics.length > 0 ? (
                  <div className="space-y-1.5">
                    {a.metrics.map((m) => (
                      <QuotaMiniRow
                        key={m.label}
                        label={m.label}
                        usedPct={m.usedPct}
                        resetAt={m.resetAt}
                        now={now}
                        delta={m.delta}
                      />
                    ))}
                  </div>
                ) : (
                  <div className="text-[10px] text-slate-500">{a.empty}</div>
                )}
              </div>
            ))}
          </div>
        </SectionCard>
      )}

      {/* 合并趋势图 */}
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

      {/* 按模型排行 */}
      {sortedModels.length > 0 && (
        <SectionCard
          title={t("common.modelUsage")}
          action={
            <SortToggle
              options={[
                { key: "cost", label: t("common.cost") },
                { key: "token", label: "Token" },
                { key: "requests", label: t("common.requests") },
              ]}
              value={sortBy}
              onChange={setSortBy}
              accent="sky"
            />
          }
        >
          <div className="max-h-36 overflow-y-auto overflow-x-hidden overscroll-contain space-y-1">
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
                  className="relative rounded-lg hover:bg-slate-900/4 transition-colors py-1.5 px-1.5 overflow-hidden min-w-0"
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
                        className="font-medium text-slate-900/90 truncate text-[11px]"
                        title={`${m.source} · ${m.name}`}
                      >
                        {m.name}
                      </span>
                      {!m.hasPrice && (
                        <span
                          className="text-[10px] text-amber-600/90 shrink-0"
                          title={t("common.noPrice")}
                        >
                          ⚠
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-1 text-slate-600/70 num shrink-0 whitespace-nowrap text-[10px]">
                      <span className="min-w-[1.5rem] text-right" title={t("common.requestCount")}>
                        {formatTokens(m.requests)}
                      </span>
                      <span className="min-w-[2rem] text-right" title={t("common.totalTokens")}>
                        {formatTokens(m.tokens)}
                      </span>
                      <span
                        className={`min-w-[2.5rem] text-right font-medium ${
                          m.hasPrice
                            ? "text-slate-900/90"
                            : "text-slate-500/50"
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
        </SectionCard>
      )}
    </div>
  );
}

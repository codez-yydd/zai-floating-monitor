import type {
  AccountQuotaEntry,
  AgentQuotaDelta,
  ClaudeSnapshot,
  CodexSnapshot,
  CostResult,
  CurrentModel,
  CursorSnapshot,
  Currency,
  KimiSnapshot,
  ModelStat,
  OverallStat,
  PricingConfig,
  Stats,
  TrendPoint,
} from "./types";
import { formatCost, formatCountdownCore, formatMs, formatResetStamp, formatTokens, formatTps, levelLabel } from "./format";
import { modelCost } from "./merge";
import {
  canonicalModelId,
  hasPositivePrice,
  type FoldedModelStat,
} from "./modelName";
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
import { AGENT_COLOR } from "./agentVisibility";
import { useResetDisplay } from "./resetDisplay";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  SwitchAccountButton,
  SwitchConfirmOverlay,
  useAccountSwitch,
} from "./accountSwitch";
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
  kimi: KimiSnapshot | null;
  currency: Currency;
  bucket: "hour" | "day";
  fxRate: number;
  pricing: PricingConfig;
  agentVisibility: AgentVisibility;
}

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
  /** 数据源已有结论（快照到达或确认报错）为 true，加载中为 false；用于隐藏零消耗卡片 */
  loaded: boolean;
  /** 最近使用的模型（额度区标题行展示） */
  currentModel?: CurrentModel | null;
  /** 平均输出速度 tok/s（数据源带耗时的 Agent 才有：ZCode/Claude） */
  avgTps?: number | null;
  /** 平均首字延迟 ms（仅 ZCode，悬浮提示展示） */
  avgTtftMs?: number | null;
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
    kimi: "kimi",
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
          {agent.avgTps != null && (
            <div
              className="num text-[9px] font-medium mt-0.5"
              style={{ color: agent.color }}
              title={
                agent.avgTtftMs != null
                  ? `${t("common.ttft")} ${formatMs(agent.avgTtftMs)}`
                  : undefined
              }
            >
              ⚡ {formatTps(agent.avgTps)} t/s
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
  /** 发生折叠合并时，被合并的原始写法明细（tooltip 用） */
  mergedNote?: string;
}

/** 折叠行的被合并写法明细（tooltip 用）；未发生合并返回 undefined */
function mergedNoteOf(m: ModelStat): string | undefined {
  const vs = (m as FoldedModelStat).variants;
  return vs && vs.length > 1
    ? vs.map((v) => `${v.model_id} ×${v.requests}`).join(" · ")
    : undefined;
}

function buildModelRows(
  stats: Stats | null,
  cost: CostResult | null,
  codex: CodexSnapshot | null,
  claude: ClaudeSnapshot | null,
  cursor: CursorSnapshot | null,
  kimi: KimiSnapshot | null,
  pricing: PricingConfig,
  currency: Currency,
  fxRate: number,
  agentVisibility: AgentVisibility
): ModelRankRow[] {
  const rows: ModelRankRow[] = [];

  // 花费按归一化模型名聚合：per_model 数组里可能仍含大小写/隐藏字符变体的多条目，
  // 折叠后的行要用同一归一键才能取到合计花费
  const costById = new Map<string, number>();
  const perModel =
    currency === "cny" ? cost?.per_model_cny : cost?.per_model_usd;
  perModel?.forEach((x) => {
    const k = canonicalModelId(x.model_id);
    costById.set(k, (costById.get(k) ?? 0) + x.cost);
  });

  for (const m of agentVisibility.zai ? stats?.by_model ?? [] : []) {
    const hasPrice = hasPositivePrice(m.model_id, pricing);
    rows.push({
      key: `zcode:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "ZCode",
      color: AGENT_COLOR.zai,
      barBg: "bg-sky-500/10",
      requests: m.requests,
      tokens: m.total_tokens,
      cost: costById.get(canonicalModelId(m.model_id)) ?? 0,
      hasPrice,
      mergedNote: mergedNoteOf(m),
    });
  }

  // Codex：后端无按模型花费命令，前端按价格表自算（与 zcode 行同款口径）
  for (const m of agentVisibility.codex ? codex?.stats.by_model ?? [] : []) {
    const hasPrice = hasPositivePrice(m.model_id, pricing);
    rows.push({
      key: `codex:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "Codex",
      color: AGENT_COLOR.codex,
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
      mergedNote: mergedNoteOf(m),
    });
  }

  // Claude：与 Codex 行同款（Anthropic 品牌橙）
  for (const m of agentVisibility.claude ? claude?.stats.by_model ?? [] : []) {
    const hasPrice = hasPositivePrice(m.model_id, pricing);
    rows.push({
      key: `claude:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "Claude",
      color: AGENT_COLOR.claude,
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
      mergedNote: mergedNoteOf(m),
    });
  }

  for (const m of agentVisibility.cursor ? cursor?.by_model ?? [] : []) {
    rows.push({
      key: `cursor:${m.model}`,
      name: m.model,
      source: "Cursor",
      color: AGENT_COLOR.cursor,
      barBg: "bg-violet-500/10",
      requests: m.requests,
      tokens: m.total_tokens,
      cost: currency === "cny" ? m.cost_usd * fxRate : m.cost_usd,
      hasPrice: true,
    });
  }

  // Kimi：与 Codex/Claude 行同款（indigo 品牌色）
  for (const m of agentVisibility.kimi ? kimi?.stats.by_model ?? [] : []) {
    const hasPrice = hasPositivePrice(m.model_id, pricing);
    rows.push({
      key: `kimi:${m.provider_id}:${m.model_id}`,
      name: m.model_id,
      source: "Kimi",
      color: AGENT_COLOR.kimi,
      barBg: "bg-indigo-500/10",
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
      mergedNote: mergedNoteOf(m),
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
  const display = useResetDisplay();
  const remain = Math.max(0, 100 - usedPct);
  const showReset = resetAt != null && resetAt > now;
  // 重置时间三态同行文本：双开「↻ 45m · 08-30 14:37」；truncate 截断时 title 兜底
  let resetText = "";
  if (showReset) {
    if (display.countdown && display.datetime) {
      resetText = `↻ ${formatCountdownCore(resetAt - now)} · ${formatResetStamp(resetAt)}`;
    } else if (display.datetime) {
      resetText = `↻ ${t("common.resetAt", { time: formatResetStamp(resetAt) })}`;
    } else if (display.countdown) {
      resetText = `↻ ${formatCountdownCore(resetAt - now)}`;
    }
  }
  return (
    <div>
      <div className="flex items-center gap-1 mb-0.5">
        <span className="text-[9px] text-slate-500 w-10 shrink-0 whitespace-nowrap">{label}</span>
        <span
          className="num text-[8px] text-slate-400 flex-1 text-right truncate min-w-0"
          title={display.datetime ? resetText : undefined}
        >
          {resetText}
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

/** ZCode 多账号额度分组：快照 ≥2 时在额度监控区按账号逐组展示。
 *  每组子标题（账号名 + 当前徽标 + 等级徽标 + 非当前组的切换按钮）
 *  + 该账号的 5小时/每周/MCP 行——各账号的 MCP 月度额度相互独立，
 *  均逐账号展示；每个账号的每周行挂各自的今日增量
 *  （entry.today_delta 来自该账号的带指纹采样；当前账号由 DataCache
 *  用 30s live 值覆盖，两路同账号采样已按 (ts, account) 防抖合流）。 */
function AccountQuotaGroup({
  entries,
  now,
  onSwitch,
  switchDisabled,
}: {
  entries: AccountQuotaEntry[];
  now: number;
  /** 非当前账号的切换回调（点击弹确认） */
  onSwitch: (e: AccountQuotaEntry) => void;
  switchDisabled?: boolean;
}) {
  const { t } = useI18n();
  return (
    <div className="space-y-2">
      {entries.map((e) => {
        const q = e.quota;
        return (
          <div key={e.id}>
            <div className="flex items-center gap-1.5 mb-1 min-w-0">
              <span className="flex items-center gap-1.5 min-w-0 flex-1">
                <span className="text-[9px] font-medium text-slate-600 truncate">
                  {e.display_name}
                </span>
                {e.is_current && (
                  <span className="shrink-0 text-[8px] px-1 py-px rounded-full bg-violet-500/10 text-violet-600">
                    {t("settings.accountsCurrent")}
                  </span>
                )}
                {q?.level && (
                  <span className="shrink-0 px-1 py-px rounded text-[8px] font-medium bg-sky-500/12 text-sky-700">
                    {levelLabel(q.level)}
                  </span>
                )}
              </span>
              {!e.is_current && (
                <SwitchAccountButton
                  onClick={() => onSwitch(e)}
                  disabled={switchDisabled}
                />
              )}
            </div>
            {q ? (
              <div className="space-y-1">
                {q.hour5 && (
                  <QuotaMiniRow
                    label={t("common.hour5")}
                    usedPct={q.hour5.percentage}
                    resetAt={q.hour5.nextResetTime}
                    now={now}
                  />
                )}
                {q.weekly && (
                  <QuotaMiniRow
                    label={t("common.weekly")}
                    usedPct={q.weekly.percentage}
                    resetAt={q.weekly.nextResetTime}
                    now={now}
                    delta={
                      e.today_delta
                        ? { pct: e.today_delta[0], samples: e.today_delta[1] }
                        : undefined
                    }
                  />
                )}
                {q.mcp && (
                  <QuotaMiniRow
                    label="MCP"
                    usedPct={q.mcp.percentage}
                    resetAt={q.mcp.nextResetTime}
                    now={now}
                  />
                )}
              </div>
            ) : (
              <div
                className="text-[9px] text-rose-500/90"
                title={e.error ?? undefined}
              >
                ⚠ {t("quota.quotaFail")}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function SummaryTab({
  stats,
  cost,
  trend,
  codex,
  claude,
  cursor,
  kimi,
  currency,
  bucket,
  fxRate,
  pricing,
  agentVisibility,
}: Props) {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
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
    cursorError,
    kimiError,
    error: zaiError,
    agentQuotaDeltas,
    accountQuotas,
  } = useDataCache();
  // 额度监控区 ZCode 多账号分组的内嵌切换
  const sw = useAccountSwitch();
  const hour5 = quota?.hour5 ?? null;
  const weekly = quota?.weekly ?? null;
  const mcp = quota?.mcp ?? null;
  const planBadge = quota?.level ? levelLabel(quota.level) : null;
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

  // Kimi 花费 & token（与 Claude 同款口径）
  const kimiRate = kimi?.rate_limits ?? null;
  const kimiCostRaw = (kimi?.trend ?? []).reduce(
    (s, p) => s + (currency === "cny" ? p.cost_cny : p.cost_usd),
    0
  );
  const kimiTokens = kimi?.stats.overall.total_tokens ?? 0;

  // Cursor 花费 & token（events 口径）
  const cursorEvents = cursor?.events;
  const cursorCostRaw = cursorEvents?.total_cost_usd ?? 0;
  const cursorCost =
    currency === "cny" ? cursorCostRaw * fxRate : cursorCostRaw;
  const cursorTokens = cursorEvents?.total_tokens ?? 0;

  // Cursor 套餐进度：与客户端一致，只展示 Auto / API。
  // used_cents/limit_cents 是套餐标价口径，不能代表 Auto 额度。
  const plan = cursor?.plan;

  // 注意：匹配 Rust 后端返回的中文错误串固定前缀（quota.rs），仅做布尔分支，不能翻译
  const zcodeEmpty = quotaError
    ? quotaError.includes("未找到 ZCode Coding Plan 凭证")
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

  // Kimi 额度：配置 API Key 且接口成功才有实时值（未配置/失败 rate_limits 为 null → 空 metrics 走文案）
  const kimiMetrics =
    kimiRate &&
    (kimiRate.primary_pct != null ||
      kimiRate.secondary_pct != null ||
      kimiRate.monthly_pct != null)
      ? [
          ...(kimiRate.primary_pct != null
            ? [
                {
                  label: t("common.hour5"),
                  usedPct: kimiRate.primary_pct,
                  resetAt: kimiRate.primary_reset_at,
                },
              ]
            : []),
          ...(kimiRate.secondary_pct != null
            ? [
                {
                  label: t("common.weekly"),
                  usedPct: kimiRate.secondary_pct,
                  resetAt: kimiRate.secondary_reset_at,
                  delta: agentQuotaDeltas.kimi?.weekly,
                },
              ]
            : []),
          // 月总额度（totalQuota）：服务端当前普遍返回空对象，有值才追加
          ...(kimiRate.monthly_pct != null
            ? [
                {
                  label: t("stats.kimiMonthlyQuota"),
                  usedPct: kimiRate.monthly_pct,
                  resetAt: kimiRate.monthly_reset_at,
                },
              ]
            : []),
        ]
      : [];
  // 注意：正则匹配 Rust 后端返回的中文错误串（未找到/未安装/会话目录），仅做布尔分支，不能翻译
  const kimiEmpty = kimiError
    ? /未找到|未安装|会话目录/i.test(kimiError)
      ? t("stats.kimiNotFound")
      : t("summary.loadFailed")
    : t("summary.noData");

  // 各 Agent 缓存命中率（口径与各详情页一致）
  const zaiCacheHitPct = cacheHitPctIncluded(stats?.overall);
  const codexCacheHitPct = cacheHitPctIncluded(codex?.stats.overall);
  const claudeCacheHitPct = cacheHitPctSeparate(claude?.stats.overall);
  const cursorCacheHitPct = cacheHitPctCursor(cursorEvents);
  const kimiCacheHitPct = cacheHitPctSeparate(kimi?.stats.overall);

  // 按 agent 组装。新增来源时在此追加，总览占比条 / 花费表 / 额度列表会一起跟上。
  const allAgents: AgentSummary[] = [
    {
      id: "zai",
      name: "ZCode",
      color: AGENT_COLOR.zai,
      tintBg: "rgba(14, 165, 233, 0.07)",
      nameClass: "text-sky-700",
      badge: planBadge,
      badgeClass: "bg-sky-500/12 text-sky-700",
      cost: zaiCost,
      tokens: zaiTokens,
      // null = 加载中或加载失败终态；有结论（数据或错误）即视为已加载
      loaded: stats != null || zaiError != null,
      currentModel: stats?.current_model ?? null,
      avgTps: stats?.overall.avg_tps ?? null,
      avgTtftMs: stats?.overall.avg_ttft_ms ?? null,
      cacheHitPct: zaiCacheHitPct,
      metrics: zcodeMetrics,
      empty: zcodeEmpty,
    },
    {
      id: "codex",
      name: "Codex",
      color: AGENT_COLOR.codex,
      tintBg: "rgba(16, 163, 127, 0.07)",
      nameClass: "text-emerald-700",
      badge: codexRate?.plan_type ?? null,
      badgeClass: "bg-emerald-500/12 text-emerald-700",
      cost: codexCostRaw,
      tokens: codexTokens,
      // 有 error 说明已确认未安装/失败，允许被隐藏
      loaded: codex != null || codexError != null,
      currentModel: codex?.stats.current_model ?? null,
      avgTps: codex?.stats.overall.avg_tps ?? null,
      cacheHitPct: codexCacheHitPct,
      metrics: codexMetrics,
      empty: codexEmpty,
    },
    {
      id: "claude",
      name: "Claude",
      color: AGENT_COLOR.claude,
      tintBg: "rgba(217, 119, 87, 0.07)",
      nameClass: "text-orange-700",
      badge: claudeRate?.plan_type ?? null,
      badgeClass: "bg-orange-500/12 text-orange-700 capitalize",
      cost: claudeCostRaw,
      tokens: claudeTokens,
      // 有 error 说明已确认未安装/失败，允许被隐藏
      loaded: claude != null || claudeError != null,
      currentModel: claude?.stats.current_model ?? null,
      avgTps: claude?.stats.overall.avg_tps ?? null,
      cacheHitPct: claudeCacheHitPct,
      metrics: claudeMetrics,
      empty: claudeEmpty,
    },
    {
      id: "cursor",
      name: "Cursor",
      color: AGENT_COLOR.cursor,
      tintBg: "rgba(139, 92, 246, 0.07)",
      nameClass: "text-violet-700",
      badge: cursor?.membership_type ?? null,
      badgeClass: "bg-violet-500/12 text-violet-700 capitalize",
      cost: cursorCost,
      tokens: cursorTokens,
      // 有 error 说明已确认未配置/未登录/加载失败，允许被隐藏
      loaded: cursor != null || cursorError != null,
      currentModel: cursor?.current_model ?? null,
      cacheHitPct: cursorCacheHitPct,
      metrics: cursorMetrics,
      empty: cursor?.logged_in ? t("summary.noQuotaData") : t("summary.notLoggedIn"),
      cycleResetAt: cycleEndMs(cursor?.billing_cycle_end),
      cycleResetDate: cursor?.billing_cycle_end
        ? cursor.billing_cycle_end.slice(0, 10)
        : null,
    },
    {
      id: "kimi",
      name: "Kimi",
      color: AGENT_COLOR.kimi,
      tintBg: "rgba(67, 56, 202, 0.07)",
      nameClass: "text-indigo-700",
      badge: kimiRate?.plan_type ?? null,
      badgeClass: "bg-indigo-500/12 text-indigo-700",
      cost: kimiCostRaw,
      tokens: kimiTokens,
      // 有 error 说明已确认未安装/失败，允许被隐藏
      loaded: kimi != null || kimiError != null,
      currentModel: kimi?.stats.current_model ?? null,
      avgTps: kimi?.stats.overall.avg_tps ?? null,
      avgTtftMs: kimi?.stats.overall.avg_ttft_ms ?? null,
      cacheHitPct: kimiCacheHitPct,
      metrics: kimiMetrics,
      empty: kimiEmpty,
    },
  ];

  const agents = allAgents.filter((agent) => agentVisibility[agent.id]);
  // 花费卡片只展示"加载中或有实际消耗"的 agent：已加载完成且 cost/tokens 双零的没有参考价值。
  // tokens>0 但 cost=0（缺价格）的必须保留。
  const activeAgents = agents.filter(
    (a) => !a.loaded || a.cost > 0 || a.tokens > 0
  );
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
  // Codex / Claude / Kimi 趋势桶自带双货币花费，按 label 索引后直接相加（与 z.ai 桶格式一致）
  const codexTrendMap = new Map<string, TrendPoint>();
  (codex?.trend ?? []).forEach((p) => codexTrendMap.set(p.label, p));
  const claudeTrendMap = new Map<string, TrendPoint>();
  (claude?.trend ?? []).forEach((p) => claudeTrendMap.set(p.label, p));
  const kimiTrendMap = new Map<string, TrendPoint>();
  (kimi?.trend ?? []).forEach((p) => kimiTrendMap.set(p.label, p));

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
  if (agentVisibility.kimi)
    kimiTrendMap.forEach((_point, label) => addTrendLabel(label));

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
    const k = agentVisibility.kimi ? kimiTrendMap.get(label) : undefined;
    return {
      label: p.label,
      total_tokens:
        (agentVisibility.zai ? p.total_tokens : 0) +
        (x?.total_tokens ?? 0) +
        (a?.total_tokens ?? 0) +
        (k?.total_tokens ?? 0) +
        (c?.tokens ?? 0),
      requests:
        (agentVisibility.zai ? p.requests : 0) +
        (x?.requests ?? 0) +
        (a?.requests ?? 0) +
        (k?.requests ?? 0),
      cost_cny:
        (agentVisibility.zai ? p.cost_cny : 0) +
        (x?.cost_cny ?? 0) +
        (a?.cost_cny ?? 0) +
        (k?.cost_cny ?? 0) +
        (c?.costCny ?? 0),
      cost_usd:
        (agentVisibility.zai ? p.cost_usd : 0) +
        (x?.cost_usd ?? 0) +
        (a?.cost_usd ?? 0) +
        (k?.cost_usd ?? 0) +
        (c?.costUsd ?? 0),
    };
  });

  const modelRows = buildModelRows(
    stats,
    cost,
    codex,
    claude,
    cursor,
    kimi,
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
            {t("summary.sources", { count: activeAgents.length })}
          </StatusBadge>
        }
        footer={
          <MetricPair
            left={{ label: t("summary.totalTokens"), value: formatTokens(totalTokens) }}
            right={{ label: t("summary.costDist"), value: <CompositionBar agents={agents} /> }}
          />
        }
      />

      {activeAgents.length > 0 && (
        <div className="flex flex-col gap-1.5 shrink-0">
          {activeAgents.map((a) => (
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
              <div
                key={a.id}
                className={`px-1 py-2 first:pt-0 last:pb-0 ${
                  a.id === "zai" ? "relative" : ""
                }`}
              >
                <div className="flex items-center justify-between gap-1 mb-1.5">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: a.color }} />
                    <span className="text-[10px] font-semibold text-slate-700 shrink-0">{a.name}</span>
                    {a.badge && (
                      <span className={`shrink-0 px-1 py-px rounded text-[8px] font-medium ${a.badgeClass}`}>{a.badge}</span>
                    )}
                    {a.currentModel && (
                      <span className="text-[9px] text-slate-500/90 truncate" title={t("common.currentModel")}>
                        {a.currentModel.model_id}
                      </span>
                    )}
                  </div>
                  {a.cycleResetAt != null && a.cycleResetAt > now ? (
                    // Cursor 的 billing_cycle_end 只有日期精度：时间点仅出 MM-DD，
                    // 禁止显示由 ISO 日期解析出的假时分；双开「↻ 5d · 09-01」
                    resetDisplay.datetime ? (
                      <span className="num text-[8px] text-slate-400 shrink-0 whitespace-nowrap">
                        ↻{" "}
                        {resetDisplay.countdown
                          ? `${formatCountdownCore(a.cycleResetAt - now)} · ${formatResetStamp(a.cycleResetAt, { withTime: false })}`
                          : formatResetStamp(a.cycleResetAt, { withTime: false })}
                      </span>
                    ) : resetDisplay.countdown ? (
                      <span className="num text-[8px] text-slate-400 shrink-0 whitespace-nowrap">
                        ↻ {formatCountdownCore(a.cycleResetAt - now)}
                      </span>
                    ) : (
                      <span />
                    )
                  ) : a.cycleResetDate ? (
                    <span className="num text-[8px] text-slate-400 shrink-0 whitespace-nowrap">
                      {t("summary.resetDate", { date: a.cycleResetDate })}
                    </span>
                  ) : null}
                </div>
                {a.id === "zai" && accountQuotas.length >= 2 ? (
                  // 多账号：按账号分组展示（当前账号条目已由 DataCache 用 live quota 覆盖），
                  // 非当前账号组带切换按钮
                  <>
                    <AccountQuotaGroup
                      entries={accountQuotas}
                      now={now}
                      onSwitch={(e) => sw.request(e)}
                      switchDisabled={sw.switching}
                    />
                    {sw.notice && (
                      <p
                        className={`text-[9px] mt-1 leading-relaxed break-all ${
                          sw.notice.kind === "ok"
                            ? "text-emerald-600"
                            : "text-rose-600"
                        }`}
                      >
                        {sw.notice.text}
                      </p>
                    )}
                    {sw.confirming && (
                      <SwitchConfirmOverlay
                        account={sw.confirming}
                        switching={sw.switching}
                        onConfirm={sw.confirm}
                        onCancel={sw.cancel}
                      />
                    )}
                  </>
                ) : a.metrics.length > 0 ? (
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
                        title={
                          m.mergedNote
                            ? `${m.source} · ${m.name}（${m.mergedNote}）`
                            : `${m.source} · ${m.name}`
                        }
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
                    <div className="flex items-center gap-1 num shrink-0 whitespace-nowrap text-[10px]">
                      {/* 请求/Token/花费三列深浅递进（浅→中→深），便于扫视区分 */}
                      <span className="min-w-[1.5rem] text-right text-slate-500/80" title={t("common.requestCount")}>
                        {formatTokens(m.requests)}
                      </span>
                      <span className="min-w-[2rem] text-right text-slate-700" title={t("common.totalTokens")}>
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

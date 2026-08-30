import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ClaudeSnapshot,
  CodexSnapshot,
  Currency,
  CursorSnapshot,
  DeviceInfo,
  KimiSnapshot,
  PricingConfig,
  QuotaSnapshot,
  RangePreset,
  RemoteSnapshot,
  RemoteUsage,
  Stats,
  SyncConfig,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchKimiUsage,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getQuotaHistory,
  getSyncConfig,
  listRemoteDevices,
  remoteSnapshots,
  remoteUsage,
  saveReport,
} from "./api";
import { formatCost, formatResetStamp, formatTokens } from "./format";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  mergeStats,
  mergeTrend,
  modelCost,
  remoteToStats,
} from "./merge";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  foldCursorModelRows,
  foldModelStatRows,
  hasPositivePrice,
} from "./modelName";
import { type AgentId, type AgentVisibility } from "./agentVisibility";
import { TrendChart, remainingGradient } from "./widgets";
import {
  PageBody,
  PageFooter,
  SectionCard,
  BtnPrimary,
  BtnSecondary,
  AlertBanner,
  LoadingState,
} from "./layout";
import { RangePicker, resolveRange } from "./RangePicker";
import { ShareCardModal } from "./ShareCard";
import { useI18n, type MessageKey, type TFn } from "./i18n";
import { dateLocale, type Locale } from "./i18n/locale";
import { loadResetDisplay, useResetDisplay, type ResetDisplay } from "./resetDisplay";

interface Props {
  pricing: PricingConfig;
  currency: Currency;
  agentVisibility: AgentVisibility;
}

type ReportMetric = "requests" | "cost" | "token";

interface ReportSource {
  stats: Stats;
  trend: TrendPoint[];
}

interface ReportModel {
  key: string;
  agentId: AgentId;
  model_id: string;
  provider_id: string;
  requests: number;
  total_tokens: number;
  cost_cny: number;
  cost_usd: number;
  priced: boolean;
}

interface ReportAgent {
  id: AgentId;
  label: string;
  brand: BrandIconName;
  color: string;
  requests: number;
  total_tokens: number;
  cost_cny: number;
  cost_usd: number;
  trend: TrendPoint[];
  models: ReportModel[];
}

interface ReportQuotaWindow {
  /** 词典键：渲染 / 导出 Markdown 时查（跟随 UI 语言） */
  labelKey: MessageKey;
  usedPct: number;
  resetAt: number | null;
}

interface ReportQuota {
  id: AgentId;
  label: string;
  brand: BrandIconName;
  color: string;
  windows: ReportQuotaWindow[];
  accountLevel?: boolean;
}

interface LoadedSource {
  source: ReportSource;
  quota: ReportQuota | null;
}

interface ReportData {
  from_ms: number;
  to_ms: number;
  bucket: TrendBucket;
  agents: ReportAgent[];
  trend: TrendPoint[];
  agentQuotas: ReportQuota[];
  warnings: string[];
  notes: string[];
}

const DAY_MS = 86_400_000;

const AGENT_META: Record<
  AgentId,
  { label: string; brand: BrandIconName; color: string }
> = {
  zai: { label: "Z.ai", brand: "zai", color: "#0284c7" },
  codex: { label: "Codex", brand: "codex", color: "#059669" },
  claude: { label: "Claude", brand: "claude", color: "#c2410c" },
  cursor: { label: "Cursor", brand: "cursor", color: "#7c3aed" },
  kimi: { label: "Kimi", brand: "kimi", color: "#4338ca" },
  // 凭证驱动的新 provider（报告暂只统计前 5 个，元数据先补齐保证类型完整）
  gemini: { label: "Gemini", brand: "gemini", color: "#2f6fe0" },
  grok: { label: "Grok", brand: "grok", color: "#0f7ecb" },
  qoder: { label: "Qoder", brand: "qoder", color: "#5a4bd0" },
  opencodego: { label: "OpenCode", brand: "opencodego", color: "#d96110" },
  minimax: { label: "MiniMax", brand: "minimax", color: "#009657" },
  moonshot: { label: "Moonshot", brand: "moonshot", color: "#3833b4" },
  deepseek: { label: "DeepSeek", brand: "deepseek", color: "#3a53d0" },
  longcat: { label: "LongCat", brand: "longcat", color: "#c9a400" },
  mimo: { label: "MiMo", brand: "mimo", color: "#d95700" },
  alibaba: { label: "通义灵码", brand: "alibaba", color: "#d95700" },
  alibabatoken: { label: "百炼Token包", brand: "alibabatoken", color: "#e5772c" },
  stepfun: { label: "StepFun", brand: "stepfun", color: "#2d68cc" },
  doubao: { label: "火山引擎", brand: "doubao", color: "#00a486" },
};

/** 本地日期 YYYY-MM-DD，避免 UTC 偏移。 */
function localDateStr(ms: number): string {
  const d = new Date(ms);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return y + "-" + m + "-" + day;
}

/** 本地日期偏移 n 天后格式化（用 Date 加减，避免毫秒减法的 DST 偏移）。 */
function offsetLocalDateStr(ms: number, days: number): string {
  const d = new Date(ms);
  d.setDate(d.getDate() + days);
  return localDateStr(d.getTime());
}

function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 结束日整天包含，取当天 23:59:59.999（本地时区）。 */
function endOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(23, 59, 59, 999);
  return d.getTime();
}

/** 解析 YYYY-MM-DD 为本地当天 0 点，格式非法时返回 NaN。 */
function parseLocalDate(date: string): number {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y, (m ?? 1) - 1, d ?? 1).getTime();
}

/** 解析所选范围并收敛到界内：from 下限 = 89 天前 0 点（数据只保留 90 天），
 *  to 上限 = 今天 23:59:59.999（自定义可点到未来日期），倒置时交换。
 *  today/1d/7d/30d 的 to=now 本就在界内，防御不改变其行为。 */
function boundedRange(
  preset: RangePreset,
  custom: { from: string; to: string },
  now: number
): [number, number] {
  const raw = resolveRange(preset, custom);
  const minFrom = startOfLocalDay(
    parseLocalDate(offsetLocalDateStr(now, -89))
  );
  const maxTo = endOfLocalDay(now);
  const from = Math.max(raw[0], minFrom);
  const to = Math.min(raw[1], maxTo);
  return from <= to ? [from, to] : [to, from];
}

function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 90 ? text.slice(0, 87) + "…" : text;
}

function makeAgent(
  id: AgentId,
  source: ReportSource,
  pricing: PricingConfig,
  fxRate: number
): ReportAgent {
  const meta = AGENT_META[id];
  // 报告数据由本组件自拉自合（不经 DataCache），折叠在此处收口
  const models = foldModelStatRows(source.stats.by_model).map((model) => ({
    key: id + "|" + model.provider_id + "|" + model.model_id,
    agentId: id,
    model_id: model.model_id,
    provider_id: model.provider_id,
    requests: model.requests,
    total_tokens: model.total_tokens,
    cost_cny: modelCost(
      model.model_id,
      model.input_tokens,
      model.output_tokens,
      model.cache_read_tokens,
      pricing,
      "cny",
      fxRate
    ),
    cost_usd: modelCost(
      model.model_id,
      model.input_tokens,
      model.output_tokens,
      model.cache_read_tokens,
      pricing,
      "usd",
      fxRate
    ),
    priced: hasPositivePrice(model.model_id, pricing),
  }));

  return {
    id,
    label: meta.label,
    brand: meta.brand,
    color: meta.color,
    requests: source.stats.overall.requests,
    total_tokens: source.stats.overall.total_tokens,
    cost_cny: models.reduce((sum, model) => sum + model.cost_cny, 0),
    cost_usd: models.reduce((sum, model) => sum + model.cost_usd, 0),
    trend: source.trend,
    models,
  };
}

/** Cursor 返回的是按日明细，转换成报告统一的 Agent 结构。 */
function makeCursorAgent(
  snapshot: CursorSnapshot,
  fromMs: number,
  toMs: number,
  fxRate: number
): ReportAgent | null {
  if (
    !snapshot.logged_in &&
    snapshot.by_model.length === 0 &&
    !snapshot.events
  ) {
    return null;
  }

  // Cursor 数据同理由本组件自拉，折叠在此处收口
  const models: ReportModel[] = foldCursorModelRows(snapshot.by_model).map((model) => ({
    key: "cursor|cursor|" + model.model,
    agentId: "cursor",
    model_id: model.model,
    provider_id: "cursor",
    requests: model.requests,
    total_tokens: model.total_tokens,
    cost_usd: model.cost_usd,
    cost_cny: model.cost_usd * fxRate,
    priced: true,
  }));
  const modelTotals = models.reduce(
    (sum, model) => ({
      requests: sum.requests + model.requests,
      input_tokens: sum.input_tokens,
      output_tokens: sum.output_tokens,
      cache_read_tokens: sum.cache_read_tokens,
      total_tokens: sum.total_tokens + model.total_tokens,
    }),
    {
      requests: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      total_tokens: 0,
    }
  );
  const events = snapshot.events;
  const stats: Stats = {
    from_ms: fromMs,
    to_ms: toMs,
    overall: {
      requests: events?.requests ?? modelTotals.requests,
      input_tokens: events?.input_tokens ?? modelTotals.input_tokens,
      output_tokens: events?.output_tokens ?? modelTotals.output_tokens,
      cache_read_tokens:
        events?.cache_read_tokens ?? modelTotals.cache_read_tokens,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: events?.total_tokens ?? modelTotals.total_tokens,
    },
    by_model: models.map((model) => ({
      model_id: model.model_id,
      provider_id: model.provider_id,
      requests: model.requests,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: model.total_tokens,
    })),
    earliest_ms: null,
    latest_ms: null,
  };
  const costUsd =
    events?.total_cost_usd ??
    models.reduce((sum, model) => sum + model.cost_usd, 0);

  return {
    id: "cursor",
    label: AGENT_META.cursor.label,
    brand: AGENT_META.cursor.brand,
    color: AGENT_META.cursor.color,
    requests: stats.overall.requests,
    total_tokens: stats.overall.total_tokens,
    cost_cny: costUsd * fxRate,
    cost_usd: costUsd,
    trend: snapshot.daily.map((point) => ({
      label: point.date,
      total_tokens: point.total_tokens,
      requests: point.requests,
      cost_usd: point.cost_usd,
      cost_cny: point.cost_usd * fxRate,
    })),
    models,
  };
}

function hasUsage(source: ReportSource): boolean {
  return (
    source.stats.overall.requests > 0 ||
    source.stats.overall.total_tokens > 0 ||
    source.trend.some(
      (point) =>
        point.requests > 0 ||
        point.total_tokens > 0 ||
        point.cost_cny > 0 ||
        point.cost_usd > 0
    )
  );
}

/** 把本地来源和远端同来源数据合成一份，避免报告重复计算。 */
function combineSource(
  local: ReportSource | null,
  remote: RemoteUsage | null,
  pricing: PricingConfig,
  fxRate: number,
  bucket: TrendBucket
): ReportSource | null {
  if (local && remote) {
    return {
      stats: mergeStats(local.stats, remote),
      trend: mergeTrend(local.trend, remote, pricing, fxRate, bucket),
    };
  }
  if (local) return local;
  if (remote) {
    return {
      stats: remoteToStats(remote),
      trend: mergeTrend([], remote, pricing, fxRate, bucket),
    };
  }
  return null;
}

/** 生成排序基准的期望标签序列：day 桶逐日 "MM-DD"（跨年也按时间顺序），
 *  hour 桶仅单日、逐小时 "HH:00"，均与后端 db.rs 的标签格式一致。 */
function expectedTrendLabels(
  bucket: TrendBucket,
  fromMs: number,
  toMs: number
): Map<string, number> {
  const order = new Map<string, number>();
  if (bucket === "hour") {
    for (let h = 0; h < 24; h++) {
      order.set(String(h).padStart(2, "0") + ":00", h);
    }
    return order;
  }
  const cursor = new Date(startOfLocalDay(fromMs));
  const end = startOfLocalDay(toMs);
  while (cursor.getTime() <= end && order.size < 400) {
    const label =
      String(cursor.getMonth() + 1).padStart(2, "0") +
      "-" +
      String(cursor.getDate()).padStart(2, "0");
    if (!order.has(label)) order.set(label, order.size);
    cursor.setDate(cursor.getDate() + 1);
  }
  return order;
}

function mergeReportTrends(
  agents: ReportAgent[],
  bucket: TrendBucket,
  fromMs: number,
  toMs: number
): TrendPoint[] {
  const byLabel = new Map<string, TrendPoint>();
  for (const agent of agents) {
    for (const point of agent.trend) {
      const previous = byLabel.get(point.label);
      if (!previous) {
        byLabel.set(point.label, { ...point });
      } else {
        previous.total_tokens += point.total_tokens;
        previous.requests += point.requests;
        previous.cost_cny += point.cost_cny;
        previous.cost_usd += point.cost_usd;
      }
    }
  }
  // 按时间范围生成的期望序列排序，避免 "MM-DD" 标签跨年时按字典序错乱；
  // 序列外的标签（如 Cursor 的 "YYYY-MM-DD"）排在最后。
  const order = expectedTrendLabels(bucket, fromMs, toMs);
  const rank = (label: string) => order.get(label) ?? Number.MAX_SAFE_INTEGER;
  return [...byLabel.values()].sort((a, b) => rank(a.label) - rank(b.label));
}

function toQuotaSnapshot(snapshot: RemoteSnapshot): QuotaSnapshot {
  const { device_id: _deviceId, ...quotaSnapshot } = snapshot;
  return quotaSnapshot;
}

function mergeQuotaSnapshots(
  local: QuotaSnapshot[],
  remote: QuotaSnapshot[]
): QuotaSnapshot[] {
  const byTs = new Map<number, QuotaSnapshot>();
  for (const snapshot of [...local, ...remote]) {
    if (!Number.isFinite(snapshot.ts)) continue;
    const previous = byTs.get(snapshot.ts);
    if (
      !previous ||
      snapshot.weekly_pct > previous.weekly_pct ||
      (snapshot.weekly_pct === previous.weekly_pct &&
        snapshot.hour5_pct > previous.hour5_pct)
    ) {
      byTs.set(snapshot.ts, snapshot);
    }
  }
  return [...byTs.values()].sort((a, b) => a.ts - b.ts);
}

function selectedCost(
  item: { cost_cny: number; cost_usd: number },
  currency: Currency
): number {
  return currency === "cny" ? item.cost_cny : item.cost_usd;
}

/** 当前筛选指标取值：请求 / 花费（随币种）/ Token，全页维度统一跟随。 */
function metricValue(
  item: {
    requests: number;
    total_tokens: number;
    cost_cny: number;
    cost_usd: number;
  },
  metric: ReportMetric,
  currency: Currency
): number {
  if (metric === "requests") return item.requests;
  if (metric === "token") return item.total_tokens;
  return currency === "cny" ? item.cost_cny : item.cost_usd;
}

/** 重置倒计时描述（文案走词典，模式 B：由调用方传 t 与展示偏好）。
 *  双开「约 N 小时后 · 重置于 08-30 14:37」；仅时间点「重置于 08-30 14:37」。 */
function quotaResetText(
  t: TFn,
  resetAt: number | null,
  now: number,
  display: ResetDisplay
): string {
  if (!resetAt) return t("report.resetUnknown");
  const hours = Math.max(0, Math.ceil((resetAt - now) / 3_600_000));
  const days = Math.ceil(hours / 24);
  if (display.datetime) {
    const stamp = t("common.resetAt", { time: formatResetStamp(resetAt) });
    // 仅时间点模式：resetAt 已过期（<= now）不显示过去时刻，兜底为未知；
    // 倒计时（双开 / 仅倒计时）保持原有照常显示的行为。
    if (!display.countdown) {
      return resetAt <= now ? t("report.resetUnknown") : stamp;
    }
    return (
      (hours >= 24
        ? t("report.resetInDays", { n: days })
        : t("report.resetInHours", { n: hours })) +
      " · " +
      stamp
    );
  }
  if (hours >= 24) return t("report.resetDays", { n: days });
  return t("report.resetHours", { n: hours });
}

function formatQuotaPct(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return String(Math.round(value * 100) / 100);
}

function makeZaiQuota(snapshots: QuotaSnapshot[]): ReportQuota | null {
  if (snapshots.length === 0) return null;
  const latest = snapshots[snapshots.length - 1];
  const windows: ReportQuotaWindow[] = [
    {
      labelKey: "report.q.weeklyCurrent",
      usedPct: latest.weekly_pct,
      resetAt: latest.weekly_reset,
    },
    { labelKey: "report.q.hour5Current", usedPct: latest.hour5_pct, resetAt: null },
    {
      labelKey: "report.q.weeklyPeak",
      usedPct: Math.max(...snapshots.map((snapshot) => snapshot.weekly_pct)),
      resetAt: latest.weekly_reset,
    },
    {
      labelKey: "report.q.hour5Peak",
      usedPct: Math.max(...snapshots.map((snapshot) => snapshot.hour5_pct)),
      resetAt: null,
    },
  ];
  if (
    latest.mcp_total != null ||
    latest.mcp_used != null ||
    latest.mcp_pct > 0
  ) {
    windows.push({ labelKey: "report.q.mcp", usedPct: latest.mcp_pct, resetAt: null });
  }
  return {
    id: "zai",
    label: AGENT_META.zai.label,
    brand: AGENT_META.zai.brand,
    color: AGENT_META.zai.color,
    windows,
    accountLevel: true,
  };
}

function makeRateLimitQuota(
  id: "codex" | "claude" | "kimi",
  rateLimits: {
    primary_pct: number | null;
    primary_reset_at: number | null;
    secondary_pct: number | null;
    secondary_reset_at: number | null;
  } | null
): ReportQuota | null {
  if (!rateLimits) return null;
  const windows: ReportQuotaWindow[] = [];
  if (rateLimits.primary_pct != null) {
    windows.push({
      labelKey: "report.q.hour5",
      usedPct: rateLimits.primary_pct,
      resetAt: rateLimits.primary_reset_at,
    });
  }
  if (rateLimits.secondary_pct != null) {
    windows.push({
      labelKey: "report.q.weekly",
      usedPct: rateLimits.secondary_pct,
      resetAt: rateLimits.secondary_reset_at,
    });
  }
  if (windows.length === 0) return null;
  return {
    id,
    label: AGENT_META[id].label,
    brand: AGENT_META[id].brand,
    color: AGENT_META[id].color,
    windows,
  };
}

function makeCursorQuota(snapshot: CursorSnapshot): ReportQuota | null {
  const windows: ReportQuotaWindow[] = [];
  const plan = snapshot.plan;
  if (plan) {
    if (plan.auto_pct != null) {
      windows.push({ labelKey: "report.q.auto", usedPct: plan.auto_pct, resetAt: null });
    }
    if (plan.api_pct != null) {
      windows.push({ labelKey: "report.q.api", usedPct: plan.api_pct, resetAt: null });
    }
    if (windows.length === 0 && plan.total_pct != null) {
      windows.push({
        labelKey: "report.q.plan",
        usedPct: plan.total_pct,
        resetAt: null,
      });
    }
  }
  const onDemand = snapshot.on_demand;
  if (
    onDemand?.used_cents != null &&
    onDemand.limit_cents != null &&
    onDemand.limit_cents > 0
  ) {
    windows.push({
      labelKey: "report.q.onDemand",
      usedPct: (onDemand.used_cents / onDemand.limit_cents) * 100,
      resetAt: null,
    });
  }
  if (windows.length === 0) return null;
  return {
    id: "cursor",
    label: AGENT_META.cursor.label,
    brand: AGENT_META.cursor.brand,
    color: AGENT_META.cursor.color,
    windows,
  };
}

/**
 * 用量报告内容区：供 ReportsPanel 壳按标签复用，页面外壳（返回/标题栏）
 * 由壳统一渲染，此处保留工具区（范围选择 + 设备筛选 + 分享/刷新）、
 * 报表主体与底部导出操作栏。
 */
export function ReportContent({
  pricing,
  currency,
  agentVisibility,
}: Props) {
  const { locale, t } = useI18n();
  const [preset, setPreset] = useState<RangePreset>("today");
  const [metric, setMetric] = useState<ReportMetric>("cost");
  // 自定义范围默认近 30 天（起 = 29 天前，止 = 今天）。
  const [custom, setCustom] = useState(() => ({
    from: localDateStr(Date.now() - 29 * DAY_MS),
    to: localDateStr(Date.now()),
  }));
  const [report, setReport] = useState<ReportData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [doneFlash, setDoneFlash] = useState<string | null>(null);
  // 分享卡片弹层（默认生成近 7 天卡片，数据由弹层自拉）
  const [shareOpen, setShareOpen] = useState(false);

  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState("all");
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;
  const loadReqId = useRef(0);

  useEffect(() => {
    getSyncConfig()
      .then((config) => {
        setSyncConfig(config);
        if (config.enabled && config.device_token) {
          listRemoteDevices()
            .then(setRemoteDevices)
            .catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    const reqId = ++loadReqId.current;
    const now = Date.now();
    const [fromMs, toMs] = boundedRange(preset, custom, now);
    // 桶规则：起止落在同一自然日用小时（"今天"与单日自定义），跨日一律按天；
    // 后端 hour 标签只有 "HH:00" 无日期，跨天会折叠重复标签（含 1d 滚动 24h）。
    const daySpan = Math.round(
      (startOfLocalDay(toMs) - startOfLocalDay(fromMs)) / DAY_MS
    );
    const bucket: TrendBucket = daySpan === 0 ? "hour" : "day";
    const wantLocal = deviceFilter === "all" || deviceFilter === "local";
    const wantRemote =
      syncEnabled &&
      deviceFilter !== "local" &&
      !!syncConfig?.device_token;
    const safe = async <T,>(task: () => Promise<T>): Promise<T | null> => {
      try {
        return await task();
      } catch {
        // 可选 Agent 未安装、未登录或没有会话时按空数据处理，不打扰页面。
        return null;
      }
    };

    setLoading(true);
    setError(null);

    const localZaiPromise: Promise<ReportSource | null> =
      agentVisibility.zai && wantLocal
        ? safe(async () => {
            const [stats, trend] = await Promise.all([
              fetchStats(fromMs, toMs),
              fetchTrend(fromMs, toMs, bucket),
            ]);
            return { stats, trend };
          })
        : Promise.resolve(null);
    const localCodexPromise: Promise<LoadedSource | null> =
      agentVisibility.codex && wantLocal
        ? safe(async () => {
            const snapshot: CodexSnapshot = await fetchCodexUsage(
              fromMs,
              toMs,
              bucket
            );
            return {
              source: { stats: snapshot.stats, trend: snapshot.trend },
              quota: makeRateLimitQuota("codex", snapshot.rate_limits),
            };
          })
        : Promise.resolve(null);
    const localClaudePromise: Promise<LoadedSource | null> =
      agentVisibility.claude && wantLocal
        ? safe(async () => {
            const snapshot: ClaudeSnapshot = await fetchClaudeUsage(
              fromMs,
              toMs,
              bucket
            );
            return {
              source: { stats: snapshot.stats, trend: snapshot.trend },
              quota: makeRateLimitQuota("claude", snapshot.rate_limits),
            };
          })
        : Promise.resolve(null);
    const localCursorPromise: Promise<CursorSnapshot | null> =
      agentVisibility.cursor && wantLocal
        ? safe(() => fetchCursorUsage(fromMs, toMs))
        : Promise.resolve(null);
    // Kimi 首期无远端同步，只读本机快照（未安装时 safe 兜底为 null）
    const localKimiPromise: Promise<LoadedSource | null> =
      agentVisibility.kimi && wantLocal
        ? safe(async () => {
            const snapshot: KimiSnapshot = await fetchKimiUsage(
              fromMs,
              toMs,
              bucket
            );
            return {
              source: { stats: snapshot.stats, trend: snapshot.trend },
              quota: makeRateLimitQuota("kimi", snapshot.rate_limits),
            };
          })
        : Promise.resolve(null);

    const remoteOptions = (source: string) =>
      deviceFilter === "all"
        ? { excludeDevice: syncConfig?.device_id ?? "", source }
        : { devices: deviceFilter, source };
    const remoteZaiPromise: Promise<RemoteUsage | null> =
      agentVisibility.zai && wantRemote
        ? safe(() =>
            remoteUsage(fromMs, toMs, bucket, remoteOptions("zcode"))
          )
        : Promise.resolve(null);
    const remoteCodexPromise: Promise<RemoteUsage | null> =
      agentVisibility.codex && wantRemote
        ? safe(() =>
            remoteUsage(fromMs, toMs, bucket, remoteOptions("codex"))
          )
        : Promise.resolve(null);
    const remoteClaudePromise: Promise<RemoteUsage | null> =
      agentVisibility.claude && wantRemote
        ? safe(() =>
            remoteUsage(fromMs, toMs, bucket, remoteOptions("claude"))
          )
        : Promise.resolve(null);

    const fxRatePromise = getCursorConfig()
      .then((config) =>
        config.usd_cny_rate > 0 ? config.usd_cny_rate : 7.2
      )
      .catch(() => 7.2);
    const localQuotaPromise: Promise<QuotaSnapshot[] | null> = wantLocal
      ? safe(async () =>
          (await getQuotaHistory()).filter(
            (snapshot) => snapshot.ts >= fromMs && snapshot.ts <= toMs
          )
        )
      : Promise.resolve([]);
    const remoteQuotaPromise: Promise<RemoteSnapshot[] | null> =
      wantRemote && syncConfig
        ? safe(() =>
            remoteSnapshots(fromMs, toMs, {
              excludeDevice:
                deviceFilter === "all" ? syncConfig.device_id : undefined,
              devices: deviceFilter === "all" ? undefined : deviceFilter,
            })
          )
        : Promise.resolve([]);

    try {
      const [
        localZai,
        localCodex,
        localClaude,
        localCursor,
        localKimi,
        remoteZai,
        remoteCodex,
        remoteClaude,
        fxRate,
        localQuota,
        remoteQuota,
      ] = await Promise.all([
        localZaiPromise,
        localCodexPromise,
        localClaudePromise,
        localCursorPromise,
        localKimiPromise,
        remoteZaiPromise,
        remoteCodexPromise,
        remoteClaudePromise,
        fxRatePromise,
        localQuotaPromise,
        remoteQuotaPromise,
      ]);

      if (reqId !== loadReqId.current) return;

      const agents: ReportAgent[] = [];
      const addSourceAgent = (
        id: AgentId,
        local: ReportSource | null,
        remote: RemoteUsage | null
      ) => {
        if (!agentVisibility[id]) return;
        const source = combineSource(
          local,
          remote,
          pricing,
          fxRate,
          bucket
        );
        if (source && hasUsage(source)) {
          agents.push(makeAgent(id, source, pricing, fxRate));
        }
      };

      addSourceAgent("zai", localZai, remoteZai);
      addSourceAgent("codex", localCodex?.source ?? null, remoteCodex);
      addSourceAgent("claude", localClaude?.source ?? null, remoteClaude);
      // Kimi 无远端来源，只合并本机数据
      addSourceAgent("kimi", localKimi?.source ?? null, null);
      const cursorAgent =
        agentVisibility.cursor && localCursor
          ? makeCursorAgent(localCursor, fromMs, toMs, fxRate)
          : null;
      if (cursorAgent && hasUsage({
        stats: {
          from_ms: fromMs,
          to_ms: toMs,
          overall: {
            requests: cursorAgent.requests,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: cursorAgent.total_tokens,
          },
          by_model: [],
          earliest_ms: null,
          latest_ms: null,
        },
        trend: cursorAgent.trend,
      })) {
        agents.push(cursorAgent);
      }

      // Cursor 官方明细按日返回，小时桶趋势无法混入，需剔除并加说明；
      // 判断依据是 bucket 而非 kind（自定义短跨度同样走小时桶）。
      const trendAgents =
        bucket === "hour"
          ? agents.filter((agent) => agent.id !== "cursor")
          : agents;
      const notes: string[] = [];
      if (bucket === "hour" && cursorAgent) {
        notes.push(t("report.noteCursorDaily"));
      }

      const mergedQuotaSnapshots = mergeQuotaSnapshots(
        localQuota ?? [],
        (remoteQuota ?? []).map(toQuotaSnapshot)
      );
      const agentQuotas: ReportQuota[] = [];
      if (agentVisibility.zai) {
        const zaiQuota = makeZaiQuota(mergedQuotaSnapshots);
        if (zaiQuota) agentQuotas.push(zaiQuota);
      }
      if (agentVisibility.codex && localCodex?.quota) {
        agentQuotas.push(localCodex.quota);
      }
      if (agentVisibility.claude && localClaude?.quota) {
        agentQuotas.push(localClaude.quota);
      }
      if (agentVisibility.kimi && localKimi?.quota) {
        agentQuotas.push(localKimi.quota);
      }
      if (agentVisibility.cursor && localCursor) {
        const cursorQuota = makeCursorQuota(localCursor);
        if (cursorQuota) agentQuotas.push(cursorQuota);
      }

      setReport({
        from_ms: fromMs,
        to_ms: toMs,
        bucket,
        agents,
        trend: mergeReportTrends(trendAgents, bucket, fromMs, toMs),
        agentQuotas,
        warnings: [],
        notes,
      });
    } catch (e) {
      if (reqId === loadReqId.current) {
        setError(shortError(e));
      }
    } finally {
      if (reqId === loadReqId.current) setLoading(false);
    }
  }, [
    agentVisibility,
    custom,
    deviceFilter,
    preset,
    pricing,
    syncConfig,
    syncEnabled,
    t,
  ]);

  useEffect(() => {
    load();
  }, [load]);

  const totals = useMemo(() => {
    const agents = report?.agents ?? [];
    return {
      cost_cny: agents.reduce((sum, agent) => sum + agent.cost_cny, 0),
      cost_usd: agents.reduce((sum, agent) => sum + agent.cost_usd, 0),
      total_tokens: agents.reduce(
        (sum, agent) => sum + agent.total_tokens,
        0
      ),
      requests: agents.reduce((sum, agent) => sum + agent.requests, 0),
    };
  }, [report]);

  const allModels = useMemo(
    () => (report?.agents ?? []).flatMap((agent) => agent.models),
    [report]
  );
  const topModels = useMemo(
    () =>
      [...allModels]
        .sort((a, b) => {
          const diff =
            metricValue(b, metric, currency) - metricValue(a, metric, currency);
          return diff !== 0 ? diff : b.total_tokens - a.total_tokens;
        })
        .slice(0, 6),
    [allModels, currency, metric]
  );
  const unpricedTokens = useMemo(
    () =>
      allModels
        .filter((model) => !model.priced)
        .reduce((sum, model) => sum + model.total_tokens, 0),
    [allModels]
  );
  const topTrend = useMemo(() => {
    if (!report || report.trend.length === 0) return null;
    return report.trend.reduce(
      (best, point) =>
        metricValue(point, metric, currency) >
        metricValue(best, metric, currency)
          ? point
          : best,
      report.trend[0]
    );
  }, [currency, metric, report]);

  // Agent 分布：按当前指标降序 + 全体指标总和作占比分母
  const sortedAgents = useMemo(
    () =>
      [...(report?.agents ?? [])].sort(
        (a, b) =>
          metricValue(b, metric, currency) - metricValue(a, metric, currency)
      ),
    [report, metric, currency]
  );
  const agentTotal = useMemo(
    () =>
      (report?.agents ?? []).reduce(
        (sum, agent) => sum + metricValue(agent, metric, currency),
        0
      ),
    [report, metric, currency]
  );
  const modelTotal = useMemo(
    () =>
      allModels.reduce(
        (sum, model) => sum + metricValue(model, metric, currency),
        0
      ),
    [allModels, metric, currency]
  );

  const markdown = buildMarkdown(preset, report, currency, t, locale);
  // 文件名拼入实际所选范围（与 load 相同的收敛解析，不依赖 report 是否已
  // 加载），避免同日导出不同范围时互相覆盖。
  const [rangeFromMs, rangeToMs] = boundedRange(preset, custom, Date.now());
  const rangeText =
    localDateStr(rangeFromMs) === localDateStr(rangeToMs)
      ? localDateStr(rangeFromMs)
      : localDateStr(rangeFromMs) + "~" + localDateStr(rangeToMs);
  const filename =
    preset === "today"
      ? t("report.file.daily") + localDateStr(Date.now()) + ".md"
      : t("report.file.custom") + rangeText + ".md";

  const handleCopy = async () => {
    try {
      await writeText(markdown);
      setDoneFlash(t("report.copied"));
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(shortError(e));
    }
  };

  const handleSave = async () => {
    setError(null);
    try {
      await saveReport(markdown, filename);
      setDoneFlash(t("report.savedOpened"));
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(shortError(e));
    }
  };

  return (
    <>
      {/* 工具区：范围选择 + 设备筛选 + 分享/刷新（原页面级 PageHeader 的操作区，
          页面外壳上移 ReportsPanel 后保留在此） */}
      <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8 shrink-0 space-y-2">
        <RangePicker
          preset={preset}
          custom={custom}
          min={offsetLocalDateStr(Date.now(), -89)}
          onChange={(nextPreset, nextCustom) => {
            setPreset(nextPreset);
            setCustom(nextCustom);
          }}
        />
        <div className="flex items-center justify-between gap-2">
          {syncEnabled ? (
            <select
              value={deviceFilter}
              onChange={(event) => setDeviceFilter(event.target.value)}
              className="input-box num flex-1 min-w-0 text-[10px] py-1"
              title={t("stats.deviceFilter")}
            >
              <option value="all">{t("report.allDevices")}</option>
              <option value="local">
                {syncConfig?.device_name
                  ? t("stats.deviceLocalName", { name: syncConfig.device_name })
                  : t("stats.deviceLocal")}
              </option>
              {remoteDevices.filter((device) => device.device_id !== syncConfig?.device_id).map((device) => (
                <option key={device.device_id} value={device.device_id}>
                  {t("common.deviceOption", {
                    name: device.device_name,
                    id: device.device_id.slice(0, 6),
                  })}
                </option>
              ))}
            </select>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-0.5 shrink-0">
            <button
              onClick={() => setShareOpen(true)}
              className="toolbar-btn"
              title={t("share.button")}
            >
              ✦
            </button>
            <button onClick={load} disabled={loading} className="toolbar-btn" title={t("report.refresh")}>↻</button>
          </div>
        </div>
      </div>

      {(error || (report?.warnings.length ?? 0) > 0) && (
        <div className="px-3 pt-2 space-y-1">
          {error && <AlertBanner>{error}</AlertBanner>}
          {report?.warnings.length ? (
            <AlertBanner type="warning">
              {report.warnings.join(locale === "zh" ? "；" : "; ")}
            </AlertBanner>
          ) : null}
        </div>
      )}

      <PageBody>
        {loading && !report ? (
          <LoadingState text={t("report.loading")} />
        ) : !report || report.agents.length === 0 ? (
          <SectionCard>
            <div className="py-6 text-center">
              <div className="text-sm text-slate-800/80">{t("report.emptyTitle")}</div>
              <div className="text-[10px] text-slate-500 leading-relaxed mt-1.5">{t("report.emptyHint")}</div>
            </div>
          </SectionCard>
        ) : (
          <div className="page-stack">
            {loading && (
              <div className="text-[10px] text-sky-700/70">{t("report.refreshing")}</div>
            )}

            <div className="grid grid-cols-2 gap-1.5">
              <MetricCard label={t("common.totalCost")} value={formatCost(currency === "cny" ? totals.cost_cny : totals.cost_usd, currency)} hint={currency === "cny" ? t("report.cnyHint") : t("report.usdHint")} />
              <MetricCard label="Token" value={formatTokens(totals.total_tokens)} hint={t("report.tokenHint")} />
              <MetricCard label={t("common.requests")} value={totals.requests.toLocaleString()} hint={t("report.requestsHint")} />
              <MetricCard label={t("report.activeAgents")} value={String(report.agents.length)} hint={report.agents.length === 1 ? t("report.agentsHintOne") : t("report.agentsHint")} />
            </div>

            {/* TrendChart 自带卡片外壳与标题，直接渲染避免双层卡片 */}
            {report.trend.length > 0 ? (
              <TrendChart
                points={report.trend}
                bucket={report.bucket}
                currency={currency}
                metric={metric}
                metrics={["requests", "cost", "token"]}
                onMetricChange={setMetric}
              />
            ) : (
              <SectionCard title={t("common.usageTrend")}>
                <div className="text-[10px] text-slate-700/45 py-5 text-center">
                  {t("report.noTrend")}
                </div>
              </SectionCard>
            )}
            {report.notes.map((note) => (
              <div
                key={note}
                className="text-[9px] text-slate-700/45 leading-relaxed"
              >
                {note}
              </div>
            ))}

            <SectionCard title={t("report.agentDist")}>
              <div className="space-y-1.5">
                {sortedAgents.map((agent) => (
                  <AgentUsageRow
                    key={agent.id}
                    agent={agent}
                    currency={currency}
                    metric={metric}
                    share={
                      agentTotal > 0
                        ? (metricValue(agent, metric, currency) / agentTotal) *
                          100
                        : 0
                    }
                  />
                ))}
              </div>
            </SectionCard>

            <SectionCard title={t("report.modelRank")}>
              {topModels.length > 0 ? (
                <div className="space-y-1.5">
                  {topModels.map((model, index) => (
                    <ModelUsageRow
                      key={model.key}
                      model={model}
                      index={index}
                      currency={currency}
                      metric={metric}
                      totalMetric={modelTotal}
                    />
                  ))}
                </div>
              ) : (
                <div className="text-[10px] text-slate-700/45 py-2">
                  {t("report.noModels")}
                </div>
              )}
            </SectionCard>

            <SectionCard title={t("report.conclusion")}>
              <div className="space-y-1.5 text-[10px] text-slate-700/65 leading-relaxed">
                {topModels[0] && (
                  <div>
                    <span className="text-slate-900/80">{t("report.mainModel")}</span>
                    {t("report.mainModelLine", {
                      model: topModels[0].model_id,
                      agent: AGENT_META[topModels[0].agentId].label,
                      tokens: formatTokens(topModels[0].total_tokens),
                    })}
                  </div>
                )}
                {topTrend && (
                  <div>
                    <span className="text-slate-900/80">{t("report.peakWindow")}</span>
                    {t("report.peakWindowLine", {
                      label: topTrend.label,
                      value:
                        metric === "requests"
                          ? topTrend.requests.toLocaleString()
                          : metric === "token"
                            ? formatTokens(topTrend.total_tokens) + " Token"
                            : formatCost(selectedCost(topTrend, currency), currency),
                    })}
                  </div>
                )}
                {unpricedTokens > 0 && (
                  <div className="text-amber-700/80">
                    {t("report.unpricedWarn", {
                      tokens: formatTokens(unpricedTokens),
                    })}
                  </div>
                )}
                {unpricedTokens === 0 && topModels.length > 0 && (
                  <div className="text-emerald-700/75">
                    {t("report.allPriced")}
                  </div>
                )}
              </div>
            </SectionCard>

            {report.agentQuotas.length > 0 && (
              <QuotaSummary quotas={report.agentQuotas} />
            )}
          </div>
        )}
      </PageBody>

      <PageFooter>
        <span className="text-slate-500 truncate">{doneFlash || filename}</span>
        <div className="flex gap-1.5 shrink-0">
          <BtnSecondary onClick={handleCopy} disabled={!report || loading}>{t("report.copy")}</BtnSecondary>
          <BtnPrimary onClick={handleSave} disabled={!report || loading}>{t("common.save")}</BtnPrimary>
        </div>
      </PageFooter>

      {/* 生成分享卡片弹层（近 7 天卡片，Canvas 实时预览） */}
      {shareOpen && (
        <ShareCardModal
          onClose={() => setShareOpen(false)}
          pricing={pricing}
          agentVisibility={agentVisibility}
        />
      )}
    </>
  );
}

function MetricCard({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint: string;
}) {
  return (
    <div className="card-base rounded-xl px-2.5 py-2 text-center">
      <div className="text-[9px] text-slate-700/50">{label}</div>
      <div className="num text-[16px] font-semibold text-slate-900/85 mt-0.5">
        {value}
      </div>
      <div className="text-[9px] text-slate-700/35 mt-0.5 truncate">{hint}</div>
    </div>
  );
}

function AgentUsageRow({
  agent,
  currency,
  metric,
  share,
}: {
  agent: ReportAgent;
  currency: Currency;
  metric: ReportMetric;
  share: number;
}) {
  const { t } = useI18n();
  return (
    <div className="rounded-lg bg-surface/20 px-2 py-1.5">
      <div className="flex items-center gap-1.5">
        <BrandIcon
          brand={agent.brand}
          className="h-3.5 w-3.5 shrink-0"
          style={{ color: agent.color }}
        />
        <span className="text-[10px] font-medium text-slate-900/80">
          {agent.label}
        </span>
        <span className="num text-[9px] text-slate-700/45 ml-auto">
          {Math.round(share)}%
        </span>
        <span className="num text-[10px] text-slate-900/80">
          {metric === "requests"
            ? agent.requests.toLocaleString()
            : metric === "token"
              ? formatTokens(agent.total_tokens)
              : formatCost(selectedCost(agent, currency), currency)}
        </span>
      </div>
      <div className="h-1 rounded-full bg-slate-900/8 overflow-hidden mt-1">
        <div
          className="h-full rounded-full opacity-75"
          style={{
            width: Math.min(100, Math.max(0, share)) + "%",
            background: agent.color,
          }}
        />
      </div>
      <div className="flex items-center justify-between text-[9px] text-slate-700/45 mt-1">
        <span className="num">{formatTokens(agent.total_tokens)} Token</span>
        <span className="num">{t("compare.requestsCount", { count: agent.requests.toLocaleString() })}</span>
      </div>
    </div>
  );
}

function ModelUsageRow({
  model,
  index,
  currency,
  metric,
  totalMetric,
}: {
  model: ReportModel;
  index: number;
  currency: Currency;
  metric: ReportMetric;
  totalMetric: number;
}) {
  const value = metricValue(model, metric, currency);
  const share = totalMetric > 0 ? Math.round((value / totalMetric) * 100) : 0;
  const barPct =
    totalMetric > 0 ? Math.min(100, Math.max(0, (value / totalMetric) * 100)) : 0;
  const color = AGENT_META[model.agentId].color;
  return (
    <div className="flex items-center gap-1.5 min-w-0">
      <span className="num text-[9px] text-slate-700/35 w-3 shrink-0 text-right">
        {index + 1}
      </span>
      <BrandIcon
        brand={AGENT_META[model.agentId].brand}
        className="h-3 w-3 shrink-0"
        style={{ color }}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1 min-w-0">
          <span className="text-[10px] text-slate-900/80 truncate">
            {model.model_id}
          </span>
          <span className="text-[8px] text-slate-700/35 shrink-0">
            {AGENT_META[model.agentId].label}
          </span>
        </div>
        <div className="h-0.5 rounded-full bg-slate-900/8 overflow-hidden mt-0.5">
          <div
            className="h-full rounded-full opacity-70"
            style={{ width: barPct + "%", background: color }}
          />
        </div>
      </div>
      <div className="text-right shrink-0">
        <div className="num text-[10px] text-slate-900/80">
          {metric === "requests"
            ? model.requests.toLocaleString()
            : metric === "token"
              ? formatTokens(model.total_tokens)
              : formatCost(selectedCost(model, currency), currency)}
          {share > 0 ? " · " + share + "%" : ""}
        </div>
        <div className="num text-[9px] text-slate-700/45">
          {formatTokens(model.total_tokens)} Token
        </div>
      </div>
    </div>
  );
}

function QuotaSummary({ quotas }: { quotas: ReportQuota[] }) {
  const { t } = useI18n();
  const display = useResetDisplay();
  return (
    <SectionCard title={t("report.quotaSnapshot")} action={<span className="text-[9px] text-slate-500">{t("report.quotaScope")}</span>}>
      <div className="space-y-1.5">
        {quotas.map((quota) => {
          const resetWindow = quota.windows.find(
            (window) => window.resetAt != null
          );
          return (
            <div
              key={quota.id}
              className="rounded-lg bg-surface/20 px-2 py-1.5"
            >
              <div className="flex items-center gap-1.5 mb-1.5">
                <BrandIcon
                  brand={quota.brand}
                  className="h-3.5 w-3.5 shrink-0"
                  style={{ color: quota.color }}
                />
                <span className="text-[10px] font-medium text-slate-900/80">
                  {quota.label}
                </span>
                <span className="text-[9px] text-slate-700/40 ml-auto">
                  {quota.accountLevel ? t("report.accountLevel") : t("report.localRealtime")}
                </span>
              </div>
              <div className="grid grid-cols-2 gap-1.5">
                {quota.windows.map((window) => (
                  <QuotaBar
                    key={window.labelKey}
                    label={t(window.labelKey)}
                    value={window.usedPct}
                  />
                ))}
              </div>
              {resetWindow && (display.countdown || display.datetime) && (
                <div className="text-[9px] text-slate-700/40 mt-1.5">
                  {quotaResetText(t, resetWindow.resetAt, Date.now(), display)}
                </div>
              )}
            </div>
          );
        })}
      </div>
      <div className="text-[9px] text-slate-700/40 mt-1.5">
        {t("report.quotaSourceNote")}
      </div>
    </SectionCard>
  );
}

function QuotaBar({ label, value }: { label: string; value: number }) {
  const pct = Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : 0;
  return (
    <div className="rounded-md bg-surface/20 px-2 py-1.5">
      <div className="flex items-center justify-between text-[9px] text-slate-700/50">
        <span>{label}</span>
        <span className="num text-slate-900/75">{formatQuotaPct(value)}%</span>
      </div>
      <div className="h-1 rounded-full bg-slate-900/8 overflow-hidden mt-1">
        <div
          className="h-full rounded-full opacity-75"
          style={{
            width: pct + "%",
            background: remainingGradient(100 - pct),
          }}
        />
      </div>
    </div>
  );
}

/** 生成报告 Markdown 全文（模式 B：接收 t / locale，报告语言跟随 UI 语言） */
function buildMarkdown(
  preset: RangePreset,
  report: ReportData | null,
  currency: Currency,
  t: TFn,
  locale: Locale
): string {
  const now = Date.now();
  // 导出文案同样跟随重置时间展示偏好（非组件上下文，直接读一次）
  const display = loadResetDisplay();
  const title =
    preset === "today" ? t("report.md.daily") : t("report.md.custom");
  if (!report || report.agents.length === 0) {
    return (
      "📊 ZBar " + title + " · " + localDateStr(now) + "\n\n" + t("report.md.noData")
    );
  }

  const range =
    preset === "today"
      ? localDateStr(report.from_ms)
      : localDateStr(report.from_ms) + " ~ " + localDateStr(report.to_ms);
  const totalCost = currency === "cny" ? report.agents.reduce(
    (sum, agent) => sum + agent.cost_cny,
    0
  ) : report.agents.reduce((sum, agent) => sum + agent.cost_usd, 0);
  const totalTokens = report.agents.reduce(
    (sum, agent) => sum + agent.total_tokens,
    0
  );
  const totalRequests = report.agents.reduce(
    (sum, agent) => sum + agent.requests,
    0
  );
  const models = report.agents
    .flatMap((agent) => agent.models)
    .sort((a, b) => {
      const costDiff = selectedCost(b, currency) - selectedCost(a, currency);
      return costDiff !== 0 ? costDiff : b.total_tokens - a.total_tokens;
    });
  const lines: string[] = [];
  lines.push("📊 ZBar " + title + " · " + range);
  lines.push("");
  lines.push(
    t("report.md.summaryLine", {
      cost: formatCost(totalCost, currency),
      tokens: formatTokens(totalTokens),
      requests: totalRequests.toLocaleString(),
    })
  );
  lines.push("");
  lines.push(t("report.agentDist"));
  for (const agent of report.agents) {
    lines.push(
      "- " +
        t("report.md.agentLine", {
          label: agent.label,
          cost: formatCost(selectedCost(agent, currency), currency),
          tokens: formatTokens(agent.total_tokens),
          requests: agent.requests.toLocaleString(),
        })
    );
  }
  if (models.length > 0) {
    lines.push("");
    lines.push(t("report.md.top5"));
    for (const model of models.slice(0, 5)) {
      lines.push(
        "- " +
          t("report.md.modelLine", {
            agent: AGENT_META[model.agentId].label,
            model: model.model_id,
            cost: formatCost(selectedCost(model, currency), currency),
            tokens: formatTokens(model.total_tokens),
          })
      );
    }
  }
  if (report.trend.length > 0) {
    const peak = report.trend.reduce((best, point) =>
      point.total_tokens > best.total_tokens ? point : best
    );
    lines.push("");
    lines.push(
      t("report.md.tokenPeak", {
        label: peak.label,
        tokens: formatTokens(peak.total_tokens),
      })
    );
  }
  if (report.agentQuotas.length > 0) {
    lines.push("");
    lines.push(t("report.md.quotaSnapshot"));
    for (const quota of report.agentQuotas) {
      const windows = quota.windows
        .map((window) => t(window.labelKey) + " " + formatQuotaPct(window.usedPct) + "%")
        .join(locale === "zh" ? "｜" : " | ");
      const resetWindow = quota.windows.find(
        (window) => window.resetAt != null
      );
      const reset =
        resetWindow && (display.countdown || display.datetime)
          ? t("report.md.quotaReset", {
              text: quotaResetText(t, resetWindow.resetAt, now, display),
            })
          : "";
      lines.push(
        "- " +
          t("report.md.quotaLine", {
            label: quota.label,
            scope: quota.accountLevel
              ? t("report.accountLevel")
              : t("report.localRealtime"),
            windows,
          }) +
          reset
      );
    }
  }
  if (report.warnings.length > 0 || report.notes.length > 0) {
    lines.push("");
    lines.push(t("report.md.notes"));
    for (const note of [...report.notes, ...report.warnings]) {
      lines.push("- " + note);
    }
  }
  lines.push("");
  lines.push(
    "> " +
      t("report.md.footer", {
        date: new Date(now).toLocaleString(dateLocale(locale)),
      })
  );
  return lines.join("\n");
}

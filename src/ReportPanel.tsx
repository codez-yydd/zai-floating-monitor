import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ClaudeSnapshot,
  CodexSnapshot,
  Currency,
  CursorSnapshot,
  DeviceInfo,
  PricingConfig,
  QuotaSnapshot,
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
import { formatCost, formatTokens } from "./format";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  mergeStats,
  mergeTrend,
  modelCost,
  remoteToStats,
} from "./merge";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import { type AgentId, type AgentVisibility } from "./agentVisibility";
import { TrendChart, remainingGradient } from "./widgets";
import {
  PageShell,
  PageHeader,
  PageBody,
  PageFooter,
  SectionCard,
  PillGroup,
  PillButton,
  BtnPrimary,
  BtnSecondary,
  AlertBanner,
  LoadingState,
  SortToggle,
} from "./layout";

interface Props {
  onBack: () => void;
  pricing: PricingConfig;
  currency: Currency;
  agentVisibility: AgentVisibility;
}

type ReportKind = "daily" | "weekly";
type ReportMetric = "cost" | "token";

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
  label: string;
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
};

/** 本地日期 YYYY-MM-DD，避免 UTC 偏移。 */
function localDateStr(ms: number): string {
  const d = new Date(ms);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return y + "-" + m + "-" + day;
}

function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function reportRange(kind: ReportKind, now: number): [number, number] {
  if (kind === "daily") return [startOfLocalDay(now), now];
  return [startOfLocalDay(now - 6 * DAY_MS), now];
}

function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 90 ? text.slice(0, 87) + "…" : text;
}

function isModelPriced(modelId: string, pricing: PricingConfig): boolean {
  if (pricing.usd[modelId]) return true;
  const target = modelId.toLowerCase().replace(/\./g, "-");
  return Object.keys(pricing.usd).some(
    (id) => id.toLowerCase().replace(/\./g, "-") === target
  );
}

function makeAgent(
  id: AgentId,
  source: ReportSource,
  pricing: PricingConfig,
  fxRate: number
): ReportAgent {
  const meta = AGENT_META[id];
  const models = source.stats.by_model.map((model) => ({
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
    priced: isModelPriced(model.model_id, pricing),
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

  const models: ReportModel[] = snapshot.by_model.map((model) => ({
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

function trendLabelOrder(label: string, bucket: TrendBucket): number {
  if (bucket === "hour") {
    const parts = label.split(":");
    const hour = Number(parts[0]);
    const minute = Number(parts[1] ?? 0);
    return Number.isFinite(hour) ? hour * 60 + minute : Number.MAX_SAFE_INTEGER;
  }
  const parts = label.split("-");
  const month = Number(parts[0]);
  const day = Number(parts[1]);
  return Number.isFinite(month) && Number.isFinite(day)
    ? month * 100 + day
    : Number.MAX_SAFE_INTEGER;
}

function mergeReportTrends(
  agents: ReportAgent[],
  bucket: TrendBucket
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
  return [...byLabel.values()].sort(
    (a, b) =>
      trendLabelOrder(a.label, bucket) - trendLabelOrder(b.label, bucket)
  );
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

function quotaResetText(resetAt: number | null, now: number): string {
  if (!resetAt) return "重置时间未知";
  const hours = Math.max(0, Math.ceil((resetAt - now) / 3_600_000));
  if (hours >= 24) return "约 " + Math.ceil(hours / 24) + " 天后重置";
  return "约 " + hours + " 小时后重置";
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
      label: "周额度当前",
      usedPct: latest.weekly_pct,
      resetAt: latest.weekly_reset,
    },
    { label: "5h 当前", usedPct: latest.hour5_pct, resetAt: null },
    {
      label: "周额度峰值",
      usedPct: Math.max(...snapshots.map((snapshot) => snapshot.weekly_pct)),
      resetAt: latest.weekly_reset,
    },
    {
      label: "5h 峰值",
      usedPct: Math.max(...snapshots.map((snapshot) => snapshot.hour5_pct)),
      resetAt: null,
    },
  ];
  if (
    latest.mcp_total != null ||
    latest.mcp_used != null ||
    latest.mcp_pct > 0
  ) {
    windows.push({ label: "MCP", usedPct: latest.mcp_pct, resetAt: null });
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
  id: "codex" | "claude",
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
      label: "5h",
      usedPct: rateLimits.primary_pct,
      resetAt: rateLimits.primary_reset_at,
    });
  }
  if (rateLimits.secondary_pct != null) {
    windows.push({
      label: "本周",
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
      windows.push({ label: "Auto", usedPct: plan.auto_pct, resetAt: null });
    }
    if (plan.api_pct != null) {
      windows.push({ label: "API", usedPct: plan.api_pct, resetAt: null });
    }
    if (windows.length === 0 && plan.total_pct != null) {
      windows.push({
        label: "套餐",
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
      label: "按需",
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

export function ReportPanel({
  onBack,
  pricing,
  currency,
  agentVisibility,
}: Props) {
  const [kind, setKind] = useState<ReportKind>("daily");
  const [metric, setMetric] = useState<ReportMetric>("cost");
  const [report, setReport] = useState<ReportData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [doneFlash, setDoneFlash] = useState<string | null>(null);
  const [showMarkdown, setShowMarkdown] = useState(false);

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
    const [fromMs, toMs] = reportRange(kind, now);
    const bucket: TrendBucket = kind === "daily" ? "hour" : "day";
    const wantLocal = deviceFilter === "all" || deviceFilter === "local";
    const wantRemote =
      syncEnabled &&
      deviceFilter !== "local" &&
      !!syncConfig?.device_token;
    const safe = async <T,>(
      _label: string,
      task: () => Promise<T>
    ): Promise<T | null> => {
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
        ? safe("本机 Z.ai", async () => {
            const [stats, trend] = await Promise.all([
              fetchStats(fromMs, toMs),
              fetchTrend(fromMs, toMs, bucket),
            ]);
            return { stats, trend };
          })
        : Promise.resolve(null);
    const localCodexPromise: Promise<LoadedSource | null> =
      agentVisibility.codex && wantLocal
        ? safe("本机 Codex", async () => {
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
        ? safe("本机 Claude", async () => {
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
        ? safe("本机 Cursor", () => fetchCursorUsage(fromMs, toMs))
        : Promise.resolve(null);

    const remoteOptions = (source: string) =>
      deviceFilter === "all"
        ? { excludeDevice: syncConfig?.device_id ?? "", source }
        : { devices: deviceFilter, source };
    const remoteZaiPromise: Promise<RemoteUsage | null> =
      agentVisibility.zai && wantRemote
        ? safe("远端 Z.ai", () =>
            remoteUsage(fromMs, toMs, bucket, remoteOptions("zcode"))
          )
        : Promise.resolve(null);
    const remoteCodexPromise: Promise<RemoteUsage | null> =
      agentVisibility.codex && wantRemote
        ? safe("远端 Codex", () =>
            remoteUsage(fromMs, toMs, bucket, remoteOptions("codex"))
          )
        : Promise.resolve(null);
    const remoteClaudePromise: Promise<RemoteUsage | null> =
      agentVisibility.claude && wantRemote
        ? safe("远端 Claude", () =>
            remoteUsage(fromMs, toMs, bucket, remoteOptions("claude"))
          )
        : Promise.resolve(null);

    const fxRatePromise = getCursorConfig()
      .then((config) =>
        config.usd_cny_rate > 0 ? config.usd_cny_rate : 7.2
      )
      .catch(() => 7.2);
    const localQuotaPromise: Promise<QuotaSnapshot[] | null> = wantLocal
      ? safe("本机额度", async () =>
          (await getQuotaHistory()).filter(
            (snapshot) => snapshot.ts >= fromMs && snapshot.ts <= toMs
          )
        )
      : Promise.resolve([]);
    const remoteQuotaPromise: Promise<RemoteSnapshot[] | null> =
      wantRemote && syncConfig
        ? safe("远端额度", () =>
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

      const trendAgents =
        kind === "daily"
          ? agents.filter((agent) => agent.id !== "cursor")
          : agents;
      const notes: string[] = [];
      if (kind === "daily" && cursorAgent) {
        notes.push("Cursor 官方明细按日返回，日报小时趋势未混入 Cursor，Agent 汇总仍包含它。");
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
      if (agentVisibility.cursor && localCursor) {
        const cursorQuota = makeCursorQuota(localCursor);
        if (cursorQuota) agentQuotas.push(cursorQuota);
      }

      setReport({
        from_ms: fromMs,
        to_ms: toMs,
        bucket,
        agents,
        trend: mergeReportTrends(trendAgents, bucket),
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
    deviceFilter,
    kind,
    pricing,
    syncConfig,
    syncEnabled,
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
          const costDiff =
            selectedCost(b, currency) - selectedCost(a, currency);
          return costDiff !== 0 ? costDiff : b.total_tokens - a.total_tokens;
        })
        .slice(0, 6),
    [allModels, currency]
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
    return report.trend.reduce((best, point) => {
      const value =
        metric === "token"
          ? point.total_tokens
          : selectedCost(point, currency);
      const bestValue =
        metric === "token"
          ? best.total_tokens
          : selectedCost(best, currency);
      return value > bestValue ? point : best;
    }, report.trend[0]);
  }, [currency, metric, report]);

  const markdown = buildMarkdown(kind, report, currency);
  const filename =
    (kind === "daily" ? "日报-" : "周报-") + localDateStr(Date.now()) + ".md";

  const handleCopy = async () => {
    try {
      await writeText(markdown);
      setDoneFlash("已复制到剪贴板 ✓");
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(shortError(e));
    }
  };

  const handleSave = async () => {
    setError(null);
    try {
      await saveReport(markdown, filename);
      setDoneFlash("已保存并在文件夹打开 ✓");
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(shortError(e));
    }
  };

  return (
    <PageShell>
      <PageHeader
        title="用量报告"
        onBack={onBack}
        right={<button onClick={load} disabled={loading} className="toolbar-btn" title="刷新报告">↻</button>}
        subtitle={
          <div className="flex gap-1 mt-0">
            <PillGroup>
              {(["daily", "weekly"] as ReportKind[]).map((item) => (
                <PillButton key={item} active={kind === item} onClick={() => setKind(item)}>
                  {item === "daily" ? "今天" : "近 7 天"}
                </PillButton>
              ))}
            </PillGroup>
            {syncEnabled && (
              <select
                value={deviceFilter}
                onChange={(event) => setDeviceFilter(event.target.value)}
                className="input-box num ml-auto min-w-0 flex-1 text-[10px] py-1"
                title="筛选设备"
              >
                <option value="all">全部设备</option>
                <option value="local">本机{syncConfig?.device_name ? `（${syncConfig.device_name}）` : ""}</option>
                {remoteDevices.filter((device) => device.device_id !== syncConfig?.device_id).map((device) => (
                  <option key={device.device_id} value={device.device_id}>{device.device_name}（{device.device_id.slice(0, 6)}）</option>
                ))}
              </select>
            )}
          </div>
        }
      />

      {(error || (report?.warnings.length ?? 0) > 0) && (
        <div className="px-3 pt-2 space-y-1">
          {error && <AlertBanner>{error}</AlertBanner>}
          {report?.warnings.length ? (
            <AlertBanner type="warning">{report.warnings.join("；")}</AlertBanner>
          ) : null}
        </div>
      )}

      <PageBody>
        {loading && !report ? (
          <LoadingState text="正在整理用量数据…" />
        ) : !report || report.agents.length === 0 ? (
          <SectionCard>
            <div className="py-6 text-center">
              <div className="text-sm text-slate-800/80">当前范围暂无用量</div>
              <div className="text-[10px] text-slate-500 leading-relaxed mt-1.5">请确认 Agent 已开启，并在今天或近 7 天内产生过请求。</div>
            </div>
          </SectionCard>
        ) : (
          <div className="page-stack">
            {loading && (
              <div className="text-[10px] text-sky-700/70">正在刷新最新数据…</div>
            )}

            <div className="grid grid-cols-2 gap-1.5">
              <MetricCard label="总花费" value={formatCost(currency === "cny" ? totals.cost_cny : totals.cost_usd, currency)} hint={currency === "cny" ? "人民币折算" : "美元原价"} />
              <MetricCard label="Token" value={formatTokens(totals.total_tokens)} hint="当前可见 Agent" />
              <MetricCard label="请求" value={totals.requests.toLocaleString()} hint="调用次数" />
              <MetricCard label="活跃 Agent" value={String(report.agents.length)} hint={report.agents.length === 1 ? "本范围仅 1 个来源" : "本范围有用量的来源"} />
            </div>

            <SectionCard title="用量趋势" action={
              <SortToggle options={[{ key: "cost", label: "花费" }, { key: "token", label: "Token" }]} value={metric} onChange={setMetric} accent="sky" />
            }>
              {report.trend.length > 0 ? (
                <TrendChart
                  points={report.trend}
                  bucket={report.bucket}
                  currency={currency}
                  metric={metric === "cost" ? "cost" : "token"}
                  onMetricChange={(next) => setMetric(next)}
                />
              ) : (
                <div className="text-[10px] text-slate-700/45 py-5 text-center">
                  当前来源没有可绘制的趋势数据
                </div>
              )}
              {report.notes.map((note) => (
                <div
                  key={note}
                  className="text-[9px] text-slate-700/45 leading-relaxed mt-1.5"
                >
                  {note}
                </div>
              ))}
            </SectionCard>

            <SectionCard title="Agent 分布" action={
              <span className="text-[9px] text-slate-500">
                {(currency === "cny" ? totals.cost_cny : totals.cost_usd) > 0 ? "按花费占比" : "未配价格，按 Token 占比"}
              </span>
            }>
              <div className="space-y-1.5">
                {report.agents.map((agent) => {
                  const totalMetric = report.agents.reduce(
                    (sum, item) => sum + selectedCost(item, currency),
                    0
                  );
                  const share =
                    totalMetric > 0
                      ? (selectedCost(agent, currency) / totalMetric) * 100
                      : (agent.total_tokens /
                          Math.max(totals.total_tokens, 1)) *
                        100;
                  return (
                    <AgentUsageRow
                      key={agent.id}
                      agent={agent}
                      currency={currency}
                      share={share}
                    />
                  );
                })}
              </div>
            </SectionCard>

            <SectionCard title="模型排行">
              {topModels.length > 0 ? (
                <div className="space-y-1.5">
                  {topModels.map((model, index) => (
                    <ModelUsageRow
                      key={model.key}
                      model={model}
                      index={index}
                      currency={currency}
                      totalCost={
                        currency === "cny" ? totals.cost_cny : totals.cost_usd
                      }
                      totalTokens={totals.total_tokens}
                    />
                  ))}
                </div>
              ) : (
                <div className="text-[10px] text-slate-700/45 py-2">
                  当前 Agent 没有返回模型明细。
                </div>
              )}
            </SectionCard>

            <SectionCard title="报告结论">
              <div className="space-y-1.5 text-[10px] text-slate-700/65 leading-relaxed">
                {topModels[0] && (
                  <div>
                    <span className="text-slate-900/80">主力模型：</span>
                    {topModels[0].model_id}（{AGENT_META[topModels[0].agentId].label}
                    ），{formatTokens(topModels[0].total_tokens)} Token。
                  </div>
                )}
                {topTrend && (
                  <div>
                    <span className="text-slate-900/80">峰值时段：</span>
                    {topTrend.label}，{" "}
                    {metric === "token"
                      ? formatTokens(topTrend.total_tokens) + " Token"
                      : formatCost(
                          selectedCost(topTrend, currency),
                          currency
                        )}
                    。
                  </div>
                )}
                {unpricedTokens > 0 && (
                  <div className="text-amber-700/80">
                    有 {formatTokens(unpricedTokens)} Token 未配置价格，花费统计会偏低；可到价格设置补充模型价格。
                  </div>
                )}
                {unpricedTokens === 0 && topModels.length > 0 && (
                  <div className="text-emerald-700/75">
                    当前用量的模型均已配置价格，花费统计可用于横向比较。
                  </div>
                )}
              </div>
            </SectionCard>

            {report.agentQuotas.length > 0 && (
              <QuotaSummary quotas={report.agentQuotas} />
            )}

            {showMarkdown && (
              <SectionCard title="Markdown 预览">
                <pre className="num text-[10px] leading-relaxed text-slate-800/80 whitespace-pre-wrap break-words">
                  {markdown}
                </pre>
              </SectionCard>
            )}
          </div>
        )}
      </PageBody>

      <PageFooter>
        <span className="text-slate-500 truncate">{doneFlash || filename}</span>
        <div className="flex gap-1.5 shrink-0">
          <BtnSecondary onClick={() => setShowMarkdown((visible) => !visible)} disabled={!report || loading}>
            {showMarkdown ? "收起预览" : "查看 Markdown"}
          </BtnSecondary>
          <BtnSecondary onClick={handleCopy} disabled={!report || loading}>复制</BtnSecondary>
          <BtnPrimary onClick={handleSave} disabled={!report || loading}>保存</BtnPrimary>
        </div>
      </PageFooter>
    </PageShell>
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
  share,
}: {
  agent: ReportAgent;
  currency: Currency;
  share: number;
}) {
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
          {formatCost(selectedCost(agent, currency), currency)}
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
        <span className="num">{agent.requests.toLocaleString()} 次请求</span>
      </div>
    </div>
  );
}

function ModelUsageRow({
  model,
  index,
  currency,
  totalCost,
  totalTokens,
}: {
  model: ReportModel;
  index: number;
  currency: Currency;
  totalCost: number;
  totalTokens: number;
}) {
  const cost = selectedCost(model, currency);
  const share =
    totalCost > 0
      ? Math.round((cost / totalCost) * 100)
      : totalTokens > 0
        ? Math.round((model.total_tokens / totalTokens) * 100)
        : 0;
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
            style={{
              width:
                (totalCost > 0
                  ? Math.min(100, Math.max(0, (cost / totalCost) * 100))
                  : totalTokens > 0
                    ? Math.min(
                        100,
                        Math.max(0, (model.total_tokens / totalTokens) * 100)
                      )
                    : 0) + "%",
              background: color,
            }}
          />
        </div>
      </div>
      <div className="text-right shrink-0">
        <div className="num text-[10px] text-slate-900/80">
          {formatCost(cost, currency)}
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
  return (
    <SectionCard title="Agent 额度快照" action={<span className="text-[9px] text-slate-500">已开启且有数据的 Agent</span>}>
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
                  className="h-3.5 w-3.5"
                  style={{ color: quota.color }}
                />
                <span className="text-[10px] font-medium text-slate-900/80">
                  {quota.label}
                </span>
                <span className="text-[9px] text-slate-700/40 ml-auto">
                  {quota.accountLevel ? "账户级" : "本机实时"}
                </span>
              </div>
              <div className="grid grid-cols-2 gap-1.5">
                {quota.windows.map((window) => (
                  <QuotaBar
                    key={window.label}
                    label={window.label}
                    value={window.usedPct}
                  />
                ))}
              </div>
              {resetWindow && (
                <div className="text-[9px] text-slate-700/40 mt-1.5">
                  {quotaResetText(resetWindow.resetAt, Date.now())}
                </div>
              )}
            </div>
          );
        })}
      </div>
      <div className="text-[9px] text-slate-700/40 mt-1.5">
        Z.ai 额度来自历史快照；Codex、Claude、Cursor 为本机实时额度接口。
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

function buildMarkdown(
  kind: ReportKind,
  report: ReportData | null,
  currency: Currency
): string {
  const now = Date.now();
  const title = kind === "daily" ? "日报" : "周报";
  if (!report || report.agents.length === 0) {
    return "📊 ZBar 用量" + title + " · " + localDateStr(now) + "\n\n（暂无数据）";
  }

  const range =
    kind === "daily"
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
  lines.push("📊 ZBar 用量" + title + " · " + range);
  lines.push("");
  lines.push(
    "总花费 " +
      formatCost(totalCost, currency) +
      "｜Token " +
      formatTokens(totalTokens) +
      "｜请求 " +
      totalRequests.toLocaleString() +
      " 次"
  );
  lines.push("");
  lines.push("Agent 分布");
  for (const agent of report.agents) {
    lines.push(
      "- " +
        agent.label +
        "：" +
        formatCost(selectedCost(agent, currency), currency) +
        "｜" +
        formatTokens(agent.total_tokens) +
        " Token｜" +
        agent.requests.toLocaleString() +
        " 次请求"
    );
  }
  if (models.length > 0) {
    lines.push("");
    lines.push("模型 TOP5");
    for (const model of models.slice(0, 5)) {
      lines.push(
        "- " +
          AGENT_META[model.agentId].label +
          " / " +
          model.model_id +
          "：" +
          formatCost(selectedCost(model, currency), currency) +
          "｜" +
          formatTokens(model.total_tokens) +
          " Token"
      );
    }
  }
  if (report.trend.length > 0) {
    const peak = report.trend.reduce((best, point) =>
      point.total_tokens > best.total_tokens ? point : best
    );
    lines.push("");
    lines.push(
      "Token 峰值：" +
        peak.label +
        "，" +
        formatTokens(peak.total_tokens) +
        " Token"
    );
  }
  if (report.agentQuotas.length > 0) {
    lines.push("");
    lines.push("额度快照");
    for (const quota of report.agentQuotas) {
      const windows = quota.windows
        .map((window) => window.label + " " + formatQuotaPct(window.usedPct) + "%")
        .join("｜");
      const resetWindow = quota.windows.find(
        (window) => window.resetAt != null
      );
      const reset = resetWindow
        ? "｜" + quotaResetText(resetWindow.resetAt, now)
        : "";
      lines.push(
        "- " +
          quota.label +
          "（" +
          (quota.accountLevel ? "账户级" : "本机实时") +
          "）：「" +
          windows +
          "」" +
          reset
      );
    }
  }
  if (report.warnings.length > 0 || report.notes.length > 0) {
    lines.push("");
    lines.push("说明");
    for (const note of [...report.notes, ...report.warnings]) {
      lines.push("- " + note);
    }
  }
  lines.push("");
  lines.push("> 由 ZBar 自动生成 · " + new Date(now).toLocaleString("zh-CN"));
  return lines.join("\n");
}

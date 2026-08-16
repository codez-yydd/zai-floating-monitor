import { useCallback, useEffect, useRef, useState } from "react";
import type {
  DeviceInfo,
  QuotaSnapshot,
  RemoteSnapshot,
  RemoteUsage,
  SyncConfig,
  WeeklyPeriod,
  WeeklyTokenBucket,
} from "./types";
import type { AgentId, AgentVisibility } from "./agentVisibility";
import {
  getCompareTokensForAgent,
  getQuotaHistory,
  getSyncConfig,
  getWeeklyCompareForSnapshots,
  listRemoteDevices,
  remoteSnapshots,
  remoteUsage,
} from "./api";
import { formatTokens } from "./format";
import { remainingGradient, remainingTextColor } from "./widgets";
import { BrandIcon, type BrandIconName } from "./BrandIcon";

interface Props {
  onBack: () => void;
  agentVisibility: AgentVisibility;
}

const COMPARE_AGENTS: AgentId[] = ["zai", "codex", "claude", "cursor"];
const AGENT_META: Record<
  AgentId,
  { label: string; brand: BrandIconName; color: string; remoteSource?: string }
> = {
  zai: { label: "Z.ai", brand: "zai", color: "#0284c7", remoteSource: "zcode" },
  codex: { label: "Codex", brand: "codex", color: "#059669", remoteSource: "codex" },
  claude: { label: "Claude", brand: "claude", color: "#c2410c", remoteSource: "claude" },
  cursor: { label: "Cursor", brand: "cursor", color: "#7c3aed" },
};

export function ComparePanel({ onBack, agentVisibility }: Props) {
  const [periods, setPeriods] = useState<WeeklyPeriod[]>([]);
  const [tokens, setTokens] = useState<WeeklyTokenBucket[]>([]);
  const [tokensByAgent, setTokensByAgent] = useState<
    Partial<Record<AgentId, WeeklyTokenBucket[]>>
  >({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedIdx, setSelectedIdx] = useState<number | null>(null);

  // 请求序号守卫：load 会被 60s 定时器/筛选变化/手动刷新并发触发，
  // 慢网络下旧响应后到会覆盖新数据（仿 DataCache 的 quotaReqId 模式）
  const loadReqId = useRef(0);

  // 多设备同步相关
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState<string>("all");
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 初始读同步配置 + 设备列表
  useEffect(() => {
    getSyncConfig()
      .then((cfg) => {
        setSyncConfig(cfg);
        if (cfg.enabled && cfg.device_token) {
          listRemoteDevices()
            .then(setRemoteDevices)
            .catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    const reqId = ++loadReqId.current;
    setLoading(true);
    setError(null);
    try {
      // 数据来源：all=本地+远端(排除本机)；local=仅本地；具体id=仅远端该设备
      const wantLocal = deviceFilter === "all" || deviceFilter === "local";
      const wantRemote =
        syncEnabled &&
        (deviceFilter === "all" ||
          (deviceFilter !== "local" && deviceFilter !== "all"));

      const snapshotOpts =
        syncConfig &&
        (deviceFilter === "all"
          ? { excludeDevice: syncConfig.device_id }
          : { devices: deviceFilter });
      const enabledAgents = COMPARE_AGENTS.filter(
        (agent) => agentVisibility[agent]
      );

      // 周期必须由当前筛选范围内的快照决定。旧逻辑永远只解析本机历史，
      // 导致远端设备没有历史时周期为空、或周期边界与远端 Token 不一致。
      // 快照保留期为 90 天，与后端滚动清理保持一致。
      const nowMs = Date.now();
      const historyFromMs = nowMs - 90 * 86_400_000;
      let localHistory: QuotaSnapshot[] = [];
      let remoteHistory: RemoteSnapshot[] = [];
      const historyTasks: Promise<unknown>[] = [];
      if (wantLocal) {
        historyTasks.push(
          getQuotaHistory().then((history) => (localHistory = history))
        );
      }
      if (wantRemote && syncConfig && snapshotOpts) {
        historyTasks.push(
          remoteSnapshots(historyFromMs, nowMs, snapshotOpts)
            .then((history) => (remoteHistory = history))
            .catch((e) => {
              // 汇总模式保留本机数据降级展示；具体设备筛选则明确提示失败。
              if (deviceFilter !== "all") {
                throw new Error(`远端额度历史获取失败：${String(e)}`);
              }
            })
        );
      }
      await Promise.all(historyTasks);
      if (reqId !== loadReqId.current) return;

      const snapshots = mergeQuotaSnapshots(
        localHistory,
        remoteHistory.map(toQuotaSnapshot)
      );
      const ps = await getWeeklyCompareForSnapshots(snapshots);
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧响应
      setPeriods(ps);

      if (ps.length === 0) {
        setTokens([]);
        setTokensByAgent({});
        setSelectedIdx(null);
        return;
      }

      // 周期区间
      const periodPairs: [number, number][] = ps.map((p) => [
        p.reset_at,
        p.end_at,
      ]);
      const fromMs = ps[0].reset_at;
      const toMs = ps[ps.length - 1].end_at;

      // 所有已开启 Agent 分别聚合，再按周期相加。
      // 本地 Codex/Claude 使用独立 SQLite 周期查询，Cursor 使用原始事件时间戳
      // 聚合；远端三种可同步来源使用 ISO 小时桶归属周期。
      const tasks: Promise<unknown>[] = [];
      const localByAgent: Partial<
        Record<AgentId, WeeklyTokenBucket[]>
      > = {};
      const remoteByAgent: Partial<
        Record<AgentId, WeeklyTokenBucket[]>
      > = {};

      if (wantLocal) {
        for (const agent of enabledAgents) {
          tasks.push(
            getCompareTokensForAgent(agent, periodPairs)
              .then((buckets) => {
                localByAgent[agent] = buckets;
              })
              .catch(() => {
                // 可选 Agent 未安装、未登录或没有会话时按空数据处理。
                localByAgent[agent] = emptyTokenBuckets(periodPairs);
              })
          );
        }
      }

      if (wantRemote && syncConfig && toMs > fromMs) {
        for (const agent of enabledAgents) {
          const source = AGENT_META[agent].remoteSource;
          if (!source) continue; // Cursor 目前只采集本机，未上传到同步服务
          const usageOpts =
            deviceFilter === "all"
              ? { excludeDevice: syncConfig.device_id, source }
              : { devices: deviceFilter, source };
          tasks.push(
            remoteUsage(fromMs, toMs, "hour", usageOpts)
              .then((remote) => {
                remoteByAgent[agent] = aggregateRemoteTokens(ps, remote);
              })
              .catch(() => {
                // 远端没有该来源或服务暂不可用时，不影响其他 Agent 展示。
                remoteByAgent[agent] = emptyTokenBuckets(periodPairs);
              })
          );
        }
      }

      await Promise.all(tasks);
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧响应

      const mergedByAgent: Partial<
        Record<AgentId, WeeklyTokenBucket[]>
      > = {};
      for (const agent of enabledAgents) {
        const local = localByAgent[agent] ?? emptyTokenBuckets(periodPairs);
        const remote = remoteByAgent[agent] ?? emptyTokenBuckets(periodPairs);
        mergedByAgent[agent] = periodPairs.map(([reset_at, end_at], index) => ({
          reset_at,
          end_at,
          total_tokens:
            (local[index]?.total_tokens ?? 0) +
            (remote[index]?.total_tokens ?? 0),
          requests:
            (local[index]?.requests ?? 0) + (remote[index]?.requests ?? 0),
        }));
      }

      const mergedTokens: WeeklyTokenBucket[] = periodPairs.map(
        ([reset_at, end_at], index) => ({
          reset_at,
          end_at,
          total_tokens: enabledAgents.reduce(
            (sum, agent) =>
              sum + (mergedByAgent[agent]?.[index]?.total_tokens ?? 0),
            0
          ),
          requests: enabledAgents.reduce(
            (sum, agent) => sum + (mergedByAgent[agent]?.[index]?.requests ?? 0),
            0
          ),
        })
      );
      setTokens(mergedTokens);
      setTokensByAgent(mergedByAgent);

      // 仅在用户未选中或选中索引超出新周期范围时才落到当前周期，
      // 否则保持用户选择：60s 自动刷新不应把用户点选的历史周期静默跳回
      setSelectedIdx((prev) =>
        prev === null || prev >= ps.length ? ps.length - 1 : prev
      );
    } catch (e) {
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧错误
      setError(String(e));
    } finally {
      // 只有最新请求才允许结束 loading，避免旧请求把新请求的加载态清掉
      if (reqId === loadReqId.current) setLoading(false);
    }
  }, [agentVisibility, deviceFilter, syncConfig, syncEnabled]);

  useEffect(() => {
    load();
  }, [load]);

  // 自动刷新 60s（对比页数据变化慢，频率放低）
  useEffect(() => {
    const timer = setInterval(load, 60_000);
    return () => clearInterval(timer);
  }, [load]);

  return (
    <div className="flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3.5 pt-3 pb-2.5 border-b border-slate-900/10">
        <div className="flex items-center justify-between mb-2">
          <button
            onClick={onBack}
            className="text-xs text-slate-700/50 hover:text-slate-900/80 transition-colors"
          >
            ← 返回
          </button>
          <h1 className="text-[13px] font-semibold text-slate-900/90">
            周额度对比
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
        {/* 设备筛选器 */}
        {syncEnabled && (
          <div className="flex items-center gap-1.5">
            <span className="text-[10px] text-slate-700/45 shrink-0">设备</span>
            <select
              value={deviceFilter}
              onChange={(e) => setDeviceFilter(e.target.value)}
              className="num flex-1 px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60"
            >
              <option value="all">全部（汇总）</option>
              <option value="local">
                本机{syncConfig?.device_name ? `（${syncConfig.device_name}）` : ""}
              </option>
              {remoteDevices
                .filter((d) => d.device_id !== syncConfig?.device_id)
                .map((d) => (
                  <option key={d.device_id} value={d.device_id}>
                    {d.device_name}（{d.device_id.slice(0, 6)}）
                  </option>
                ))}
            </select>
          </div>
        )}
      </div>

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}
      {periods.length === 0 && !loading && !error && (
        <div className="flex-1 flex items-center justify-center px-6">
          <div className="text-center">
            <div className="text-2xl mb-2 opacity-40">📊</div>
            <div className="text-xs text-slate-700/60 mb-1">
              暂无周额度历史数据
            </div>
            <div className="text-[10px] text-slate-700/40 leading-relaxed">
              应用开启后会自动采样周额度
              <br />
              请保持运行以积累数据
            </div>
          </div>
        </div>
      )}

      {periods.length > 0 && (
        <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
          {/* 周额度百分比柱状图 */}
          <PeriodChart
            periods={periods}
            selectedIdx={selectedIdx}
            onSelect={setSelectedIdx}
          />

          {/* 选中周期明细 */}
          {selectedIdx !== null && periods[selectedIdx] && (
            <PeriodDetail
              period={periods[selectedIdx]}
              token={tokens[selectedIdx]}
              tokensByAgent={tokensByAgent}
            />
          )}

          {/* 全部周期列表 */}
          <div>
            <div className="text-[10px] uppercase tracking-wide text-slate-700/55 mb-1.5 mt-1">
              全部周期
            </div>
            <div className="space-y-0.5">
              {periods
                .slice()
                .reverse()
                .map((p, ri) => {
                  const realIdx = periods.length - 1 - ri;
                  const isSel = realIdx === selectedIdx;
                  const tk = tokens[realIdx]?.total_tokens ?? 0;
                  return (
                    <button
                      key={p.reset_at}
                      onClick={() => setSelectedIdx(realIdx)}
                      className={`w-full flex items-center justify-between text-xs py-1.5 px-2 -mx-2 rounded-lg transition-colors ${
                        isSel
                          ? "bg-sky-500/10"
                          : "hover:bg-slate-900/5"
                      }`}
                    >
                      <div className="flex items-center gap-1.5 min-w-0">
                        <span className="font-medium text-slate-900/90">
                          {dateLabel(p.reset_at)}
                        </span>
                        {p.is_current && (
                          <span className="px-1 py-0 rounded text-[9px] font-semibold bg-sky-500/15 text-sky-700">
                            本周
                          </span>
                        )}
                      </div>
                      <div className="flex items-center gap-2 text-slate-700/60 num shrink-0">
                        <span>{formatTokens(tk)}</span>
                        <span className="text-slate-700/25">·</span>
                        <span style={{ color: remainingTextColor(100 - p.pct_end) }}>{p.pct_end}%</span>
                      </div>
                    </button>
                  );
                })}
            </div>
          </div>
        </div>
      )}

      {/* 底部说明 */}
      <div className="px-3.5 py-2 border-t border-slate-900/10 text-[9px] text-slate-700/40 leading-relaxed">
        周百分比是智谱账户级额度（含所有设备/工具）；
        <br />
        Token 统计当前筛选范围内设置中开启的 Agent；二者不是同一计量单位。
        <br />
        Cursor 目前只采集本机，远端设备筛选不会包含 Cursor 数据。
      </div>
    </div>
  );
}

/** 周期柱状图：每根柱=一个周期，高度=周期结束时的已用百分比 */
function PeriodChart({
  periods,
  selectedIdx,
  onSelect,
}: {
  periods: WeeklyPeriod[];
  selectedIdx: number | null;
  onSelect: (i: number) => void;
}) {
  const n = periods.length;
  const barGap = n > 20 ? "gap-px" : n > 10 ? "gap-0.5" : "gap-1";

  return (
    <div className="rounded-lg bg-surface/25 border border-surface/30 px-2.5 py-2">
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
          周额度结束用量趋势
        </span>
      </div>
      <div className={`flex items-end ${barGap} h-16`}>
        {periods.map((p, i) => {
          const h = p.pct_end; // 0-100，避免峰值把历史周期夸大
          const isSel = i === selectedIdx;
          const bg = remainingGradient(100 - p.pct_end);
          return (
            <button
              key={p.reset_at}
              onClick={() => onSelect(i)}
              className="flex-1 h-full flex items-end justify-center min-w-0 group"
              title={`${dateLabel(p.reset_at)}: 结束 ${p.pct_end}% · 峰值 ${p.pct_peak}%`}
            >
              <div
                className={`w-full rounded-t-sm transition-all duration-300 ${
                  isSel ? "opacity-100 ring-1 ring-sky-400" : "opacity-70 group-hover:opacity-90"
                }`}
                style={{ height: `${Math.max(h, 2)}%`, background: bg }}
              />
            </button>
          );
        })}
      </div>
      <div className={`flex ${barGap} mt-1`}>
        {periods.map((p, i) => {
          const labelStep = n <= 6 ? 1 : Math.max(2, Math.ceil(n / 5));
          const showLabel = i === n - 1 || i % labelStep === 0;
          return (
            <span
              key={p.reset_at}
              className={`flex-1 text-center text-[8px] num min-w-0 ${
                i === selectedIdx ? "text-sky-600/80 font-medium" : "text-slate-700/40"
              } ${showLabel ? "" : "opacity-0"}`}
            >
              {dateLabel(p.reset_at)}
            </span>
          );
        })}
      </div>
    </div>
  );
}

/** 选中周期明细卡片 */
function PeriodDetail({
  period,
  token,
  tokensByAgent,
}: {
  period: WeeklyPeriod;
  token?: WeeklyTokenBucket;
  tokensByAgent: Partial<Record<AgentId, WeeklyTokenBucket[]>>;
}) {
  const totalTokens = token?.total_tokens ?? 0;
  const sampleLow = period.sample_count < 10;
  const breakdown = COMPARE_AGENTS.map((agent) => ({
    agent,
    bucket: tokensByAgent[agent]?.find(
      (item) => item.reset_at === period.reset_at
    ),
  })).filter((item) => (item.bucket?.total_tokens ?? 0) > 0);

  return (
    <div className="rounded-lg bg-surface/30 border border-surface/40 px-3 py-2.5 space-y-2">
      {/* 标题行 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          <span className="text-[11px] font-semibold text-slate-900/90">
            {dateLabel(period.reset_at)}
            {period.is_current ? " ~ 进行中" : ` ~ ${dateLabel(period.end_at)}`}
          </span>
        </div>
        <span
          className="text-[11px] num font-semibold"
          style={{ color: remainingTextColor(100 - period.pct_end) }}
        >
          已用 {period.pct_end}%
        </span>
      </div>

      {/* token 统计 */}
      <div className="rounded-md bg-surface/25 py-1.5 px-2">
        <div className="text-[9px] text-slate-700/55">
          所有开启 Agent Token
        </div>
        <div className="num text-[13px] font-semibold text-slate-900/85 mt-0.5">
          {formatTokens(totalTokens)}
        </div>
        {token && (
          <div className="text-[9px] num text-slate-700/45 mt-0.5">
            {token.requests} 次请求
          </div>
        )}
        {breakdown.length > 0 && (
          <div className="flex flex-wrap gap-1 mt-1.5">
            {breakdown.map(({ agent, bucket }) => (
              <span
                key={agent}
                className="inline-flex items-center gap-1 rounded bg-slate-900/5 px-1 py-0.5 text-[9px] text-slate-700/60"
              >
                <BrandIcon
                  brand={AGENT_META[agent].brand}
                  className="h-2.5 w-2.5"
                  style={{ color: AGENT_META[agent].color }}
                />
                <span>{AGENT_META[agent].label}</span>
                <span className="num">
                  {formatTokens(bucket?.total_tokens ?? 0)}
                </span>
              </span>
            ))}
          </div>
        )}
        {totalTokens === 0 && (
          <div className="text-[9px] text-slate-700/40 mt-1">
            当前周期没有开启 Agent 用量
          </div>
        )}
      </div>

      {/* 百分比进度 + 采样可信度 */}
      <div>
        <div className="flex items-center justify-between text-[10px] mb-0.5">
          <span className="text-slate-700/55">
            起 {period.pct_start}% → 止 {period.pct_end}% · 峰值 {period.pct_peak}%
          </span>
          <span className={`num ${sampleLow ? "text-amber-600/80" : "text-slate-700/45"}`}>
            采样 {period.sample_count} 条{sampleLow ? " · 不足" : ""}
          </span>
        </div>
        <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
          <div
            className="h-full rounded-full opacity-80"
            style={{
              width: `${period.pct_end}%`,
              background: remainingGradient(100 - period.pct_end),
            }}
          />
        </div>
      </div>
    </div>
  );
}

// ===== 辅助 =====

/** 远端快照去掉设备字段，转换为周期解析所需的本地结构。 */
function toQuotaSnapshot(snapshot: RemoteSnapshot): QuotaSnapshot {
  const {
    device_id: _deviceId,
    ...quotaSnapshot
  } = snapshot;
  return quotaSnapshot;
}

/** 合并本机与远端额度采样；同一毫秒多设备重复采样时只保留一条。 */
function mergeQuotaSnapshots(
  local: QuotaSnapshot[],
  remote: QuotaSnapshot[]
): QuotaSnapshot[] {
  const byTs = new Map<number, QuotaSnapshot>();
  for (const snapshot of [...local, ...remote]) {
    if (!Number.isFinite(snapshot.ts)) continue;
    const previous = byTs.get(snapshot.ts);
    // 同一时刻应是同一账户状态；若多设备值略有差异，保留已用比例更高的
    // 采样，避免汇总视图低估额度峰值。
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

function emptyTokenBuckets(
  periods: Array<[number, number]>
): WeeklyTokenBucket[] {
  return periods.map(([reset_at, end_at]) => ({
    reset_at,
    end_at,
    total_tokens: 0,
    requests: 0,
  }));
}

/** 把远端指定来源的小时桶按真实时间戳分配到额度周期。 */
function aggregateRemoteTokens(
  periods: WeeklyPeriod[],
  remote: RemoteUsage
): WeeklyTokenBucket[] {
  const buckets = periods.map((period) => ({
    reset_at: period.reset_at,
    end_at: period.end_at,
    total_tokens: 0,
    requests: 0,
  }));
  for (const point of remote.trend) {
    const timestamp = parseTrendTimestamp(point.label);
    if (timestamp === null) continue;
    const index = periods.findIndex(
      (period) =>
        timestamp >= period.reset_at && timestamp < period.end_at
    );
    if (index < 0) continue;
    buckets[index].total_tokens += point.total_tokens;
    buckets[index].requests += point.requests;
  }
  return buckets;
}

/** 兼容远端服务返回的 ISO 时间、毫秒时间戳和秒时间戳。 */
function parseTrendTimestamp(label: string): number | null {
  const numeric = Number(label);
  if (Number.isFinite(numeric) && numeric > 0) {
    return numeric < 1_000_000_000_000 ? numeric * 1000 : numeric;
  }
  const parsed = Date.parse(label);
  return Number.isFinite(parsed) ? parsed : null;
}

/** 毫秒 → "MM-DD" label */
function dateLabel(ms: number): string {
  const d = new Date(ms);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}

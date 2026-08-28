import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AccountMeta,
  AgentQuotaSnapshot,
  AgentQuotaWindowKey,
  DeviceInfo,
  QuotaSnapshot,
  RemoteSnapshot,
  RemoteAgentQuotaSnapshot,
  RemoteUsage,
  SyncConfig,
  WeeklyTokenBucket,
} from "./types";
import type { AgentId, AgentVisibility } from "./agentVisibility";
import { AGENT_COLOR, AGENT_COLOR_SCALE } from "./agentVisibility";
import {
  getAgentQuotaHistory,
  getCompareTokensForAgent,
  getQuotaHistory,
  getSyncConfig,
  listAccounts,
  listRemoteDevices,
  remoteAgentQuotaSnapshots,
  remoteSnapshots,
  remoteUsage,
} from "./api";
import { formatTokens } from "./format";
import { remainingTextColor } from "./widgets";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  PageBody,
  SectionCard,
  EmptyState,
  LoadingState,
  AlertBanner,
} from "./layout";
import { useI18n } from "./i18n";
import { mergeAgentQuotaSnapshots } from "./agentQuota";
import { loadCache, saveCache } from "./cache";

interface Props {
  agentVisibility: AgentVisibility;
}

const COMPARE_AGENTS: AgentId[] = ["zai", "codex", "claude", "cursor", "kimi"];
const AGENT_META: Record<
  AgentId,
  { label: string; brand: BrandIconName; remoteSource?: string }
> = {
  zai: { label: "Z.ai", brand: "zai", remoteSource: "zcode" },
  codex: { label: "Codex", brand: "codex", remoteSource: "codex" },
  claude: { label: "Claude", brand: "claude", remoteSource: "claude" },
  cursor: { label: "Cursor", brand: "cursor" },
  kimi: { label: "Kimi", brand: "kimi" },
};

/** 各 Agent 参与对比的周额度窗口（Z.ai 走 QuotaSnapshot.weekly_pct，不走这里） */
const AGENT_WINDOW_KEY: Partial<Record<AgentId, AgentQuotaWindowKey>> = {
  codex: "weekly",
  claude: "weekly",
  cursor: "cursor_auto",
  kimi: "weekly",
};

const DAY_MS = 86_400_000;
/** 横轴展示的自然周数量（含本周） */
const WEEK_COUNT = 12;
/** 系列过多（多账号）时柱会过细，周数降档到 8 保证可读性 */
const DENSE_WEEK_COUNT = 8;

const COMPARE_CACHE_KEY = "zbar-compare-cache-v2";

/** 一个自然周槽位（本地时区，周一 00:00 起，[startMs, endMs)） */
interface WeekSlot {
  startMs: number;
  endMs: number;
  isCurrent: boolean;
}

/** 图表中的一个额度数据系列：Z.ai 每账号一个，其余 Agent 各一个 */
interface QuotaSeries {
  id: string;
  agent: AgentId;
  /** Z.ai 账号指纹（旧快照无指纹时为 null） */
  account: string | null;
  label: string;
  /** 无账号指纹的历史采样组（与真实账号系列并存时）：明细行悬停解释归属 */
  legacy?: boolean;
}

interface CompareCacheEntry {
  weeks: { startMs: number; isCurrent: boolean }[];
  series: QuotaSeries[];
  /** 每系列每周峰值已用%（无采样为 null），下标与 weeks 对齐 */
  peaks: Record<string, (number | null)[]>;
  /** 每系列每周有效采样数，下标与 weeks 对齐 */
  sampleCounts: Record<string, number[]>;
  tokens: WeeklyTokenBucket[];
  tokensByAgent: Partial<Record<AgentId, WeeklyTokenBucket[]>>;
  ts: number;
}

type CompareCacheStore = Record<string, CompareCacheEntry>;

function readCompareCache(scope: string): CompareCacheEntry | null {
  const store = loadCache<CompareCacheStore>(COMPARE_CACHE_KEY);
  const entry = store?.[scope];
  if (
    !entry ||
    !Array.isArray(entry.weeks) ||
    !Array.isArray(entry.series) ||
    !entry.peaks ||
    !entry.sampleCounts ||
    !Array.isArray(entry.tokens) ||
    !entry.tokensByAgent ||
    !Number.isFinite(entry.ts)
  ) {
    return null;
  }
  // 每个系列的峰值/采样数组长度必须与周槽数一致，防止半截结构渲染越界
  for (const item of entry.series) {
    const peaks = entry.peaks[item.id];
    const counts = entry.sampleCounts[item.id];
    if (
      !Array.isArray(peaks) ||
      peaks.length !== entry.weeks.length ||
      !Array.isArray(counts) ||
      counts.length !== entry.weeks.length
    ) {
      return null;
    }
  }
  return entry;
}

function writeCompareCache(scope: string, entry: CompareCacheEntry): void {
  const store = loadCache<CompareCacheStore>(COMPARE_CACHE_KEY) ?? {};
  const next = { ...store, [scope]: entry };
  const keys = Object.keys(next);
  if (keys.length > 6) {
    keys
      .sort((a, b) => next[a].ts - next[b].ts)
      .slice(0, keys.length - 6)
      .forEach((key) => delete next[key]);
  }
  saveCache(COMPARE_CACHE_KEY, next);
}

/** 返回 ms 所在自然周（本地时区）的周一 0 点。 */
function weekStartOf(ms: number): number {
  const d = new Date(ms);
  const day = d.getDay(); // 周日为 0，转换成周一基数
  d.setDate(d.getDate() - (day === 0 ? 6 : day - 1));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 最近 WEEK_COUNT 个自然周（含本周），升序。
 *  周边界不能用毫秒减法生成（DST 时区一周不一定 7*24h，会偏 1 小时）：
 *  先按日期运算逐周回退再经 weekStartOf 归一到周一 0 点，相邻两两成槽。 */
function buildWeekSlots(nowMs: number): WeekSlot[] {
  const mondays: number[] = [];
  // 多生成一个周一作为最后一周的结束锚点
  for (let i = WEEK_COUNT; i >= 0; i--) {
    const d = new Date(nowMs);
    d.setDate(d.getDate() - i * 7);
    mondays.push(weekStartOf(d.getTime()));
  }
  return slotsFromMondays(
    mondays.slice(0, WEEK_COUNT),
    weekStartOf(nowMs)
  );
}

/** 由升序周一 0 点列表两两成槽；末槽结束点用归一化的"下周一"收口，
 *  保证 endMs 永远是真实的周一 0 点（DST 下毫秒加减会偏 1 小时）。 */
function slotsFromMondays(starts: number[], currentMonday: number): WeekSlot[] {
  const slots: WeekSlot[] = [];
  for (let i = 0; i < starts.length; i++) {
    slots.push({
      startMs: starts[i],
      endMs:
        i + 1 < starts.length
          ? starts[i + 1]
          : weekStartOf(starts[i] + 7 * DAY_MS),
      isCurrent: starts[i] === currentMonday,
    });
  }
  return slots;
}

/** 快照时间戳落在哪个周槽（-1 = 范围外）。 */
function slotIndexOf(slots: WeekSlot[], ts: number): number {
  for (let i = 0; i < slots.length; i++) {
    if (ts >= slots[i].startMs && ts < slots[i].endMs) return i;
  }
  return -1;
}

/** 取系列颜色：单系列用品牌色，同一 Agent 的多账号系列取色阶档位。 */
function seriesColorOf(series: QuotaSeries[], id: string): string {
  const target = series.find((item) => item.id === id);
  if (!target) return AGENT_COLOR.zai; // 找不到系列时的兜底，正常不可达
  const scale = AGENT_COLOR_SCALE[target.agent];
  const index = series
    .filter((item) => item.agent === target.agent)
    .indexOf(target);
  return scale[Math.min(Math.max(index, 0), scale.length - 1)];
}

/** 快照百分比统一按 0-100 收敛；非法值视为无效采样。 */
function clampUsedPct(value: number | null | undefined): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  return Math.min(100, Math.max(0, value));
}

/**
 * 周额度对比内容区：供 ReportsPanel 壳按标签复用，页面外壳（返回/标题栏）
 * 由壳统一渲染，此处只保留工具行（设备筛选 + 刷新）与图表主体。
 */
export function CompareContent({ agentVisibility }: Props) {
  const { t } = useI18n();
  const [weekSlots, setWeekSlots] = useState<WeekSlot[]>([]);
  const [series, setSeries] = useState<QuotaSeries[]>([]);
  const [peaks, setPeaks] = useState<Record<string, (number | null)[]>>({});
  const [sampleCounts, setSampleCounts] = useState<Record<string, number[]>>({});
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
  const compareCacheScope = deviceFilter;

  const hasData =
    series.length > 0 ||
    tokens.some((bucket) => bucket.total_tokens > 0 || bucket.requests > 0);

  const applyCompareCache = useCallback((entry: CompareCacheEntry) => {
    // isCurrent 与周边界都按读取时刻重算：跨周缓存的"本周"会标错位置，
    // 结束点用归一化的"下周一"收口避免 DST 偏差
    const slots = slotsFromMondays(
      entry.weeks.map((week) => week.startMs),
      weekStartOf(Date.now())
    );
    setWeekSlots(slots);
    setSeries(entry.series);
    setPeaks(entry.peaks);
    setSampleCounts(entry.sampleCounts);
    setTokens(entry.tokens);
    setTokensByAgent(entry.tokensByAgent);
    setSelectedIdx((previous) =>
      slots.length === 0
        ? null
        : previous === null || previous >= slots.length
          ? slots.length - 1
          : previous
    );
  }, []);

  // 页面切换回来或设备筛选变化时先读本地缓存，后台刷新完成后再替换。
  useEffect(() => {
    const cached = readCompareCache(compareCacheScope);
    if (cached) {
      applyCompareCache(cached);
      setError(null);
      return;
    }
    setWeekSlots([]);
    setSeries([]);
    setPeaks({});
    setSampleCounts({});
    setTokens([]);
    setTokensByAgent({});
    setSelectedIdx(null);
  }, [applyCompareCache, compareCacheScope]);

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
      // 数据来源：all=本地+远端(排除本机)；local=仅本地；具体id=仅远端该设备。
      // 同步未启用时设备下拉不展示，但 deviceFilter 可能残留具体设备 id，
      // 强制回退本地数据，避免什么都拉不到。
      const wantLocal =
        !syncEnabled || deviceFilter === "all" || deviceFilter === "local";
      const wantRemote = syncEnabled && deviceFilter !== "local";

      const snapshotOpts =
        syncConfig &&
        (deviceFilter === "all"
          ? { excludeDevice: syncConfig.device_id }
          : { devices: deviceFilter });
      const enabledAgents = COMPARE_AGENTS.filter(
        (agent) => agentVisibility[agent]
      );

      // 横轴固定为最近 12 个自然周；查询起点再往前推一周（并归一到周一 0 点，
      // 规避 DST 偏差），容纳快照时间戳与周边界之间的时区/写入延迟误差。
      const weekSlots = buildWeekSlots(Date.now());
      const fromMs = weekStartOf(weekSlots[0].startMs - 7 * DAY_MS);
      const nowMs = Date.now();

      let localHistory: QuotaSnapshot[] = [];
      let remoteHistory: RemoteSnapshot[] = [];
      let localAgentHistory: AgentQuotaSnapshot[] = [];
      let remoteAgentHistory: RemoteAgentQuotaSnapshot[] = [];
      let accounts: AccountMeta[] | null = null;
      const historyTasks: Promise<unknown>[] = [];
      if (wantLocal) {
        // all=true：本机历史含全部账号的快照，按指纹分组正是要利用它
        historyTasks.push(
          getQuotaHistory(true, fromMs).then((history) => (localHistory = history))
        );
      }
      if (wantRemote && syncConfig && snapshotOpts) {
        historyTasks.push(
          remoteSnapshots(fromMs, nowMs, snapshotOpts)
            .then((history) => (remoteHistory = history))
            .catch((e) => {
              // 汇总模式保留本机数据降级展示；具体设备筛选则明确提示失败。
              if (deviceFilter !== "all") {
                throw new Error(
                  t("compare.remoteHistoryFailed", { msg: String(e) })
                );
              }
            })
        );
      }
      if (wantLocal) {
        historyTasks.push(
          getAgentQuotaHistory(fromMs, nowMs)
            .then((history) => (localAgentHistory = history))
            .catch(() => {
              // Agent 历史读取失败不应阻断其余订阅的对比。
            })
        );
      }
      if (wantRemote && syncConfig && snapshotOpts) {
        historyTasks.push(
          remoteAgentQuotaSnapshots(fromMs, nowMs, snapshotOpts)
            .then((history) => (remoteAgentHistory = history))
            .catch(() => {
              // 汇总页保留本机快照；指定设备没有远端快照时仍显示其他可用数据。
            })
        );
      }
      if (enabledAgents.includes("zai")) {
        historyTasks.push(
          listAccounts()
            .then((state) => (accounts = state.accounts))
            .catch(() => {
              // 账号列表读取失败时系列标签退化为指纹前 6 位。
            })
        );
      }
      await Promise.all(historyTasks);
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧响应

      const snapshots = mergeQuotaSnapshots(
        localHistory,
        remoteHistory.map(toQuotaSnapshot)
      );
      const mergedAgentSnapshots = mergeAgentQuotaSnapshots(
        localAgentHistory,
        remoteAgentHistory
      );
      const seriesData = buildQuotaSeries(
        weekSlots,
        snapshots,
        mergedAgentSnapshots,
        enabledAgents,
        accounts,
        t
      );

      // 系列过多（Z.ai 多账号）时每根柱会过细，周数降档到 8；
      // 周槽与各系列峰值/采样数组同步截尾，保持下标对齐
      const cut = seriesData.series.length > 4
        ? weekSlots.length - DENSE_WEEK_COUNT
        : 0;
      const slots = cut > 0 ? weekSlots.slice(cut) : weekSlots;
      const cutSeriesArrays = <T,>(map: Record<string, T[]>): Record<string, T[]> =>
        cut > 0
          ? Object.fromEntries(
              Object.entries(map).map(([id, arr]) => [id, arr.slice(cut)])
            )
          : map;
      const seriesPeaks = cutSeriesArrays(seriesData.peaks);
      const seriesSampleCounts = cutSeriesArrays(seriesData.sampleCounts);

      // Token 口径沿用"按 Agent 实际用量"：本地按自然周区间聚合，
      // 远端小时桶按真实时间戳归入所在自然周。
      const weekPairs = slots.map(
        (slot) => [slot.startMs, slot.endMs] as [number, number]
      );
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
            getCompareTokensForAgent(agent, weekPairs)
              .then((buckets) => {
                localByAgent[agent] = buckets;
              })
              .catch(() => {
                // 可选 Agent 未安装、未登录或没有会话时按空数据处理。
                localByAgent[agent] = emptyTokenBuckets(slots);
              })
          );
        }
      }

      if (wantRemote && syncConfig) {
        for (const agent of enabledAgents) {
          const source = AGENT_META[agent].remoteSource;
          if (!source) continue; // Cursor / Kimi 目前只采集本机，未上传到同步服务
          const usageOpts =
            deviceFilter === "all"
              ? { excludeDevice: syncConfig.device_id, source }
              : { devices: deviceFilter, source };
          tasks.push(
            remoteUsage(
              weekPairs[0][0],
              slots[slots.length - 1].endMs,
              "hour",
              usageOpts
            )
              .then((remote) => {
                remoteByAgent[agent] = aggregateRemoteTokens(slots, remote);
              })
              .catch(() => {
                // 远端没有该来源或服务暂不可用时，不影响其他 Agent 展示。
                remoteByAgent[agent] = emptyTokenBuckets(slots);
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
        const local = localByAgent[agent] ?? emptyTokenBuckets(slots);
        const remote = remoteByAgent[agent] ?? emptyTokenBuckets(slots);
        mergedByAgent[agent] = slots.map((slot, index) => ({
          reset_at: slot.startMs,
          end_at: slot.endMs,
          total_tokens:
            (local[index]?.total_tokens ?? 0) +
            (remote[index]?.total_tokens ?? 0),
          requests:
            (local[index]?.requests ?? 0) + (remote[index]?.requests ?? 0),
        }));
      }

      const mergedTokens: WeeklyTokenBucket[] = slots.map(
        (slot, index) => ({
          reset_at: slot.startMs,
          end_at: slot.endMs,
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
      setWeekSlots(slots);
      setSeries(seriesData.series);
      setPeaks(seriesPeaks);
      setSampleCounts(seriesSampleCounts);
      setTokens(mergedTokens);
      setTokensByAgent(mergedByAgent);
      writeCompareCache(compareCacheScope, {
        weeks: slots.map((slot) => ({
          startMs: slot.startMs,
          isCurrent: slot.isCurrent,
        })),
        series: seriesData.series,
        peaks: seriesPeaks,
        sampleCounts: seriesSampleCounts,
        tokens: mergedTokens,
        tokensByAgent: mergedByAgent,
        ts: Date.now(),
      });

      // 仅在用户未选中或选中索引超出新周范围时才落到本周，
      // 否则保持用户选择：60s 自动刷新不应把用户点选的历史周静默跳回
      setSelectedIdx((prev) =>
        prev === null || prev >= slots.length ? slots.length - 1 : prev
      );
    } catch (e) {
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧错误
      // 取 Error.message，避免 "Error: 中文" 之类的混排前缀
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      // 只有最新请求才允许结束 loading，避免旧请求把新请求的加载态清掉
      if (reqId === loadReqId.current) setLoading(false);
    }
  }, [
    agentVisibility,
    compareCacheScope,
    deviceFilter,
    syncConfig,
    syncEnabled,
    t,
  ]);

  useEffect(() => {
    load();
  }, [load]);

  // 自动刷新 60s（对比页数据变化慢，频率放低）
  useEffect(() => {
    const timer = setInterval(load, 60_000);
    return () => clearInterval(timer);
  }, [load]);

  return (
    <>
      {/* 工具行：设备筛选 + 手动刷新（原页面级 PageHeader 的操作区，
          页面外壳上移 ReportsPanel 后保留在此） */}
      <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8 shrink-0 flex items-center justify-between gap-2">
        {syncEnabled ? (
          <div className="flex items-center gap-1.5 flex-1 min-w-0">
            <span className="text-[10px] text-slate-500 shrink-0">{t("compare.device")}</span>
            <select
              value={deviceFilter}
              onChange={(e) => setDeviceFilter(e.target.value)}
              className="input-box flex-1 text-[10px] py-1"
            >
              <option value="all">{t("compare.all")}</option>
              <option value="local">
                {syncConfig?.device_name
                  ? t("stats.deviceLocalName", { name: syncConfig.device_name })
                  : t("stats.deviceLocal")}
              </option>
              {remoteDevices.filter((d) => d.device_id !== syncConfig?.device_id).map((d) => (
                <option key={d.device_id} value={d.device_id}>
                  {t("common.deviceOption", {
                    name: d.device_name,
                    id: d.device_id.slice(0, 6),
                  })}
                </option>
              ))}
            </select>
          </div>
        ) : (
          <span />
        )}
        <button onClick={load} disabled={loading} className="toolbar-btn shrink-0" title={t("common.refresh")}>↻</button>
      </div>

      {error && <div className="px-3 pt-2"><AlertBanner>{error}</AlertBanner></div>}

      {/* 首次加载（无缓存可用）时显示加载占位，避免整块空白 */}
      {!hasData && loading && !error && <LoadingState />}

      {!hasData && !loading && !error && (
        <EmptyState title={t("compare.emptyTitle")} hint={t("compare.emptyHint")} />
      )}

      {hasData && (
        <PageBody className="page-stack">
          {/* 各订阅周额度峰值分组柱状图 */}
          {series.length > 0 && (
            <WeekChart
              weeks={weekSlots}
              series={series}
              peaks={peaks}
              selectedIdx={selectedIdx}
              onSelect={setSelectedIdx}
            />
          )}

          {/* 选中周明细 */}
          {selectedIdx !== null && weekSlots[selectedIdx] && (
            <WeekDetail
              index={selectedIdx}
              weeks={weekSlots}
              series={series}
              peaks={peaks}
              sampleCounts={sampleCounts}
              token={tokens[selectedIdx]}
              tokensByAgent={tokensByAgent}
            />
          )}

          {weekSlots.length > 0 && (
            <SectionCard title={t("compare.allPeriods")}>
              {/* 行数多时内部滚动（参照汇总页模型排行的做法），避免长列表把面板撑高 */}
              <div className="max-h-48 overflow-y-auto overscroll-contain space-y-0.5">
                {weekSlots
                  .slice()
                  .reverse()
                  .map((week, ri) => {
                    const realIdx = weekSlots.length - 1 - ri;
                    const isSel = realIdx === selectedIdx;
                    const tk = tokens[realIdx]?.total_tokens ?? 0;
                    return (
                      <button
                        key={week.startMs}
                        onClick={() => setSelectedIdx(realIdx)}
                        className={`w-full flex items-center justify-between text-xs py-1.5 px-2 -mx-2 rounded-lg transition-colors ${
                          isSel
                            ? "bg-sky-500/10"
                            : "hover:bg-slate-900/5"
                        }`}
                      >
                        <div className="flex items-center gap-1.5 min-w-0">
                          <span className="font-medium text-slate-900/90 num whitespace-nowrap truncate">
                            {t("compare.weekRange", {
                              from: dateLabel(week.startMs),
                              to: dateLabel(week.endMs - 1),
                            })}
                          </span>
                          {week.isCurrent && (
                            <span className="px-1 py-0 rounded text-[9px] font-semibold bg-sky-500/15 text-sky-700 shrink-0">
                              {t("compare.thisWeek")}
                            </span>
                          )}
                        </div>
                        {/* 峰值%在明细卡里已有，这里只留 Token 总量，防止窄窗溢出 */}
                        <div className="flex items-center gap-1 text-slate-700/60 num shrink-0">
                          <span className="whitespace-nowrap">
                            {formatTokens(tk)} {t("compare.tokenShort")}
                          </span>
                        </div>
                      </button>
                    );
                  })}
              </div>
            </SectionCard>
          )}
        </PageBody>
      )}

      <div className="px-3 py-1.5 border-t border-slate-900/8 text-[9px] text-slate-500 leading-relaxed shrink-0">
        {t("compare.footer")}
      </div>
    </>
  );
}

interface QuotaSeriesData {
  series: QuotaSeries[];
  peaks: Record<string, (number | null)[]>;
  sampleCounts: Record<string, number[]>;
}

/** 从额度快照构建"每系列每周已用峰值"数据。
 *  Z.ai 按账号指纹分组（每账号一个系列）；Codex/Claude/Cursor 各一个系列。
 *  旧快照 account 为 null 的归入一个"Z.ai"系列；若同时存在带指纹系列则
 *  标记为"Z.ai·历史"，避免与真实账号系列混淆。 */
function buildQuotaSeries(
  slots: WeekSlot[],
  zaiSnapshots: QuotaSnapshot[],
  agentSnapshots: AgentQuotaSnapshot[],
  enabledAgents: AgentId[],
  accounts: AccountMeta[] | null,
  t: ReturnType<typeof useI18n>["t"]
): QuotaSeriesData {
  const series: QuotaSeries[] = [];
  const peaks: Record<string, (number | null)[]> = {};
  const sampleCounts: Record<string, number[]> = {};
  const initSeries = (item: QuotaSeries) => {
    series.push(item);
    // null = 该周无采样（不画柱）；采样数只统计有效百分比
    peaks[item.id] = slots.map(() => null);
    sampleCounts[item.id] = slots.map(() => 0);
  };
  const applyPeak = (id: string, ts: number, pct: number | null) => {
    if (pct == null) return;
    const index = slotIndexOf(slots, ts);
    if (index < 0) return;
    const current = peaks[id][index];
    peaks[id][index] = current == null ? pct : Math.max(current, pct);
    sampleCounts[id][index] += 1;
  };

  if (enabledAgents.includes("zai")) {
    const groups = new Map<string, QuotaSnapshot[]>();
    for (const snapshot of zaiSnapshots) {
      if (!Number.isFinite(snapshot.ts)) continue;
      const key = snapshot.account ?? "";
      const list = groups.get(key) ?? [];
      list.push(snapshot);
      groups.set(key, list);
    }
    const keys = [...groups.keys()];
    const single = keys.length <= 1;
    const hasFingerprint = keys.some((key) => key !== "");
    // 指纹组在前（按指纹排序），无指纹的历史组最后
    keys.sort((a, b) => {
      if (a === "") return 1;
      if (b === "") return -1;
      return a.localeCompare(b);
    });
    for (const key of keys) {
      const id = `zai:${key || "history"}`;
      let label: string;
      // 与真实账号系列并存的无指纹历史组，标记 legacy 供明细行悬停解释
      const legacy = key === "" && !single && hasFingerprint;
      if (key === "") {
        label =
          single || !hasFingerprint
            ? AGENT_META.zai.label
            : `${AGENT_META.zai.label}·${t("compare.legacyAccount")}`;
      } else if (single) {
        label = AGENT_META.zai.label;
      } else {
        const name = accounts
          ?.find((account) => account.fingerprint === key)
          ?.display_name?.trim();
        label = `${AGENT_META.zai.label}·${name || key.slice(0, 6)}`;
      }
      initSeries({ id, agent: "zai", account: key || null, label, legacy });
      for (const snapshot of groups.get(key) ?? []) {
        applyPeak(id, snapshot.ts, clampUsedPct(snapshot.weekly_pct));
      }
    }
  }

  for (const agent of COMPARE_AGENTS) {
    const windowKey = AGENT_WINDOW_KEY[agent];
    if (!windowKey || !enabledAgents.includes(agent)) continue;
    const snapshots = agentSnapshots.filter(
      (snapshot) => snapshot.source === agent
    );
    if (snapshots.length === 0) continue; // 无采样的 Agent 不建空系列
    initSeries({ id: agent, agent, account: null, label: AGENT_META[agent].label });
    for (const snapshot of snapshots) {
      const window = snapshot.windows.find((item) => item.key === windowKey);
      if (!window) continue;
      applyPeak(agent, snapshot.ts, clampUsedPct(window.used_pct));
    }
  }

  return { series, peaks, sampleCounts };
}

/** 分组柱状图：每周一组、每系列一根柱，高度 = 该周已用峰值百分比。 */
function WeekChart({
  weeks,
  series,
  peaks,
  selectedIdx,
  onSelect,
}: {
  weeks: WeekSlot[];
  series: QuotaSeries[];
  peaks: Record<string, (number | null)[]>;
  selectedIdx: number | null;
  onSelect: (i: number) => void;
}) {
  const { t } = useI18n();
  const n = weeks.length;
  const barGap = n > 20 ? "gap-px" : n > 10 ? "gap-0.5" : "gap-1";
  const labelStep = n <= 6 ? 1 : Math.max(2, Math.ceil(n / 5));

  return (
    <div className="card-base rounded-2xl px-2.5 py-2">
      <div className="section-title mb-1">{t("compare.chartTitle")}</div>
      <div className="text-[9px] text-slate-700/50 leading-relaxed">
        {t("compare.chartHint")}
      </div>

      {/* 图例：各系列颜色（本周在图内用淡色底 + 轴标签高亮双重标识） */}
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 mt-1.5 mb-2">
        {series.map((item) => (
          <span
            key={item.id}
            className="inline-flex items-center gap-1 text-[9px] text-slate-700/65 min-w-0"
          >
            <span
              className="h-2 w-2 rounded-sm shrink-0"
              style={{ background: seriesColorOf(series, item.id) }}
            />
            <span className="truncate">{item.label}</span>
          </span>
        ))}
      </div>

      {/* 柱区：每周一组按钮（本周淡色底），点击选中该周 */}
      <div className={`flex ${barGap} h-16`}>
        {weeks.map((week, i) => {
          const isSel = i === selectedIdx;
          return (
            <button
              key={week.startMs}
              onClick={() => onSelect(i)}
              className={`flex-1 h-full min-w-0 rounded-sm transition-colors ${
                week.isCurrent ? "bg-sky-500/6" : ""
              } ${isSel ? "ring-1 ring-sky-400/70" : "hover:bg-slate-900/5"}`}
            >
              <div
                className={`flex ${series.length > 3 ? "gap-px" : "gap-0.5"} items-end h-full px-0.5`}
              >
                {series.map((item) => {
                  const pct = peaks[item.id]?.[i] ?? null;
                  return (
                    <div
                      key={item.id}
                      className="flex-1 h-full flex items-end min-w-0"
                      title={
                        pct == null
                          ? undefined
                          : `${dateLabel(week.startMs)} ${item.label} ${t(
                              "compare.peakUsedPct",
                              { pct: Math.round(pct) }
                            )}`
                      }
                    >
                      {pct == null ? null : (
                        <div
                          className="w-full rounded-t-sm transition-all duration-300"
                          style={{
                            height: `${Math.max(pct, 2)}%`,
                            background: seriesColorOf(series, item.id),
                            opacity: isSel ? 1 : 0.85,
                          }}
                        />
                      )}
                    </div>
                  );
                })}
              </div>
            </button>
          );
        })}
      </div>

      {/* X 轴：每周周一日期 "MM-DD"，沿用旧的抽稀规则 */}
      <div className={`flex ${barGap} mt-1`}>
        {weeks.map((week, i) => {
          const showLabel = i === n - 1 || i % labelStep === 0;
          return (
            <span
              key={week.startMs}
              className={`flex-1 text-center text-[8px] num min-w-0 ${
                i === selectedIdx
                  ? "text-sky-600/80 font-medium"
                  : week.isCurrent
                    ? "text-sky-600/60"
                    : "text-slate-700/40"
              } ${showLabel ? "" : "opacity-0"}`}
            >
              {dateLabel(week.startMs)}
            </span>
          );
        })}
      </div>
    </div>
  );
}

/** 选中周明细：各系列峰值已用% + 采样数 + 按 Agent 汇总的实际 Token。 */
function WeekDetail({
  weeks,
  index,
  series,
  peaks,
  sampleCounts,
  token,
  tokensByAgent,
}: {
  weeks: WeekSlot[];
  index: number;
  series: QuotaSeries[];
  peaks: Record<string, (number | null)[]>;
  sampleCounts: Record<string, number[]>;
  token?: WeeklyTokenBucket;
  tokensByAgent: Partial<Record<AgentId, WeeklyTokenBucket[]>>;
}) {
  const { t } = useI18n();
  const week = weeks[index];
  const totalTokens = token?.total_tokens ?? 0;
  // endMs 是下周一 0 点，减 1ms 落回该周周日，保证区间右端显示为周日日期
  const rangeLabel = t("compare.weekRange", {
    from: dateLabel(week.startMs),
    to: dateLabel(week.endMs - 1),
  });
  // Token 按 Agent 汇总（Z.ai 不按账号拆分：用量库没有账号维度）
  const breakdown = COMPARE_AGENTS.map((agent) => ({
    agent,
    bucket: tokensByAgent[agent]?.find(
      (item) => item.reset_at === week.startMs
    ),
  })).filter((item) => (item.bucket?.total_tokens ?? 0) > 0);

  return (
    <SectionCard title={t("compare.selectedWeek")}>
      {/* 标题行：周区间 + 本周徽标；右侧该周 Token 合计（Token 块内不再重复） */}
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 min-w-0">
          <span className="text-[11px] font-semibold text-slate-900/90 num">
            {rangeLabel}
          </span>
          {week.isCurrent && (
            <span className="px-1 py-0 rounded text-[9px] font-semibold bg-sky-500/15 text-sky-700 shrink-0">
              {t("compare.thisWeek")}
            </span>
          )}
        </div>
        <span className="num text-[11px] font-semibold text-slate-900/85 shrink-0">
          {formatTokens(totalTokens)}
          <span className="font-normal text-slate-700/45 ml-1">
            {t("compare.tokenShort")}
          </span>
        </span>
      </div>

      {/* 各额度系列：该周峰值已用% + 采样数（多账号 Z.ai 系列只显示额度，不显示 Token）。
          峰值语义对用户并不自解释，块首补一行口径说明（见 seriesHint）。 */}
      {series.length > 0 && (
        <div className="space-y-1 mt-2">
          <div className="text-[9px] text-slate-700/50">
            {t("compare.seriesHint")}
          </div>
          {series.map((item) => {
            const pct = peaks[item.id]?.[index] ?? null;
            const count = sampleCounts[item.id]?.[index] ?? 0;
            return (
              <div key={item.id} className="flex items-center gap-1.5">
                <BrandIcon
                  brand={AGENT_META[item.agent].brand}
                  className="h-3 w-3 shrink-0"
                  style={{ color: seriesColorOf(series, item.id) }}
                />
                <span
                  className="text-[10px] text-slate-800/80 truncate"
                  title={item.legacy ? t("compare.legacyAccountHint") : undefined}
                >
                  {item.label}
                </span>
                {pct == null ? (
                  <span
                    className="ml-auto text-[9px] text-slate-700/35 shrink-0"
                    title={t("compare.noSample")}
                  >
                    {t("compare.noDataShort")}
                  </span>
                ) : (
                  <span className="ml-auto flex items-baseline gap-1.5 shrink-0">
                    <span className="text-[8px] text-slate-700/40 num">
                      {t("compare.samples", { count })}
                    </span>
                    <span
                      className="text-[11px] num font-semibold whitespace-nowrap"
                      style={{ color: remainingTextColor(100 - pct) }}
                    >
                      {t("compare.peakShort")} {Math.round(pct)}%
                    </span>
                  </span>
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Token 部分：按 Agent 实际用量（与额度百分比是两个口径）。
          总数已在标题行右侧展示，这里只保留口径说明与分 Agent 明细 */}
      <div className="rounded-md bg-surface/25 py-1.5 px-2 mt-2">
        <div className="text-[9px] text-slate-700/55">
          {t("compare.tokensOfAgents")}
        </div>
        {token && (
          <div className="text-[9px] num text-slate-700/45 mt-0.5">
            {t("compare.requestsCount", { count: token.requests })}
          </div>
        )}
        {breakdown.length > 0 && (
          <div className="space-y-0.5 mt-1.5">
            {breakdown.map(({ agent, bucket }) => (
              <div key={agent} className="flex items-center gap-1.5">
                <BrandIcon
                  brand={AGENT_META[agent].brand}
                  className="h-2.5 w-2.5 shrink-0"
                  style={{ color: AGENT_COLOR[agent] }}
                />
                <span className="text-[9px] text-slate-700/70">
                  {AGENT_META[agent].label}
                </span>
                <span className="ml-auto text-[9px] num text-slate-800/75">
                  {formatTokens(bucket?.total_tokens ?? 0)}
                </span>
                <span className="text-[8px] num text-slate-700/40">
                  {t("compare.requestsCount", { count: bucket?.requests ?? 0 })}
                </span>
              </div>
            ))}
          </div>
        )}
        {totalTokens === 0 && (
          <div className="text-[9px] text-slate-700/40 mt-1">
            {t("compare.noUsage")}
          </div>
        )}
      </div>
    </SectionCard>
  );
}

// ===== 辅助 =====

/** 远端快照去掉设备字段，转换为按指纹分组所需的本地结构。 */
function toQuotaSnapshot(snapshot: RemoteSnapshot): QuotaSnapshot {
  const {
    device_id: _deviceId,
    ...quotaSnapshot
  } = snapshot;
  return quotaSnapshot;
}

/** 合并本机与远端额度采样；按"时间 + 账号指纹"去重，同一时刻多设备
 *  重复采样时保留已用比例更高的一条，避免汇总视图低估额度峰值。 */
function mergeQuotaSnapshots(
  local: QuotaSnapshot[],
  remote: QuotaSnapshot[]
): QuotaSnapshot[] {
  const byKey = new Map<string, QuotaSnapshot>();
  for (const snapshot of [...local, ...remote]) {
    if (!Number.isFinite(snapshot.ts)) continue;
    const key = `${snapshot.ts}:${snapshot.account ?? ""}`;
    const previous = byKey.get(key);
    if (
      !previous ||
      snapshot.weekly_pct > previous.weekly_pct ||
      (snapshot.weekly_pct === previous.weekly_pct &&
        snapshot.hour5_pct > previous.hour5_pct)
    ) {
      byKey.set(key, snapshot);
    }
  }
  return [...byKey.values()].sort((a, b) => a.ts - b.ts);
}

function emptyTokenBuckets(slots: WeekSlot[]): WeeklyTokenBucket[] {
  return slots.map(({ startMs, endMs }) => ({
    reset_at: startMs,
    end_at: endMs,
    total_tokens: 0,
    requests: 0,
  }));
}

/** 把远端指定来源的小时桶按真实时间戳分配到自然周槽。 */
function aggregateRemoteTokens(
  slots: WeekSlot[],
  remote: RemoteUsage
): WeeklyTokenBucket[] {
  const buckets = slots.map((slot) => ({
    reset_at: slot.startMs,
    end_at: slot.endMs,
    total_tokens: 0,
    requests: 0,
  }));
  for (const point of remote.trend) {
    const timestamp = parseTrendTimestamp(point.label);
    if (timestamp === null) continue;
    const index = slots.findIndex(
      (slot) => timestamp >= slot.startMs && timestamp < slot.endMs
    );
    if (index < 0) continue;
    buckets[index].total_tokens += point.total_tokens;
    buckets[index].requests += point.requests;
  }
  return buckets;
}

/** 兼容远端服务返回的 ISO 时间、毫秒时间戳和秒时间戳。
 *  ISO 解析依赖服务端 label 携带 UTC 时区标记（"Z" 或 +00:00），
 *  与 src/merge.ts 的 msToLocalLabel 是同一契约。 */
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

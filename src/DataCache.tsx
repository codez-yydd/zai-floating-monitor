import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";
import type {
  AgentQuotaDeltaMap,
  AccountQuotaEntry,
  ClaudeSnapshot,
  CostResult,
  CodexSnapshot,
  CursorSnapshot,
  DeviceInfo,
  PricingConfig,
  QuotaResult,
  RangePreset,
  RemoteUsage,
  Stats,
  SyncConfig,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  accountQuotas as fetchAccountQuotas,
  computeCost,
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchQuota,
  getAgentQuotaHistory,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getSyncConfig,
  getTodayDelta,
  listRemoteDevices,
  remoteUsage,
  remoteAgentQuotaSnapshots,
} from "./api";
import { resolveRange } from "./RangePicker";
import { dateStr } from "./format";
import {
  computeRemoteCost,
  mergeCost,
  mergeStats,
  mergeTrend,
  remoteToStats,
  remoteTrendToLocal,
} from "./merge";
import { loadCache, saveCache } from "./cache";
import { useI18n } from "./i18n";
import {
  calculateAgentQuotaDeltas,
  mergeAgentQuotaSnapshots,
  todayStartMs,
} from "./agentQuota";

/**
 * 全局数据缓存层（v2：按范围缓存 + 后台定时刷新 + 展示只读）。
 *
 * 核心思路：
 *  - 展示层「只读缓存」：切换时间范围 = 改当前 key → 直接读对应缓存条目，
 *    零请求、秒显。stats/cost/trend/codex/claude/cursor 全部取自当前 key 的缓存。
 *  - 后台独立定时任务：定期把各预设范围（today/1d/7d/30d）的数据刷新进缓存，
 *    与范围切换完全解耦——切回来时数据已在缓存里；用户停留在自定义视图时，
 *    当前 custom 范围（z.ai + Codex + Claude + Cursor）也顺带刷新，保证 lastUpdate 常新。
 *  - 按需补刷：切到无缓存/过期范围（如全新 custom、首次进入、数据老化、配置
 *    就绪）才触发一次加载，之后也进缓存。
 *  - 刷新期间不清空旧值（只设 refreshing），新数据到达后平滑替换 → 无闪烁。
 *  - pricing 未就绪时保留旧 cost，避免空价格表把花费覆盖成 0。
 *  - localStorage 持久化：进程重启后各范围仍秒显。
 */

// 需要后台持续刷新的预设范围（custom 无法预缓存，仅当用户停留在自定义视图时
// 由后台 tick / visibilitychange 顺带刷新当前 custom 范围）
const PRESET_RANGES: RangePreset[] = ["today", "1d", "7d", "30d"];

// custom 缓存条目上限：超过则按 ts 淘汰最老的，避免长期累积撑爆 localStorage
const MAX_CUSTOM_ENTRIES = 8;

/** 由 preset 派生趋势分桶：今日/24h 按小时，更长范围按日 */
function bucketOf(preset: RangePreset): TrendBucket {
  return preset === "today" || preset === "1d" ? "hour" : "day";
}

/** 范围缓存 key：预设用 preset 名，custom 用日期区间 */
function rangeKey(
  preset: RangePreset,
  custom: { from: string; to: string }
): string {
  return preset === "custom" ? `custom:${custom.from}~${custom.to}` : preset;
}

/** 价格表是否为空（未加载占位）——空时不覆盖 cost，避免把花费算成 0 */
function isPricingEmpty(p: PricingConfig): boolean {
  return Object.keys(p.usd).length === 0;
}

/**
 * 淘汰超额的 custom 缓存条目（按 ts 升序删除最老的）。
 * key 形如 `${df}|custom:...` 或 `custom:...`，均含 "custom:"。
 * 预设范围 key（today/1d/7d/30d）固定 4 个，不参与淘汰。
 */
function trimCustomEntries<T extends { ts: number }>(
  map: Record<string, T>,
  max: number
): Record<string, T> {
  const customKeys = Object.keys(map).filter((k) => k.includes("custom:"));
  if (customKeys.length <= max) return map;
  customKeys.sort((a, b) => map[a].ts - map[b].ts);
  const next = { ...map };
  for (const k of customKeys.slice(0, customKeys.length - max)) delete next[k];
  return next;
}

// 缓存过期阈值：超过则按需补刷（略大于后台刷新间隔，避免边界抖动）
const ZAI_STALE_MS = 60_000;
const CURSOR_STALE_MS = 240_000;
const CODEX_STALE_MS = 60_000;
const CLAUDE_STALE_MS = 60_000;

interface ZaiEntry {
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];
  error: string | null;
  refreshing: boolean;
  ts: number;
}

interface CursorEntry {
  snapshot: CursorSnapshot | null;
  error: string | null;
  refreshing: boolean;
  ts: number;
}

/** Codex / Claude 快照的公共形状（两者结构完全一致，TS 结构化类型互通，
 *  各自的名义类型可直接赋给本接口）。 */
interface AgentSnapshot {
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

interface AgentEntry {
  snapshot: AgentSnapshot | null;
  error: string | null;
  refreshing: boolean;
  ts: number;
}

const EMPTY_AGENT: AgentEntry = {
  snapshot: null,
  error: null,
  refreshing: false,
  ts: 0,
};

const EMPTY_ZAI: ZaiEntry = {
  stats: null,
  cost: null,
  trend: [],
  error: null,
  refreshing: false,
  ts: 0,
};

const EMPTY_CURSOR: CursorEntry = {
  snapshot: null,
  error: null,
  refreshing: false,
  ts: 0,
};

const EMPTY_CODEX: AgentEntry = EMPTY_AGENT;
const EMPTY_CLAUDE: AgentEntry = EMPTY_AGENT;

/** 冷启动加载缓存时清掉可能被持久化的 refreshing 标志（崩溃恢复场景）。 */
function stripRefreshing<T extends { refreshing: boolean }>(
  map: Record<string, T>
): Record<string, T> {
  for (const k of Object.keys(map)) map[k] = { ...map[k], refreshing: false };
  return map;
}

export interface DataCacheValue {
  // ===== 查询参数（UI 可修改，变化时只切当前 key，不触发请求）=====
  preset: RangePreset;
  custom: { from: string; to: string };
  /** 由 preset 派生：今日/24h 按小时，更长范围按日 */
  trendBucket: TrendBucket;
  deviceFilter: string;
  setPreset: (p: RangePreset) => void;
  setCustom: (c: { from: string; to: string }) => void;
  setDeviceFilter: (d: string) => void;

  // ===== z.ai 数据（当前范围，读缓存）=====
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];

  // ===== Codex 数据（当前范围，读缓存；stats/trend 与 z.ai 同构）=====
  codex: CodexSnapshot | null;
  /** Codex 错误信息（如未安装，不阻塞其他来源展示） */
  codexError: string | null;

  // ===== Claude 数据（当前范围，读缓存；stats/trend 与 z.ai 同构）=====
  claude: ClaudeSnapshot | null;
  /** Claude 错误信息（如未安装，不阻塞其他来源展示） */
  claudeError: string | null;

  // ===== Cursor 数据（当前范围，读缓存）=====
  cursor: CursorSnapshot | null;
  cursorError: string | null;
  /** USD→CNY 汇率（汇总页合并花费用） */
  fxRate: number;

  // ===== Quota 数据（与范围无关）=====
  quota: QuotaResult | null;
  todayDelta: [number, number] | null;
  /** Codex / Claude / Cursor 今日额度增量，按来源和窗口索引。 */
  agentQuotaDeltas: AgentQuotaDeltaMap;
  quotaError: string | null;
  /** 全部账号快照的订阅额度（当前账号条目已用 30s live quota 覆盖，见 value 派生） */
  accountQuotas: AccountQuotaEntry[];
  /** 手动刷新多账号额度（账号捕获/切换/删除/重命名后调用） */
  refreshAccountQuotas: () => void;

  // ===== 同步配置（启动时加载一次）=====
  syncConfig: SyncConfig | null;
  remoteDevices: DeviceInfo[];
  syncEnabled: boolean;

  // ===== 元信息 =====
  /** 当前各数据源中最旧一次成功刷新的时间（最保守的新鲜度下限） */
  lastUpdate: number;
  /** 当前范围是否在后台拉取（仅用于刷新按钮转圈，不阻塞渲染） */
  refreshing: boolean;
  /** z.ai 错误信息（不阻塞已有数据展示） */
  error: string | null;
  /** 手动强制刷新当前范围（z.ai + Codex + Claude + Cursor） */
  refresh: () => void;
  /** 手动强制刷新 Coding Plan 额度 */
  refreshQuota: () => void;
}

const Ctx = createContext<DataCacheValue | null>(null);

interface ProviderProps {
  /** 价格表（合并远程花费时需要） */
  pricing: PricingConfig;
  children: ReactNode;
}

export function DataProvider({ pricing, children }: ProviderProps) {
  // ===== i18n：t 走 ref 镜像读取，避免把 t 列进刷新回调依赖导致
  //      语言切换重建全部定时器 / 触发全范围刷新 =====
  const { t } = useI18n();
  const tRef = useRef(t);
  useEffect(() => {
    tRef.current = t;
  }, [t]);

  // ===== 查询参数（持久化，冷启动恢复上次视图，与上次缓存匹配）=====
  const [preset, setPreset] = useState<RangePreset>(
    () => loadCache<RangePreset>("zbar-preset") ?? "today"
  );
  const [custom, setCustom] = useState<{ from: string; to: string }>(() => {
    const saved = loadCache<{ from: string; to: string }>("zbar-custom");
    if (saved) return saved;
    // 本地时区日期（dateStr）；不能用 toISOString（UTC），东八区凌晨会错一天
    const today = dateStr(Date.now());
    const week = dateStr(Date.now() - 6 * 86400000);
    return { from: week, to: today };
  });
  const [deviceFilter, setDeviceFilter] = useState<string>(
    () => loadCache<string>("zbar-device") ?? "all"
  );

  const trendBucket = bucketOf(preset);

  // ===== 按范围缓存（持久化，key: zai/codex=`${df}|${rangeKey}`，cursor=rangeKey）=====
  const [zaiCache, setZaiCache] = useState<Record<string, ZaiEntry>>(
    () => stripRefreshing(loadCache<Record<string, ZaiEntry>>("zbar-zai-cache") ?? {})
  );
  const [cursorCache, setCursorCache] = useState<Record<string, CursorEntry>>(
    () => stripRefreshing(loadCache<Record<string, CursorEntry>>("zbar-cursor-cache") ?? {})
  );
  const [codexCache, setCodexCache] = useState<Record<string, AgentEntry>>(
    () => stripRefreshing(loadCache<Record<string, AgentEntry>>("zbar-codex-cache") ?? {})
  );
  const [claudeCache, setClaudeCache] = useState<Record<string, AgentEntry>>(
    () => stripRefreshing(loadCache<Record<string, AgentEntry>>("zbar-claude-cache") ?? {})
  );

  // ===== 其他数据（持久化）=====
  const [fxRate, setFxRate] = useState<number>(
    () => loadCache<number>("zbar-fxrate") ?? 7.2
  );
  const [quota, setQuota] = useState<QuotaResult | null>(
    () => loadCache<QuotaResult>("zbar-quota")
  );
  const [todayDelta, setTodayDelta] = useState<[number, number] | null>(
    () => loadCache<[number, number]>("zbar-today-delta")
  );
  const [agentQuotaDeltas, setAgentQuotaDeltas] = useState<AgentQuotaDeltaMap>({});
  const [quotaError, setQuotaError] = useState<string | null>(null);
  // 多账号额度（低频 5 分钟一轮 + 账号操作后手动刷，持久化供冷启动秒显）
  const [accountQuotas, setAccountQuotas] = useState<AccountQuotaEntry[]>(
    () => loadCache<AccountQuotaEntry[]>("zbar-account-quotas") ?? []
  );

  // ===== 同步配置 =====
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // ===== 并发保护：同范围正在刷新则跳过，避免重复请求 =====
  const zaiInflight = useRef<Set<string>>(new Set());
  const cursorInflight = useRef<Set<string>>(new Set());
  const codexInflight = useRef<Set<string>>(new Set());
  const claudeInflight = useRef<Set<string>>(new Set());
  const quotaReqId = useRef(0);
  const accountQuotasInflight = useRef(false);
  // 两路额度数据各自的最近成功时刻：切换账号后 refreshQuota（live，读 ~/.zcode
  // 实时凭证）与 refreshAccountQuotas（读快照 + 实时指纹）并行、到达顺序不定，
  // 合并时只允许较新一路覆盖当前账号条目，避免把旧账号额度贴到新账号名下
  const quotaTsRef = useRef(0);
  const accountQuotasTsRef = useRef(0);
  const agentQuotaReqId = useRef(0);
  const agentQuotaDeltaRefreshRef = useRef<() => void>(() => {});
  const agentQuotaReloadTimer = useRef<number | null>(null);

  // 各 Agent 的额度查询完成后立即重读历史，避免网络较慢时首轮 3 秒读取
  // 先于快照写入，用户要等到下一轮 30 秒定时器才看到今日增量。
  const scheduleAgentQuotaReload = useCallback(() => {
    if (agentQuotaReloadTimer.current != null) return;
    agentQuotaReloadTimer.current = window.setTimeout(() => {
      agentQuotaReloadTimer.current = null;
      agentQuotaDeltaRefreshRef.current();
    }, 200);
  }, []);

  // ===== refs：按需补刷 effect 读取最新缓存，避免闭包过期 / 反复触发 =====
  const zaiCacheRef = useRef(zaiCache);
  useEffect(() => {
    zaiCacheRef.current = zaiCache;
  }, [zaiCache]);
  const cursorCacheRef = useRef(cursorCache);
  useEffect(() => {
    cursorCacheRef.current = cursorCache;
  }, [cursorCache]);
  const codexCacheRef = useRef(codexCache);
  useEffect(() => {
    codexCacheRef.current = codexCache;
  }, [codexCache]);
  const claudeCacheRef = useRef(claudeCache);
  useEffect(() => {
    claudeCacheRef.current = claudeCache;
  }, [claudeCache]);

  // ===== refs：后台 tick 读取最新 preset/custom（用户停留在自定义视图时把当前
  //      custom 范围纳入刷新用）。经 ref 读取而不是列入定时器 effect 依赖，
  //      避免每次切换范围都重建 30s 定时器 =====
  const presetRef = useRef(preset);
  useEffect(() => {
    presetRef.current = preset;
  }, [preset]);
  const customRef = useRef(custom);
  useEffect(() => {
    customRef.current = custom;
  }, [custom]);

  /**
   * 刷新单个 z.ai 范围（范围无关，参数化 df/from/to/bucket）。
   * 保留原 local/remote/merge 合并逻辑；刷新期间不清空旧值，只设 refreshing。
   * pricing 未就绪时保留旧 cost，避免空价格表把花费覆盖成 0（冷启动保护）。
   */
  const refreshZaiRange = useCallback(
    async (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) => {
      if (zaiInflight.current.has(key)) return; // 同范围已在刷新，跳过
      zaiInflight.current.add(key);
      setZaiCache((prev) => ({
        ...prev,
        [key]: { ...(prev[key] ?? EMPTY_ZAI), refreshing: true },
      }));

      try {
        let localStats: Stats | null = null;
        let localCost: CostResult | null = null;
        let localTrend: TrendPoint[] = [];
        let remote: RemoteUsage | null = null;

        const wantLocal = df === "all" || df === "local";
        // all 或指定远端设备时拉远程数据；选 local 时不拉
        const wantRemote = syncEnabled && df !== "local";

        const tasks: Promise<unknown>[] = [];
        if (wantLocal) {
          tasks.push(
            fetchStats(from, to).then((s) => (localStats = s)),
            computeCost(from, to).then((c) => (localCost = c)),
            fetchTrend(from, to, bucket).then((t) => (localTrend = t))
          );
        }
        if (wantRemote && syncConfig) {
          // source 固定 zcode：服务端不传 source 会合并全部来源，
          // 其他设备上传的 Codex 数据会混入 Z.ai 口径并在汇总页重复计数
          const opts =
            df === "all"
              ? { excludeDevice: syncConfig.device_id, source: "zcode" }
              : { devices: df, source: "zcode" };
          const isSpecificRemote = df !== "all";
          tasks.push(
            remoteUsage(from, to, bucket, opts)
              .then((r) => (remote = r))
              .catch((e) => {
                if (isSpecificRemote) throw e;
                // "全部"模式：远端失败静默，仅用本地数据
              })
          );
        }

        await Promise.all(tasks);

        // 合并本地 + 远端
        let stats: Stats | null;
        let cost: CostResult | null;
        let trend: TrendPoint[];
        if (df === "local") {
          stats = localStats;
          cost = localCost;
          trend = localTrend;
        } else if (remote && !localStats) {
          stats = remoteToStats(remote);
          cost = computeRemoteCost(remote, pricing, fxRate);
          trend = remoteTrendToLocal(remote, pricing, fxRate, bucket);
        } else if (localStats && remote) {
          stats = mergeStats(localStats, remote);
          cost = mergeCost(localCost, remote, pricing, fxRate);
          trend = mergeTrend(localTrend, remote, pricing, fxRate, bucket);
        } else {
          stats = localStats;
          cost = localCost;
          trend = localTrend;
        }

        setZaiCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                stats,
                // pricing 未就绪时保留旧 cost，避免空价格表覆盖成 0
                cost: isPricingEmpty(pricing) ? (prev[key]?.cost ?? null) : cost,
                trend,
                error: null,
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } catch (e) {
        setZaiCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                ...(prev[key] ?? EMPTY_ZAI),
                error: String(e),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } finally {
        zaiInflight.current.delete(key);
      }
    },
    // 注意不含 currency：花费是双货币一次算齐的，切货币只需展示层换字段，
    // 不需要重刷数据；若把 currency 列进依赖会导致切货币触发全范围刷新风暴（UI 卡顿）
    [syncConfig, syncEnabled, pricing, fxRate]
  );

  /** 刷新单个 Cursor 范围（账号级，不受设备筛选影响）。刷新期间不清空旧值。 */
  const refreshCursorRange = useCallback((key: string, from: number, to: number) => {
    if (cursorInflight.current.has(key)) return;
    cursorInflight.current.add(key);
    setCursorCache((prev) => ({
      ...prev,
      [key]: { ...(prev[key] ?? EMPTY_CURSOR), refreshing: true },
    }));

    fetchCursorUsage(from, to)
      .then((data) => {
        setCursorCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: { snapshot: data, error: null, refreshing: false, ts: Date.now() },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      })
      .catch((e) => {
        setCursorCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                ...(prev[key] ?? EMPTY_CURSOR),
                error: String(e),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      })
      .finally(() => {
        cursorInflight.current.delete(key);
        scheduleAgentQuotaReload();
      });
  }, [scheduleAgentQuotaReload]);

  /**
   * 刷新单个派生来源（Codex / Claude）范围。两者数据链路完全同构，仅数据源
   * 不同，抽成参数化实现（fetchLocal + 远端 source 参数 + 各自缓存）。
   * 复用 z.ai 的 Stats/RemoteUsage 合并函数——快照的 stats/trend 与 z.ai 同构。
   * 远端合并开关与 z.ai 一致：syncEnabled 且设备筛选非 "local" 时拉
   * remote_usage(source=...)；本地未安装时软失败，远端有数据仍展示远端；
   * 完全无数据才记录 error。rate_limits 始终保持本地值。
   */
  const refreshAgentRange = useCallback(
    async (
      agent: {
        source: "codex" | "claude";
        label: string;
        fetchLocal: (
          from: number,
          to: number,
          bucket: TrendBucket
        ) => Promise<AgentSnapshot>;
        inflight: Set<string>;
        setCache: React.Dispatch<
          React.SetStateAction<Record<string, AgentEntry>>
        >;
      },
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) => {
      const { source, label, fetchLocal, inflight, setCache } = agent;
      if (inflight.has(key)) return;
      inflight.add(key);
      setCache((prev) => ({
        ...prev,
        [key]: { ...(prev[key] ?? EMPTY_AGENT), refreshing: true },
      }));

      // 本地命令错误单独记录（未安装等）：远端有数据时仍可展示，不整体失败
      let localError: string | null = null;
      // 用对象盒子承接 then 回调里的赋值：回调内赋值不参与控制流分析，
      // 直接用 let 会被 TS 收窄成初始 null，后面无法安全解引用 stats/trend
      const localBox: { snapshot: AgentSnapshot | null } = { snapshot: null };
      try {
        let remote: RemoteUsage | null = null;

        const wantLocal = df === "all" || df === "local";
        const wantRemote = syncEnabled && df !== "local";

        const tasks: Promise<unknown>[] = [];
        if (wantLocal) {
          tasks.push(
            fetchLocal(from, to, bucket)
              .then((s) => (localBox.snapshot = s))
              .catch((e) => {
                localError = String(e);
              })
          );
        }
        if (wantRemote && syncConfig) {
          const opts =
            df === "all"
              ? { excludeDevice: syncConfig.device_id, source }
              : { devices: df, source };
          const isSpecificRemote = df !== "all";
          tasks.push(
            remoteUsage(from, to, bucket, opts)
              .then((r) => (remote = r))
              .catch((e) => {
                if (isSpecificRemote) throw e;
                // "全部"模式：远端失败静默，仅用本地数据
              })
          );
        }

        await Promise.all(tasks);
        const local = localBox.snapshot;

        // 合并本地 + 远端（与 z.ai 同款三分支）
        let snapshot: AgentSnapshot | null;
        if (df === "local") {
          snapshot = local;
        } else if (remote && !local) {
          snapshot = {
            stats: remoteToStats(remote),
            trend: remoteTrendToLocal(remote, pricing, fxRate, bucket),
            rate_limits: null,
          };
        } else if (local && remote) {
          snapshot = {
            stats: mergeStats(local.stats, remote),
            trend: mergeTrend(local.trend, remote, pricing, fxRate, bucket),
            rate_limits: local.rate_limits,
          };
        } else {
          snapshot = local;
        }

        setCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                snapshot,
                // 有数据（含仅远端）即清错误；完全无数据时透出本地错误
                error: snapshot
                  ? null
                  : localError ?? tRef.current("stats.noDataFor", { name: label }),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } catch (e) {
        setCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                ...(prev[key] ?? EMPTY_AGENT),
                error: String(e),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } finally {
        inflight.delete(key);
        scheduleAgentQuotaReload();
      }
    },
    [syncConfig, syncEnabled, pricing, fxRate, scheduleAgentQuotaReload]
  );

  const refreshCodexRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshAgentRange(
        {
          source: "codex",
          label: "Codex",
          fetchLocal: fetchCodexUsage,
          inflight: codexInflight.current,
          setCache: setCodexCache,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshAgentRange]
  );

  const refreshClaudeRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshAgentRange(
        {
          source: "claude",
          label: "Claude",
          fetchLocal: fetchClaudeUsage,
          inflight: claudeInflight.current,
          setCache: setClaudeCache,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshAgentRange]
  );

  // Quota 数据加载（与范围无关，每次调用会采样写入 quota_history）。
  const loadQuota = useCallback(() => {
    const reqId = ++quotaReqId.current;
    fetchQuota()
      .then((r) => {
        if (reqId !== quotaReqId.current) return;
        quotaTsRef.current = Date.now();
        setQuota(r);
        setQuotaError(null);
        // 额度刷新成功后顺带读今日增量（快照由 fetch_quota 采样写入）
        getTodayDelta().then(setTodayDelta).catch(() => {});
      })
      .catch((e) => {
        if (reqId !== quotaReqId.current) return;
        setQuotaError(String(e));
      });
  }, []);

  // 多账号额度加载（读各账号快照里的凭证并行查询，不写额度历史）。
  // 失败静默保留旧值：账号级失败原因在各条目 error 字段里，不设全局 error。
  const loadAccountQuotas = useCallback(() => {
    if (accountQuotasInflight.current) return;
    accountQuotasInflight.current = true;
    fetchAccountQuotas()
      .then((r) => {
        accountQuotasTsRef.current = Date.now();
        setAccountQuotas(r);
      })
      .catch(() => {})
      .finally(() => {
        accountQuotasInflight.current = false;
      });
  }, []);

  /** 读取并合并今日 Agent 额度快照，供详情页与汇总页共用。 */
  const loadAgentQuotaDeltas = useCallback(() => {
    const reqId = ++agentQuotaReqId.current;
    const from = todayStartMs();
    const to = Date.now() + 1;
    const localPromise = getAgentQuotaHistory(from, to);
    const wantRemote = syncEnabled && deviceFilter !== "local" && !!syncConfig;
    const remotePromise = wantRemote
      ? remoteAgentQuotaSnapshots(from, to, deviceFilter === "all"
          ? { excludeDevice: syncConfig!.device_id }
          : { devices: deviceFilter })
      : Promise.resolve([]);

    Promise.all([
      localPromise,
      remotePromise.catch(() => []),
    ])
      .then(([local, remote]) => {
        if (reqId !== agentQuotaReqId.current) return;
        const localSelected = deviceFilter === "local" || deviceFilter === "all"
          ? local
          : [];
        const merged = mergeAgentQuotaSnapshots(localSelected, remote);
        setAgentQuotaDeltas(calculateAgentQuotaDeltas(merged, from, to));
      })
      .catch(() => {
        if (reqId === agentQuotaReqId.current) setAgentQuotaDeltas({});
      });
  }, [deviceFilter, syncConfig, syncEnabled]);

  // 刷新函数定义在数据源刷新函数之后，因此用 ref 让前面的完成回调
  // 能调用到最新的设备筛选与同步配置。
  agentQuotaDeltaRefreshRef.current = loadAgentQuotaDeltas;

  // 设备或同步身份切换时立即作废上一轮结果，避免新筛选器短暂显示旧设备的增量。
  useEffect(() => {
    agentQuotaReqId.current += 1;
    setAgentQuotaDeltas({});
  }, [deviceFilter, syncConfig?.device_id, syncEnabled]);

  // 初次加载：同步配置 + 设备列表（仅一次）
  useEffect(() => {
    getSyncConfig()
      .then((cfg) => {
        setSyncConfig(cfg);
        if (cfg.enabled && cfg.device_token) {
          listRemoteDevices().then(setRemoteDevices).catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  // ===== 后台定时刷新 z.ai + Codex + Claude：当前 deviceFilter 的所有预设范围（与 preset/custom 解耦）。
  //      每 30s 刷一遍 today/1d/7d/30d → 切换预设范围时缓存命中、秒显。
  //      用户停留在自定义视图时，当前 custom 范围也顺带刷一轮（复用既有
  //      refreshZaiRange/refreshAgentRange 路径与 inflight 去重；custom 缓存 key 独立，
  //      不混淆预设缓存）。Codex/Claude 与 z.ai 同一轮刷新，无需单独定时器错峰。
  //      依赖不含 preset/custom：预设范围的 resolveRange 忽略 custom，custom 经 ref 读最新值。 =====
  useEffect(() => {
    const df = deviceFilter;
    const tick = () => {
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshZaiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCodexRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshClaudeRange(df, `${df}|${p}`, f, t, bucketOf(p));
      }
      // 自定义视图当前展示的范围纳入本轮刷新：custom 无法预缓存（日期区间任意），
      // 不顺带刷的话该视图只能靠按需补刷，lastUpdate 会长期显示旧时间
      if (presetRef.current === "custom") {
        const key = `${df}|${rangeKey("custom", customRef.current)}`;
        const [f, t] = resolveRange("custom", customRef.current);
        refreshZaiRange(df, key, f, t, bucketOf("custom"));
        refreshCodexRange(df, key, f, t, bucketOf("custom"));
        refreshClaudeRange(df, key, f, t, bucketOf("custom"));
      }
    };
    // 延后首刷（500ms）：WebView 重载（首次打开 / 长期隐藏后被系统回收）后，
    // 首屏先靠 localStorage 缓存渲染；挂载即并发发起十几个数据命令的话，
    // 响应陆续回来的主线程处理（IPC 解析 + 全量缓存写盘）会推迟首帧，造成白屏
    const first = setTimeout(tick, 500);
    const timer = setInterval(tick, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
    // 故意不把 preset/custom 列入依赖：后台刷预设范围与二者无关（custom 仅在
    // 自定义视图时经 ref 顺带刷新），列入会导致每次切范围重建定时器
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceFilter, syncConfig, syncEnabled, pricing, refreshZaiRange, refreshCodexRange, refreshClaudeRange]);

  // ===== 后台定时刷新 Cursor：所有预设范围，降频 180s（网络慢，4 范围并行）。
  //      账号级，不受 deviceFilter 影响。汇率每 tick 只读一次（与范围循环解耦）。
  //      用户停留在自定义视图时，当前 custom 范围也顺带刷一轮（与 z.ai 后台
  //      tick 同款 ref 模式；cursorCache 的 inflight 去重复用）。 =====
  useEffect(() => {
    const tick = () => {
      // 每 tick 读一次汇率（用户可能在设置页改过），避免每个范围重复读。
      // >0 校验：非法值（手改文件等）保持现值，与后端 load_fx_rate 的 7.2 兜底口径一致
      getCursorConfig()
        .then((cfg) => {
          if (cfg.usd_cny_rate > 0) setFxRate(cfg.usd_cny_rate);
        })
        .catch(() => {});
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshCursorRange(p, f, t);
      }
      // 自定义视图当前范围纳入本轮：lastUpdate 取 zai/cursor 两者较旧值，
      // 只刷 z.ai 不刷 cursor 的话仍会被旧 cursor 条目拉低
      if (presetRef.current === "custom") {
        const [f, t] = resolveRange("custom", customRef.current);
        refreshCursorRange(rangeKey("custom", customRef.current), f, t);
      }
    };
    // 延后首刷（1.5s）错峰：避免与 z.ai 首刷、首屏渲染竞争主线程
    const first = setTimeout(tick, 1_500);
    const timer = setInterval(tick, 180_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshCursorRange]);

  // Quota 定时 30s（与范围无关）。延后首刷（2.5s）错峰，避免与首屏渲染竞争
  useEffect(() => {
    const first = setTimeout(loadQuota, 2_500);
    const timer = setInterval(loadQuota, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadQuota]);

  // 多账号额度定时 300s：非当前账号的额度只随窗口翻转变化，低频足够；
  // 当前账号条目展示时由 30s 的 quota 实时覆盖（见下方 value 派生），
  // 不必与 quota 同频。延后首刷（3.5s）再错一峰。
  useEffect(() => {
    const first = setTimeout(loadAccountQuotas, 3_500);
    const timer = setInterval(loadAccountQuotas, 300_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadAccountQuotas]);

  // Agent 额度快照在各数据源首刷后读取，后续与额度查询同频刷新。
  useEffect(() => {
    const first = setTimeout(loadAgentQuotaDeltas, 3_000);
    const timer = setInterval(loadAgentQuotaDeltas, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadAgentQuotaDeltas]);

  // ===== 按需补刷：切到无缓存/过期范围，或配置就绪后补一次。
  //      预设范围通常已被后台任务刷新 → 命中新鲜缓存 → 不请求、秒显。
  //      依赖含 refreshZaiRange/refreshCursorRange/refreshCodexRange：pricing/syncConfig
  //      就绪后函数重建，本 effect 重跑 → custom 范围用就绪配置重刷自愈（修复冷启动覆盖问题）。 =====
  useEffect(() => {
    const [f, t] = resolveRange(preset, custom);
    const zKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
    const zEntry = zaiCacheRef.current[zKey];
    if (!zEntry || Date.now() - zEntry.ts > ZAI_STALE_MS) {
      refreshZaiRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const cKey = rangeKey(preset, custom);
    const cEntry = cursorCacheRef.current[cKey];
    if (!cEntry || Date.now() - cEntry.ts > CURSOR_STALE_MS) {
      refreshCursorRange(cKey, f, t);
    }
    const xEntry = codexCacheRef.current[zKey];
    if (!xEntry || Date.now() - xEntry.ts > CODEX_STALE_MS) {
      refreshCodexRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const aEntry = claudeCacheRef.current[zKey];
    if (!aEntry || Date.now() - aEntry.ts > CLAUDE_STALE_MS) {
      refreshClaudeRange(deviceFilter, zKey, f, t, trendBucket);
    }
    // 仅在范围/设备/刷新函数变化时触发；不依赖 cache 内容（靠 ref 读最新值）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preset, custom, deviceFilter, trendBucket, refreshZaiRange, refreshCursorRange, refreshCodexRange, refreshClaudeRange]);

  // ===== 窗口恢复可见时主动补刷：隐藏期间 setInterval 常被节流，恢复后立即补齐
  //      当前 deviceFilter 的预设范围，保证"常驻新鲜"。
  //      用户停留在自定义视图时，当前 custom 范围（z.ai + Codex + Cursor）同样补刷。 =====
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      const df = deviceFilter;
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshZaiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCodexRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshClaudeRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCursorRange(p, f, t);
      }
      if (presetRef.current === "custom") {
        const cKey = rangeKey("custom", customRef.current);
        const [f, t] = resolveRange("custom", customRef.current);
        refreshZaiRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshCodexRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshClaudeRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshCursorRange(cKey, f, t);
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceFilter, refreshZaiRange, refreshCursorRange, refreshCodexRange, refreshClaudeRange]);

  // ===== 持久化：各 state 变化即落盘，供下次冷启动各范围秒显 =====
  useEffect(() => {
    saveCache("zbar-preset", preset);
  }, [preset]);
  useEffect(() => {
    saveCache("zbar-custom", custom);
  }, [custom]);
  useEffect(() => {
    saveCache("zbar-device", deviceFilter);
  }, [deviceFilter]);
  useEffect(() => {
    saveCache("zbar-zai-cache", zaiCache);
  }, [zaiCache]);
  useEffect(() => {
    saveCache("zbar-cursor-cache", cursorCache);
  }, [cursorCache]);
  useEffect(() => {
    saveCache("zbar-codex-cache", codexCache);
  }, [codexCache]);
  useEffect(() => {
    saveCache("zbar-claude-cache", claudeCache);
  }, [claudeCache]);
  useEffect(() => {
    saveCache("zbar-fxrate", fxRate);
  }, [fxRate]);
  useEffect(() => {
    if (quota) saveCache("zbar-quota", quota);
  }, [quota]);
  useEffect(() => {
    if (todayDelta) saveCache("zbar-today-delta", todayDelta);
  }, [todayDelta]);
  useEffect(() => {
    saveCache("zbar-account-quotas", accountQuotas);
  }, [accountQuotas]);

  // 手动刷新：刷新当前范围（z.ai + Codex + Claude + Cursor）一次
  const refresh = useCallback(() => {
    const [f, t] = resolveRange(preset, custom);
    const zKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
    refreshZaiRange(deviceFilter, zKey, f, t, trendBucket);
    refreshCodexRange(deviceFilter, zKey, f, t, trendBucket);
    refreshClaudeRange(deviceFilter, zKey, f, t, trendBucket);
    refreshCursorRange(rangeKey(preset, custom), f, t);
    // 额度采样由上述后台命令写入，稍后读取以覆盖本次手动刷新产生的快照。
    window.setTimeout(loadAgentQuotaDeltas, 1_000);
  }, [preset, custom, deviceFilter, trendBucket, refreshZaiRange, refreshCodexRange, refreshClaudeRange, refreshCursorRange, loadAgentQuotaDeltas]);

  const refreshQuota = useCallback(() => loadQuota(), [loadQuota]);
  const refreshAccountQuotas = useCallback(
    () => loadAccountQuotas(),
    [loadAccountQuotas]
  );

  // ===== 当前范围数据（展示层只读这些）=====
  const curZaiKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
  const curCursorKey = rangeKey(preset, custom);
  const curZai = zaiCache[curZaiKey] ?? EMPTY_ZAI;
  const curCursor = cursorCache[curCursorKey] ?? EMPTY_CURSOR;
  const curCodex = codexCache[curZaiKey] ?? EMPTY_CODEX;
  const curClaude = claudeCache[curZaiKey] ?? EMPTY_CLAUDE;

  // lastUpdate 取当前范围各数据源里最旧的成功时间（保守的新鲜度下限）
  const updateTs = [curZai.ts, curCursor.ts, curCodex.ts, curClaude.ts].filter(
    (t) => t > 0
  );
  const lastUpdate = updateTs.length ? Math.min(...updateTs) : 0;
  const refreshing =
    curZai.refreshing || curCursor.refreshing || curCodex.refreshing || curClaude.refreshing;

  // 展示派生：当前账号条目用 30s live quota 覆盖（用量实时变化，5 分钟一轮的
  // 多账号查询跟不上）。仅当 live 一路更新（时间戳较新）时才覆盖——切换账号后
  // 两路数据到达顺序不定，旧一路的额度可能是另一个账号的；live 查询失败
  // （quota=null 或时间戳较旧）时保留后端值，避免闪空或错配
  const accountQuotasMerged = useMemo(
    () =>
      accountQuotas.map((e) =>
        e.is_current && quota && quotaTsRef.current >= accountQuotasTsRef.current
          ? { ...e, quota }
          : e
      ),
    [accountQuotas, quota]
  );

  const value = useMemo<DataCacheValue>(
    () => ({
      preset,
      custom,
      trendBucket,
      deviceFilter,
      setPreset,
      setCustom,
      setDeviceFilter,
      stats: curZai.stats,
      cost: curZai.cost,
      trend: curZai.trend,
      error: curZai.error,
      codex: curCodex.snapshot,
      codexError: curCodex.error,
      claude: curClaude.snapshot,
      claudeError: curClaude.error,
      cursor: curCursor.snapshot,
      cursorError: curCursor.error,
      fxRate,
      quota,
      todayDelta,
      agentQuotaDeltas,
      quotaError,
      accountQuotas: accountQuotasMerged,
      refreshAccountQuotas,
      syncConfig,
      remoteDevices,
      syncEnabled,
      lastUpdate,
      refreshing,
      refresh,
      refreshQuota,
    }),
    [
      preset,
      custom,
      trendBucket,
      deviceFilter,
      curZai,
      curCodex,
      curClaude,
      curCursor,
      fxRate,
      quota,
      todayDelta,
      agentQuotaDeltas,
      quotaError,
      accountQuotasMerged,
      refreshAccountQuotas,
      syncConfig,
      remoteDevices,
      syncEnabled,
      lastUpdate,
      refreshing,
      refresh,
      refreshQuota,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** 读取全局数据缓存。必须在 <DataProvider> 内使用。 */
export function useDataCache(): DataCacheValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useDataCache must be used within a DataProvider");
  return v;
}

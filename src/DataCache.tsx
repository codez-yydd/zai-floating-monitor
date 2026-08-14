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
  CostResult,
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
  computeCost,
  fetchCursorUsage,
  fetchQuota,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getSyncConfig,
  getTodayDelta,
  listRemoteDevices,
  remoteUsage,
} from "./api";
import { resolveRange } from "./RangePicker";
import {
  computeRemoteCost,
  mergeCost,
  mergeStats,
  mergeTrend,
  remoteToStats,
  remoteTrendToLocal,
} from "./merge";
import { loadCache, saveCache } from "./cache";

/**
 * 全局数据缓存层（v2：按范围缓存 + 后台定时刷新 + 展示只读）。
 *
 * 核心思路：
 *  - 展示层「只读缓存」：切换时间范围 = 改当前 key → 直接读对应缓存条目，
 *    零请求、秒显。stats/cost/trend/cursor 全部取自当前 key 的缓存。
 *  - 后台独立定时任务：定期把各预设范围（today/1d/7d/30d）的数据刷新进缓存，
 *    与范围切换完全解耦——切回来时数据已在缓存里。
 *  - 按需补刷：切到无缓存/过期范围（如全新 custom、首次进入、数据老化、配置
 *    就绪）才触发一次加载，之后也进缓存。
 *  - 刷新期间不清空旧值（只设 refreshing），新数据到达后平滑替换 → 无闪烁。
 *  - pricing 未就绪时保留旧 cost，避免空价格表把花费覆盖成 0。
 *  - localStorage 持久化：进程重启后各范围仍秒显。
 */

// 需要后台持续刷新的预设范围（custom 由按需补刷负责，无法预缓存）
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
  return Object.keys(p.cny).length === 0 && Object.keys(p.usd).length === 0;
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

  // ===== Cursor 数据（当前范围，读缓存）=====
  cursor: CursorSnapshot | null;
  cursorError: string | null;
  /** USD→CNY 汇率（汇总页合并花费用） */
  fxRate: number;

  // ===== Quota 数据（与范围无关）=====
  quota: QuotaResult | null;
  todayDelta: [number, number] | null;
  quotaError: string | null;

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
  /** 手动强制刷新当前范围（z.ai + Cursor） */
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
  // ===== 查询参数（持久化，冷启动恢复上次视图，与上次缓存匹配）=====
  const [preset, setPreset] = useState<RangePreset>(
    () => loadCache<RangePreset>("zbar-preset") ?? "today"
  );
  const [custom, setCustom] = useState<{ from: string; to: string }>(() => {
    const saved = loadCache<{ from: string; to: string }>("zbar-custom");
    if (saved) return saved;
    const today = new Date().toISOString().slice(0, 10);
    const week = new Date(Date.now() - 6 * 86400000)
      .toISOString()
      .slice(0, 10);
    return { from: week, to: today };
  });
  const [deviceFilter, setDeviceFilter] = useState<string>(
    () => loadCache<string>("zbar-device") ?? "all"
  );

  const trendBucket = bucketOf(preset);

  // ===== 按范围缓存（持久化，key: zai=`${df}|${rangeKey}`，cursor=rangeKey）=====
  const [zaiCache, setZaiCache] = useState<Record<string, ZaiEntry>>(
    () => stripRefreshing(loadCache<Record<string, ZaiEntry>>("zbar-zai-cache") ?? {})
  );
  const [cursorCache, setCursorCache] = useState<Record<string, CursorEntry>>(
    () => stripRefreshing(loadCache<Record<string, CursorEntry>>("zbar-cursor-cache") ?? {})
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
  const [quotaError, setQuotaError] = useState<string | null>(null);

  // ===== 同步配置 =====
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // ===== 并发保护：同范围正在刷新则跳过，避免重复请求 =====
  const zaiInflight = useRef<Set<string>>(new Set());
  const cursorInflight = useRef<Set<string>>(new Set());
  const quotaReqId = useRef(0);

  // ===== refs：按需补刷 effect 读取最新缓存，避免闭包过期 / 反复触发 =====
  const zaiCacheRef = useRef(zaiCache);
  useEffect(() => {
    zaiCacheRef.current = zaiCache;
  }, [zaiCache]);
  const cursorCacheRef = useRef(cursorCache);
  useEffect(() => {
    cursorCacheRef.current = cursorCache;
  }, [cursorCache]);

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
          const opts =
            df === "all"
              ? { excludeDevice: syncConfig.device_id }
              : { devices: df };
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
          cost = computeRemoteCost(remote, pricing);
          trend = remoteTrendToLocal(remote, pricing, bucket);
        } else if (localStats && remote) {
          stats = mergeStats(localStats, remote);
          cost = mergeCost(localCost, remote, pricing);
          trend = mergeTrend(localTrend, remote, pricing, bucket);
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
    [syncConfig, syncEnabled, pricing]
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
      });
  }, []);

  // Quota 数据加载（与范围无关，每次调用会采样写入 quota_history）。
  const loadQuota = useCallback(() => {
    const reqId = ++quotaReqId.current;
    fetchQuota()
      .then((r) => {
        if (reqId !== quotaReqId.current) return;
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

  // ===== 后台定时刷新 z.ai：当前 deviceFilter 的所有预设范围（与 preset/custom 解耦）。
  //      每 30s 刷一遍 today/1d/7d/30d → 切换预设范围时缓存命中、秒显。
  //      依赖不含 preset/custom：预设范围的 resolveRange 忽略 custom，故无需进依赖。 =====
  useEffect(() => {
    const df = deviceFilter;
    const tick = () => {
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshZaiRange(df, `${df}|${p}`, f, t, bucketOf(p));
      }
    };
    // 延后首刷（500ms）：WebView 重载（首次打开 / 长期隐藏后被系统回收）后，
    // 首屏先靠 localStorage 缓存渲染；挂载即并发发起 12 个数据命令的话，
    // 响应陆续回来的主线程处理（IPC 解析 + 全量缓存写盘）会推迟首帧，造成白屏
    const first = setTimeout(tick, 500);
    const timer = setInterval(tick, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
    // 故意不把 custom 列入依赖：后台只刷预设范围，custom 由按需补刷负责
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceFilter, syncConfig, syncEnabled, pricing, refreshZaiRange]);

  // ===== 后台定时刷新 Cursor：所有预设范围，降频 180s（网络慢，4 范围并行）。
  //      账号级，不受 deviceFilter 影响。汇率每 tick 只读一次（与范围循环解耦）。 =====
  useEffect(() => {
    const tick = () => {
      // 每 tick 读一次汇率（用户可能在设置页改过），避免每个范围重复读
      getCursorConfig()
        .then((cfg) => setFxRate(cfg.usd_cny_rate))
        .catch(() => {});
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshCursorRange(p, f, t);
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

  // ===== 按需补刷：切到无缓存/过期范围，或配置就绪后补一次。
  //      预设范围通常已被后台任务刷新 → 命中新鲜缓存 → 不请求、秒显。
  //      依赖含 refreshZaiRange/refreshCursorRange：pricing/syncConfig 就绪后函数重建，
  //      本 effect 重跑 → custom 范围用就绪配置重刷自愈（修复冷启动覆盖问题）。 =====
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
    // 仅在范围/设备/刷新函数变化时触发；不依赖 cache 内容（靠 ref 读最新值）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preset, custom, deviceFilter, trendBucket, refreshZaiRange, refreshCursorRange]);

  // ===== 窗口恢复可见时主动补刷：隐藏期间 setInterval 常被节流，恢复后立即补齐
  //      当前 deviceFilter 的预设范围，保证"常驻新鲜"。 =====
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      const df = deviceFilter;
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshZaiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCursorRange(p, f, t);
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceFilter, refreshZaiRange, refreshCursorRange]);

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
    saveCache("zbar-fxrate", fxRate);
  }, [fxRate]);
  useEffect(() => {
    if (quota) saveCache("zbar-quota", quota);
  }, [quota]);
  useEffect(() => {
    if (todayDelta) saveCache("zbar-today-delta", todayDelta);
  }, [todayDelta]);

  // 手动刷新：刷新当前范围（z.ai + Cursor）一次
  const refresh = useCallback(() => {
    const [f, t] = resolveRange(preset, custom);
    const zKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
    refreshZaiRange(deviceFilter, zKey, f, t, trendBucket);
    refreshCursorRange(rangeKey(preset, custom), f, t);
  }, [preset, custom, deviceFilter, trendBucket, refreshZaiRange, refreshCursorRange]);

  const refreshQuota = useCallback(() => loadQuota(), [loadQuota]);

  // ===== 当前范围数据（展示层只读这些）=====
  const curZaiKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
  const curCursorKey = rangeKey(preset, custom);
  const curZai = zaiCache[curZaiKey] ?? EMPTY_ZAI;
  const curCursor = cursorCache[curCursorKey] ?? EMPTY_CURSOR;

  // lastUpdate 取当前范围各数据源里最旧的成功时间（保守的新鲜度下限）
  const updateTs = [curZai.ts, curCursor.ts].filter((t) => t > 0);
  const lastUpdate = updateTs.length ? Math.min(...updateTs) : 0;
  const refreshing = curZai.refreshing || curCursor.refreshing;

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
      cursor: curCursor.snapshot,
      cursorError: curCursor.error,
      fxRate,
      quota,
      todayDelta,
      quotaError,
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
      curCursor,
      fxRate,
      quota,
      todayDelta,
      quotaError,
      syncConfig,
      remoteDevices,
      syncEnabled,
      lastUpdate,
      refreshing,
      refresh,
      refreshQuota,
    ]
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** 读取全局数据缓存。必须在 <DataProvider> 内使用。 */
export function useDataCache(): DataCacheValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useDataCache 必须在 DataProvider 内使用");
  return v;
}

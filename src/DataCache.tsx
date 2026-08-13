import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";
import type {
  CostResult,
  Currency,
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

/**
 * 全局数据缓存层。
 *
 * 设计目标：把"数据获取"与"数据展示"彻底解耦。
 *  - Provider 挂在 view 切换之外，永不被卸载 → 切到设置再切回来数据瞬时恢复。
 *  - 应用启动即预加载，不等面板打开。
 *  - 刷新期间**不清空旧值**（只设 refreshing=true），新数据到达后平滑替换 → 无闪烁。
 *  - 面板/额度组件退化为纯展示层，直接读缓存瞬时渲染。
 */

export interface DataCacheValue {
  // ===== 查询参数（UI 可修改，变化时自动刷新对应数据）=====
  preset: RangePreset;
  custom: { from: string; to: string };
  /** 由 preset 派生：今日/24h 按小时，更长范围按日 */
  trendBucket: TrendBucket;
  deviceFilter: string;
  setPreset: (p: RangePreset) => void;
  setCustom: (c: { from: string; to: string }) => void;
  setDeviceFilter: (d: string) => void;

  // ===== z.ai 数据（旧值在刷新期间保留，不清空 → 无闪烁）=====
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];

  // ===== Cursor 数据 =====
  cursor: CursorSnapshot | null;
  cursorError: string | null;
  /** USD→CNY 汇率（汇总页合并花费用） */
  fxRate: number;

  // ===== Quota 数据 =====
  quota: QuotaResult | null;
  todayDelta: [number, number] | null;
  quotaError: string | null;

  // ===== 同步配置（启动时加载一次）=====
  syncConfig: SyncConfig | null;
  remoteDevices: DeviceInfo[];
  syncEnabled: boolean;

  // ===== 元信息 =====
  lastUpdate: number;
  /** 后台是否在拉取 z.ai 数据（仅用于刷新按钮转圈，不阻塞渲染） */
  refreshing: boolean;
  /** z.ai 错误信息（不阻塞已有数据展示） */
  error: string | null;
  /** 手动强制刷新 z.ai + Cursor */
  refresh: () => void;
  /** 手动强制刷新 Coding Plan 额度 */
  refreshQuota: () => void;
}

const Ctx = createContext<DataCacheValue | null>(null);

interface ProviderProps {
  /** 价格表（loadZai 合并远程花费时需要） */
  pricing: PricingConfig;
  /** 货币偏好（loadZai 合并远程花费时需要） */
  currency: Currency;
  children: ReactNode;
}

export function DataProvider({ pricing, currency, children }: ProviderProps) {
  // ===== 查询参数 =====
  const [preset, setPreset] = useState<RangePreset>("today");
  const [custom, setCustom] = useState(() => {
    const today = new Date().toISOString().slice(0, 10);
    const week = new Date(Date.now() - 6 * 86400000)
      .toISOString()
      .slice(0, 10);
    return { from: week, to: today };
  });
  const trendBucket: TrendBucket =
    preset === "today" || preset === "1d" ? "hour" : "day";
  const [deviceFilter, setDeviceFilter] = useState<string>("all");

  // ===== z.ai 数据 =====
  const [stats, setStats] = useState<Stats | null>(null);
  const [cost, setCost] = useState<CostResult | null>(null);
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<number>(0);
  const [refreshing, setRefreshing] = useState(false);

  // ===== Cursor 数据 =====
  const [cursor, setCursor] = useState<CursorSnapshot | null>(null);
  const [cursorError, setCursorError] = useState<string | null>(null);
  const [fxRate, setFxRate] = useState(7.2);

  // ===== Quota 数据 =====
  const [quota, setQuota] = useState<QuotaResult | null>(null);
  const [todayDelta, setTodayDelta] = useState<[number, number] | null>(null);
  const [quotaError, setQuotaError] = useState<string | null>(null);

  // ===== 同步配置 =====
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // ===== 竞态保护：快速切换时间范围时丢弃过期的旧响应 =====
  const zaiReqId = useRef(0);
  const cursorReqId = useRef(0);
  const quotaReqId = useRef(0);

  // 初次加载：同步配置 + 设备列表 + Cursor 汇率（仅一次）
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
    getCursorConfig()
      .then((cfg) => setFxRate(cfg.usd_cny_rate))
      .catch(() => {});
  }, []);

  // z.ai 数据加载（本地 SQLite + 远端同步合并）。
  // 刷新期间不清空旧值，只设 refreshing —— 面板始终展示上次成功的结果。
  const loadZai = useCallback(async () => {
    const reqId = ++zaiReqId.current;
    setRefreshing(true);
    const [from, to] = resolveRange(preset, custom);

    try {
      // 根据设备筛选决定数据来源
      let localStats: Stats | null = null;
      let localCost: CostResult | null = null;
      let localTrend: TrendPoint[] = [];
      let remote: RemoteUsage | null = null;

      const wantLocal = deviceFilter === "all" || deviceFilter === "local";
      // all 或指定远端设备时拉远程数据；选 local 时不拉
      const wantRemote = syncEnabled && deviceFilter !== "local";

      const tasks: Promise<unknown>[] = [];
      if (wantLocal) {
        tasks.push(
          fetchStats(from, to).then((s) => (localStats = s)),
          computeCost(from, to).then((c) => (localCost = c)),
          fetchTrend(from, to, trendBucket).then((t) => (localTrend = t))
        );
      }
      if (wantRemote && syncConfig) {
        const opts =
          deviceFilter === "all"
            ? { excludeDevice: syncConfig.device_id }
            : { devices: deviceFilter };
        const isSpecificRemote = deviceFilter !== "all";
        tasks.push(
          remoteUsage(from, to, trendBucket, opts)
            .then((r) => (remote = r))
            .catch((e) => {
              if (isSpecificRemote) throw e;
            })
        );
      }

      await Promise.all(tasks);
      if (reqId !== zaiReqId.current) return; // 过期请求，丢弃

      // 合并本地 + 远端
      if (deviceFilter === "local") {
        setStats(localStats);
        setCost(localCost);
        setTrend(localTrend);
      } else if (remote && !localStats) {
        setStats(remoteToStats(remote));
        setCost(computeRemoteCost(remote, pricing, currency));
        setTrend(remoteTrendToLocal(remote, pricing, trendBucket));
      } else if (localStats && remote) {
        setStats(mergeStats(localStats, remote));
        setCost(mergeCost(localCost, remote, pricing, currency));
        setTrend(mergeTrend(localTrend, remote, pricing, trendBucket));
      } else {
        setStats(localStats);
        setCost(localCost);
        setTrend(localTrend);
      }
      setError(null);
      setLastUpdate(Date.now());
    } catch (e) {
      if (reqId !== zaiReqId.current) return;
      setError(String(e));
    } finally {
      if (reqId !== zaiReqId.current) return;
      setRefreshing(false);
    }
  }, [
    preset,
    custom,
    trendBucket,
    deviceFilter,
    syncConfig,
    syncEnabled,
    pricing,
    currency,
  ]);

  // Cursor 数据加载（账号级别，不受设备筛选影响）。刷新期间不清空旧值。
  const loadCursor = useCallback(() => {
    const [from, to] = resolveRange(preset, custom);
    const reqId = ++cursorReqId.current;
    fetchCursorUsage(from, to)
      .then((data) => {
        if (reqId !== cursorReqId.current) return;
        setCursor(data);
        setCursorError(null);
        setLastUpdate(Date.now());
      })
      .catch((e) => {
        if (reqId !== cursorReqId.current) return;
        setCursorError(String(e));
      });
    // 顺带刷新 USD→CNY 汇率：用户可能在设置页改过，这里同步更新缓存。
    // getCursorConfig 是本地配置读取（非 API），开销可忽略。
    getCursorConfig()
      .then((cfg) => setFxRate(cfg.usd_cny_rate))
      .catch(() => {});
  }, [preset, custom]);

  // Quota 数据加载（每次调用会采样写入 quota_history）。
  const loadQuota = useCallback(() => {
    const reqId = ++quotaReqId.current;
    fetchQuota()
      .then((r) => {
        if (reqId !== quotaReqId.current) return;
        setQuota(r);
        setQuotaError(null);
        // 额度刷新成功后顺带读今日增量（快照由 fetch_quota 采样写入）
        getTodayDelta()
          .then(setTodayDelta)
          .catch(() => {});
      })
      .catch((e) => {
        if (reqId !== quotaReqId.current) return;
        setQuotaError(String(e));
      });
  }, []);

  // 预加载：Provider 挂载立即拉取，不等面板打开
  useEffect(() => {
    loadZai();
  }, [loadZai]);

  useEffect(() => {
    loadCursor();
  }, [loadCursor]);

  useEffect(() => {
    loadQuota();
  }, [loadQuota]);

  // 定时刷新：z.ai 30s、Cursor 2min、quota 30s
  useEffect(() => {
    const timer = setInterval(loadZai, 30_000);
    return () => clearInterval(timer);
  }, [loadZai]);

  useEffect(() => {
    const timer = setInterval(loadCursor, 120_000);
    return () => clearInterval(timer);
  }, [loadCursor]);

  useEffect(() => {
    const timer = setInterval(loadQuota, 30_000);
    return () => clearInterval(timer);
  }, [loadQuota]);

  // 手动刷新：同时触发 z.ai + Cursor
  const refresh = useCallback(() => {
    loadZai();
    loadCursor();
  }, [loadZai, loadCursor]);

  // 手动刷新额度
  const refreshQuota = useCallback(() => {
    loadQuota();
  }, [loadQuota]);

  const value: DataCacheValue = {
    preset,
    custom,
    trendBucket,
    deviceFilter,
    setPreset,
    setCustom,
    setDeviceFilter,
    stats,
    cost,
    trend,
    cursor,
    cursorError,
    fxRate,
    quota,
    todayDelta,
    quotaError,
    syncConfig,
    remoteDevices,
    syncEnabled,
    lastUpdate,
    refreshing,
    error,
    refresh,
    refreshQuota,
  };

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** 读取全局数据缓存。必须在 <DataProvider> 内使用。 */
export function useDataCache(): DataCacheValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useDataCache 必须在 DataProvider 内使用");
  return v;
}

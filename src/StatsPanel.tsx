import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  CostResult,
  Currency,
  CursorSnapshot,
  DeviceInfo,
  PricingConfig,
  RangePreset,
  RemoteUsage,
  Stats,
  StatsTab,
  SyncConfig,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  computeCost,
  fetchCursorUsage,
  fetchPin,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getSyncConfig,
  listRemoteDevices,
  remoteUsage,
  setPin,
} from "./api";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker, resolveRange } from "./RangePicker";
import { ZaiStatsContent } from "./ZaiStatsContent";
import { CursorPanel } from "./CursorPanel";
import { SummaryTab } from "./SummaryTab";
import {
  computeRemoteCost,
  mergeCost,
  mergeStats,
  mergeTrend,
  remoteToStats,
  remoteTrendToLocal,
} from "./merge";

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  onGoPricing: () => void;
  onGoSync: () => void;
  onGoCompare: () => void;
  onGoReport: () => void;
}

export function StatsPanel({ currency, pricing, onGoPricing, onGoSync, onGoCompare, onGoReport }: Props) {
  const [preset, setPreset] = useState<RangePreset>("today");
  const [custom, setCustom] = useState(() => {
    const today = new Date().toISOString().slice(0, 10);
    const week = new Date(Date.now() - 6 * 86400000).toISOString().slice(0, 10);
    return { from: week, to: today };
  });

  // 三标签：汇总 | z.ai | Cursor
  const [tab, setTab] = useState<StatsTab>(
    () => (localStorage.getItem("zbar-tab") as StatsTab) || "summary"
  );

  const [stats, setStats] = useState<Stats | null>(null);
  const [cost, setCost] = useState<CostResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<number>(0);
  // 趋势图：分桶数据（小时/日，跟随所选时间范围）
  const [trend, setTrend] = useState<TrendPoint[]>([]);

  // ===== Cursor 用量相关状态 =====
  const [cursor, setCursor] = useState<CursorSnapshot | null>(null);
  const [cursorError, setCursorError] = useState<string | null>(null);
  const [cursorLoading, setCursorLoading] = useState(false);
  // USD→CNY 汇率（汇总页合并花费用）
  const [fxRate, setFxRate] = useState(7.2);

  // ===== 多设备同步相关状态 =====
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState<string>("all");

  // ===== 窗口置顶常驻（仅 Windows）=====
  const isWindows =
    typeof navigator !== "undefined" &&
    /windows/i.test(navigator.userAgent);
  const [pinned, setPinned] = useState(false);

  // Cursor 请求竞态保护：快速切换时间范围时，丢弃过期的 fire-and-forget 响应，
  // 防止旧请求后到覆盖新数据（events API 较慢，容易触发）。
  const cursorReqId = useRef(0);

  useEffect(() => {
    if (!isWindows) return;
    fetchPin()
      .then(setPinned)
      .catch(() => {});
  }, [isWindows]);

  // 记忆当前标签
  useEffect(() => {
    localStorage.setItem("zbar-tab", tab);
  }, [tab]);

  // preset → 桶粒度：今日/24h 按小时，更长范围按日
  const trendBucket: TrendBucket =
    preset === "today" || preset === "1d" ? "hour" : "day";

  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 初次加载时读取同步配置 + 设备列表 + Cursor 配置
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

  // z.ai 数据加载（本地 SQLite + 远端同步），每 30 秒刷新
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    const [from, to] = resolveRange(preset, custom);

    try {
      // 根据设备筛选决定数据来源：
      let localStats: Stats | null = null;
      let localCost: CostResult | null = null;
      let localTrend: TrendPoint[] = [];
      let remote: RemoteUsage | null = null;

      const wantLocal = deviceFilter === "all" || deviceFilter === "local";
      const wantRemote =
        syncEnabled &&
        (deviceFilter === "all" ||
          (deviceFilter !== "local" && deviceFilter !== "all"));

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

      // 合并
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
      setLastUpdate(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [preset, custom, trendBucket, deviceFilter, syncConfig, syncEnabled, pricing, currency]);

  // Cursor 数据独立加载（账号级别，不受设备筛选影响）。
  // 用递增 reqId 做竞态保护：快速切换时间范围时丢弃过期的旧响应。
  const loadCursor = useCallback(() => {
    const [from, to] = resolveRange(preset, custom);
    const reqId = ++cursorReqId.current;
    setCursorLoading(true);
    setCursorError(null);
    fetchCursorUsage(from, to)
      .then((data) => {
        if (reqId !== cursorReqId.current) return;
        setCursor(data);
        setLastUpdate(Date.now());
      })
      .catch((e) => {
        if (reqId !== cursorReqId.current) return;
        setCursor(null);
        setCursorError(String(e));
      })
      .finally(() => {
        if (reqId !== cursorReqId.current) return;
        setCursorLoading(false);
      });
  }, [preset, custom]);

  // z.ai：挂载 + 时间范围/设备变化时加载 + 每 30 秒刷新
  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [load]);

  // Cursor：挂载 + 时间范围变化时加载 + 每 2 分钟刷新
  // 数据常驻 React state，切换标签即时展示，无需重新加载
  useEffect(() => {
    loadCursor();
  }, [loadCursor]);

  useEffect(() => {
    const timer = setInterval(loadCursor, 120_000);
    return () => clearInterval(timer);
  }, [loadCursor]);

  // 手动刷新：同时触发 z.ai + Cursor
  const refreshAll = useCallback(() => {
    load();
    loadCursor();
  }, [load, loadCursor]);

  return (
    <div className="flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3.5 pt-3 pb-2.5 border-b border-slate-900/10">
        {/* Windows 无边框窗口拖动 */}
        <div
          className={`flex items-center justify-between mb-2.5 ${
            isWindows ? "cursor-default" : ""
          }`}
          onMouseDown={
            isWindows
              ? (e) => {
                  if (!(e.target as HTMLElement).closest("button")) {
                    getCurrentWindow().startDragging();
                  }
                }
              : undefined
          }
        >
          <h1 className="text-[13px] font-semibold text-slate-900/90 select-none">
            ZCode Token
          </h1>
          <div className="flex items-center gap-2.5">
            <button
              onClick={onGoCompare}
              className="text-xs text-slate-700/40 hover:text-slate-900/70 transition-colors"
              title="周额度对比"
            >
              📊
            </button>
            <button
              onClick={onGoReport}
              className="text-xs text-slate-700/40 hover:text-slate-900/70 transition-colors"
              title="用量报告"
            >
              📄
            </button>
            <button
              onClick={onGoSync}
              className={`text-xs transition-colors ${
                syncEnabled
                  ? "text-emerald-600 hover:text-emerald-700"
                  : "text-slate-700/40 hover:text-slate-900/70"
              }`}
              title={syncEnabled ? "设备同步" : "配置设备同步"}
            >
              ⇅
            </button>
            {isWindows && (
              <button
                onClick={() => {
                  const next = !pinned;
                  setPinned(next);
                  setPin(next).catch(() => setPinned(!next));
                }}
                className={`text-xs transition-colors ${
                  pinned
                    ? "text-sky-600 hover:text-sky-700"
                    : "text-slate-700/40 hover:text-slate-900/70"
                }`}
                title={pinned ? "取消常驻" : "常驻置顶"}
              >
                📌
              </button>
            )}
            <button
              onClick={refreshAll}
              disabled={loading}
              className="text-slate-700/50 hover:text-slate-900/80 text-xs transition-colors"
              title="刷新"
            >
              ↻
            </button>
          </div>
        </div>
        <RangePicker
          preset={preset}
          custom={custom}
          onChange={(p, c) => {
            setPreset(p);
            setCustom(c);
          }}
        />
        {/* 设备筛选器：仅在同步启用时显示 */}
        {syncEnabled && (
          <div className="mt-2 flex items-center gap-1.5">
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
        {/* 三标签栏：汇总 | Z.ai | Cursor */}
        <div className="mt-2 flex gap-0.5 p-0.5 rounded-lg bg-slate-900/5">
          {(["summary", "zai", "cursor"] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`flex-1 py-1 rounded-md text-[10px] font-medium transition-colors ${
                tab === t
                  ? t === "cursor"
                    ? "bg-white text-violet-700 shadow-sm"
                    : t === "zai"
                      ? "bg-white text-sky-700 shadow-sm"
                      : "bg-white text-slate-900 shadow-sm"
                  : "text-slate-700/50 hover:text-slate-900/70"
              }`}
            >
              {t === "summary" ? "汇总" : t === "zai" ? "Z.ai" : "Cursor"}
            </button>
          ))}
        </div>
      </div>

      {/* Coding Plan 额度监控
          始终挂载（非条件卸载），保证 QuotaPanel 内部的 30s 轮询持续运行、
          额度快照持续写入 quota_history —— 即使停留在「汇总」或「Cursor」标签也不中断。
          非 z.ai 标签时用 CSS 隐藏而非卸载。 */}
      <div style={{ display: tab === "zai" ? "block" : "none" }}>
        <QuotaPanel onGoSettings={onGoPricing} />
      </div>

      {/* 标签内容 */}
      {tab === "zai" ? (
        <>
          {error && (
            <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
              {error}
            </div>
          )}
          <ZaiStatsContent
            stats={stats}
            cost={cost}
            trend={trend}
            pricing={pricing}
            currency={currency}
            trendBucket={trendBucket}
          />
        </>
      ) : tab === "cursor" ? (
        <CursorPanel
          snapshot={cursor}
          loading={cursorLoading}
          error={cursorError}
          currency={currency}
          fxRate={fxRate}
        />
      ) : (
        <SummaryTab
          stats={stats}
          cost={cost}
          trend={trend}
          cursor={cursor}
          currency={currency}
          bucket={trendBucket}
          fxRate={fxRate}
        />
      )}

      {/* 底部 */}
      <div className="px-3.5 py-2 border-t border-slate-900/10 flex items-center justify-between text-[10px] text-slate-700/50">
        <span className="num">
          {lastUpdate
            ? new Date(lastUpdate).toLocaleTimeString("zh-CN", {
                hour: "2-digit",
                minute: "2-digit",
              })
            : ""}
        </span>
        <button
          onClick={onGoPricing}
          className="hover:text-sky-600 transition-colors"
        >
          ⚙ 价格设置
        </button>
      </div>
    </div>
  );
}

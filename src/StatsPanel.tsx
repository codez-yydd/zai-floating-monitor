import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  CostResult,
  Currency,
  DeviceInfo,
  ModelStat,
  OverallStat,
  PricingConfig,
  RangePreset,
  RemoteUsage,
  Stats,
  SyncConfig,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  computeCost,
  fetchPin,
  fetchStats,
  fetchTrend,
  getSyncConfig,
  listRemoteDevices,
  remoteUsage,
  setPin,
} from "./api";
import { formatCost, formatPct, formatTokens } from "./format";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker, resolveRange } from "./RangePicker";

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  onGoPricing: () => void;
  onGoSync: () => void;
  onGoCompare: () => void;
}

export function StatsPanel({ currency, pricing, onGoPricing, onGoSync, onGoCompare }: Props) {
  const [preset, setPreset] = useState<RangePreset>("today");
  const [custom, setCustom] = useState(() => {
    const today = new Date().toISOString().slice(0, 10);
    const week = new Date(Date.now() - 6 * 86400000).toISOString().slice(0, 10);
    return { from: week, to: today };
  });

  const [stats, setStats] = useState<Stats | null>(null);
  const [cost, setCost] = useState<CostResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<number>(0);
  // 趋势图：分桶数据（小时/日，跟随所选时间范围）
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  // 趋势图度量切换：花费 / Token
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");

  // ===== 多设备同步相关状态 =====
  // syncConfig：判断是否启用同步（null=未读取/未启用）
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  // 远端设备列表（供筛选器）
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  // 设备筛选："all" 汇总 | "local" 仅本机 | 具体 device_id 仅远端该设备
  const [deviceFilter, setDeviceFilter] = useState<string>("all");

  // ===== 窗口置顶常驻（仅 Windows）=====
  // 平台判断：仅 Windows 显示置顶开关，macOS 完全不渲染该功能
  const isWindows =
    typeof navigator !== "undefined" &&
    /windows/i.test(navigator.userAgent);
  // pinned：是否已开启常驻置顶（失焦不隐藏 + 始终置顶）
  const [pinned, setPinned] = useState(false);

  // 初始化时读取持久化的 pin 状态，恢复上次开关状态
  useEffect(() => {
    if (!isWindows) return;
    fetchPin()
      .then(setPinned)
      .catch(() => {});
  }, [isWindows]);

  // preset → 桶粒度：今日/24h 按小时，更长范围按日
  const trendBucket: TrendBucket =
    preset === "today" || preset === "1d" ? "hour" : "day";

  // 同步是否启用：有 device_token 才算
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 初次加载时读取同步配置 + 设备列表（只读一次，不随时间范围刷新）
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
    setLoading(true);
    setError(null);
    const [from, to] = resolveRange(preset, custom);
    try {
      // 根据设备筛选决定数据来源：
      // - "all"：本地 + 远端(排除本机) 合并
      // - "local"：仅本地
      // - 具体 id：仅远端该设备
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
        // 远端请求失败时静默降级（服务器不可达不影响本地展示）。
        // 仅当选了具体远端设备且请求失败时，才把错误透出给用户。
        const isSpecificRemote = deviceFilter !== "all";
        tasks.push(
          remoteUsage(from, to, trendBucket, opts)
            .then((r) => (remote = r))
            .catch((e) => {
              if (isSpecificRemote) throw e;
              // "全部"模式：远端失败静默，仅用本地数据
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
        // 仅远端：远端无 cost，需用 pricing 自算（复用本地逻辑）
        setStats(remoteToStats(remote));
        setCost(computeRemoteCost(remote, pricing, currency));
        setTrend(remoteTrendToLocal(remote, pricing, trendBucket));
      } else if (localStats && remote) {
        // 合并：本地 + 远端
        setStats(mergeStats(localStats, remote));
        setCost(mergeCost(localCost, remote, pricing, currency));
        setTrend(mergeTrend(localTrend, remote, pricing, trendBucket));
      } else {
        // 仅本地（未启用同步时）
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

  useEffect(() => {
    load();
  }, [load]);

  // 自动刷新：30 秒
  useEffect(() => {
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [load]);

  const totalCost =
    currency === "cny" ? cost?.total_cny ?? 0 : cost?.total_usd ?? 0;
  const perModelCost =
    currency === "cny" ? cost?.per_model_cny : cost?.per_model_usd;

  const cacheRate =
    stats && stats.overall.input_tokens > 0
      ? stats.overall.cache_read_tokens / stats.overall.input_tokens
      : 0;

  return (
    <div className="flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3.5 pt-3 pb-2.5 border-b border-slate-900/10">
        {/* Windows 无边框窗口拖动：整行都可作为手柄。
            用 JS API startDragging 而非 data-tauri-drag-region——后者会让容器内
            按钮因 closest() 命中而失效。这里在 mousedown 时判断是否点在按钮上，
            只有非按钮区域才触发原生拖动，按钮点击不受影响。macOS 的 popover
            固定在菜单栏下方，不需要拖动。 */}
        <div
          className={`flex items-center justify-between mb-2.5 ${
            isWindows ? "cursor-default" : ""
          }`}
          onMouseDown={
            isWindows
              ? (e) => {
                  // 点在按钮（或其子元素）上时不拖动，交给按钮 onClick 处理
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
              onClick={onGoSync}
              className={`text-xs transition-colors ${
                syncEnabled
                  ? "text-emerald-600 hover:text-emerald-700"
                  : "text-slate-700/40 hover:text-slate-900/70"
              }`}
              title={syncEnabled ? "设备同步" : "配置设备同步"}
            >
              {syncEnabled ? "⇅" : "⇅"}
            </button>
            {/* Windows 专属：置顶常驻开关。开启后面板失焦不隐藏、始终置顶 */}
            {isWindows && (
              <button
                onClick={() => {
                  const next = !pinned;
                  setPinned(next);
                  // 先乐观更新 UI，失败则回滚状态
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
              onClick={load}
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
      </div>

      {/* Coding Plan 额度监控 */}
      <QuotaPanel onGoSettings={onGoPricing} />

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      {stats && (
        <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3">
          {/* 总览：花费为主，token 次之 */}
          <div className="flex items-end justify-between">
            <div>
              <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
                总花费
              </div>
              <div className="num text-[26px] font-bold text-slate-900 leading-none mt-0.5">
                {formatCost(totalCost, currency)}
              </div>
            </div>
            <div className="text-right">
              <div className="text-[10px] uppercase tracking-wide text-slate-700/55">
                总 Token
              </div>
              <div className="num text-[15px] font-semibold text-slate-900/70 leading-none mt-1">
                {formatTokens(stats.overall.total_tokens)}
              </div>
            </div>
          </div>

          {/* 趋势图（粒度跟随所选时间范围） */}
          <TrendChart
            points={trend}
            bucket={trendBucket}
            currency={currency}
            metric={trendMetric}
            onMetricChange={setTrendMetric}
          />

          {/* 三个指标 */}
          <div className="grid grid-cols-3 gap-1.5">
            <Metric label="请求" value={String(stats.overall.requests)} />
            <Metric
              label="缓存率"
              value={formatPct(cacheRate)}
              accent="text-emerald-600"
            />
            <Metric
              label="输出"
              value={formatTokens(stats.overall.output_tokens)}
            />
          </div>

          {/* 明细条 */}
          <div className="space-y-1.5 pt-1">
            <DetailRow
              label="输入"
              value={formatTokens(stats.overall.input_tokens)}
              pct={
                stats.overall.total_tokens > 0
                  ? stats.overall.input_tokens / stats.overall.total_tokens
                  : 0
              }
              color="bg-sky-400"
            />
            <DetailRow
              label="缓存"
              value={formatTokens(stats.overall.cache_read_tokens)}
              pct={cacheRate}
              color="bg-emerald-400"
            />
            <DetailRow
              label="输出"
              value={formatTokens(stats.overall.output_tokens)}
              pct={
                stats.overall.total_tokens > 0
                  ? stats.overall.output_tokens / stats.overall.total_tokens
                  : 0
              }
              color="bg-violet-400"
            />
            {stats.overall.reasoning_tokens > 0 && (
              <DetailRow
                label="推理"
                value={formatTokens(stats.overall.reasoning_tokens)}
                pct={
                  stats.overall.total_tokens > 0
                    ? stats.overall.reasoning_tokens /
                      stats.overall.total_tokens
                    : 0
                }
                color="bg-amber-400"
              />
            )}
          </div>

          {/* 按模型分组 */}
          <div>
            <div className="text-[10px] uppercase tracking-wide text-slate-700/55 mb-1.5 mt-1">
              按模型
            </div>
            <div className="space-y-0.5">
              {stats.by_model.map((m) => {
                const mc = perModelCost?.find(
                  (x) => x.model_id === m.model_id
                );
                const hasPrice = Boolean(
                  pricing[currency][m.model_id] &&
                    (pricing[currency][m.model_id].input > 0 ||
                      pricing[currency][m.model_id].output > 0)
                );
                return (
                  <div
                    key={m.provider_id + m.model_id}
                    className="flex items-center justify-between text-xs py-1.5 px-2 -mx-2 rounded-lg hover:bg-slate-900/5 transition-colors"
                  >
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span className="font-medium text-slate-900/90 truncate">
                        {m.model_id}
                      </span>
                      {!hasPrice && (
                        <span
                          className="text-[10px] text-amber-600/90"
                          title="未配置价格"
                        >
                          ⚠
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-2 text-slate-700/60 num shrink-0">
                      <span>{m.requests}</span>
                      <span className="text-slate-700/25">·</span>
                      <span>{formatTokens(m.total_tokens)}</span>
                      <span
                        className={`w-12 text-right ${
                          hasPrice
                            ? "text-slate-900/90"
                            : "text-slate-700/35"
                        }`}
                      >
                        {hasPrice ? formatCost(mc?.cost ?? 0, currency) : "—"}
                      </span>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
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

function Metric({
  label,
  value,
  accent,
}: {
  label: string;
  value: string;
  accent?: string;
}) {
  return (
    <div className="rounded-lg bg-white/25 border border-white/30 py-2 text-center">
      <div className="text-[10px] text-slate-700/55">{label}</div>
      <div
        className={`num text-[13px] font-semibold mt-0.5 ${
          accent || "text-slate-900/80"
        }`}
      >
        {value}
      </div>
    </div>
  );
}

function DetailRow({
  label,
  value,
  pct,
  color,
}: {
  label: string;
  value: string;
  pct: number;
  color: string;
}) {
  return (
    <div className="flex items-center gap-2 text-[11px]">
      <span className="text-slate-700/60 w-14 shrink-0">{label}</span>
      <div className="flex-1 h-1 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className={`h-full rounded-full ${color} opacity-70`}
          style={{ width: `${Math.min(pct * 100, 100)}%` }}
        />
      </div>
      <span className="num text-slate-900/85 font-medium w-14 text-right">
        {value}
      </span>
    </div>
  );
}

/** 趋势图：迷你柱状图 + 最新桶环比 + 花费/Token 切换。
 *  粒度跟随所选时间范围：今日/24h 按小时，更长范围按日。 */
function TrendChart({
  points,
  bucket,
  currency,
  metric,
  onMetricChange,
}: {
  points: TrendPoint[];
  bucket: TrendBucket;
  currency: Currency;
  metric: "cost" | "token";
  onMetricChange: (m: "cost" | "token") => void;
}) {
  // 取每根柱子的数值
  const values = points.map((d) =>
    metric === "cost"
      ? currency === "cny"
        ? d.cost_cny
        : d.cost_usd
      : d.total_tokens
  );
  const maxValue = Math.max(...values, 1); // 至少为 1，避免除 0

  // 最新桶 vs 上一桶 环比（始终按花费比较，更直观）
  const last = points[points.length - 1];
  const prev = points[points.length - 2];
  const lastCost = last
    ? currency === "cny"
      ? last.cost_cny
      : last.cost_usd
    : 0;
  const prevCost = prev
    ? currency === "cny"
      ? prev.cost_cny
      : prev.cost_usd
    : 0;
  let deltaText: string | null = null;
  let deltaUp = false;
  if (last && prev) {
    if (prevCost > 0) {
      const pct = ((lastCost - prevCost) / prevCost) * 100;
      if (Math.abs(pct) < 0.5) {
        deltaText = "持平";
      } else {
        deltaUp = pct > 0;
        deltaText = `${deltaUp ? "↑" : "↓"}${Math.abs(pct).toFixed(0)}%`;
      }
    } else if (lastCost > 0) {
      deltaText = "新增";
    }
  }

  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const isHour = bucket === "hour";
  const n = points.length;
  // 间距：柱子越多越收紧。日桶 30 天/大跨度可达 30+ 根。
  const barGap = n > 20 ? "gap-px" : n > 10 ? "gap-0.5" : "gap-1";
  // 标签步长：目标约显示 6~8 个标签，避免文字重叠。
  // n<=8 全显示；否则向上取整到合适的步长。
  const labelStep =
    n <= 8 ? 1 : Math.max(2, Math.ceil(n / 7));

  return (
    <div className="rounded-lg bg-white/25 border border-white/30 px-2.5 py-2">
      {/* 标题行 */}
      <div className="flex items-center justify-between mb-1.5">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
            趋势
          </span>
          {deltaText && (
            <span
              className={`text-[10px] num ${
                deltaText === "持平" || deltaText === "新增"
                  ? "text-slate-700/50"
                  : deltaUp
                    ? "text-rose-600/90"
                    : "text-emerald-600/90"
              }`}
              title={`最新${isHour ? "小时" : "日"} vs 上一${isHour ? "小时" : "日"}`}
            >
              {deltaText}
            </span>
          )}
        </div>
        {/* 花费/Token 切换 */}
        <div className="flex gap-0.5">
          {(["cost", "token"] as const).map((m) => (
            <button
              key={m}
              onClick={() => onMetricChange(m)}
              className={`px-1.5 rounded text-[9px] transition-colors ${
                metric === m
                  ? "bg-sky-500/80 text-white"
                  : "text-slate-700/45 hover:text-slate-900/70"
              }`}
            >
              {m === "cost" ? "花费" : "Token"}
            </button>
          ))}
        </div>
      </div>

      {/* 柱状图 */}
      <div className={`flex items-end ${barGap} h-12 relative`}>
        {points.map((d, i) => {
          const v = values[i];
          const h = maxValue > 0 ? (v / maxValue) * 100 : 0;
          const isLast = i === points.length - 1;
          const isHover = hoverIdx === i;
          return (
            <div
              key={d.label}
              className="flex-1 h-full flex items-end justify-center relative min-w-0"
              onMouseEnter={() => setHoverIdx(i)}
              onMouseLeave={() => setHoverIdx(null)}
            >
              {/* tooltip */}
              {isHover && (
                <div className="absolute bottom-full mb-1 left-1/2 -translate-x-1/2 z-10 whitespace-nowrap rounded-md bg-slate-900/85 text-white px-1.5 py-1 text-[9px] leading-tight pointer-events-none">
                  <div className="num">{d.label}</div>
                  <div className="num">
                    {formatCost(
                      currency === "cny" ? d.cost_cny : d.cost_usd,
                      currency
                    )}
                  </div>
                  <div className="num opacity-70">
                    {formatTokens(d.total_tokens)}
                  </div>
                </div>
              )}
              <div
                className={`w-full rounded-t-sm transition-all duration-300 ${
                  isLast
                    ? "bg-sky-500/80"
                    : isHover
                      ? "bg-slate-700/70"
                      : "bg-slate-700/35"
                }`}
                style={{
                  height: `${Math.max(h, v > 0 ? 4 : 0)}%`,
                  // 柱子少时限制最大宽度，避免单根过粗
                  maxWidth: n <= 7 ? "14px" : undefined,
                }}
              />
            </div>
          );
        })}
      </div>

      {/* 标签：柱子多时隔几个显示一个，避免文字重叠 */}
      <div className={`flex ${barGap} mt-1`}>
        {points.map((d, i) => {
          // 按 labelStep 隔行；最后一个总是显示
          const showLabel = i === points.length - 1 || i % labelStep === 0;
          const isLast = i === points.length - 1;
          return (
            <span
              key={d.label}
              className={`flex-1 text-center text-[8px] num min-w-0 ${
                isLast
                  ? "text-sky-600/80 font-medium"
                  : "text-slate-700/40"
              } ${showLabel ? "" : "opacity-0"}`}
            >
              {d.label}
            </span>
          );
        })}
      </div>
    </div>
  );
}

// ===== 本地 + 远端数据合并辅助函数 =====

/** 复刻 Rust cost_for：单模型花费（每百万 token）。
 * input_tokens 已含 cache_read，缓存读部分按缓存价，剩余非缓存输入按输入价。 */
function modelCost(
  modelId: string,
  inputTokens: number,
  outputTokens: number,
  cacheReadTokens: number,
  pricing: PricingConfig,
  currency: Currency
): number {
  const map = pricing[currency];
  const p = map[modelId];
  if (!p) return 0;
  const nonCacheInput = Math.max(0, inputTokens - cacheReadTokens);
  return (
    (nonCacheInput * p.input +
      outputTokens * p.output +
      cacheReadTokens * p.cache_read) /
    1_000_000
  );
}

/** 把远端 RemoteUsage 转成 Stats 结构（仅远端时用） */
function remoteToStats(r: RemoteUsage): Stats {
  return {
    from_ms: r.from_ms,
    to_ms: r.to_ms,
    overall: {
      requests: r.overall.requests,
      input_tokens: r.overall.input_tokens,
      output_tokens: r.overall.output_tokens,
      cache_read_tokens: r.overall.cache_read_tokens,
      cache_write_tokens: r.overall.cache_write_tokens,
      reasoning_tokens: r.overall.reasoning_tokens,
      total_tokens: r.overall.total_tokens,
    },
    by_model: r.by_model.map((m) => ({
      model_id: m.model_id,
      provider_id: m.provider_id,
      requests: m.requests,
      input_tokens: m.input_tokens,
      output_tokens: m.output_tokens,
      cache_read_tokens: m.cache_read_tokens,
      cache_write_tokens: m.cache_write_tokens,
      reasoning_tokens: m.reasoning_tokens,
      total_tokens: m.total_tokens,
    })),
    earliest_ms: null,
    latest_ms: null,
  };
}

/** 仅远端时算花费（远端不含 cost，前端用 pricing 自算） */
function computeRemoteCost(
  r: RemoteUsage,
  pricing: PricingConfig,
  _currency: Currency
): CostResult {
  const perModel = (currency: Currency) =>
    r.by_model.map((m) => ({
      model_id: m.model_id,
      cost: modelCost(
        m.model_id,
        m.input_tokens,
        m.output_tokens,
        m.cache_read_tokens,
        pricing,
        currency
      ),
    }));
  const cny = perModel("cny");
  const usd = perModel("usd");
  return {
    total_cny: cny.reduce((s, x) => s + x.cost, 0),
    total_usd: usd.reduce((s, x) => s + x.cost, 0),
    per_model_cny: cny,
    per_model_usd: usd,
  };
}

/** 合并本地 stats + 远端 usage → 汇总 stats */
function mergeStats(local: Stats, remote: RemoteUsage): Stats {
  const addOverall = (a: OverallStat, b: RemoteUsage["overall"]): OverallStat => ({
    requests: a.requests + b.requests,
    input_tokens: a.input_tokens + b.input_tokens,
    output_tokens: a.output_tokens + b.output_tokens,
    cache_read_tokens: a.cache_read_tokens + b.cache_read_tokens,
    cache_write_tokens: a.cache_write_tokens + b.cache_write_tokens,
    reasoning_tokens: a.reasoning_tokens + b.reasoning_tokens,
    total_tokens: a.total_tokens + b.total_tokens,
  });

  // by_model 按 model_id+provider_id 合并相加
  const key = (m: { model_id: string; provider_id: string }) =>
    `${m.provider_id}|${m.model_id}`;
  const merged = new Map<string, ModelStat>();
  for (const m of local.by_model) {
    merged.set(key(m), { ...m });
  }
  for (const m of remote.by_model) {
    const k = key(m);
    const ex = merged.get(k);
    if (ex) {
      ex.requests += m.requests;
      ex.input_tokens += m.input_tokens;
      ex.output_tokens += m.output_tokens;
      ex.cache_read_tokens += m.cache_read_tokens;
      ex.cache_write_tokens += m.cache_write_tokens;
      ex.reasoning_tokens += m.reasoning_tokens;
      ex.total_tokens += m.total_tokens;
    } else {
      merged.set(k, {
        model_id: m.model_id,
        provider_id: m.provider_id,
        requests: m.requests,
        input_tokens: m.input_tokens,
        output_tokens: m.output_tokens,
        cache_read_tokens: m.cache_read_tokens,
        cache_write_tokens: m.cache_write_tokens,
        reasoning_tokens: m.reasoning_tokens,
        total_tokens: m.total_tokens,
      });
    }
  }
  // 按 total_tokens 降序
  const by_model = Array.from(merged.values()).sort(
    (a, b) => b.total_tokens - a.total_tokens
  );

  return {
    from_ms: local.from_ms,
    to_ms: local.to_ms,
    overall: addOverall(local.overall, remote.overall),
    by_model,
    earliest_ms: local.earliest_ms,
    latest_ms: local.latest_ms,
  };
}

/** 合并花费：本地 cost + 远端（用 pricing 自算） */
function mergeCost(
  local: CostResult | null,
  remote: RemoteUsage,
  pricing: PricingConfig,
  _currency: Currency
): CostResult {
  const base = local ?? {
    total_cny: 0,
    total_usd: 0,
    per_model_cny: [],
    per_model_usd: [],
  };
  const cnyExtra = remote.by_model.map((m) => ({
    model_id: m.model_id,
    cost: modelCost(
      m.model_id,
      m.input_tokens,
      m.output_tokens,
      m.cache_read_tokens,
      pricing,
      "cny"
    ),
  }));
  const usdExtra = remote.by_model.map((m) => ({
    model_id: m.model_id,
    cost: modelCost(
      m.model_id,
      m.input_tokens,
      m.output_tokens,
      m.cache_read_tokens,
      pricing,
      "usd"
    ),
  }));
  return {
    total_cny:
      base.total_cny + cnyExtra.reduce((s, x) => s + x.cost, 0),
    total_usd:
      base.total_usd + usdExtra.reduce((s, x) => s + x.cost, 0),
    per_model_cny: [...base.per_model_cny, ...cnyExtra],
    per_model_usd: [...base.per_model_usd, ...usdExtra],
  };
}

/** 把远端桶的 ms label 转成本地时区 label，与本地 trend 的 label 对齐。
 * - hour 桶：本地用 "HH:00"，远端 ms 同样格式化为本地时区 "HH:00"
 * - day 桶：本地用 "MM-DD"，远端 ms 格式化为本地时区 "MM-DD"
 * 关键：本地 hour 桶和服务端 hour 桶都用 UTC 整点对齐，ms 一致，
 * 格式化后 label 也一致，可精确匹配。day 桶本地按本地0点、服务端按UTC0点，
 * 在非 UTC 时区可能错位——这是已知的可接受偏差（长周期趋势影响小）。 */
function msToLocalLabel(msStr: string, bucket: TrendBucket): string | null {
  const ms = parseInt(msStr, 10);
  if (isNaN(ms)) return null;
  const d = new Date(ms);
  if (bucket === "hour") {
    const hh = String(d.getHours()).padStart(2, "0");
    return `${hh}:00`;
  }
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}

/** 远端趋势桶 → 本地 TrendPoint 格式（远端无 cost，自算）。
 * label 由 ms 转成本地时区格式，便于与本地 trend 按 label 合并。 */
function remoteTrendToLocal(
  remote: RemoteUsage,
  pricing: PricingConfig,
  bucket: TrendBucket
): TrendPoint[] {
  return remote.trend
    .map((b) => {
      const label = msToLocalLabel(b.label, bucket);
      if (label === null) return null;
      const cost_cny = b.by_model.reduce(
        (s, m) =>
          s +
          modelCost(
            m.model_id,
            m.input_tokens,
            m.output_tokens,
            m.cache_read_tokens,
            pricing,
            "cny"
          ),
        0
      );
      const cost_usd = b.by_model.reduce(
        (s, m) =>
          s +
          modelCost(
            m.model_id,
            m.input_tokens,
            m.output_tokens,
            m.cache_read_tokens,
            pricing,
            "usd"
          ),
        0
      );
      return {
        label,
        total_tokens: b.total_tokens,
        requests: b.requests,
        cost_cny,
        cost_usd,
      };
    })
    .filter((x): x is TrendPoint => x !== null);
}

/** 合并本地趋势 + 远端趋势：按 label 匹配相加，保持本地顺序 */
function mergeTrend(
  local: TrendPoint[],
  remote: RemoteUsage,
  pricing: PricingConfig,
  bucket: TrendBucket
): TrendPoint[] {
  const remotePts = remoteTrendToLocal(remote, pricing, bucket);
  // 远端按 label 建索引
  const remoteMap = new Map<string, TrendPoint>();
  for (const r of remotePts) {
    remoteMap.set(r.label, r);
  }
  // 本地顺序为主，合并远端同 label 桶；远端多出的桶追加到末尾
  const usedLabels = new Set<string>();
  const out: TrendPoint[] = local.map((l) => {
    usedLabels.add(l.label);
    const r = remoteMap.get(l.label);
    return {
      label: l.label,
      total_tokens: l.total_tokens + (r?.total_tokens ?? 0),
      requests: l.requests + (r?.requests ?? 0),
      cost_cny: l.cost_cny + (r?.cost_cny ?? 0),
      cost_usd: l.cost_usd + (r?.cost_usd ?? 0),
    };
  });
  for (const r of remotePts) {
    if (!usedLabels.has(r.label)) {
      out.push(r);
    }
  }
  return out;
}

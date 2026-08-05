import { useCallback, useEffect, useState } from "react";
import type {
  ConsumedBucket,
  DeviceInfo,
  PeakConfig,
  SyncConfig,
  WeeklyPeriod,
  WeeklyTokenBucket,
} from "./types";
import {
  getCompareConsumed,
  getCompareTokens,
  getPeakConfig,
  getSyncConfig,
  getWeeklyCompare,
  listRemoteDevices,
  remoteUsage,
} from "./api";
import { formatTokens } from "./format";

interface Props {
  onBack: () => void;
}

export function ComparePanel({ onBack }: Props) {
  const [periods, setPeriods] = useState<WeeklyPeriod[]>([]);
  const [tokens, setTokens] = useState<WeeklyTokenBucket[]>([]);
  const [consumed, setConsumed] = useState<ConsumedBucket[]>([]);
  const [peakCfg, setPeakCfg] = useState<PeakConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedIdx, setSelectedIdx] = useState<number | null>(null);

  // 多设备同步相关
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState<string>("all");
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 初始读同步配置 + 设备列表 + 高峰配置
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
    getPeakConfig()
      .then(setPeakCfg)
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // 1. 周期列表
      const ps = await getWeeklyCompare();
      setPeriods(ps);

      if (ps.length === 0) {
        setTokens([]);
        setConsumed([]);
        return;
      }

      // 2. 本地 token + 折算消耗（一次请求各自）
      const periodPairs: [number, number][] = ps.map((p) => [p.reset_at, p.end_at]);
      const [localTokens, localConsumed] = await Promise.all([
        getCompareTokens(periodPairs),
        getCompareConsumed(periodPairs),
      ]);

      // 3. 远端 token 合并（按设备筛选）
      const wantLocal = deviceFilter === "all" || deviceFilter === "local";
      const wantRemote =
        syncEnabled &&
        (deviceFilter === "all" || (deviceFilter !== "local" && deviceFilter !== "all"));

      // 远端：对整段时间跨度调一次 trend(day)，按 reset_at 归属到周期累加
      let remoteByPeriod: Map<number, number> = new Map();
      const fromMs = ps[0].reset_at;
      const toMs = ps[ps.length - 1].end_at;
      if (wantRemote && syncConfig && toMs > fromMs) {
        const opts =
          deviceFilter === "all"
            ? { excludeDevice: syncConfig.device_id }
            : { devices: deviceFilter };
        try {
          const remote = await remoteUsage(fromMs, toMs, "day", opts);
          // 远端 trend 桶 label 是毫秒字符串，按归属周期累加
          for (const b of remote.trend) {
            const ms = parseInt(b.label, 10);
            if (isNaN(ms)) continue;
            // 找到 ms 所属的周期
            const idx = ps.findIndex((p) => ms >= p.reset_at && ms < p.end_at);
            if (idx >= 0) {
              remoteByPeriod.set(
                idx,
                (remoteByPeriod.get(idx) ?? 0) + b.total_tokens
              );
            }
          }
        } catch {
          // 远端失败静默降级
          if (deviceFilter !== "all") throw new Error("远端数据获取失败");
        }
      }

      // 4. 合并：本地 token + 远端 token
      const mergedTokens: WeeklyTokenBucket[] = localTokens.map((t, i) => ({
        ...t,
        total_tokens:
          t.total_tokens + (wantLocal ? 0 : 0) + (remoteByPeriod.get(i) ?? 0),
      }));
      // 若选了"仅远端某设备"，本地不该算 → 重置为远端值
      if (!wantLocal) {
        for (let i = 0; i < mergedTokens.length; i++) {
          mergedTokens[i] = {
            ...mergedTokens[i],
            total_tokens: remoteByPeriod.get(i) ?? 0,
            requests: 0,
          };
        }
      }

      setTokens(mergedTokens);
      // 折算消耗只基于本地（远端明细不足以做折算）
      setConsumed(localConsumed);
      setSelectedIdx(ps.length - 1); // 默认选中当前周期
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [deviceFilter, syncConfig, syncEnabled]);

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
              consumed={consumed[selectedIdx]}
              peakCfg={peakCfg}
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
                        <span className={pctColor(p.pct_peak)}>{p.pct_peak}%</span>
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
        周百分比来自智谱账户级采样（含所有设备/工具消耗）；
        <br />
        {peakCfg?.plan_type
          ? peakCfg.plan_type === "v2"
            ? "消耗按 V2 倍率折算（token×倍率）"
            : "消耗按 V3 积分公式折算"
          : "Token 仅统计 ZCode CLI"}
        {peakCfg?.zcode_discount ? "，含 ZCode ×0.67 优惠" : ""}。
      </div>
    </div>
  );
}

/** 周期柱状图：每根柱=一个周期，高度=峰值百分比 */
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
    <div className="rounded-lg bg-white/25 border border-white/30 px-2.5 py-2">
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
          周额度峰值趋势
        </span>
      </div>
      <div className={`flex items-end ${barGap} h-16`}>
        {periods.map((p, i) => {
          const h = p.pct_peak; // 0-100
          const isSel = i === selectedIdx;
          const color = pctBarColor(p.pct_peak);
          return (
            <button
              key={p.reset_at}
              onClick={() => onSelect(i)}
              className="flex-1 h-full flex items-end justify-center min-w-0 group"
              title={`${dateLabel(p.reset_at)}: 峰值 ${p.pct_peak}%`}
            >
              <div
                className={`w-full rounded-t-sm transition-all duration-300 ${color} ${
                  isSel ? "opacity-100 ring-1 ring-sky-400" : "opacity-70 group-hover:opacity-90"
                }`}
                style={{ height: `${Math.max(h, 2)}%` }}
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
  consumed,
  peakCfg,
}: {
  period: WeeklyPeriod;
  token?: WeeklyTokenBucket;
  consumed?: ConsumedBucket;
  peakCfg: PeakConfig | null;
}) {
  const totalTokens = token?.total_tokens ?? 0;
  const consumedVal = consumed?.consumed ?? 0;
  const sampleLow = period.sample_count < 10;

  const hasPlan = !!peakCfg?.plan_type;
  // 折算列的标签和副文案
  const consumedLabel =
    peakCfg?.plan_type === "v3" ? "积分消耗" : "额度消耗";
  const consumedSub =
    peakCfg?.plan_type === "v3"
      ? `积分公式${peakCfg?.zcode_discount ? " + ZCode优惠" : ""}`
      : `token×倍率${peakCfg?.zcode_discount ? " + ZCode优惠" : ""}`;
  // 格式化：V2 用 token 格式（M/K），V3 积分用普通数字
  const fmtConsumed = (v: number) =>
    peakCfg?.plan_type === "v3"
      ? v.toLocaleString("zh-CN", { maximumFractionDigits: 0 })
      : formatTokens(Math.round(v));

  return (
    <div className="rounded-lg bg-white/30 border border-white/40 px-3 py-2.5 space-y-2">
      {/* 标题行 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          <span className="text-[11px] font-semibold text-slate-900/90">
            {dateLabel(period.reset_at)}
            {period.is_current ? " ~ 进行中" : ` ~ ${dateLabel(period.end_at)}`}
          </span>
        </div>
        <span className={`text-[11px] num font-semibold ${pctColor(period.pct_peak)}`}>
          峰值 {period.pct_peak}%
        </span>
      </div>

      {/* token 两列 */}
      <div className="grid grid-cols-2 gap-2">
        <div className="rounded-md bg-white/25 py-1.5 px-2">
          <div className="text-[9px] text-slate-700/55">实际 Token</div>
          <div className="num text-[13px] font-semibold text-slate-900/85 mt-0.5">
            {formatTokens(totalTokens)}
          </div>
          {token && (
            <div className="text-[9px] num text-slate-700/45 mt-0.5">
              {token.requests} 次请求
            </div>
          )}
        </div>
        {hasPlan && (
          <div className="rounded-md bg-white/25 py-1.5 px-2">
            <div className="text-[9px] text-slate-700/55">{consumedLabel}</div>
            <div className="num text-[13px] font-semibold text-violet-700/85 mt-0.5">
              {fmtConsumed(consumedVal)}
            </div>
            <div className="text-[9px] text-slate-700/45 mt-0.5">{consumedSub}</div>
          </div>
        )}
      </div>

      {/* 百分比进度 + 采样可信度 */}
      <div>
        <div className="flex items-center justify-between text-[10px] mb-0.5">
          <span className="text-slate-700/55">起 {period.pct_start}% → 止 {period.pct_end}%</span>
          <span className={`num ${sampleLow ? "text-amber-600/80" : "text-slate-700/45"}`}>
            采样 {period.sample_count} 条{sampleLow ? " · 不足" : ""}
          </span>
        </div>
        <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
          <div
            className={`h-full rounded-full ${pctBarColor(period.pct_peak)} opacity-80`}
            style={{ width: `${period.pct_peak}%` }}
          />
        </div>
      </div>
    </div>
  );
}

// ===== 辅助 =====

/** 百分比对应的文字颜色 */
function pctColor(pct: number): string {
  if (pct >= 90) return "text-red-600";
  if (pct >= 70) return "text-amber-600";
  return "text-emerald-600";
}

/** 百分比对应的柱子背景色 */
function pctBarColor(pct: number): string {
  if (pct >= 90) return "bg-red-400";
  if (pct >= 70) return "bg-amber-400";
  return "bg-emerald-400";
}

/** 毫秒 → "MM-DD" label */
function dateLabel(ms: number): string {
  const d = new Date(ms);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}

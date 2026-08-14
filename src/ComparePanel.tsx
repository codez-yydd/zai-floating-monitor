import { useCallback, useEffect, useRef, useState } from "react";
import type {
  DeviceInfo,
  SyncConfig,
  WeeklyPeriod,
  WeeklyTokenBucket,
} from "./types";
import {
  getCompareTokens,
  getSyncConfig,
  getWeeklyCompare,
  listRemoteDevices,
  remoteUsage,
} from "./api";
import { formatTokens } from "./format";
import { remainingGradient, remainingTextColor } from "./widgets";

interface Props {
  onBack: () => void;
}

export function ComparePanel({ onBack }: Props) {
  const [periods, setPeriods] = useState<WeeklyPeriod[]>([]);
  const [tokens, setTokens] = useState<WeeklyTokenBucket[]>([]);
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
      // 1. 周期列表（账户级，本机快照解析即代表全局额度周期）
      const ps = await getWeeklyCompare();
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧响应
      setPeriods(ps);

      if (ps.length === 0) {
        setTokens([]);
        return;
      }

      // 周期区间
      const periodPairs: [number, number][] = ps.map((p) => [
        p.reset_at,
        p.end_at,
      ]);
      const fromMs = ps[0].reset_at;
      const toMs = ps[ps.length - 1].end_at;

      // 数据来源：all=本地+远端(排除本机)；local=仅本地；具体id=仅远端该设备
      const wantLocal = deviceFilter === "all" || deviceFilter === "local";
      const wantRemote =
        syncEnabled &&
        (deviceFilter === "all" ||
          (deviceFilter !== "local" && deviceFilter !== "all"));

      // source 固定 zcode：对比页口径是智谱 quota vs ZCode 实际消耗，
      // 不含 Codex（服务端不传 source 会合并全部来源）
      const opts =
        syncConfig &&
        (deviceFilter === "all"
          ? { excludeDevice: syncConfig.device_id, source: "zcode" }
          : { devices: deviceFilter, source: "zcode" });

      // 2. 并发：本地 token + 远端（trend 折 token）
      const tasks: Promise<unknown>[] = [];

      // 本地 token
      let localTokens: WeeklyTokenBucket[] = [];
      if (wantLocal) {
        tasks.push(
          getCompareTokens(periodPairs).then((t) => (localTokens = t))
        );
      } else {
        // 仅远端：构造空本地骨架，后面填远端值
        localTokens = periodPairs.map(([reset_at, end_at]) => ({
          reset_at,
          end_at,
          total_tokens: 0,
          requests: 0,
        }));
      }

      // 远端 token（按周期累加 trend.total_tokens）
      let remoteTokenByPeriod = new Map<number, number>();
      let remoteReqByPeriod = new Map<number, number>();

      if (wantRemote && syncConfig && opts && toMs > fromMs) {
        // token：trend(day) 按归属周期累加
        tasks.push(
          remoteUsage(fromMs, toMs, "day", opts)
            .then((remote) => {
              for (const b of remote.trend) {
                const ms = parseInt(b.label, 10);
                if (isNaN(ms)) continue;
                const idx = ps.findIndex(
                  (p) => ms >= p.reset_at && ms < p.end_at
                );
                if (idx >= 0) {
                  remoteTokenByPeriod.set(
                    idx,
                    (remoteTokenByPeriod.get(idx) ?? 0) + b.total_tokens
                  );
                  remoteReqByPeriod.set(
                    idx,
                    (remoteReqByPeriod.get(idx) ?? 0) + b.requests
                  );
                }
              }
            })
            .catch(() => {
              // "全部"模式远端失败静默降级；具体设备失败透出
              if (deviceFilter !== "all") throw new Error("远端数据获取失败");
            })
        );
      }

      await Promise.all(tasks);
      if (reqId !== loadReqId.current) return; // 已有更新的请求，丢弃旧响应

      // 3. 合并 token
      const mergedTokens: WeeklyTokenBucket[] = localTokens.map((t, i) => ({
        ...t,
        total_tokens:
          (wantLocal ? t.total_tokens : 0) +
          (remoteTokenByPeriod.get(i) ?? 0),
        requests:
          (wantLocal ? t.requests : 0) + (remoteReqByPeriod.get(i) ?? 0),
      }));
      setTokens(mergedTokens);

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
                        <span style={{ color: remainingTextColor(100 - p.pct_peak) }}>{p.pct_peak}%</span>
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
        Token 仅统计 ZCode CLI。
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
          // 峰值高度按已用，颜色统一按剩余渐变（与全局额度条一致）
          const bg = remainingGradient(100 - p.pct_peak);
          return (
            <button
              key={p.reset_at}
              onClick={() => onSelect(i)}
              className="flex-1 h-full flex items-end justify-center min-w-0 group"
              title={`${dateLabel(p.reset_at)}: 峰值 ${p.pct_peak}%`}
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
}: {
  period: WeeklyPeriod;
  token?: WeeklyTokenBucket;
}) {
  const totalTokens = token?.total_tokens ?? 0;
  const sampleLow = period.sample_count < 10;

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
        <span
          className="text-[11px] num font-semibold"
          style={{ color: remainingTextColor(100 - period.pct_peak) }}
        >
          峰值 {period.pct_peak}%
        </span>
      </div>

      {/* token 统计 */}
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
            className="h-full rounded-full opacity-80"
            style={{
              width: `${period.pct_peak}%`,
              background: remainingGradient(100 - period.pct_peak),
            }}
          />
        </div>
      </div>
    </div>
  );
}

// ===== 辅助 =====

/** 毫秒 → "MM-DD" label */
function dateLabel(ms: number): string {
  const d = new Date(ms);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}

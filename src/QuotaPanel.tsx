import { useCallback, useEffect, useState } from "react";
import type { NotifyConfig, QuotaResult } from "./types";
import { fetchQuota, getNotifyConfig, getTodayDelta } from "./api";
import { loadCache, saveCache } from "./cache";

interface Props {
  /** 点击「去设置」回调 */
  onGoSettings: () => void;
}

const LEVEL_LABEL: Record<string, string> = {
  lite: "Lite",
  pro: "Pro",
  max: "Max",
  ultra: "Ultra",
};

/** 默认阈值（与后端 notify.rs 一致），配置加载前/失败时兜底 */
const DEFAULT_THRESHOLDS = { hour5: 75, weekly: 80, mcp: 75 };

export function QuotaPanel({ onGoSettings }: Props) {
  const [quota, setQuota] = useState<QuotaResult | null>(() =>
    loadCache<QuotaResult>("zbar-quota")
  );
  const [notifyCfg, setNotifyCfg] = useState<NotifyConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [now, setNow] = useState(Date.now());
  // 今日周额度增量：[增量百分比, 今日采样数]
  const [todayDelta, setTodayDelta] = useState<[number, number] | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const r = await fetchQuota();
      setQuota(r);
      // 缓存额度结果，供下次冷启动首屏秒开（额度是网络请求，最慢）
      saveCache("zbar-quota", r);
      // 额度刷新成功后顺带读今日增量（快照由 fetch_quota 采样写入）
      getTodayDelta()
        .then(setTodayDelta)
        .catch(() => {});
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
    // 阈值配置只加载一次（设置页保存后会重新拉，这里初值即可）
    getNotifyConfig()
      .then(setNotifyCfg)
      .catch(() => {});
  }, [load]);

  // 每 30 秒刷新额度 + 每秒更新倒计时
  useEffect(() => {
    const quotaTimer = setInterval(load, 30_000);
    const tickTimer = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      clearInterval(quotaTimer);
      clearInterval(tickTimer);
    };
  }, [load]);

  // 未配置 token 的错误 → 显示引导
  if (error && /未配置|Token/i.test(error)) {
    return (
      <div className="mx-3.5 mb-1 rounded-lg bg-sky-500/10 border border-sky-500/20 px-2.5 py-1.5">
        <div className="flex items-center justify-between">
          <span className="text-[11px] text-sky-700/80">
            Coding Plan 额度监控
          </span>
          <button
            onClick={onGoSettings}
            className="text-[10px] text-sky-600 hover:text-sky-700 transition-colors"
          >
            去配置 →
          </button>
        </div>
        <p className="text-[10px] text-slate-700/50 mt-0.5">
          填写 API Token 后显示用量进度
        </p>
      </div>
    );
  }

  // 其他错误 → 简短提示
  if (error || !quota) {
    return (
      <div className="mx-3.5 mb-1 rounded-lg bg-amber-500/10 border border-amber-500/20 px-2.5 py-1.5">
        <span className="text-[10px] text-amber-700/80">
          {error ? `额度查询失败：${error}` : "加载中…"}
        </span>
      </div>
    );
  }

  const levelLabel = LEVEL_LABEL[quota.level] || quota.level || "—";

  // 阈值：优先用设置页配置，未加载时用默认值
  const t5 = notifyCfg?.hour5_threshold ?? DEFAULT_THRESHOLDS.hour5;
  const tw = notifyCfg?.weekly_threshold ?? DEFAULT_THRESHOLDS.weekly;
  const tm = notifyCfg?.mcp_threshold ?? DEFAULT_THRESHOLDS.mcp;

  return (
    <div className="mx-3.5 mb-1 rounded-lg bg-white/30 border border-white/40 px-2.5 py-2">
      {/* 标题行 */}
      <div className="flex items-center justify-between mb-1.5">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
            Coding Plan
          </span>
          <span className="px-1.5 py-0 rounded text-[9px] font-semibold bg-violet-500/15 text-violet-700">
            {levelLabel}
          </span>
        </div>
        <button
          onClick={load}
          disabled={loading}
          className="text-slate-700/40 hover:text-slate-900/70 text-[11px] transition-colors"
          title="刷新额度"
        >
          ↻
        </button>
      </div>

      {/* 5小时窗口 */}
      {quota.hour5 && (
        <QuotaBar
          label="5小时"
          usedPct={quota.hour5.percentage}
          resetAt={quota.hour5.nextResetTime}
          now={now}
          threshold={t5}
        />
      )}

      {/* 每周 */}
      {quota.weekly && (
        <QuotaBar
          label="本周"
          usedPct={quota.weekly.percentage}
          resetAt={quota.weekly.nextResetTime}
          now={now}
          threshold={tw}
          className="mt-1.5"
          delta={todayDelta}
        />
      )}

      {/* MCP 月度额度 */}
      {quota.mcp && (
        <McpBar
          usedPct={quota.mcp.percentage}
          resetAt={quota.mcp.nextResetTime}
          now={now}
          threshold={tm}
          currentValue={quota.mcp.currentValue}
          total={quota.mcp.usage}
          className="mt-1.5"
        />
      )}
    </div>
  );
}

/** 单条额度进度条 */
function QuotaBar({
  label,
  usedPct,
  resetAt,
  now,
  threshold,
  className = "",
  delta,
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
  /** 警告阈值（百分比，来自设置页）。≥ threshold 黄，≥ threshold+15 红 */
  threshold: number;
  className?: string;
  /** 今日增量：[增量百分比, 采样数]。仅 weekly 传入 */
  delta?: [number, number] | null;
}) {
  const pct = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - pct;

  // 颜色按设置页阈值：< threshold 绿；[threshold, threshold+15) 黄；≥ threshold+15 红
  const color = pctColorClass(pct, threshold);

  // 今日增量徽标：采样不足(<2)不显示数字
  const deltaPct = delta ? delta[0] : 0;
  const hasDelta = delta != null && delta[1] >= 2 && deltaPct > 0;

  return (
    <div className={className}>
      <div className="grid grid-cols-[3.25rem_minmax(0,1fr)_2.75rem] items-center gap-2 text-[10px] mb-0.5">
        <span className="text-slate-700/60">{label}</span>
        {resetAt && resetAt > now ? (
          <span className="num text-slate-700/40 text-right pr-1 whitespace-nowrap">
            ↻ {formatCountdown(resetAt - now)}
          </span>
        ) : (
          <span />
        )}
        <span className="num font-semibold text-slate-900/85 text-right">
          {pct}%
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className={`h-full rounded-full ${color} opacity-80 transition-all duration-500`}
          style={{ width: `${pct}%` }}
        />
      </div>
      {/* 今日增量 + 不足警示 */}
      {(hasDelta || (remaining < 20 && remaining > 0)) && (
        <div className="text-[9px] mt-0.5 flex items-center gap-2">
          {hasDelta && (
            <span className={`num ${pctTextClass(pct, threshold)}`}>
              ↑今日 {deltaPct}%
            </span>
          )}
          {remaining < 20 && remaining > 0 && (
            <span className="text-red-600/80">仅剩 {remaining.toFixed(0)}%</span>
          )}
        </div>
      )}
    </div>
  );
}

/** 毫秒差 → "17m" / "3h 12m" / "2d 5h" */
function formatCountdown(ms: number): string {
  const totalMin = Math.floor(ms / 60_000);
  if (totalMin < 1) return "<1m";
  const days = Math.floor(totalMin / (60 * 24));
  const hours = Math.floor((totalMin % (60 * 24)) / 60);
  const mins = totalMin % 60;
  if (days > 0) return `${days}d ${hours}h 后刷新`;
  if (hours > 0) return `${hours}h ${mins}m 后刷新`;
  return `${mins}m 后刷新`;
}

/** 百分比 → 进度条背景色 class，按阈值驱动：
 * < threshold 绿 / [threshold, threshold+15) 黄 / ≥ threshold+15 红 */
function pctColorClass(pct: number, threshold: number): string {
  if (pct >= threshold + 15) return "bg-red-400";
  if (pct >= threshold) return "bg-amber-400";
  return "bg-emerald-400";
}

/** 百分比 → 文字色 class（与进度条同阈值） */
function pctTextClass(pct: number, threshold: number): string {
  if (pct >= threshold + 15) return "text-red-600/90";
  if (pct >= threshold) return "text-amber-600/90";
  return "text-emerald-600/90";
}

/** MCP 月度额度条：与 QuotaBar 同款进度条。
 *  - 中间列显示刷新倒计时（与 5h/周 一致；若接口无 nextResetTime 则留空）
 *  - 进度条下方副信息行：绝对值「已用 / 总量」 + 不足时的「仅剩 X%」 */
function McpBar({
  usedPct,
  resetAt,
  now,
  threshold,
  currentValue,
  total,
  className = "",
}: {
  usedPct: number;
  resetAt?: number | null;
  now: number;
  /** 警告阈值（百分比，来自设置页） */
  threshold: number;
  currentValue?: number;
  total?: number;
  className?: string;
}) {
  const pct = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - pct;

  // 与 QuotaBar 统一阈值驱动（原本用 sky-400，现改为绿/黄/红）
  const color = pctColorClass(pct, threshold);

  // 有绝对值时显示 "已用 / 总量"
  const hasAbs =
    typeof currentValue === "number" && typeof total === "number";

  // 副信息：绝对值 + 不足警示，合并成一行
  const subParts: string[] = [];
  if (hasAbs) {
    subParts.push(`${currentValue} / ${total}`);
  }
  if (remaining < 20 && remaining > 0) {
    subParts.push(`仅剩 ${remaining.toFixed(0)}%`);
  }

  return (
    <div className={className}>
      <div className="grid grid-cols-[3.25rem_minmax(0,1fr)_2.75rem] items-center gap-2 text-[10px] mb-0.5">
        <span className="text-slate-700/60">MCP</span>
        {resetAt && resetAt > now ? (
          <span className="num text-slate-700/40 text-right pr-1 whitespace-nowrap">
            ↻ {formatCountdown(resetAt - now)}
          </span>
        ) : (
          <span />
        )}
        <span className="num font-semibold text-slate-900/85 text-right">
          {pct}%
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className={`h-full rounded-full ${color} opacity-80 transition-all duration-500`}
          style={{ width: `${pct}%` }}
        />
      </div>
      {subParts.length > 0 && (
        <div className="text-[9px] text-slate-700/50 mt-0.5 flex items-center gap-2">
          {hasAbs && <span className="num">{currentValue} / {total}</span>}
          {remaining < 20 && remaining > 0 && (
            <span className="text-red-600/80">仅剩 {remaining.toFixed(0)}%</span>
          )}
        </div>
      )}
    </div>
  );
}

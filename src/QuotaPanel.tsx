import { useCallback, useEffect, useState } from "react";
import type { QuotaResult } from "./types";
import { fetchQuota } from "./api";

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

export function QuotaPanel({ onGoSettings }: Props) {
  const [quota, setQuota] = useState<QuotaResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [now, setNow] = useState(Date.now());

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const r = await fetchQuota();
      setQuota(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
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
        />
      )}

      {/* 每周 */}
      {quota.weekly && (
        <QuotaBar
          label="本周"
          usedPct={quota.weekly.percentage}
          resetAt={quota.weekly.nextResetTime}
          now={now}
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
  className = "",
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
  className?: string;
}) {
  const pct = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - pct;

  // 颜色：用量越高越警示
  const color =
    pct >= 90
      ? "bg-red-400"
      : pct >= 70
        ? "bg-amber-400"
        : "bg-emerald-400";

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
      {remaining < 20 && remaining > 0 && (
        <div className="text-[9px] text-red-600/80 mt-0.5">
          仅剩 {remaining.toFixed(0)}%
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

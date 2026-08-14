import { useEffect, useState } from "react";
import { useDataCache } from "./DataCache";
import { remainingGradient, remainingTextColor } from "./widgets";

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
  // 额度数据由全局 DataProvider 统一预加载 + 30s 定时刷新，
  // 此组件仅负责展示（纯展示层），不再自己请求。
  const { quota, quotaError, todayDelta, refreshQuota } = useDataCache();
  const [now, setNow] = useState(Date.now());

  // 每秒更新倒计时显示
  useEffect(() => {
    const tickTimer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tickTimer);
  }, []);

  // 未配置 token 的错误 → 显示引导
  if (quotaError && /未配置|Token/i.test(quotaError)) {
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
  if (quotaError || !quota) {
    return (
      <div className="mx-3.5 mb-1 rounded-lg bg-amber-500/10 border border-amber-500/20 px-2.5 py-1.5">
        <span className="text-[10px] text-amber-700/80">
          {quotaError ? `额度查询失败：${quotaError}` : "加载中…"}
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
          onClick={refreshQuota}
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
          delta={todayDelta}
        />
      )}

      {/* MCP 月度额度 */}
      {quota.mcp && (
        <McpBar
          usedPct={quota.mcp.percentage}
          resetAt={quota.mcp.nextResetTime}
          now={now}
          currentValue={quota.mcp.currentValue}
          total={quota.mcp.usage}
          className="mt-1.5"
        />
      )}
    </div>
  );
}

/** 单条额度进度条（剩余版：填充剩余量，颜色随剩余渐变） */
function QuotaBar({
  label,
  usedPct,
  resetAt,
  now,
  className = "",
  delta,
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
  className?: string;
  /** 今日增量：[增量百分比, 采样数]。仅 weekly 传入 */
  delta?: [number, number] | null;
}) {
  const used = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - used;

  // 今日增量徽标：采样不足(<2)不显示数字
  const deltaPct = delta ? delta[0] : 0;
  const hasDelta = delta != null && delta[1] >= 2 && deltaPct > 0;

  return (
    <div className={className}>
      <div className="grid grid-cols-[3.25rem_minmax(0,1fr)_3.25rem] items-center gap-2 text-[10px] mb-0.5">
        <span className="text-slate-700/60">{label}</span>
        {resetAt && resetAt > now ? (
          <span className="num text-slate-700/40 text-right pr-1 whitespace-nowrap">
            ↻ {formatCountdown(resetAt - now)}
          </span>
        ) : (
          <span />
        )}
        <span
          className="num font-semibold text-right"
          style={{ color: remainingTextColor(remaining) }}
        >
          剩 {Math.round(remaining)}%
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
      {/* 今日增量 */}
      {hasDelta && (
        <div className="text-[9px] mt-0.5">
          <span className="num text-slate-700/50">↑今日 {deltaPct}%</span>
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

/** MCP 月度额度条：与 QuotaBar 同款（剩余版）。
 *  - 中间列显示刷新倒计时（若接口无 nextResetTime 则留空）
 *  - 进度条下方副信息行：绝对值「已用 / 总量」 */
function McpBar({
  usedPct,
  resetAt,
  now,
  currentValue,
  total,
  className = "",
}: {
  usedPct: number;
  resetAt?: number | null;
  now: number;
  currentValue?: number;
  total?: number;
  className?: string;
}) {
  const used = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - used;

  // 有绝对值时显示 "已用 / 总量"
  const hasAbs =
    typeof currentValue === "number" && typeof total === "number";

  return (
    <div className={className}>
      <div className="grid grid-cols-[3.25rem_minmax(0,1fr)_3.25rem] items-center gap-2 text-[10px] mb-0.5">
        <span className="text-slate-700/60">MCP</span>
        {resetAt && resetAt > now ? (
          <span className="num text-slate-700/40 text-right pr-1 whitespace-nowrap">
            ↻ {formatCountdown(resetAt - now)}
          </span>
        ) : (
          <span />
        )}
        <span
          className="num font-semibold text-right"
          style={{ color: remainingTextColor(remaining) }}
        >
          剩 {Math.round(remaining)}%
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
      {hasAbs && (
        <div className="text-[9px] text-slate-700/50 mt-0.5 num">
          {currentValue} / {total}
        </div>
      )}
    </div>
  );
}

import { useState } from "react";
import type { Currency, TrendPoint } from "./types";
import { formatCost, formatTokens } from "./format";

/** 通用进度条 */
export function ProgressBar({
  pct,
  color = "bg-sky-400",
  track = "bg-slate-900/10",
  height = "h-1.5",
}: {
  pct: number;
  color?: string;
  track?: string;
  height?: string;
}) {
  return (
    <div className={`flex-1 ${height} rounded-full overflow-hidden ${track}`}>
      <div
        className={`h-full rounded-full ${color} transition-all duration-500`}
        style={{ width: `${Math.min(Math.max(pct * 100, 0), 100)}%` }}
      />
    </div>
  );
}

/** 三栏指标卡片 */
export function Metric({
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

/** 明细行（带占比条） */
export function DetailRow({
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
export function TrendChart({
  points,
  bucket,
  currency,
  metric,
  onMetricChange,
  showMetricToggle = true,
}: {
  points: TrendPoint[];
  bucket: "hour" | "day";
  currency: Currency;
  metric: "cost" | "token";
  onMetricChange?: (m: "cost" | "token") => void;
  showMetricToggle?: boolean;
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
        {showMetricToggle && onMetricChange && (
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
        )}
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

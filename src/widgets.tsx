import { useState } from "react";
import type { Currency, CurrentModel, TrendPoint } from "./types";
import { formatCost, formatMs, formatTps, formatTokens, dateStr } from "./format";
import { useI18n } from "./i18n";

// ============================================================
// 额度剩余渐变色：所有额度进度条统一按「剩余百分比」着色。
// 分档：>80% 绿、30-80% 黄、<30% 红；档内随剩余连续渐变
// （剩余越低颜色越深，如 <10% 时明显更红）。边界色连续无跳变。
// ============================================================

/** 插值锚点：[剩余%, 色相, 饱和度, 亮度]。色相用 -10 等价 350，避免跨 360 插值走反方向。
 *  饱和度控制在 55~70：与界面低饱和毛玻璃风格协调，避免高饱和纯色扎眼 */
const REMAINING_STOPS: [number, number, number, number][] = [
  [0, -2, 66, 41], // 砖红（额度耗尽）
  [30, 24, 70, 50], // 柔橙（红/黄边界）
  [80, 48, 68, 46], // 暖黄（黄/绿边界，色相偏暖降低荧光感）
  [100, 152, 56, 40], // 灰绿（额度充裕）
];

/** 剩余百分比 → 基色 HSL（分段线性插值） */
function remainingHsl(remaining: number): [number, number, number] {
  const r = Math.min(Math.max(remaining, 0), 100);
  for (let i = 0; i < REMAINING_STOPS.length - 1; i++) {
    const [r0, h0, s0, l0] = REMAINING_STOPS[i];
    const [r1, h1, s1, l1] = REMAINING_STOPS[i + 1];
    if (r <= r1) {
      const t = (r - r0) / (r1 - r0);
      const h = (((h0 + (h1 - h0) * t) % 360) + 360) % 360;
      return [h, s0 + (s1 - s0) * t, l0 + (l1 - l0) * t];
    }
  }
  const [, h, s, l] = REMAINING_STOPS[REMAINING_STOPS.length - 1];
  return [h, s, l];
}

/** 剩余百分比 → 进度条填充渐变（左深右浅，基色随剩余分段渐变） */
export function remainingGradient(remaining: number): string {
  const [h, s, l] = remainingHsl(remaining);
  // 右端仅提亮 6 个点、封顶 52：保留立体感但不发荧光
  return `linear-gradient(90deg, hsl(${h}, ${s}%, ${l}%), hsl(${h}, ${s}%, ${Math.min(l + 6, 52)}%))`;
}

/** 剩余百分比 → 文字颜色（同基色但压暗 10 个亮度点：
 *  小号数字（9~11px）需要更高对比度，条可以浅、文字要深） */
export function remainingTextColor(remaining: number): string {
  const [h, s, l] = remainingHsl(remaining);
  return `hsl(${h}, ${s}%, ${Math.max(l - 10, 28)}%)`;
}

/** 通用进度条 */
export function ProgressBar({
  pct,
  color = "bg-sky-500",
  gradient,
  track = "bg-slate-900/10",
  height = "h-1.5",
}: {
  pct: number;
  color?: string;
  /** CSS background 渐变（额度剩余条用，优先于 color） */
  gradient?: string;
  track?: string;
  height?: string;
}) {
  return (
    <div className={`flex-1 ${height} rounded-full overflow-hidden ${track}`}>
      <div
        className={`h-full rounded-full ${gradient ? "" : color} transition-all duration-500`}
        style={{
          width: `${Math.min(Math.max(pct * 100, 0), 100)}%`,
          ...(gradient ? { background: gradient } : null),
        }}
      />
    </div>
  );
}

/** 三栏指标卡片 */
export function Metric({
  label,
  value,
  accent,
  title,
}: {
  label: string;
  value: string;
  accent?: string;
  /** 悬浮提示（如速度卡展示最快值） */
  title?: string;
}) {
  return (
    <div className="card-base rounded-xl py-2 text-center" title={title}>
      <div className="text-[9px] text-slate-500">{label}</div>
      <div
        className={`num text-[13px] font-semibold mt-0.5 ${
          accent || "text-slate-900/85"
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

/** 速度/首字延迟指标卡行。仅数据源带耗时（ZCode/Claude）时 overall 上有值，
 *  无值返回 null（Codex/Cursor 面板自动不渲染，不出现空卡片）。 */
export function SpeedMetricsGrid({
  overall,
}: {
  overall: {
    avg_tps?: number | null;
    max_tps?: number | null;
    avg_ttft_ms?: number | null;
  };
}) {
  const { t } = useI18n();
  const hasTps = overall.avg_tps != null;
  const hasTtft = overall.avg_ttft_ms != null;
  if (!hasTps && !hasTtft) return null;
  const cols = hasTps && hasTtft ? "grid-cols-2" : "grid-cols-1";
  return (
    <div className={`grid ${cols} gap-1.5`}>
      {hasTps && (
        <Metric
          label={t("common.avgSpeed")}
          value={`${formatTps(overall.avg_tps!)} t/s`}
          accent="text-sky-700"
          title={
            overall.max_tps != null
              ? `${t("common.fastest")} ${formatTps(overall.max_tps)} tok/s`
              : undefined
          }
        />
      )}
      {hasTtft && (
        <Metric
          label={t("common.ttft")}
          value={formatMs(overall.avg_ttft_ms!)}
          accent="text-sky-700"
        />
      )}
    </div>
  );
}

/** 当前模型条：最近使用的模型 + 相对时间（null 不渲染） */
export function CurrentModelBar({ model }: { model?: CurrentModel | null }) {
  const { t } = useI18n();
  if (!model) return null;
  // 相对时间：刚刚 / N分钟前 / N小时前 / N天前，超 30 天回退日期
  const min = Math.floor(Math.max(0, Date.now() - model.last_used_ms) / 60_000);
  const rel =
    min < 1
      ? t("common.justNow")
      : min < 60
        ? t("common.minutesAgo", { n: min })
        : min < 1440
          ? t("common.hoursAgo", { n: Math.floor(min / 60) })
          : min < 43200
            ? t("common.daysAgo", { n: Math.floor(min / 1440) })
            : dateStr(model.last_used_ms);
  return (
    <div className="card-base rounded-xl px-2.5 py-1.5 flex items-center justify-between gap-2">
      <div className="flex items-center gap-1.5 min-w-0">
        <span className="text-[9px] text-slate-500 shrink-0">
          {t("common.currentModel")}
        </span>
        <span className="text-[11px] font-medium text-slate-900/85 truncate">
          {model.model_id}
        </span>
      </div>
      <span className="num text-[9px] text-slate-400 shrink-0">{rel}</span>
    </div>
  );
}

/** 趋势图指标：花费 / Token / 请求（报告页三选，其余页面默认前两者） */
export type TrendMetric = "cost" | "token" | "requests";

/** 趋势图：迷你柱状图 + 最新桶环比 + 指标切换（按钮组由 metrics 决定）。
 *  粒度跟随所选时间范围：今日/24h 按小时，更长范围按日。 */
export function TrendChart<M extends TrendMetric>({
  points,
  bucket,
  currency,
  metric,
  metrics,
  onMetricChange,
  showMetricToggle = true,
  fill = false,
}: {
  points: TrendPoint[];
  bucket: "hour" | "day";
  currency: Currency;
  metric: M;
  /** 可选：切换按钮组渲染哪些指标；不传则保持默认花费/Token 两选 */
  metrics?: M[];
  onMetricChange?: (m: M) => void;
  showMetricToggle?: boolean;
  /** 撑满父级剩余高度（汇总页用，避免柱图下方大块留白） */
  fill?: boolean;
}) {
  const { t } = useI18n();
  const metricOptions: M[] = metrics ?? (["cost", "token"] as M[]);

  // 取每根柱子的数值（跟随当前指标：请求 / 花费 / Token）
  const valueOf = (d: TrendPoint): number =>
    metric === "cost"
      ? currency === "cny"
        ? d.cost_cny
        : d.cost_usd
      : metric === "token"
        ? d.total_tokens
        : d.requests;
  const values = points.map(valueOf);
  const maxValue = Math.max(...values, 1); // 至少为 1，避免除 0

  // 最新桶 vs 上一桶 环比（跟随当前指标比较）
  const last = points[points.length - 1];
  const prev = points[points.length - 2];
  const lastValue = last ? valueOf(last) : 0;
  const prevValue = prev ? valueOf(prev) : 0;
  let deltaText: string | null = null;
  // deltaKind：flat/new 用词典文案（灰底），pct 为涨跌百分比（红/绿底）
  let deltaKind: "flat" | "new" | "pct" = "pct";
  let deltaUp = false;
  if (last && prev) {
    if (prevValue > 0) {
      const pct = ((lastValue - prevValue) / prevValue) * 100;
      if (Math.abs(pct) < 0.5) {
        deltaKind = "flat";
        deltaText = t("common.trendFlat");
      } else {
        deltaUp = pct > 0;
        deltaText = `${deltaUp ? "↑" : "↓"}${Math.abs(pct).toFixed(0)}%`;
      }
    } else if (lastValue > 0) {
      deltaKind = "new";
      deltaText = t("common.trendNew");
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
    <div
      className={`card-base rounded-2xl px-3 pt-2.5 pb-2 ${
        fill ? "flex-1 min-h-0 flex flex-col" : ""
      }`}
    >
      {/* 标题行 */}
      <div className="flex items-center justify-between mb-2 shrink-0">
        <div className="flex items-center gap-1.5">
          <div className="section-title">
            <span>{t("common.usageTrend")}</span>
          </div>
          {deltaText && (
            <span
              className={`text-[9px] num px-1 py-px rounded ${
                deltaKind !== "pct"
                  ? "text-slate-500 bg-slate-900/5"
                  : deltaUp
                    ? "text-rose-600 bg-rose-500/10"
                    : "text-emerald-600 bg-emerald-500/10"
              }`}
              title={isHour ? t("common.trendVsHour") : t("common.trendVsDay")}
            >
              {deltaText}
            </span>
          )}
        </div>
        {/* 指标切换：按钮组按 metrics 渲染 */}
        {showMetricToggle && onMetricChange && (
          <div className="flex gap-0.5 p-0.5 rounded-lg bg-slate-900/4">
            {metricOptions.map((m) => (
              <button
                key={m}
                onClick={() => onMetricChange(m)}
                className={`px-2 py-0.5 rounded-md text-[9px] font-medium transition-all duration-150 ${
                  metric === m
                    ? "bg-sky-500 text-white shadow-sm"
                    : "text-slate-500 hover:text-slate-700"
                }`}
              >
                {m === "cost"
                  ? t("common.cost")
                  : m === "token"
                    ? "Token"
                    : t("common.requests")}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* 柱状图 */}
      <div
        className={`flex items-end ${barGap} relative ${
          fill ? "flex-1 min-h-14" : "h-16"
        }`}
      >
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
                <div className="absolute bottom-full mb-1 left-1/2 -translate-x-1/2 z-10 whitespace-nowrap rounded-lg bg-slate-900/90 text-white px-2 py-1.5 text-[9px] leading-tight pointer-events-none shadow-lg">
                  <div className="num font-medium">{d.label}</div>
                  <div className="num">
                    {formatCost(
                      currency === "cny" ? d.cost_cny : d.cost_usd,
                      currency
                    )}
                  </div>
                  <div className="num opacity-70">
                    {formatTokens(d.total_tokens)}
                  </div>
                  <div className="num opacity-70">
                    {d.requests.toLocaleString()} {t("common.requests")}
                  </div>
                </div>
              )}
              <div
                className={`w-full rounded-t-md transition-all duration-300 ${
                  isLast
                    ? "bg-gradient-to-t from-sky-600 to-sky-400"
                    : isHover
                      ? "bg-gradient-to-t from-slate-600 to-slate-400"
                      : "bg-gradient-to-t from-slate-500/70 to-slate-400/50"
                }`}
                style={{
                  height: `${Math.max(h, v > 0 ? 4 : 0)}%`,
                  maxWidth: n <= 7 ? "16px" : undefined,
                }}
              />
            </div>
          );
        })}
      </div>

      {/* 标签：柱子多时隔几个显示一个；靠近末柱的步进标签去掉，避免与「始终显示的最后一根」挤在一起 */}
      <div className={`flex ${barGap} mt-1 shrink-0`}>
        {points.map((d, i) => {
          const isFirst = i === 0;
          const isLast = i === n - 1;
          const farFromLast = n - 1 - i >= Math.max(1, Math.ceil(labelStep / 2));
          const showLabel =
            isFirst || isLast || (i % labelStep === 0 && farFromLast);
          return (
            <span
              key={d.label}
              className={`flex-1 text-[8px] num min-w-0 whitespace-nowrap ${
                isFirst ? "text-left" : isLast ? "text-right" : "text-center"
              } ${
                isLast
                  ? "text-sky-600 font-medium"
                  : "text-slate-500/80"
              } ${showLabel ? "" : "invisible"}`}
            >
              {d.label}
            </span>
          );
        })}
      </div>
    </div>
  );
}

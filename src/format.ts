import type { Currency } from "./types";

/** 格式化 token 数量：3.7M / 1280 / 1.2B */
export function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(2) + "B";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

/** 格式化金额 */
export function formatCost(n: number, currency: Currency): string {
  const symbol = currency === "cny" ? "¥" : "$";
  if (n === 0) return `${symbol}0.00`;
  if (n < 0.01) return `${symbol}${n.toFixed(4)}`;
  return `${symbol}${n.toFixed(2)}`;
}

/** 格式化百分比 */
export function formatPct(n: number): string {
  if (!isFinite(n)) return "—";
  return (n * 100).toFixed(1) + "%";
}

/** 毫秒差 → 倒计时核心（"2h 05m" 等纯数字部分）。
 *  原带的「后刷新」本地化后缀已拆出：由调用方经 t("common.refreshIn", { time }) 拼接。 */
export function formatCountdownCore(ms: number): string {
  const totalMin = Math.floor(ms / 60_000);
  if (totalMin < 1) return "<1m";
  const days = Math.floor(totalMin / (60 * 24));
  const hours = Math.floor((totalMin % (60 * 24)) / 60);
  const mins = totalMin % 60;
  return days > 0
    ? `${days}d ${hours}h`
    : hours > 0
      ? `${hours}h ${mins}m`
      : `${mins}m`;
}

/** 绝对时间戳 → 重置时间点短格式（"MM-DD HH:mm"，本地时区、24 小时制、不带年份）。
 *  withTime=false 只出日期（Cursor 日期精度专用：billing_cycle_end 只有日期，
 *  解析出的时分是假值，禁止展示）。无效时间戳返回空串。 */
export function formatResetStamp(
  ms: number,
  opts?: { withTime?: boolean }
): string {
  if (!Number.isFinite(ms)) return "";
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  const date = `${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  if (opts?.withTime === false) return date;
  return `${date} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// ============================================================
// Coding Plan 套餐等级 → 展示标签。
// 原先在 QuotaPanel / SummaryTab 各有一份 LEVEL_LABEL，多账号额度
// 展示（AccountsCard 等）也需要，统一收敛到这里共用。
// ============================================================

const LEVEL_LABEL: Record<string, string> = {
  lite: "Lite",
  pro: "Pro",
  max: "Max",
  ultra: "Ultra",
};

/** 套餐等级标签：已知等级（lite/pro/max/ultra）映射为首字母大写短标，
 *  未知等级原样返回（接口未来加档位时不至于显示空白） */
export function levelLabel(level: string): string {
  return LEVEL_LABEL[level] || level;
}

/** 时间范围预设 → [from_ms, to_ms] 毫秒时间戳 */
export function rangeToMs(
  preset: string,
  custom?: { from: string; to: string }
): [number, number] {
  const now = Date.now();
  switch (preset) {
    case "today": {
      const d = new Date();
      d.setHours(0, 0, 0, 0);
      return [d.getTime(), now];
    }
    case "1d":
      return [now - 86400000, now];
    case "7d":
      return [now - 7 * 86400000, now];
    case "30d":
      return [now - 30 * 86400000, now];
    case "custom": {
      if (!custom) return [now - 86400000, now];
      const from = new Date(custom.from + "T00:00:00").getTime();
      const to = new Date(custom.to + "T23:59:59").getTime();
      return [from, to];
    }
    default:
      return [now - 86400000, now];
  }
}

/** 格式化日期为 YYYY-MM-DD（本地时区）。
 *  不能用 toISOString（UTC）：东八区凌晨 0-8 点会得到"昨天"，
 *  导致 RangePicker 的日期上限错一天。 */
export function dateStr(ms: number): string {
  const d = new Date(ms);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** 输出速度 tok/s：≥100 取整、否则一位小数（如 53.4 / 212） */
export function formatTps(v: number): string {
  return v >= 100 ? String(Math.round(v)) : v.toFixed(1);
}

/** 耗时格式化：<1s 毫秒、<1min 秒（一位小数）、否则分钟（一位小数） */
export function formatMs(v: number): string {
  if (v < 1000) return `${Math.round(v)}ms`;
  if (v < 60_000) return `${(v / 1000).toFixed(1)}s`;
  return `${(v / 60_000).toFixed(1)}min`;
}

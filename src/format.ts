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

/** 毫秒差 → 倒计时。compact 时不加「后刷新」，给窄行用。 */
export function formatCountdown(ms: number, compact = false): string {
  const totalMin = Math.floor(ms / 60_000);
  if (totalMin < 1) return "<1m";
  const days = Math.floor(totalMin / (60 * 24));
  const hours = Math.floor((totalMin % (60 * 24)) / 60);
  const mins = totalMin % 60;
  const core =
    days > 0
      ? `${days}d ${hours}h`
      : hours > 0
        ? `${hours}h ${mins}m`
        : `${mins}m`;
  return compact ? core : `${core} 后刷新`;
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

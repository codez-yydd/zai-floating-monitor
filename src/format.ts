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

/** 格式化日期为 YYYY-MM-DD */
export function dateStr(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

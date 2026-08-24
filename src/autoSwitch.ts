/**
 * ZCode 账号自动切换：目标挑选算法（纯函数）+ 无人值守开关/日志的 localStorage 读写。
 * 无 React 依赖。AccountsCard（手动挑选 + 确认弹窗）与 DataCache（额度满无人值守
 * 触发）共用，保证两条链路选出来的账号口径一致。
 */
import type { AccountQuotaEntry, QuotaResult } from "./types";

/** 自动切换完成事件：DataCache 写完日志后广播，AccountsCard 监听后重读日志刷新展示 */
export const AUTO_SWITCH_DONE_EVENT = "zbar:auto-switch-done";

/** 无人值守开关 localStorage key（"1"/"0"；读取异常默认关） */
const AUTO_SWITCH_ENABLED_KEY = "zbar-auto-switch";

/** 最近一次自动切换日志 localStorage key */
const AUTO_SWITCH_LOG_KEY = "zbar-auto-switch-log";

/** 候选分层：1=立即可用（5h 与周均有剩余）；2=等 5h 重置（5h 满但周有剩余） */
export type AutoSwitchLayer = 1 | 2;

/** 挑选结果：entry 为目标账号快照条目，layer/hour5Remain/weeklyRemain 供确认弹窗展示理由 */
export interface AutoSwitchPick {
  entry: AccountQuotaEntry;
  /** 1=立即可用 2=等 5h 重置 */
  layer: AutoSwitchLayer;
  /** 5h 窗口剩余百分比 0-100 */
  hour5Remain: number;
  /** 周窗口剩余百分比 0-100 */
  weeklyRemain: number;
  /** layer 2 时的 5h 重置时间（ms 时间戳，接口未给为 null） */
  resetAt: number | null;
}

/** 触发原因（当前只有"5h 额度用满"一种，预留扩展） */
export type AutoSwitchReason = "quotaFull";

/** 最近一次自动切换日志（localStorage 持久化，设置页展示） */
export interface AutoSwitchLogEntry {
  /** 发生时间（ms 时间戳） */
  ts: number;
  /** 切换到的账号名（失败为 null） */
  to: string | null;
  ok: boolean;
  /** 触发原因 */
  reasonKey: AutoSwitchReason;
}

/** 读无人值守开关（每次触发检查时现读，不缓存——与 locale 即时生效同模式） */
export function readAutoSwitchEnabled(): boolean {
  try {
    return localStorage.getItem(AUTO_SWITCH_ENABLED_KEY) === "1";
  } catch {
    return false;
  }
}

/** 写无人值守开关（"1"/"0"；存储异常静默，开关仅影响触发逻辑，不影响主流程） */
export function writeAutoSwitchEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(AUTO_SWITCH_ENABLED_KEY, enabled ? "1" : "0");
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/** 读最近一次自动切换日志；不存在或结构损坏返回 null */
export function readAutoSwitchLog(): AutoSwitchLogEntry | null {
  try {
    const raw = localStorage.getItem(AUTO_SWITCH_LOG_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<AutoSwitchLogEntry> | null;
    if (
      parsed &&
      typeof parsed.ts === "number" &&
      typeof parsed.ok === "boolean"
    ) {
      return {
        ts: parsed.ts,
        to: typeof parsed.to === "string" ? parsed.to : null,
        ok: parsed.ok,
        reasonKey: "quotaFull",
      };
    }
    return null;
  } catch {
    return null;
  }
}

/** 写最近一次自动切换日志（只保留一条，覆盖式；存储异常静默） */
export function writeAutoSwitchLog(entry: AutoSwitchLogEntry): void {
  try {
    localStorage.setItem(AUTO_SWITCH_LOG_KEY, JSON.stringify(entry));
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/**
 * 按 5h/周额度算法挑出最合适的其他账号。
 *
 * 算法分两层，第 1 层优先：
 *  1. 「立即可用」：5h 与周窗口均有剩余，切过去马上能继续干活。
 *     排序：主键 min(5h 剩, 周剩) 降序——实际续航由先耗尽的窗口（瓶颈）决定；
 *     并列再按周剩降序——周窗口 7 天才重置，比 5h 窗口更稀缺。
 *  2. 「等 5h 重置」：5h 已满但周有剩余（5h 重置后即可用），只用于手动切换
 *     （无人值守 readyOnly 时跳过——切过去还要干等没有意义）。
 *     排序：主键 5h 重置时间升序（最快恢复优先），重置时间未知（null）的沉底；
 *     并列再按周剩降序。
 * 两层都空返回 null。
 */
export function pickAutoSwitchTarget(
  entries: AccountQuotaEntry[],
  opts?: { readyOnly?: boolean }
): AutoSwitchPick | null {
  // 第一步 候选过滤：只考虑"数据完整可信"的其他账号——
  //  - 排除当前登录账号（is_current，切换它没有意义）
  //  - 查询失败（quota 为 null 或带 error）的账号不知道真实剩余，不赌
  //  - 5h / 周任一窗口数据缺失即排除（与账号列表展示层"周缺失不冒充剩 100%"
  //    的口径一致：缺数据 ≠ 有额度）
  const candidates: {
    entry: AccountQuotaEntry;
    hour5: NonNullable<QuotaResult["hour5"]>;
    weekly: NonNullable<QuotaResult["weekly"]>;
    /** 剩余 = max(0, 100 - 已用百分比)；剩余 0 即该窗口满 */
    hour5Remain: number;
    weeklyRemain: number;
  }[] = [];
  for (const e of entries) {
    if (e.is_current) continue;
    if (!e.quota || e.error) continue;
    const { hour5, weekly } = e.quota;
    if (!hour5 || !weekly) continue;
    candidates.push({
      entry: e,
      hour5,
      weekly,
      hour5Remain: Math.max(0, 100 - hour5.percentage),
      weeklyRemain: Math.max(0, 100 - weekly.percentage),
    });
  }

  // 第二步 第 1 层「立即可用」：两窗口剩余均 > 0
  const ready = candidates
    .filter((c) => c.hour5Remain > 0 && c.weeklyRemain > 0)
    .sort(
      (a, b) =>
        // 主键：瓶颈剩余（两窗口较小者）降序
        Math.min(b.hour5Remain, b.weeklyRemain) -
          Math.min(a.hour5Remain, a.weeklyRemain) ||
        // 并列：周剩余降序（周窗口更稀缺）
        b.weeklyRemain - a.weeklyRemain
    );
  if (ready.length > 0) {
    const top = ready[0];
    return {
      entry: top.entry,
      layer: 1,
      hour5Remain: top.hour5Remain,
      weeklyRemain: top.weeklyRemain,
      resetAt: null,
    };
  }

  // 第三步 第 2 层「等 5h 重置」：5h 满（剩余 0）但周有剩余。
  // readyOnly（无人值守）时跳过本层直接返回 null
  if (opts?.readyOnly) return null;
  const waiting = candidates
    .filter((c) => c.hour5Remain === 0 && c.weeklyRemain > 0)
    .map((c) => ({ c, resetAt: c.hour5.nextResetTime }))
    .sort((x, y) => {
      // 重置时间未知（null）的沉底；已知按恢复快慢升序
      if (x.resetAt == null && y.resetAt == null)
        return y.c.weeklyRemain - x.c.weeklyRemain;
      if (x.resetAt == null) return 1;
      if (y.resetAt == null) return -1;
      return x.resetAt - y.resetAt || y.c.weeklyRemain - x.c.weeklyRemain;
    });
  if (waiting.length > 0) {
    const top = waiting[0];
    return {
      entry: top.c.entry,
      layer: 2,
      hour5Remain: top.c.hour5Remain,
      weeklyRemain: top.c.weeklyRemain,
      resetAt: top.resetAt,
    };
  }
  return null;
}

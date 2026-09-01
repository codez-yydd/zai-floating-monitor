import type {
  AgentQuotaDelta,
  AgentQuotaDeltaMap,
  AgentQuotaSource,
  AgentQuotaSnapshot,
  AgentQuotaWindow,
  AgentQuotaWindowKey,
  RemoteAgentQuotaSnapshot,
} from "./types";

export function todayStartMs(nowMs = Date.now()): number {
  const date = new Date(nowMs);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

const AGENT_QUOTA_WINDOWS: Record<AgentQuotaSource, ReadonlySet<AgentQuotaWindowKey>> = {
  codex: new Set(["hour5", "weekly"]),
  claude: new Set(["hour5", "weekly"]),
  cursor: new Set(["cursor_auto", "cursor_api"]),
  kimi: new Set(["hour5", "weekly"]),
};

function isAgentQuotaSource(value: string): value is AgentQuotaSource {
  return (
    value === "codex" || value === "claude" || value === "cursor" || value === "kimi"
  );
}

function isValidUsedPct(value: number): boolean {
  return Number.isFinite(value) && value >= 0 && value <= 100;
}

function isValidWindow(
  source: string,
  window: AgentQuotaWindow
): window is AgentQuotaWindow {
  return (
    isAgentQuotaSource(source) &&
    AGENT_QUOTA_WINDOWS[source].has(window.key) &&
    isValidUsedPct(window.used_pct)
  );
}

/** 同一时刻多设备采样合并：额度百分比不相加，取较高值避免低估。 */
export function mergeAgentQuotaSnapshots(
  local: AgentQuotaSnapshot[],
  remote: RemoteAgentQuotaSnapshot[]
): AgentQuotaSnapshot[] {
  const byKey = new Map<string, AgentQuotaSnapshot>();
  for (const snapshot of [...local, ...remote]) {
    if (!Number.isFinite(snapshot.ts) || !isAgentQuotaSource(snapshot.source)) continue;
    const validWindows = snapshot.windows.filter((window) =>
      isValidWindow(snapshot.source, window)
    );
    if (validWindows.length === 0) continue;
    // 各设备的同一轮采样时间可能相差几毫秒，按本地同秒去重口径合并。
    const key = `${snapshot.source}:${Math.floor(snapshot.ts / 1000)}`;
    const previous = byKey.get(key);
    if (!previous) {
      byKey.set(key, {
        source: snapshot.source,
        ts: snapshot.ts,
        plan_type: snapshot.plan_type,
        windows: validWindows.map((window) => ({ ...window })),
      });
      continue;
    }

    const windowMap = new Map<AgentQuotaWindowKey, AgentQuotaWindow>();
    for (const window of previous.windows) windowMap.set(window.key, { ...window });
    for (const window of snapshot.windows.filter((item) =>
      isValidWindow(snapshot.source, item)
    )) {
      const old = windowMap.get(window.key);
      if (!old || window.used_pct > old.used_pct) {
        windowMap.set(window.key, { ...window });
      }
    }
    previous.windows = [...windowMap.values()];
    previous.plan_type ||= snapshot.plan_type;
    previous.ts = Math.min(previous.ts, snapshot.ts);
  }
  return [...byKey.values()].sort((a, b) => a.ts - b.ts);
}

function sameResetAt(a: number | null, b: number | null): boolean {
  return (a ?? null) === (b ?? null);
}

/** 远端 Cursor 失真快照过滤的百分比容差（百分点）。
 *  旧版本设备会持续上传「美元/百分比两套口径混用」的合成快照（实测虚高约 37 点）。
 *  周期百分比是账户级单调值：任何设备任何时刻采到的真实值都不可能超过本机刚
 *  拉到的实时值，超出容差的必然是失真合成值；取 2 点既足以区分失真样本，
 *  也容忍锚点几分钟滞后期间的真实增长。 */
const INFLATED_CURSOR_TOLERANCE_PCT = 2.0;

/**
 * 用本机最近一次实时拉取的 Cursor plan 百分比作锚点，剔除远端快照里
 * 当前周期内超出锚点 + 容差的失真窗口；剔除后 windows 为空的快照整条丢弃。
 * 锚点缺失（anchors 为 null）时原样返回，宁缺毋滥。
 */
export function filterInflatedRemoteCursorSnapshots(
  remote: RemoteAgentQuotaSnapshot[],
  anchors: {
    autoPct: number | null;
    apiPct: number | null;
    resetAtMs: number | null;
  } | null
): RemoteAgentQuotaSnapshot[] {
  if (!anchors) return remote;
  const out: RemoteAgentQuotaSnapshot[] = [];
  for (const snapshot of remote) {
    if (snapshot.source !== "cursor") {
      out.push(snapshot);
      continue;
    }
    const windows = snapshot.windows.filter((window) => {
      if (window.key !== "cursor_auto" && window.key !== "cursor_api") return true;
      const anchor = window.key === "cursor_auto" ? anchors.autoPct : anchors.apiPct;
      // 该侧无可用锚点时不过滤（宁缺毋滥）
      if (anchor === null || !Number.isFinite(anchor) || anchor <= 0) return true;
      // 仅精确匹配当前周期；reset 不匹配（含双方为 null）说明是其他周期
      // 的历史数据，不属于当前周期，直接保留（与 Rust 端清理同口径）
      const resetAt = window.reset_at ?? null;
      if (anchors.resetAtMs === null || resetAt === null || resetAt !== anchors.resetAtMs) {
        return true;
      }
      return window.used_pct <= anchor + INFLATED_CURSOR_TOLERANCE_PCT;
    });
    if (windows.length > 0) {
      out.push({ ...snapshot, windows });
    }
  }
  return out;
}

/**
 * 计算“今日首个快照到今日峰值”的增量。
 * 每个来源/窗口独立处理；reset_at 变化时分段重新起算，并汇总当天各段的正增量。
 */
export function calculateAgentQuotaDeltas(
  snapshots: AgentQuotaSnapshot[],
  dayStartMs: number,
  nowMs = Date.now()
): AgentQuotaDeltaMap {
  const grouped = new Map<string, AgentQuotaSnapshot[]>();
  for (const snapshot of snapshots) {
    if (
      snapshot.ts < dayStartMs ||
      snapshot.ts > nowMs ||
      !isAgentQuotaSource(snapshot.source)
    ) continue;
    for (const window of snapshot.windows) {
      if (!isValidWindow(snapshot.source, window)) continue;
      const key = `${snapshot.source}:${window.key}`;
      const item = grouped.get(key) ?? [];
      item.push({
        source: snapshot.source,
        ts: snapshot.ts,
        plan_type: snapshot.plan_type,
        windows: [{ ...window }],
      });
      grouped.set(key, item);
    }
  }

  const result: AgentQuotaDeltaMap = {};
  for (const [key, items] of grouped) {
    items.sort((a, b) => a.ts - b.ts);
    const [, windowKey] = key.split(":") as [string, AgentQuotaWindowKey];
    let segment: AgentQuotaSnapshot[] = [];
    let deltaPct = 0;
    let validSamples = 0;
    let previousReset: number | null = null;
    let hasPreviousReset = false;
    const finishSegment = () => {
      if (segment.length < 2) return;
      const values = segment.map((item) => item.windows[0]?.used_pct ?? 0);
      const start = values[0];
      const peak = Math.max(...values);
      deltaPct += Math.max(0, peak - start);
      validSamples += segment.length;
    };
    for (const item of items) {
      const resetAt = item.windows[0]?.reset_at ?? null;
      if (hasPreviousReset && !sameResetAt(previousReset, resetAt)) {
        finishSegment();
        segment = [];
      }
      segment.push(item);
      previousReset = resetAt;
      hasPreviousReset = true;
    }
    finishSegment();

    if (validSamples < 2 || deltaPct <= 0) continue;

    const source = items[0].source as keyof AgentQuotaDeltaMap;
    const sourceResult = result[source] ?? {};
    sourceResult[windowKey] = {
      pct: Math.round(deltaPct * 100) / 100,
      samples: items.length,
    } satisfies AgentQuotaDelta;
    result[source] = sourceResult;
  }
  return result;
}

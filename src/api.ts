import { invoke } from "@tauri-apps/api/core";
import type {
  AutoCleanupConfig,
  CleanupResult,
  CleanupStatus,
  ClaudeSnapshot,
  CodexSnapshot,
  CostResult,
  Currency,
  CursorConfig,
  CursorSnapshot,
  DeviceInfo,
  MergeResult,
  ModelInfo,
  PricingConfig,
  PricingDiff,
  ApplyPriceItem,
  AgentQuotaSnapshot,
  QuotaResult,
  QuotaSnapshot,
  RegisterRequest,
  RenameResult,
  RemoteSnapshot,
  RemoteAgentQuotaSnapshot,
  RemoteUsage,
  ShortcutConfig,
  Stats,
  SyncConfig,
  SyncOutcome,
  TrendBucket,
  TrendPoint,
  WeeklyPeriod,
  WeeklyTokenBucket,
} from "./types";

export async function fetchStats(
  fromMs: number,
  toMs: number
): Promise<Stats> {
  return invoke<Stats>("get_stats", {
    req: { from_ms: fromMs, to_ms: toMs },
  });
}

export async function fetchModels(): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("list_models");
}

export async function fetchPricing(): Promise<PricingConfig> {
  return invoke<PricingConfig>("get_pricing");
}

export async function savePricing(config: PricingConfig): Promise<void> {
  await invoke("set_pricing", { config });
}

/** 读取货币偏好（"cny" | "usd"），后端据此决定菜单栏显示 ¥ 还是 $ */
export async function fetchCurrency(): Promise<Currency> {
  return invoke<Currency>("get_currency");
}

/** 保存货币偏好，同步给后端（菜单栏标题随之刷新） */
export async function saveCurrency(currency: Currency): Promise<void> {
  await invoke("set_currency", { currency });
}

/** 对比内置参考表（编译期嵌入，无网络请求）与用户当前配置，返回差异（不修改任何文件） */
export async function checkPricingUpdates(): Promise<PricingDiff> {
  return invoke<PricingDiff>("check_pricing_updates");
}

/** 把用户勾选的价格项合并进 pricing 并保存 */
export async function applyPricingUpdates(
  items: ApplyPriceItem[]
): Promise<PricingConfig> {
  return invoke<PricingConfig>("apply_pricing_updates", { items });
}

export async function fetchQuota(): Promise<QuotaResult> {
  return invoke<QuotaResult>("fetch_quota");
}

/** 读取全局快捷键配置 */
export async function getShortcutConfig(): Promise<ShortcutConfig> {
  return invoke<ShortcutConfig>("get_shortcut_config");
}

/** 保存并立即应用快捷键（失败抛错，前端提示用户改键） */
export async function setShortcutConfig(
  config: ShortcutConfig
): Promise<void> {
  await invoke("set_shortcut_config", { config });
}

export async function fetchTrend(
  fromMs: number,
  toMs: number,
  bucket: TrendBucket
): Promise<TrendPoint[]> {
  return invoke<TrendPoint[]>("get_trend", {
    req: { from_ms: fromMs, to_ms: toMs, bucket },
  });
}

export async function computeCost(
  fromMs: number,
  toMs: number
): Promise<CostResult> {
  return invoke<CostResult>("compute_cost", {
    req: { from_ms: fromMs, to_ms: toMs },
  });
}

/** 把报告保存为 .md 文件并在文件管理器打开所在目录 */
export async function saveReport(
  content: string,
  filename: string
): Promise<string> {
  return invoke<string>("save_report", { content, filename });
}

// ===== 多设备同步 =====

export async function getSyncConfig(): Promise<SyncConfig> {
  return invoke<SyncConfig>("get_sync_config");
}

export async function setSyncConfig(config: SyncConfig): Promise<void> {
  await invoke("set_sync_config", { config });
}

export async function registerDevice(
  req: RegisterRequest
): Promise<SyncConfig> {
  return invoke<SyncConfig>("register_device", { req });
}

export async function syncNow(): Promise<SyncOutcome> {
  return invoke<SyncOutcome>("sync_now");
}

export async function disconnectDevice(): Promise<void> {
  await invoke("disconnect_device");
}

export async function remoteUsage(
  fromMs: number,
  toMs: number,
  bucket: TrendBucket,
  options: {
    excludeDevice?: string;
    devices?: string;
    /** 数据来源筛选："zcode" | "codex" | "claude"，空串 = 全部 */
    source?: string;
  } = {}
): Promise<RemoteUsage> {
  return invoke<RemoteUsage>("remote_usage", {
    req: {
      from_ms: fromMs,
      to_ms: toMs,
      bucket,
      exclude_device: options.excludeDevice ?? "",
      devices: options.devices ?? "",
      source: options.source ?? "",
    },
  });
}

/** 拉取远端额度快照（带 device_id）。options 同 remoteUsage。 */
export async function remoteSnapshots(
  fromMs: number,
  toMs: number,
  options: { excludeDevice?: string; devices?: string } = {}
): Promise<RemoteSnapshot[]> {
  return invoke<RemoteSnapshot[]>("remote_snapshots", {
    req: {
      from_ms: fromMs,
      to_ms: toMs,
      exclude_device: options.excludeDevice ?? "",
      devices: options.devices ?? "",
    },
  });
}

/** 读取本地 Codex / Claude / Cursor 额度快照。 */
export async function getAgentQuotaHistory(
  fromMs: number,
  toMs: number
): Promise<AgentQuotaSnapshot[]> {
  return invoke<AgentQuotaSnapshot[]>("get_agent_quota_history", {
    req: { from_ms: fromMs, to_ms: toMs },
  });
}

/** 拉取远端 Codex / Claude / Cursor 额度快照（带 device_id）。 */
export async function remoteAgentQuotaSnapshots(
  fromMs: number,
  toMs: number,
  options: {
    source?: string;
    excludeDevice?: string;
    devices?: string;
  } = {}
): Promise<RemoteAgentQuotaSnapshot[]> {
  return invoke<RemoteAgentQuotaSnapshot[]>("remote_agent_quota_snapshots", {
    req: {
      from_ms: fromMs,
      to_ms: toMs,
      source: options.source ?? "",
      exclude_device: options.excludeDevice ?? "",
      devices: options.devices ?? "",
    },
  });
}

export async function listRemoteDevices(): Promise<DeviceInfo[]> {
  return invoke<DeviceInfo[]>("list_remote_devices");
}

export async function getCleanupStatus(): Promise<CleanupStatus> {
  return invoke<CleanupStatus>("get_cleanup_status");
}

export async function cleanupServer(
  masterToken: string,
  action: "device" | "before" | "all" | "reset",
  deviceId?: string,
  days?: number
): Promise<CleanupResult> {
  return invoke<CleanupResult>("cleanup_server", {
    req: {
      master_token: masterToken,
      action,
      device_id: deviceId ?? "",
      days: days ?? 0,
    },
  });
}

export async function mergeDevices(
  masterToken: string,
  sourceDeviceId: string,
  targetDeviceId: string
): Promise<MergeResult> {
  return invoke<MergeResult>("merge_devices", {
    req: {
      master_token: masterToken,
      source_device_id: sourceDeviceId,
      target_device_id: targetDeviceId,
    },
  });
}

export async function renameDevice(
  masterToken: string,
  deviceId: string,
  deviceName: string
): Promise<RenameResult> {
  return invoke<RenameResult>("rename_device", {
    req: {
      master_token: masterToken,
      device_id: deviceId,
      device_name: deviceName,
    },
  });
}

export async function setAutoCleanup(
  masterToken: string,
  autoEnabled: boolean,
  autoDays: number
): Promise<AutoCleanupConfig> {
  return invoke<AutoCleanupConfig>("set_auto_cleanup", {
    req: {
      master_token: masterToken,
      auto_enabled: autoEnabled,
      auto_days: autoDays,
    },
  });
}

export async function pendingUploadCount(): Promise<number> {
  return invoke<number>("pending_upload_count");
}

// ===== 窗口置顶常驻（仅 Windows 有意义）=====

/** 读取窗口置顶状态 */
export async function fetchPin(): Promise<boolean> {
  return invoke<boolean>("get_pin");
}

/** 设置窗口置顶状态并立即应用到原生窗口 */
export async function setPin(enabled: boolean): Promise<void> {
  await invoke("set_pin", { enabled });
}

// ===== 周额度追踪 / 对比页 =====

/** 读取全部额度快照历史（按 ts 升序） */
export async function getQuotaHistory(): Promise<QuotaSnapshot[]> {
  return invoke<QuotaSnapshot[]>("get_quota_history");
}

/** 解析快照为"智谱重置周期"列表 */
export async function getWeeklyCompare(): Promise<WeeklyPeriod[]> {
  return invoke<WeeklyPeriod[]>("get_weekly_compare");
}

/** 用指定快照解析周额度周期（对比页多设备筛选用） */
export async function getWeeklyCompareForSnapshots(
  snapshots: QuotaSnapshot[]
): Promise<WeeklyPeriod[]> {
  return invoke<WeeklyPeriod[]>("get_weekly_compare_for_snapshots", {
    snapshots,
  });
}

/** 今日增量：[增量百分比, 今日采样数] */
export async function getTodayDelta(): Promise<[number, number]> {
  return invoke<[number, number]>("get_today_delta");
}

/** 清空额度快照历史 */
export async function clearQuotaHistory(): Promise<void> {
  await invoke("clear_quota_history");
}

/** 对比页"实际 token"（本地部分）：对一组周期 [reset_at, end_at) 逐周期聚合 */
export async function getCompareTokens(
  periods: [number, number][]
): Promise<WeeklyTokenBucket[]> {
  return invoke<WeeklyTokenBucket[]>("get_compare_tokens", { periods });
}

/** 对比页：按指定 Agent 和周期聚合 Token（支持 zai/codex/claude/cursor） */
export async function getCompareTokensForAgent(
  source: "zai" | "codex" | "claude" | "cursor",
  periods: [number, number][]
): Promise<WeeklyTokenBucket[]> {
  return invoke<WeeklyTokenBucket[]>("get_compare_tokens_for_agent", {
    source,
    periods,
  });
}

// ===== Cursor 用量统计 =====

/** 拉取 Cursor 用量快照（套餐额度 + events 明细） */
export async function fetchCursorUsage(
  fromMs: number,
  toMs: number
): Promise<CursorSnapshot> {
  return invoke<CursorSnapshot>("get_cursor_usage", {
    req: { from_ms: fromMs, to_ms: toMs },
  });
}

/** 读取 Cursor 配置 */
export async function getCursorConfig(): Promise<CursorConfig> {
  return invoke<CursorConfig>("get_cursor_config");
}

/** 保存 Cursor 配置 */
export async function setCursorConfig(config: CursorConfig): Promise<void> {
  await invoke("set_cursor_config", { config });
}

/** 立即联网获取最新 USD→CNY 汇率（多源容错）并写入后端配置，返回 [汇率, 来源名] */
export async function fetchFxRate(): Promise<[number, string]> {
  return invoke<[number, string]>("fetch_fx_rate");
}

// ===== Codex 用量统计 =====

/** 拉取 Codex 用量快照（stats + trend + 速率限制）。
 *  Codex 未安装 / 无会话目录时后端返回 Err（中文提示），调用方需容错。 */
export async function fetchCodexUsage(
  fromMs: number,
  toMs: number,
  bucket: TrendBucket
): Promise<CodexSnapshot> {
  return invoke<CodexSnapshot>("get_codex_usage", {
    req: { from_ms: fromMs, to_ms: toMs, bucket },
  });
}

// ===== Claude 用量统计 =====

/** 拉取 Claude 用量快照（stats + trend + 订阅额度）。
 *  Claude Code 未安装 / 无会话目录时后端返回 Err（中文提示），调用方需容错。 */
export async function fetchClaudeUsage(
  fromMs: number,
  toMs: number,
  bucket: TrendBucket
): Promise<ClaudeSnapshot> {
  return invoke<ClaudeSnapshot>("get_claude_usage", {
    req: { from_ms: fromMs, to_ms: toMs, bucket },
  });
}

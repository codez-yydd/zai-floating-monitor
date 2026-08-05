import { invoke } from "@tauri-apps/api/core";
import type {
  AutoCleanupConfig,
  CleanupResult,
  CleanupStatus,
  ConsumedBucket,
  CostResult,
  DeviceInfo,
  ModelInfo,
  PeakConfig,
  PlanType,
  PricingConfig,
  QuotaConfig,
  QuotaResult,
  QuotaSnapshot,
  RegisterRequest,
  RemoteUsage,
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

export async function fetchQuotaConfig(): Promise<QuotaConfig> {
  return invoke<QuotaConfig>("get_quota_config");
}

export async function saveQuotaConfig(config: QuotaConfig): Promise<void> {
  await invoke("set_quota_config", { config });
}

export async function fetchQuota(): Promise<QuotaResult> {
  return invoke<QuotaResult>("fetch_quota");
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

export async function openConfigDir(): Promise<void> {
  await invoke("open_config_dir");
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
  options: { excludeDevice?: string; devices?: string } = {}
): Promise<RemoteUsage> {
  return invoke<RemoteUsage>("remote_usage", {
    req: {
      from_ms: fromMs,
      to_ms: toMs,
      bucket,
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

// ===== 周额度追踪 / 对比页 / 高峰期 =====

/** 读取全部额度快照历史（按 ts 升序） */
export async function getQuotaHistory(): Promise<QuotaSnapshot[]> {
  return invoke<QuotaSnapshot[]>("get_quota_history");
}

/** 解析快照为"智谱重置周期"列表 */
export async function getWeeklyCompare(): Promise<WeeklyPeriod[]> {
  return invoke<WeeklyPeriod[]>("get_weekly_compare");
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

/** 对比页"折算消耗"（本地部分）：按订阅类型折算（V2=等效token, V3=积分） */
export async function getCompareConsumed(
  periods: [number, number][]
): Promise<ConsumedBucket[]> {
  return invoke<ConsumedBucket[]>("get_compare_consumed", { periods });
}

/** 读取高峰期配置 */
export async function getPeakConfig(): Promise<PeakConfig> {
  return invoke<PeakConfig>("get_peak_config");
}

/** 保存高峰期配置 */
export async function setPeakConfig(config: PeakConfig): Promise<void> {
  await invoke("set_peak_config", { config });
}

/** 切换订阅类型并重置该类型默认时段（保留 zcode_discount） */
export async function setPlanType(plan: PlanType): Promise<PeakConfig> {
  return invoke<PeakConfig>("set_plan_type", { plan });
}

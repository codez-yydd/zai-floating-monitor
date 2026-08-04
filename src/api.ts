import { invoke } from "@tauri-apps/api/core";
import type {
  CostResult,
  ModelInfo,
  PricingConfig,
  QuotaConfig,
  QuotaResult,
  Stats,
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

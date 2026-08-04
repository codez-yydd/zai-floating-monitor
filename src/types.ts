// 与 Rust 后端 serde 结构一一对应

export interface ModelStat {
  model_id: string;
  provider_id: string;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
}

export interface OverallStat {
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
}

export interface Stats {
  from_ms: number;
  to_ms: number;
  overall: OverallStat;
  by_model: ModelStat[];
  earliest_ms: number | null;
  latest_ms: number | null;
}

export interface ModelInfo {
  provider_id: string;
  model_id: string;
}

export interface ModelPrice {
  input: number;
  output: number;
  cache_read: number;
}

export interface PricingConfig {
  cny: Record<string, ModelPrice>;
  usd: Record<string, ModelPrice>;
}

export interface CostResult {
  total_cny: number;
  total_usd: number;
  per_model_cny: { model_id: string; cost: number }[];
  per_model_usd: { model_id: string; cost: number }[];
}

export type Currency = "cny" | "usd";

export type RangePreset = "today" | "1d" | "7d" | "30d" | "custom";

// ===== Coding Plan 额度查询（与 Rust quota.rs 结构一一对应） =====

export type QuotaEndpoint = "cn" | "global";

/** 额度查询配置（token + 端点） */
export interface QuotaConfig {
  token: string;
  endpoint: QuotaEndpoint;
}

/** 单条用量限制 */
export interface QuotaLimit {
  /** 已用百分比 0-100 */
  percentage: number;
  /** 下次重置时间（毫秒时间戳，可能为 null）
   *  注：后端 serde 用 rename 对齐 API 的驼峰字段，输出给前端也是 nextResetTime */
  nextResetTime: number | null;
}

/** 额度查询结果：套餐等级 + 5小时/每周用量 */
export interface QuotaResult {
  /** 套餐等级："pro" / "max" ... */
  level: string;
  hour5: QuotaLimit | null;
  weekly: QuotaLimit | null;
}

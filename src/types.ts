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

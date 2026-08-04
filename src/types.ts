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

// ===== 趋势图：时间序列分桶（与 Rust lib.rs TrendBucket 结构一一对应） =====

/** 趋势图分桶粒度 */
export type TrendBucket = "hour" | "day";

/** 单个桶的汇总（含两种货币花费，前端无需再算） */
export interface TrendPoint {
  /** 桶标签："14:00"（小时）或 "08-04"（日） */
  label: string;
  /** 桶内总 token */
  total_tokens: number;
  /** 桶内总请求数 */
  requests: number;
  /** 桶内人民币花费 */
  cost_cny: number;
  /** 桶内美元花费 */
  cost_usd: number;
}

// ===== Coding Plan 额度查询（与 Rust quota.rs 结构一一对应） =====

export type QuotaEndpoint = "cn" | "global";

/** 额度查询配置（token + 端点） */
export interface QuotaConfig {
  token: string;
  endpoint: QuotaEndpoint;
}

/** MCP 工具用量明细（仅 MCP 月度额度会出现） */
export interface McpUsageDetail {
  /** 工具代号：search-prime / web-reader / zread ... */
  modelCode: string;
  /** 该工具已用次数 */
  usage: number;
}

/** 单条用量限制 */
export interface QuotaLimit {
  /** "TOKENS_LIMIT" | "TIME_LIMIT"（TIME_LIMIT 即 MCP 月度额度） */
  type: string;
  /** 接口窗口单位：3 = 小时，6 = 周（仅 token 额度有） */
  unit?: number;
  /** 窗口数量：5 小时窗口为 5，周窗口为 1（仅 token 额度有） */
  number?: number;
  /** 已用百分比 0-100 */
  percentage: number;
  /** 下次重置时间（毫秒时间戳，可能为 null；MCP 月度额度通常无此字段）
   *  注：后端 serde 用 rename 对齐 API 的驼峰字段，输出给前端也是 nextResetTime */
  nextResetTime: number | null;
  /** MCP 已用次数（仅 MCP 月度额度有） */
  currentValue?: number;
  /** MCP 总额度次数（仅 MCP 月度额度有；注意后端字段名是 usage，不是 total） */
  usage?: number;
  /** MCP 按工具拆分明细（仅 MCP 月度额度有） */
  usageDetails?: McpUsageDetail[];
}

/** 额度查询结果：套餐等级 + 5小时/每周/MCP 用量 */
export interface QuotaResult {
  /** 套餐等级："pro" / "max" ... */
  level: string;
  hour5: QuotaLimit | null;
  weekly: QuotaLimit | null;
  /** MCP 月度用量（已用次数 + 总量 + 百分比） */
  mcp: QuotaLimit | null;
}

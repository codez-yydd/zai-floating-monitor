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

// ===== 多设备同步（与 Rust sync.rs 结构一一对应） =====

export type SyncMode = "manual" | "auto";

/** 同步配置（~/.zbar/sync.json） */
export interface SyncConfig {
  enabled: boolean;
  mode: SyncMode;
  interval_seconds: number;
  server_url: string;
  device_id: string;
  device_name: string;
  device_token: string;
  last_uploaded_rowid: number;
  /** Codex 会话已上传到的 rowid（与 last_uploaded_rowid 同款水位线机制） */
  last_uploaded_codex_rowid: number;
  /** Claude 会话已上传到的 rowid（与上面两个游标相互独立） */
  last_uploaded_claude_rowid: number;
  /** Claude 修订行已补传到的 updated_at 毫秒时间戳（流式终值修正） */
  last_uploaded_claude_rev_ts: number;
  last_uploaded_snapshot_ts: number;
  /** Agent 额度快照已上传到的时间戳游标 */
  last_uploaded_agent_quota_snapshot_ts: number;
  last_sync_at: number;
}

/** 注册请求（UI 填写 server_url + master_token + name） */
export interface RegisterRequest {
  server_url: string;
  master_token: string;
  device_name: string;
}

/** 手动同步结果 */
export interface SyncOutcome {
  uploaded: number;
  new_max_rowid: number;
  last_sync_at: number;
}

/** 设备信息 */
export interface DeviceInfo {
  device_id: string;
  device_name: string;
  created_at: number;
  record_count?: number;
}

/** 远端整体汇总（与本地 OverallStat 字段一致） */
export interface RemoteOverall {
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
}

/** 远端模型分组（与本地 ModelStat 字段一致） */
export interface RemoteModelStat {
  model_id: string;
  provider_id: string;
  /** 数据来源："zcode" | "codex" | "claude"（旧数据无此字段时后端默认 "zcode"） */
  source: string;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
}

/** 远端趋势桶内模型 */
export interface RemoteTrendBucketModel {
  model_id: string;
  provider_id: string;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  total_tokens: number;
}

/** 远端趋势桶（服务端按 UTC 分桶，label 为 ISO 字符串） */
export interface RemoteTrendBucket {
  label: string;
  by_model: RemoteTrendBucketModel[];
  total_tokens: number;
  requests: number;
}

/** /usage 返回的远端聚合 */
export interface RemoteUsage {
  from_ms: number;
  to_ms: number;
  overall: RemoteOverall;
  by_model: RemoteModelStat[];
  trend: RemoteTrendBucket[];
}

/** 清理状态 */
export interface AutoCleanupConfig {
  auto_enabled: boolean;
  auto_days: number;
}

export interface CleanupStatus {
  total_records: number;
  devices: DeviceInfo[];
  auto_config: AutoCleanupConfig;
}

/** 清理执行结果 */
export interface CleanupResult {
  action: string;
  records_deleted: number;
  devices_deleted?: number;
}

/** 设备合并结果 */
export interface MergeResult {
  records_moved: number;
  snapshots_moved: number;
}

/** 设备改名结果 */
export interface RenameResult {
  updated: number;
}

/** 设备筛选选项 */
export type DeviceFilter = "all" | "local" | string; // string 为具体 device_id

// ===== 周额度追踪 / 对比页（与 Rust quota_history.rs 一一对应）=====

/** 单条额度快照（jsonl 一行） */
export interface QuotaSnapshot {
  /** 采样毫秒时间戳 */
  ts: number;
  /** 套餐等级："pro" / "max" ... */
  level: string;
  /** weekly 已用百分比 0-100 */
  weekly_pct: number;
  /** weekly 下次重置时间（毫秒） */
  weekly_reset: number | null;
  /** 5 小时窗口已用百分比 */
  hour5_pct: number;
  /** MCP 月度已用百分比 */
  mcp_pct: number;
  /** MCP 已用次数 */
  mcp_used: number | null;
  /** MCP 总额度次数 */
  mcp_total: number | null;
}

// ===== Agent 额度快照（Codex / Claude / Cursor） =====

export type AgentQuotaSource = "codex" | "claude" | "cursor";

/** Agent 额度窗口键。hour5/weekly 用于 Codex / Claude，cursor_* 用于 Cursor。 */
export type AgentQuotaWindowKey =
  | "hour5"
  | "weekly"
  | "cursor_auto"
  | "cursor_api";

export interface AgentQuotaWindow {
  key: AgentQuotaWindowKey;
  used_pct: number;
  reset_at: number | null;
}

/** Agent 实时额度采样（本地 JSONL 与同步协议共用）。 */
export interface AgentQuotaSnapshot {
  source: AgentQuotaSource;
  ts: number;
  plan_type: string | null;
  windows: AgentQuotaWindow[];
}

/** 带来源设备的远端 Agent 额度采样。 */
export interface RemoteAgentQuotaSnapshot extends AgentQuotaSnapshot {
  device_id: string;
}

/** 某个来源/窗口的今日增量。 */
export interface AgentQuotaDelta {
  pct: number;
  samples: number;
}

export type AgentQuotaDeltaMap = Partial<
  Record<AgentQuotaSource, Partial<Record<AgentQuotaWindowKey, AgentQuotaDelta>>>
>;

/** 一个"智谱重置周期"的汇总 */
export interface WeeklyPeriod {
  /** 周期开始（重置时间） */
  reset_at: number;
  /** 周期结束时间 */
  end_at: number;
  /** 是否当前未结束的周期 */
  is_current: boolean;
  /** 周期内 weekly 起始百分比 */
  pct_start: number;
  /** 周期内 weekly 峰值百分比 */
  pct_peak: number;
  /** 周期内 weekly 结束百分比 */
  pct_end: number;
  /** 周期内采样数 */
  sample_count: number;
}

// ===== 远端额度快照（多设备跨设备支持）=====

/** 远端额度快照（带 device_id，字段与 QuotaSnapshot 对齐 + device_id） */
export interface RemoteSnapshot extends QuotaSnapshot {
  /** 来源设备 id */
  device_id: string;
}

/** 对比页：单个周期的 token 聚合结果 */
export interface WeeklyTokenBucket {
  reset_at: number;
  end_at: number;
  total_tokens: number;
  requests: number;
}

// ===== 价格同步（内置参考表 diff 提示，不自动覆盖）=====

/** 单条价格差异（模型级：以 USD 参考原始价为判定基准） */
export interface PriceDiffItem {
  /** 模型 id */
  model_id: string;
  /** 用户当前 USD 价格（新增模型时为 null） */
  user: ModelPrice | null;
  /** 参考 USD 价格（每百万 token） */
  default: ModelPrice;
  /** 变体名回退匹配时实际命中的参考表模型 id（如 "gpt-5.6-sol" 命中 "gpt-5"），
   *  精确/点号归一命中时为 null */
  reference_id: string | null;
}

/** 完整差异结果 */
export interface PricingDiff {
  /** 内置参考表版本号 */
  version: string;
  /** 新增模型（参考有、用户未配置，应用时写入 USD） */
  new_models: PriceDiffItem[];
  /** USD 价格变动（用户已配但与参考不同） */
  changed: PriceDiffItem[];
  /** 实际在用但无任何价格的模型（花费按 0 计，需手动补价） */
  missing: string[];
}

/** 应用价格更新的单条请求 */
export interface ApplyPriceItem {
  model_id: string;
  currency: Currency;
  price: ModelPrice;
}

// ===== 全局快捷键（~/.zbar/shortcut.json）=====

/** 全局快捷键配置 */
export interface ShortcutConfig {
  enabled: boolean;
  /** Tauri accelerator 格式，如 "alt+shift+z" */
  accelerator: string;
}

// ===== Cursor 用量统计（与 Rust cursor.rs 结构一一对应）=====

/** Cursor 配置（~/.zbar/cursor.json） */
export interface CursorConfig {
  /** cookie 来源："auto"（读 Cursor 应用本地 DB）| "manual"（手动粘贴） */
  cookie_source: "auto" | "manual";
  /** 手动 cookie 头 */
  cookie_header: string;
  /** USD→CNY 汇率（汇总页合并花费用） */
  usd_cny_rate: number;
  /** 是否每日自动联网更新汇率 */
  fx_rate_auto: boolean;
  /** 汇率最近一次联网获取的时间（ms 时间戳，null=从未获取过） */
  fx_rate_fetched_at: number | null;
  /** 汇率最近一次获取成功的来源名（如 "er-api"） */
  fx_rate_source: string | null;
}

/** Cursor 套餐额度信息（金额单位为美分） */
export interface CursorPlanInfo {
  enabled: boolean | null;
  used_cents: number | null;
  limit_cents: number | null;
  remaining_cents: number | null;
  total_pct: number | null;
  auto_pct: number | null;
  api_pct: number | null;
}

/** Cursor 按需用量（金额单位为美分） */
export interface CursorOnDemandInfo {
  enabled: boolean | null;
  used_cents: number | null;
  limit_cents: number | null;
  remaining_cents: number | null;
}

/** Cursor events 聚合汇总 */
export interface CursorEventsSummary {
  /** API 标价总花费（美元） */
  total_cost_usd: number;
  /** 套餐实际扣费（美元），null 表示部分事件缺失 */
  metered_cost_usd: number | null;
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  requests: number;
}

/** Cursor 根据今天 events 扣费换算出的额度增量（百分比）。 */
export interface CursorTodayQuota {
  auto_pct: number | null;
  api_pct: number | null;
}

/** Cursor 每日明细（趋势图用） */
export interface CursorDailyEntry {
  /** 日期标签 "08-13"（MM-DD） */
  date: string;
  cost_usd: number;
  total_tokens: number;
  requests: number;
}

/** Cursor 按模型聚合 */
export interface CursorModelStat {
  model: string;
  cost_usd: number;
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  requests: number;
}

/** Cursor 完整快照（get_cursor_usage 返回） */
export interface CursorSnapshot {
  /** 是否成功登录并获取到数据 */
  logged_in: boolean;
  error: string | null;
  /** events 拉取失败时的错误信息（套餐数据可能仍可用） */
  events_error: string | null;
  account_email: string | null;
  account_name: string | null;
  membership_type: string | null;
  billing_cycle_start: string | null;
  billing_cycle_end: string | null;
  plan: CursorPlanInfo | null;
  on_demand: CursorOnDemandInfo | null;
  events: CursorEventsSummary | null;
  today_quota?: CursorTodayQuota | null;
  daily: CursorDailyEntry[];
  by_model: CursorModelStat[];
}

/** 主面板五标签 */
export type StatsTab = "summary" | "zai" | "codex" | "claude" | "cursor";

// ===== Codex 用量统计（与 Rust codex 模块结构一一对应）=====

/** Codex（OpenAI Codex CLI）速率限制（解析本地 ~/.codex 会话得到） */
export interface CodexRateLimits {
  /** 套餐类型："plus" / "pro" 等 */
  plan_type: string | null;
  /** 5 小时窗口已用百分比（0-100） */
  primary_pct: number | null;
  /** 5 小时窗口重置时间（毫秒时间戳） */
  primary_reset_at: number | null;
  /** 周窗口已用百分比（0-100） */
  secondary_pct: number | null;
  /** 周窗口重置时间（毫秒时间戳） */
  secondary_reset_at: number | null;
}

/** Codex 用量快照（get_codex_usage 返回）。
 *  stats / trend 与 z.ai 的 get_stats / get_trend 同构，展示层可直接复用。 */
export interface CodexSnapshot {
  stats: Stats;
  trend: TrendPoint[];
  /** 速率限制（API 中转等无本地限流数据的模式为 null） */
  rate_limits: CodexRateLimits | null;
}

// ===== Claude 用量统计（与 Rust claude 模块结构一一对应）=====

/** Claude（Anthropic Claude Code）订阅额度（OAuth 实时接口获取） */
export interface ClaudeRateLimits {
  /** 订阅类型："pro" / "max" 等（第三方中转模式为 null） */
  plan_type: string | null;
  /** 5 小时会话窗口已用百分比（0-100） */
  primary_pct: number | null;
  /** 5 小时会话窗口重置时间（毫秒时间戳） */
  primary_reset_at: number | null;
  /** 周窗口已用百分比（0-100） */
  secondary_pct: number | null;
  /** 周窗口重置时间（毫秒时间戳） */
  secondary_reset_at: number | null;
}

/** Claude 用量快照（get_claude_usage 返回）。
 *  stats / trend 与 z.ai 同构；rate_limits 仅订阅登录的机器上有值。 */
export interface ClaudeSnapshot {
  stats: Stats;
  trend: TrendPoint[];
  /** 订阅额度（未登录 claude.ai 订阅 / 第三方中转模式为 null） */
  rate_limits: ClaudeRateLimits | null;
}

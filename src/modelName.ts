// 模型名归一化与用量行折叠。
//
// 背景：整条数据链路（SQLite GROUP BY → 服务端聚合 → 前端合并键）都用原始字符串
// 精确比较，无大小写折叠、无不可见字符清洗；分组键还包含界面不显示的 provider_id，
// 导致「GLM-5.3-flash / GLM-5.3-Flash」「同名 stealth/ox-alpha（provider 或隐藏字符不同）」
// 这类只在字节层面有差异的行在界面上重复展示。
//
// 收口策略：Rust 只读查询与服务端存储保持原始数据不动，前端在 DataCache 派生层
// （含 localStorage 冷启动缓存）与 ReportPanel 自拉数据处统一折叠后再暴露。
// 当前模型徽标、mergeCost 的 per_model 数组内部条目等「单值展示/下游已归一」的场景不在此处理。

import type {
  CursorModelStat,
  ModelPrice,
  ModelStat,
  PricingConfig,
} from "./types";

/**
 * 归一化模型 ID 作为分组键：剔除零宽/方向控制/软连字符等不可见字符，
 * NBSP/全角空格/tab 归一为普通空格，折叠连续空白，最后转小写。
 *
 * 口径边界（与既有实现对齐，勿随手改动）：
 * - 用 toLowerCase 而非 toLocaleLowerCase：与后端 lib.rs cost_for（to_lowercase）、
 *   merge.ts modelCost 兜底、pricing.rs 价格 diff 去重的既有口径一致；
 * - 不做 NFKC（会把全角字母折成半角，合并面失控且后端无此口径）；
 * - 不做点号归一（glm-5.3 ≠ glm-5-3）：点号归一只用于价格表查找域
 *   （cost_for / modelCost / hasPositivePrice），从不用于模型身份判定。
 */
export function canonicalModelId(raw: string): string {
  return raw
    // 零宽空格/连接符、双向控制符、词接符、软连字符、BOM：纯不可见，直接删除
    .replace(/[\u00AD\u200B-\u200F\u202A-\u202E\u2060\uFEFF]/g, "")
    // 各类 Unicode 空白（NBSP、全角空格等）与 tab 归一为普通空格
    .replace(/[\u00A0\u1680\u2000-\u200A\u3000\t]/g, " ")
    .trim()
    .replace(/\s+/g, " ")
    .toLowerCase();
}

/** 价格查找键：归一化名 + 点号换横线。比后端 cost_for 的「to_lowercase + 点号归一」
 * 多剔除了不可见字符（超集口径），仅用于前端展示/计费兜底，多命中的只可能是
 * 身份折叠后本就相同的条目，不会误配。 */
export function priceLookupKey(raw: string): string {
  return canonicalModelId(raw).replace(/\./g, "-");
}

/** 价格表条目查找：精确命中优先，其次按 priceLookupKey 兜底。
 * 供 hasPositivePrice / merge.ts modelCost / PricingPanel 表单显示共用，
 * 保证「⚠ 判定、花费计算、价格编辑」三处读到同一份价格。 */
export function lookupPrice(
  usd: Record<string, ModelPrice>,
  modelId: string
): ModelPrice | undefined {
  const exact = usd[modelId];
  if (exact) return exact;
  const target = priceLookupKey(modelId);
  for (const [k, v] of Object.entries(usd)) {
    if (priceLookupKey(k) === target) return v;
  }
  return undefined;
}

/** 该模型是否在价格表中有有效定价（条目存在且任一价 > 0）。 */
export function hasPositivePrice(
  modelId: string,
  pricing: PricingConfig
): boolean {
  const p = lookupPrice(pricing.usd, modelId);
  return Boolean(p && (p.input > 0 || p.output > 0));
}

/** 合并来源明细：同一分组里被合并的原始写法与请求量（供 tooltip 展示）。 */
export interface MergedModelVariant {
  model_id: string;
  requests: number;
}

/** 折叠后的模型行：比 ModelStat 多一个可选的 variants 明细（仅发生实际合并时携带）。 */
export type FoldedModelStat = ModelStat & { variants?: MergedModelVariant[] };

/**
 * 把仅大小写/空白/不可见字符/provider 差异的模型行折叠为一行。
 * - 数值字段全部求和；
 * - avg_tps 仅对非空行按 output_tokens 加权平均，分母为 0 时回退 null（避免 NaN 打开速度列）；
 * - max_tps 取组内最大；avg_ttft_ms 取代表行值（TTFT 仅本机库有，维持本机样本口径）；
 * - 显示名与 provider 取 total_tokens 最大行的原始写法（并列时取先出现者），保留用户熟悉的形态；
 * - 输出按合并后 total_tokens 降序；Map 保持插入序，结果确定。幂等，可安全重复调用。
 *
 * 注意：分组键刻意不含 provider_id —— 界面只显示 model_id，显示相同的行应合并为一行；
 * 也刻意不跨数据源合并（ZCode/Codex/Claude 的同名模型各自独立折叠，汇总页仍按来源分行展示）。
 */
export function foldModelStatRows(rows: ModelStat[]): FoldedModelStat[] {
  interface Group {
    rep: ModelStat;
    variants: MergedModelVariant[];
    requests: number;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cache_write_tokens: number;
    reasoning_tokens: number;
    total_tokens: number;
    /** avg_tps 加权分子/分母（仅非空行参与） */
    tpsNum: number;
    tpsDen: number;
    maxTps: number | null;
  }
  const groups = new Map<string, Group>();
  for (const m of rows) {
    const key = canonicalModelId(m.model_id);
    let g = groups.get(key);
    if (!g) {
      g = {
        rep: m,
        variants: [],
        requests: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 0,
        tpsNum: 0,
        tpsDen: 0,
        maxTps: null,
      };
      groups.set(key, g);
    }
    g.requests += m.requests;
    g.input_tokens += m.input_tokens;
    g.output_tokens += m.output_tokens;
    g.cache_read_tokens += m.cache_read_tokens;
    g.cache_write_tokens += m.cache_write_tokens;
    g.reasoning_tokens += m.reasoning_tokens;
    g.total_tokens += m.total_tokens;
    // 输入若已是折叠结果则沿用其明细，保证重复折叠结果完全一致（幂等）
    const vs = (m as FoldedModelStat).variants;
    if (vs && vs.length > 0) g.variants.push(...vs);
    else g.variants.push({ model_id: m.model_id, requests: m.requests });
    if (m.total_tokens > g.rep.total_tokens) g.rep = m;
    if (m.avg_tps != null && Number.isFinite(m.avg_tps)) {
      const w = Math.max(0, m.output_tokens);
      g.tpsNum += m.avg_tps * w;
      g.tpsDen += w;
    }
    if (m.max_tps != null && Number.isFinite(m.max_tps)) {
      g.maxTps = g.maxTps == null ? m.max_tps : Math.max(g.maxTps, m.max_tps);
    }
  }
  const out: FoldedModelStat[] = [];
  for (const g of groups.values()) {
    out.push({
      model_id: g.rep.model_id,
      provider_id: g.rep.provider_id,
      requests: g.requests,
      input_tokens: g.input_tokens,
      output_tokens: g.output_tokens,
      cache_read_tokens: g.cache_read_tokens,
      cache_write_tokens: g.cache_write_tokens,
      reasoning_tokens: g.reasoning_tokens,
      total_tokens: g.total_tokens,
      avg_tps: g.tpsDen > 0 ? g.tpsNum / g.tpsDen : null,
      max_tps: g.maxTps,
      avg_ttft_ms: g.rep.avg_ttft_ms ?? null,
      variants: g.variants.length > 1 ? g.variants : undefined,
    });
  }
  return out.sort((a, b) => b.total_tokens - a.total_tokens);
}

/** 对任意含 by_model: ModelStat[] 的统计对象做折叠（null 透传）。
 * 适用 Stats 本体及 CodexSnapshot/ClaudeSnapshot 内嵌的 stats 子对象。 */
export function foldByModelStats<S extends { by_model: ModelStat[] }>(s: S): S;
export function foldByModelStats<S extends { by_model: ModelStat[] }>(
  s: S | null
): S | null;
export function foldByModelStats(s: { by_model: ModelStat[] } | null) {
  if (!s) return s;
  return { ...s, by_model: foldModelStatRows(s.by_model) };
}

/** Cursor 的按模型行折叠：字段集与 ModelStat 不同（名为 model、多 cost_usd、无速度数据）。 */
export function foldCursorModelRows(
  rows: CursorModelStat[]
): (CursorModelStat & { variants?: MergedModelVariant[] })[] {
  interface Group {
    rep: CursorModelStat;
    variants: MergedModelVariant[];
    cost_usd: number;
    total_tokens: number;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    requests: number;
  }
  const groups = new Map<string, Group>();
  for (const m of rows) {
    const key = canonicalModelId(m.model);
    let g = groups.get(key);
    if (!g) {
      g = {
        rep: m,
        variants: [],
        cost_usd: 0,
        total_tokens: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        requests: 0,
      };
      groups.set(key, g);
    }
    g.cost_usd += m.cost_usd;
    g.total_tokens += m.total_tokens;
    g.input_tokens += m.input_tokens;
    g.output_tokens += m.output_tokens;
    g.cache_read_tokens += m.cache_read_tokens;
    g.requests += m.requests;
    // 与 foldModelStatRows 一致：输入已是折叠结果则沿用其明细（幂等）
    const vs = (m as { variants?: MergedModelVariant[] }).variants;
    if (vs && vs.length > 0) g.variants.push(...vs);
    else g.variants.push({ model_id: m.model, requests: m.requests });
    if (m.total_tokens > g.rep.total_tokens) g.rep = m;
  }
  const out: (CursorModelStat & { variants?: MergedModelVariant[] })[] = [];
  for (const g of groups.values()) {
    out.push({
      model: g.rep.model,
      cost_usd: g.cost_usd,
      total_tokens: g.total_tokens,
      input_tokens: g.input_tokens,
      output_tokens: g.output_tokens,
      cache_read_tokens: g.cache_read_tokens,
      requests: g.requests,
      variants: g.variants.length > 1 ? g.variants : undefined,
    });
  }
  return out.sort((a, b) => b.total_tokens - a.total_tokens);
}

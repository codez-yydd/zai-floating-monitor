// 跨设备数据合并的公共工具：本地 stats/cost/trend 与远端 RemoteUsage 合并。
// 抽自 StatsPanel，供 StatsPanel / ReportPanel 共用。

import type {
  CostResult,
  Currency,
  ModelStat,
  OverallStat,
  PricingConfig,
  RemoteUsage,
  Stats,
  TrendBucket,
  TrendPoint,
} from "./types";

/** 单个模型的花费（按 input/output/cache_read 三段计价）。
 * 价格表无该模型 → 返回 0。input 已减去 cache_read 部分（避免重复计费）。 */
export function modelCost(
  modelId: string,
  inputTokens: number,
  outputTokens: number,
  cacheReadTokens: number,
  pricing: PricingConfig,
  currency: Currency
): number {
  const map = pricing[currency];
  const p = map[modelId];
  if (!p) return 0;
  const nonCacheInput = Math.max(0, inputTokens - cacheReadTokens);
  return (
    (nonCacheInput * p.input +
      outputTokens * p.output +
      cacheReadTokens * p.cache_read) /
    1_000_000
  );
}

/** 把远端 RemoteUsage 转成 Stats 结构（仅远端时用） */
export function remoteToStats(r: RemoteUsage): Stats {
  return {
    from_ms: r.from_ms,
    to_ms: r.to_ms,
    overall: {
      requests: r.overall.requests,
      input_tokens: r.overall.input_tokens,
      output_tokens: r.overall.output_tokens,
      cache_read_tokens: r.overall.cache_read_tokens,
      cache_write_tokens: r.overall.cache_write_tokens,
      reasoning_tokens: r.overall.reasoning_tokens,
      total_tokens: r.overall.total_tokens,
    },
    by_model: r.by_model.map((m) => ({
      model_id: m.model_id,
      provider_id: m.provider_id,
      requests: m.requests,
      input_tokens: m.input_tokens,
      output_tokens: m.output_tokens,
      cache_read_tokens: m.cache_read_tokens,
      cache_write_tokens: m.cache_write_tokens,
      reasoning_tokens: m.reasoning_tokens,
      total_tokens: m.total_tokens,
    })),
    earliest_ms: null,
    latest_ms: null,
  };
}

/** 仅远端时算花费（远端不含 cost，前端用 pricing 自算；双货币一次算齐） */
export function computeRemoteCost(
  r: RemoteUsage,
  pricing: PricingConfig
): CostResult {
  const perModel = (currency: Currency) =>
    r.by_model.map((m) => ({
      model_id: m.model_id,
      cost: modelCost(
        m.model_id,
        m.input_tokens,
        m.output_tokens,
        m.cache_read_tokens,
        pricing,
        currency
      ),
    }));
  const cny = perModel("cny");
  const usd = perModel("usd");
  return {
    total_cny: cny.reduce((s, x) => s + x.cost, 0),
    total_usd: usd.reduce((s, x) => s + x.cost, 0),
    per_model_cny: cny,
    per_model_usd: usd,
  };
}

/** 合并本地 stats + 远端 usage → 汇总 stats */
export function mergeStats(local: Stats, remote: RemoteUsage): Stats {
  const addOverall = (a: OverallStat, b: RemoteUsage["overall"]): OverallStat => ({
    requests: a.requests + b.requests,
    input_tokens: a.input_tokens + b.input_tokens,
    output_tokens: a.output_tokens + b.output_tokens,
    cache_read_tokens: a.cache_read_tokens + b.cache_read_tokens,
    cache_write_tokens: a.cache_write_tokens + b.cache_write_tokens,
    reasoning_tokens: a.reasoning_tokens + b.reasoning_tokens,
    total_tokens: a.total_tokens + b.total_tokens,
  });

  // by_model 按 model_id+provider_id 合并相加
  const key = (m: { model_id: string; provider_id: string }) =>
    `${m.provider_id}|${m.model_id}`;
  const merged = new Map<string, ModelStat>();
  for (const m of local.by_model) {
    merged.set(key(m), { ...m });
  }
  for (const m of remote.by_model) {
    const k = key(m);
    const ex = merged.get(k);
    if (ex) {
      ex.requests += m.requests;
      ex.input_tokens += m.input_tokens;
      ex.output_tokens += m.output_tokens;
      ex.cache_read_tokens += m.cache_read_tokens;
      ex.cache_write_tokens += m.cache_write_tokens;
      ex.reasoning_tokens += m.reasoning_tokens;
      ex.total_tokens += m.total_tokens;
    } else {
      merged.set(k, {
        model_id: m.model_id,
        provider_id: m.provider_id,
        requests: m.requests,
        input_tokens: m.input_tokens,
        output_tokens: m.output_tokens,
        cache_read_tokens: m.cache_read_tokens,
        cache_write_tokens: m.cache_write_tokens,
        reasoning_tokens: m.reasoning_tokens,
        total_tokens: m.total_tokens,
      });
    }
  }
  // 按 total_tokens 降序
  const by_model = Array.from(merged.values()).sort(
    (a, b) => b.total_tokens - a.total_tokens
  );

  return {
    from_ms: local.from_ms,
    to_ms: local.to_ms,
    overall: addOverall(local.overall, remote.overall),
    by_model,
    earliest_ms: local.earliest_ms,
    latest_ms: local.latest_ms,
  };
}

/** 合并花费：本地 cost + 远端（用 pricing 自算；双货币一次算齐） */
export function mergeCost(
  local: CostResult | null,
  remote: RemoteUsage,
  pricing: PricingConfig
): CostResult {
  const base = local ?? {
    total_cny: 0,
    total_usd: 0,
    per_model_cny: [],
    per_model_usd: [],
  };
  const cnyExtra = remote.by_model.map((m) => ({
    model_id: m.model_id,
    cost: modelCost(
      m.model_id,
      m.input_tokens,
      m.output_tokens,
      m.cache_read_tokens,
      pricing,
      "cny"
    ),
  }));
  const usdExtra = remote.by_model.map((m) => ({
    model_id: m.model_id,
    cost: modelCost(
      m.model_id,
      m.input_tokens,
      m.output_tokens,
      m.cache_read_tokens,
      pricing,
      "usd"
    ),
  }));
  return {
    total_cny:
      base.total_cny + cnyExtra.reduce((s, x) => s + x.cost, 0),
    total_usd:
      base.total_usd + usdExtra.reduce((s, x) => s + x.cost, 0),
    per_model_cny: [...base.per_model_cny, ...cnyExtra],
    per_model_usd: [...base.per_model_usd, ...usdExtra],
  };
}

/** 把远端桶的 ms label 转成本地时区 label，与本地 trend 的 label 对齐。
 * - hour 桶：本地用 "HH:00"，远端 ms 同样格式化为本地时区 "HH:00"
 * - day 桶：本地用 "MM-DD"，远端 ms 格式化为本地时区 "MM-DD"
 * 关键：本地 hour 桶和服务端 hour 桶都用 UTC 整点对齐，ms 一致，
 * 格式化后 label 也一致，可精确匹配。day 桶本地按本地0点、服务端按UTC0点，
 * 在非 UTC 时区可能错位——这是已知的可接受偏差（长周期趋势影响小）。 */
export function msToLocalLabel(msStr: string, bucket: TrendBucket): string | null {
  const ms = parseInt(msStr, 10);
  if (isNaN(ms)) return null;
  const d = new Date(ms);
  if (bucket === "hour") {
    const hh = String(d.getHours()).padStart(2, "0");
    return `${hh}:00`;
  }
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}

/** 远端趋势桶 → 本地 TrendPoint 格式（远端无 cost，自算）。
 * label 由 ms 转成本地时区格式，便于与本地 trend 按 label 合并。 */
export function remoteTrendToLocal(
  remote: RemoteUsage,
  pricing: PricingConfig,
  bucket: TrendBucket
): TrendPoint[] {
  return remote.trend
    .map((b) => {
      const label = msToLocalLabel(b.label, bucket);
      if (label === null) return null;
      const cost_cny = b.by_model.reduce(
        (s, m) =>
          s +
          modelCost(
            m.model_id,
            m.input_tokens,
            m.output_tokens,
            m.cache_read_tokens,
            pricing,
            "cny"
          ),
        0
      );
      const cost_usd = b.by_model.reduce(
        (s, m) =>
          s +
          modelCost(
            m.model_id,
            m.input_tokens,
            m.output_tokens,
            m.cache_read_tokens,
            pricing,
            "usd"
          ),
        0
      );
      return {
        label,
        total_tokens: b.total_tokens,
        requests: b.requests,
        cost_cny,
        cost_usd,
      };
    })
    .filter((x): x is TrendPoint => x !== null);
}

/** 合并本地趋势 + 远端趋势：按 label 匹配相加，保持本地顺序 */
export function mergeTrend(
  local: TrendPoint[],
  remote: RemoteUsage,
  pricing: PricingConfig,
  bucket: TrendBucket
): TrendPoint[] {
  const remotePts = remoteTrendToLocal(remote, pricing, bucket);
  // 远端按 label 建索引
  const remoteMap = new Map<string, TrendPoint>();
  for (const r of remotePts) {
    remoteMap.set(r.label, r);
  }
  // 本地顺序为主，合并远端同 label 桶；远端多出的桶追加到末尾
  const usedLabels = new Set<string>();
  const out: TrendPoint[] = local.map((l) => {
    usedLabels.add(l.label);
    const r = remoteMap.get(l.label);
    return {
      label: l.label,
      total_tokens: l.total_tokens + (r?.total_tokens ?? 0),
      requests: l.requests + (r?.requests ?? 0),
      cost_cny: l.cost_cny + (r?.cost_cny ?? 0),
      cost_usd: l.cost_usd + (r?.cost_usd ?? 0),
    };
  });
  for (const r of remotePts) {
    if (!usedLabels.has(r.label)) {
      out.push(r);
    }
  }
  return out;
}

//
//  Billing.swift
//  ZBar
//
//  翻译自 src/merge.ts。计费 + 合并逻辑，必须与桌面端 1:1。
//

import Foundation

public enum Billing {
    /// 单个模型的花费（按 input/output/cache_read 三段计价）。
    /// 价格表无该模型 → 返回 0。input 已减去 cache_read 部分（避免重复计费）。
    /// 对应 merge.ts::modelCost
    public static func modelCost(modelId: String,
                                  inputTokens: Int,
                                  outputTokens: Int,
                                  cacheReadTokens: Int,
                                  pricing: PricingConfig,
                                  currency: Currency) -> Double {
        let map = (currency == .cny) ? pricing.cny : pricing.usd
        guard let p = map[modelId] else { return 0 }
        let nonCacheInput = max(0, inputTokens - cacheReadTokens)
        return (Double(nonCacheInput) * p.input
                + Double(outputTokens) * p.output
                + Double(cacheReadTokens) * p.cache_read) / 1_000_000
    }

    /// 把远端 RemoteUsage 转成 Stats 结构（仅远端时用）。
    /// 对应 merge.ts::remoteToStats
    public static func remoteToStats(_ r: RemoteUsage) -> Stats {
        Stats(from_ms: r.from_ms,
              to_ms: r.to_ms,
              overall: r.overall,
              by_model: r.by_model,
              earliest_ms: nil,
              latest_ms: nil)
    }

    /// 仅远端时算花费（远端不含 cost，前端用 pricing 自算）。
    /// 对应 merge.ts::computeRemoteCost
    public static func computeRemoteCost(_ r: RemoteUsage,
                                          pricing: PricingConfig) -> CostResult {
        func perModel(_ cur: Currency) -> [ModelCost] {
            r.by_model.map { m in
                ModelCost(model_id: m.model_id,
                          cost: modelCost(modelId: m.model_id,
                                          inputTokens: m.input_tokens,
                                          outputTokens: m.output_tokens,
                                          cacheReadTokens: m.cache_read_tokens,
                                          pricing: pricing,
                                          currency: cur))
            }
        }
        let cny = perModel(.cny)
        let usd = perModel(.usd)
        return CostResult(total_cny: cny.reduce(0) { $0 + $1.cost },
                          total_usd: usd.reduce(0) { $0 + $1.cost },
                          per_model_cny: cny,
                          per_model_usd: usd)
    }

    /// 合并本地 stats + 远端 usage → 汇总 stats。
    /// iOS 端没有本地数据，本函数保留供"本地（占位）+远端"扩展用。
    /// 对应 merge.ts::mergeStats
    public static func mergeStats(local: Stats, remote: RemoteUsage) -> Stats {
        var overall = local.overall
        overall.requests += remote.overall.requests
        overall.input_tokens += remote.overall.input_tokens
        overall.output_tokens += remote.overall.output_tokens
        overall.cache_read_tokens += remote.overall.cache_read_tokens
        overall.cache_write_tokens += remote.overall.cache_write_tokens
        overall.reasoning_tokens += remote.overall.reasoning_tokens
        overall.total_tokens += remote.overall.total_tokens

        // by_model 按 model_id+provider_id 合并相加
        var merged: [String: ModelStat] = [:]
        for m in local.by_model {
            merged["\(m.provider_id)|\(m.model_id)"] = m
        }
        for m in remote.by_model {
            let k = "\(m.provider_id)|\(m.model_id)"
            if var ex = merged[k] {
                ex.requests += m.requests
                ex.input_tokens += m.input_tokens
                ex.output_tokens += m.output_tokens
                ex.cache_read_tokens += m.cache_read_tokens
                ex.cache_write_tokens += m.cache_write_tokens
                ex.reasoning_tokens += m.reasoning_tokens
                ex.total_tokens += m.total_tokens
                merged[k] = ex
            } else {
                merged[k] = m
            }
        }
        let byModel = merged.values.sorted { $0.total_tokens > $1.total_tokens }

        return Stats(from_ms: local.from_ms,
                     to_ms: local.to_ms,
                     overall: overall,
                     by_model: byModel,
                     earliest_ms: local.earliest_ms,
                     latest_ms: local.latest_ms)
    }

    /// 远端趋势桶 → 本地 TrendPoint 格式（远端无 cost，自算）。
    /// label 由 ms 转成本地时区格式，便于按 label 合并。
    /// 对应 merge.ts::remoteTrendToLocal
    public static func remoteTrendToLocal(_ remote: RemoteUsage,
                                           pricing: PricingConfig,
                                           bucket: TrendBucket) -> [TrendPoint] {
        remote.trend.compactMap { b -> TrendPoint? in
            // label 可能是毫秒数字符串
            guard let ms = Int(b.label) else { return nil }
            guard let label = Format.msToLocalLabel(ms, bucket: bucket) else { return nil }
            let costCny = b.by_model.reduce(0.0) { acc, m in
                acc + modelCost(modelId: m.model_id,
                                inputTokens: m.input_tokens,
                                outputTokens: m.output_tokens,
                                cacheReadTokens: m.cache_read_tokens,
                                pricing: pricing,
                                currency: .cny)
            }
            let costUsd = b.by_model.reduce(0.0) { acc, m in
                acc + modelCost(modelId: m.model_id,
                                inputTokens: m.input_tokens,
                                outputTokens: m.output_tokens,
                                cacheReadTokens: m.cache_read_tokens,
                                pricing: pricing,
                                currency: .usd)
            }
            return TrendPoint(label: label,
                              total_tokens: b.total_tokens,
                              requests: b.requests,
                              cost_cny: costCny,
                              cost_usd: costUsd)
        }
    }
}

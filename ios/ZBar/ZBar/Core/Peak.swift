//
//  Peak.swift
//  ZBar
//
//  翻译自 src/peak.ts（前端复刻 src-tauri/src/peak.rs）。
//  高峰期倍率折算：把远端逐条明细折算成消耗（服务端无 peak 配置，折算在客户端做）。
//

import Foundation

public enum Peak {
    /// V3 积分系数表（与 Rust credit_coef / TS CREDIT_COEF 一致）。key = model_id（小写）。
    private static let CREDIT_COEF: [String: (input: Double, cache: Double, output: Double)] = [
        "glm-5.2": (input: 6.9, cache: 1.7, output: 24.0),
        "glm-5-turbo": (input: 5.7, cache: 1.5, output: 21.0),
        "glm-4.7": (input: 4.6, cache: 1.2, output: 16.0),
        "glm-4.6v": (input: 1.2, cache: 0.3, output: 2.7),
    ]

    /// ZCode 优惠系数（与 Rust ZCODE_DISCOUNT 一致）
    private static let ZCODE_DISCOUNT = 0.67

    /// "HH:MM" → 当天分钟数 (0-1439)，非法返回 nil
    private static func parseHhmm(_ s: String) -> Int? {
        let parts = s.split(separator: ":")
        guard parts.count == 2,
              let h = Int(parts[0]), let m = Int(parts[1]),
              h <= 23, m <= 59 else { return nil }
        return h * 60 + m
    }

    /// ms → (weekday_bit, 当天分钟数)。
    /// weekday_bit: 0=周日 ... 6=周六（与 Rust peak.rs / TS 一致，即 Calendar 的 weekday：1=周日→bit0）
    /// 注意：iOS Calendar.weekday 是 1=周日…7=周六，转 0 基。
    private static func msToLocal(_ ms: Int) -> (weekdayBit: Int, nowMin: Int)? {
        let d = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        let cal = Calendar.current
        let comps = cal.dateComponents([.weekday, .hour, .minute], from: d)
        guard let weekday = comps.weekday, let h = comps.hour, let m = comps.minute else { return nil }
        let weekdayBit = weekday - 1   // 1=周日 → 0
        return (weekdayBit, h * 60 + m)
    }

    /// 判断 ms 落在哪个时段，返回对应倍率（未启用/未匹配 → 1.0）。
    /// 与 Rust multiplier_at 一致。
    public static func multiplierAt(_ ms: Int, cfg: PeakConfig) -> Double {
        guard cfg.enabled, !cfg.segments.isEmpty else { return 1.0 }
        guard let (weekdayBit, nowMin) = msToLocal(ms) else { return 1.0 }
        for seg in cfg.segments {
            // bit(weekdayBit) 是否在 mask 中
            if ((seg.weekday_mask >> weekdayBit) & 1) != 1 { continue }
            guard let start = parseHhmm(seg.start), let end = parseHhmm(seg.end) else { continue }
            if end > start, nowMin >= start, nowMin < end {
                return seg.multiplier
            }
        }
        return 1.0
    }

    /// ZCode 优惠系数（启用 → 0.67，否则 1.0）
    public static func zcodeFactor(_ cfg: PeakConfig) -> Double {
        cfg.zcode_discount ? ZCODE_DISCOUNT : 1.0
    }

    /// V3 单条调用的积分消耗（含时段倍率，不含 ZCode 优惠）。
    /// 模型无系数返回 nil。与 Rust credits_for_call 一致。
    public static func creditsForCall(modelId: String,
                                       inputTokens: Int,
                                       cacheReadTokens: Int,
                                       outputTokens: Int,
                                       multiplier: Double) -> Double? {
        guard let c = CREDIT_COEF[modelId.lowercased()] else { return nil }
        let base = (Double(inputTokens) * c.input
                    + Double(cacheReadTokens) * c.cache
                    + Double(outputTokens) * c.output) / 10_000
        return base * multiplier
    }

    /// 把一组周期内的远端明细折算成消耗，与本地 db::query_period_consumed 口径一致。
    /// - V2：Σ total_tokens × 时段倍率
    /// - V3：Σ 积分公式 × 时段倍率（无系数模型按 0 计）
    /// - 都 × ZCode 优惠系数
    /// 返回每个周期的 (consumed, requests)，顺序与 periods 一致。
    public static func detailToConsumed(_ detail: [RemotePeriodDetail],
                                         cfg: PeakConfig) -> [(consumed: Double, requests: Int)] {
        let zcode = zcodeFactor(cfg)
        let plan = cfg.plan_type
        return detail.map { bucket in
            var consumed: Double = 0
            var requests = 0
            for r in bucket.rows {
                let mult = multiplierAt(r.started_at, cfg: cfg)
                var callConsumed: Double = 0
                if plan == .v3 {
                    callConsumed = creditsForCall(modelId: r.model_id,
                                                  inputTokens: r.input_tokens,
                                                  cacheReadTokens: r.cache_read_tokens,
                                                  outputTokens: r.output_tokens,
                                                  multiplier: mult) ?? 0
                } else if plan == .v2 {
                    callConsumed = Double(r.total_tokens) * mult
                }
                consumed += callConsumed * zcode
                requests += 1
            }
            return (consumed, requests)
        }
    }
}

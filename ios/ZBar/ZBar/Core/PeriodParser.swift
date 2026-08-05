//
//  PeriodParser.swift
//  ZBar
//
//  翻译自 src-tauri/src/quota_history.rs::split_periods。
//  把额度快照序列按 weekly_reset 跳变点切成多个"智谱重置周期"。
//

import Foundation

public enum PeriodParser {
    /// 把快照序列切成多个周期。
    public static func splitPeriods(_ snaps: [QuotaSnapshot],
                                     nowMs: Int = Int(Date().timeIntervalSince1970 * 1000))
    -> [WeeklyPeriod] {
        guard !snaps.isEmpty else { return [] }
        var periods: [WeeklyPeriod] = []
        var startIdx = 0
        var curStart = snaps[0].ts

        for i in 1..<snaps.count {
            let prev = snaps[i - 1].weekly_reset
            let cur = snaps[i].weekly_reset
            // 跳变：cur 比 prev 大 >= 1 天 → 发生了重置
            let jumped: Bool
            if let p = prev, let c = cur {
                jumped = c > p + 86_400_000
            } else {
                jumped = false
            }
            if jumped {
                let prevEnd = snaps[i - 1].weekly_reset ?? snaps[i - 1].ts
                periods.append(buildPeriod(Array(snaps[startIdx..<i]),
                                            resetAt: curStart,
                                            nextReset: prevEnd,
                                            isCurrent: false,
                                            nowMs: nowMs))
                startIdx = i
                curStart = snaps[i].ts
            }
        }
        let last = snaps[snaps.count - 1]
        periods.append(buildPeriod(Array(snaps[startIdx...]),
                                    resetAt: curStart,
                                    nextReset: last.weekly_reset ?? last.ts,
                                    isCurrent: true,
                                    nowMs: nowMs))
        return periods
    }

    private static func buildPeriod(_ snaps: [QuotaSnapshot],
                                     resetAt: Int,
                                     nextReset: Int,
                                     isCurrent: Bool,
                                     nowMs: Int) -> WeeklyPeriod {
        let pctStart = snaps.first?.weekly_pct ?? 0
        let pctEnd = snaps.last?.weekly_pct ?? 0
        let pctPeak = snaps.map { $0.weekly_pct }.max() ?? 0
        let endAt = isCurrent ? nowMs : nextReset
        return WeeklyPeriod(reset_at: resetAt,
                            end_at: endAt,
                            is_current: isCurrent,
                            pct_start: pctStart,
                            pct_peak: pctPeak,
                            pct_end: pctEnd,
                            sample_count: snaps.count)
    }
}

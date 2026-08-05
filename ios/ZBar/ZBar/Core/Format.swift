//
//  Format.swift
//  ZBar
//
//  翻译自 src/format.ts。纯计算，必须与桌面端 1:1 一致。
//

import Foundation

public enum Format {
    /// 格式化 token 数量：3.7M / 1280 / 1.2B
    public static func tokens(_ n: Int) -> String {
        let abs = Double(n)
        if abs >= 1_000_000_000 { return String(format: "%.2fB", abs / 1_000_000_000) }
        if abs >= 1_000_000 { return String(format: "%.2fM", abs / 1_000_000) }
        if abs >= 1_000 { return String(format: "%.1fK", abs / 1_000) }
        return String(n)
    }

    /// 格式化金额（按货币加符号）
    public static func cost(_ n: Double, _ currency: Currency) -> String {
        let sym = currency == .cny ? "¥" : "$"
        if n == 0 { return "\(sym)0.00" }
        if n < 0.01 { return "\(sym)\(String(format: "%.4f", n))" }
        return "\(sym)\(String(format: "%.2f", n))"
    }

    /// 格式化百分比（0-1 → "12.3%"）
    public static func pct(_ fraction: Double) -> String {
        if !fraction.isFinite { return "—" }
        return String(format: "%.1f%%", fraction * 100)
    }

    /// 百分比 0-100 → "12.3%"
    public static func pctFromInt(_ p: Int) -> String {
        String(format: "%.1f%%", Double(p))
    }

    /// 时间范围预设 → [from_ms, to_ms]
    /// custom 的 from/to 格式为 "yyyy-MM-dd"
    public static func rangeToMs(preset: RangePreset,
                                  now: Date = Date(),
                                  custom: (from: String, to: String)? = nil) -> (Int, Int) {
        let nowMs = Int(now.timeIntervalSince1970 * 1000)
        switch preset {
        case .today:
            let cal = Calendar.current
            let start = cal.startOfDay(for: now)
            return (Int(start.timeIntervalSince1970 * 1000), nowMs)
        case .d1:
            return (nowMs - 86_400_000, nowMs)
        case .d7:
            return (nowMs - 7 * 86_400_000, nowMs)
        case .d30:
            return (nowMs - 30 * 86_400_000, nowMs)
        case .custom:
            guard let c = custom else { return (nowMs - 86_400_000, nowMs) }
            let f = dateFromYMD(c.from)?.setting(hour: 0, min: 0, sec: 0)
            let t = dateFromYMD(c.to)?.setting(hour: 23, min: 59, sec: 59)
            guard let from = f, let to = t else { return (nowMs - 86_400_000, nowMs) }
            return (Int(from.timeIntervalSince1970 * 1000), Int(to.timeIntervalSince1970 * 1000))
        }
    }

    /// 格式化日期为 yyyy-MM-dd
    public static func dateStr(_ ms: Int) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd"
        fmt.timeZone = .current
        return fmt.string(from: d)
    }

    /// 解析 "yyyy-MM-dd"
    public static func dateFromYMD(_ s: String) -> Date? {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd"
        fmt.timeZone = .current
        return fmt.date(from: s)
    }

    /// 毫秒 → "HH:00"（hour 桶）或 "MM-dd"（day 桶）
    public static func msToLocalLabel(_ ms: Int, bucket: TrendBucket) -> String? {
        let d = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        let cal = Calendar.current
        let comps = cal.dateComponents([.month, .day, .hour], from: d)
        switch bucket {
        case .hour:
            guard let h = comps.hour else { return nil }
            return String(format: "%02d:00", h)
        case .day:
            guard let m = comps.month, let day = comps.day else { return nil }
            return String(format: "%02d-%02d", m, day)
        }
    }

    /// 把毫秒时间戳格式化为友好显示（用于额度重置倒计时）
    public static func countdown(to resetMs: Int?, now: Date = Date()) -> String {
        guard let r = resetMs else { return "—" }
        let diff = TimeInterval(r) / 1000 - now.timeIntervalSince1970
        if diff <= 0 { return "即将重置" }
        let h = Int(diff) / 3600
        let m = (Int(diff) % 3600) / 60
        if h > 24 {
            return "\(h / 24)天\(h % 24)小时后"
        }
        if h > 0 { return "\(h)小时\(m)分后" }
        return "\(m)分钟后"
    }
}

extension Date {
    /// 设置时分秒
    func setting(hour: Int, min: Int, sec: Int) -> Date? {
        let cal = Calendar.current
        return cal.date(bySettingHour: hour, minute: min, second: sec, of: self)
    }
}

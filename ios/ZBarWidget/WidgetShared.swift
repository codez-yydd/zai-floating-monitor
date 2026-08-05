//
//  WidgetShared.swift
//  ZBarWidget
//
//  ⚠️ 本文件【只加入 Widget target】，不要加入主 App target。
//  主 App 的对应类型在 Settings.swift / Format.swift / Types.swift 中定义，
//  字段与本文件保持一致，通过 App Group 的 JSON 序列化互通。
//
//  如果两个 target 都包含本文件，会出现"重复定义"编译错误。
//

import Foundation

/// Widget 专用：从 App Group 读取的数据快照（字段与主 App 的 WidgetSnapshot 对齐）。
public struct WidgetSnapshot: Codable, Hashable {
    public var todayCostCny: Double
    public var todayCostUsd: Double
    public var todayTokens: Int
    public var todayRequests: Int
    public var hour5Pct: Int?
    public var weeklyPct: Int?
    public var mcpPct: Int?
    public var mcpUsed: Int?
    public var mcpTotal: Int?
    public var weeklyResetMs: Int?
    public var hour5ResetMs: Int?
    public var level: String
    public var deviceName: String
    public var updatedAt: Double

    public static let placeholder = WidgetSnapshot(
        todayCostCny: 12.34, todayCostUsd: 1.73,
        todayTokens: 370000, todayRequests: 42,
        hour5Pct: 35, weeklyPct: 58, mcpPct: nil,
        mcpUsed: nil, mcpTotal: nil,
        weeklyResetMs: Int(Date().addingTimeInterval(2*24*3600).timeIntervalSince1970 * 1000),
        hour5ResetMs: Int(Date().addingTimeInterval(3*3600).timeIntervalSince1970 * 1000),
        level: "PRO", deviceName: "iPhone", updatedAt: Date().timeIntervalSince1970)

    public static let preview = placeholder

    /// 数据为空时的占位
    public static let empty = WidgetSnapshot(
        todayCostCny: 0, todayCostUsd: 0, todayTokens: 0, todayRequests: 0,
        hour5Pct: nil, weeklyPct: nil, mcpPct: nil, mcpUsed: nil, mcpTotal: nil,
        weeklyResetMs: nil, hour5ResetMs: nil, level: "", deviceName: "",
        updatedAt: 0)
}

public enum Currency: String, Codable {
    case cny, usd
}

/// Widget 端精简格式化（避免引入主 App 的 Format.swift 全部代码）。
/// 字段含义与主 App Format 一致。
public enum Format {
    public static func tokens(_ n: Int) -> String {
        let abs = Double(n)
        if abs >= 1_000_000_000 { return String(format: "%.2fB", abs / 1_000_000_000) }
        if abs >= 1_000_000 { return String(format: "%.2fM", abs / 1_000_000) }
        if abs >= 1_000 { return String(format: "%.1fK", abs / 1_000) }
        return String(n)
    }
    public static func cost(_ n: Double, _ currency: Currency) -> String {
        let sym = currency == .cny ? "¥" : "$"
        if n == 0 { return "\(sym)0.00" }
        if n < 0.01 { return "\(sym)\(String(format: "%.4f", n))" }
        return "\(sym)\(String(format: "%.2f", n))"
    }
    public static func countdown(to resetMs: Int?, now: Date = Date()) -> String {
        guard let r = resetMs else { return "—" }
        let diff = TimeInterval(r) / 1000 - now.timeIntervalSince1970
        if diff <= 0 { return "即将重置" }
        let h = Int(diff) / 3600
        let m = (Int(diff) % 3600) / 60
        if h > 24 { return "\(h/24)d\(h%24)h" }
        if h > 0 { return "\(h)h\(m)m" }
        return "\(m)m"
    }
}

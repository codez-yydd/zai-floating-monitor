//
//  Types.swift
//  ZBar
//
//  1:1 翻译自 src/types.ts + src-tauri/src/quota.rs + server/db.py 的数据结构。
//  字段名严格对齐服务端 JSON（snake_case），用 CodingKeys 保证解码正确。
//

import Foundation

// MARK: - 统计（对应 Stats / OverallStat / ModelStat）

public struct OverallStat: Codable, Hashable {
    public var requests: Int = 0
    public var input_tokens: Int = 0
    public var output_tokens: Int = 0
    public var cache_read_tokens: Int = 0
    public var cache_write_tokens: Int = 0
    public var reasoning_tokens: Int = 0
    public var total_tokens: Int = 0
}

public struct ModelStat: Codable, Hashable, Identifiable {
    public var model_id: String
    public var provider_id: String
    public var requests: Int
    public var input_tokens: Int
    public var output_tokens: Int
    public var cache_read_tokens: Int
    public var cache_write_tokens: Int
    public var reasoning_tokens: Int
    public var total_tokens: Int

    // Identifiable 用，避免 SwiftUI ForEach 警告
    public var id: String { "\(provider_id)|\(model_id)" }
}

public struct Stats: Codable, Hashable {
    public var from_ms: Int
    public var to_ms: Int
    public var overall: OverallStat
    public var by_model: [ModelStat]
    public var earliest_ms: Int?
    public var latest_ms: Int?
}

// MARK: - 价格（对应 PricingConfig / ModelPrice / PricingDefaults）

public struct ModelPrice: Codable, Hashable {
    public var input: Double
    public var output: Double
    public var cache_read: Double

    public init(input: Double = 0, output: Double = 0, cache_read: Double = 0) {
        self.input = input
        self.output = output
        self.cache_read = cache_read
    }
}

public struct PricingConfig: Codable, Hashable {
    public var cny: [String: ModelPrice]
    public var usd: [String: ModelPrice]

    public init(cny: [String: ModelPrice] = [:], usd: [String: ModelPrice] = [:]) {
        self.cny = cny
        self.usd = usd
    }
}

/// 内置默认价格表（public/pricing-defaults.json 的结构）
public struct PricingDefaults: Codable {
    public var version: String
    public var cny: [String: ModelPrice]
    public var usd: [String: ModelPrice]
}

public enum Currency: String, Codable, CaseIterable {
    case cny, usd
}

// MARK: - 花费结果（对应 CostResult）

public struct ModelCost: Codable, Hashable, Identifiable {
    public var model_id: String
    public var cost: Double
    public var id: String { model_id }
}

public struct CostResult: Codable, Hashable {
    public var total_cny: Double
    public var total_usd: Double
    public var per_model_cny: [ModelCost]
    public var per_model_usd: [ModelCost]
}

// MARK: - 趋势（对应 TrendPoint / TrendBucket）

public enum TrendBucket: String, Codable, CaseIterable {
    case hour, day
}

public struct TrendPoint: Codable, Hashable, Identifiable {
    public var label: String
    public var total_tokens: Int
    public var requests: Int
    public var cost_cny: Double
    public var cost_usd: Double

    public var id: String { label }
}

// MARK: - 远端用量聚合（对应 RemoteUsage / RemoteOverall / RemoteModelStat / RemoteTrendBucket）

public struct RemoteTrendBucketModel: Codable, Hashable {
    public var model_id: String
    public var provider_id: String
    public var requests: Int
    public var input_tokens: Int
    public var output_tokens: Int
    public var cache_read_tokens: Int
    public var total_tokens: Int
}

public struct RemoteTrendBucket: Codable, Hashable {
    /// 服务端按 UTC 分桶，label 是 ISO 字符串或毫秒字符串
    public var label: String
    public var by_model: [RemoteTrendBucketModel]
    public var total_tokens: Int
    public var requests: Int
}

public struct RemoteUsage: Codable, Hashable {
    public var from_ms: Int
    public var to_ms: Int
    public var overall: OverallStat
    public var by_model: [ModelStat]
    public var trend: [RemoteTrendBucket]
}

// MARK: - 额度（对应 QuotaResult / QuotaLimit，字段驼峰对齐智谱接口）

public struct McpUsageDetail: Codable, Hashable {
    // 注意：智谱接口 usageDetails[] 里字段就是 snake_case（与 Rust 对齐，未做 rename）
    public var model_code: String
    public var usage: Int
}

public struct QuotaLimit: Codable, Hashable {
    public var type: String                 // "TOKENS_LIMIT" | "TIME_LIMIT"
    public var unit: Int?                   // 3=小时, 6=周
    public var number: Int?                 // 5=5小时, 1=每周
    public var percentage: Int              // 0-100
    public var nextResetTime: Int?          // 毫秒时间戳
    public var currentValue: Int?           // MCP 已用次数
    public var usage: Int?                  // MCP 总额度次数（注意：不是 total）
    public var usageDetails: [McpUsageDetail]?
}

public struct QuotaResult: Codable, Hashable {
    public var level: String
    public var hour5: QuotaLimit?
    public var weekly: QuotaLimit?
    public var mcp: QuotaLimit?
}

public enum QuotaEndpoint: String, Codable, CaseIterable {
    case cn, global

    public var base: String {
        switch self {
        case .cn: return "https://open.bigmodel.cn"
        case .global: return "https://api.z.ai"
        }
    }
}

/// 额度查询配置（对应 quota.json）
public struct QuotaConfig: Codable, Hashable {
    public var token: String
    public var endpoint: QuotaEndpoint

    public init(token: String = "", endpoint: QuotaEndpoint = .cn) {
        self.token = token
        self.endpoint = endpoint
    }
}

// MARK: - 同步配置（对应 sync.json）

public enum SyncMode: String, Codable {
    case manual, auto
}

public struct SyncConfig: Codable, Hashable {
    public var enabled: Bool
    public var mode: SyncMode
    public var interval_seconds: Int
    public var server_url: String
    public var device_id: String
    public var device_name: String
    public var device_token: String
    public var last_uploaded_rowid: Int
    public var last_sync_at: Int
}

public struct RegisterRequest: Codable {
    public let server_url: String
    public let master_token: String
    public let device_name: String
}

public struct RegisterResponse: Codable {
    public let device_id: String
    public let device_token: String
    public let device_name: String
}

public struct SyncOutcome: Codable, Hashable {
    public var uploaded: Int
    public var new_max_rowid: Int
    public var last_sync_at: Int
}

public struct DeviceInfo: Codable, Hashable, Identifiable {
    public var device_id: String
    public var device_name: String
    public var created_at: Int
    public var record_count: Int?

    public var id: String { device_id }
}

// MARK: - 周额度 / 快照（对应 QuotaSnapshot / WeeklyPeriod）

public struct QuotaSnapshot: Codable, Hashable {
    public var ts: Int
    public var level: String
    public var weekly_pct: Int
    public var weekly_reset: Int?
    public var hour5_pct: Int
    public var mcp_pct: Int
    public var mcp_used: Int?
    public var mcp_total: Int?
}

public struct RemoteSnapshot: Codable, Hashable {
    public var ts: Int
    public var level: String
    public var weekly_pct: Int
    public var weekly_reset: Int?
    public var hour5_pct: Int
    public var mcp_pct: Int
    public var mcp_used: Int?
    public var mcp_total: Int?
    public var device_id: String

    // 与本地 QuotaSnapshot 互转的便利
    public func toLocal() -> QuotaSnapshot {
        QuotaSnapshot(ts: ts, level: level, weekly_pct: weekly_pct,
                      weekly_reset: weekly_reset, hour5_pct: hour5_pct,
                      mcp_pct: mcp_pct, mcp_used: mcp_used, mcp_total: mcp_total)
    }
}

public struct WeeklyPeriod: Codable, Hashable, Identifiable {
    public var reset_at: Int
    public var end_at: Int
    public var is_current: Bool
    public var pct_start: Int
    public var pct_peak: Int
    public var pct_end: Int
    public var sample_count: Int

    public var id: Int { reset_at }
}

// MARK: - 远端周期明细 + 折算（对应 RemotePeriodDetail / WeeklyTokenBucket / ConsumedBucket）

public struct RemoteDetailRow: Codable, Hashable {
    public var started_at: Int
    public var model_id: String
    public var input_tokens: Int
    public var output_tokens: Int
    public var cache_read_tokens: Int
    public var total_tokens: Int
}

public struct RemotePeriodDetail: Codable, Hashable {
    public var reset_at: Int
    public var end_at: Int
    public var rows: [RemoteDetailRow]
}

public struct WeeklyTokenBucket: Codable, Hashable, Identifiable {
    public var reset_at: Int
    public var end_at: Int
    public var total_tokens: Int
    public var requests: Int
    public var id: Int { reset_at }
}

public enum PlanType: String, Codable, CaseIterable {
    case v2, v3
}

public struct PeakSegment: Codable, Hashable {
    public var start: String        // "HH:MM"
    public var end: String          // "HH:MM"
    public var multiplier: Double
    public var weekday_mask: Int    // bit0=周日...bit6=周六
}

public struct PeakConfig: Codable, Hashable {
    public var plan_type: PlanType?
    public var zcode_discount: Bool
    public var enabled: Bool
    public var segments: [PeakSegment]
}

public struct ConsumedBucket: Codable, Hashable, Identifiable {
    public var reset_at: Int
    public var end_at: Int
    public var consumed: Double
    public var requests: Int
    public var id: Int { reset_at }
}

// MARK: - 清理相关

public struct AutoCleanupConfig: Codable, Hashable {
    public var auto_enabled: Bool
    public var auto_days: Int
}

public struct CleanupStatus: Codable, Hashable {
    public var total_records: Int
    public var devices: [DeviceInfo]
    public var auto_config: AutoCleanupConfig
}

public struct CleanupResult: Codable, Hashable {
    public var action: String
    public var records_deleted: Int
    public var devices_deleted: Int?
}

// MARK: - 设备筛选

public enum DeviceFilter: Hashable {
    case all
    case local
    case specific(String)   // device_id
}

// MARK: - 时间范围预设

public enum RangePreset: String, CaseIterable, Identifiable {
    case today, d1 = "1d", d7 = "7d", d30 = "30d", custom

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .today: return "今日"
        case .d1: return "24小时"
        case .d7: return "7天"
        case .d30: return "30天"
        case .custom: return "自定义"
        }
    }
}

// MARK: - 额度预警 / 快捷键（对应 NotifyConfig / ShortcutConfig，iOS 仅保留 notify）

public struct NotifyConfig: Codable, Hashable {
    public var enabled: Bool
    public var hour5_threshold: Int
    public var weekly_threshold: Int
    public var mcp_threshold: Int
}

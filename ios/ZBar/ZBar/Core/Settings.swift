//
//  Settings.swift
//  ZBar
//
//  配置持久化层。所有用户配置存在 UserDefaults（标准域），
//  Widget 需要的数据额外写入 App Group 共享容器。
//
//  桌面端用 ~/.zbar/*.json，iOS 用 UserDefaults 对应：
//   - sync.json      → SyncConfig
//   - pricing.json   → PricingConfig
//   - quota.json     → QuotaConfig
//   - currency 偏好  → Currency
//   - peak.json      → PeakConfig
//

import Foundation
import Combine

/// App Group 标识符（Widget 与 App 共享数据用）。
/// 免费账号也能用 App Group，只需在 Xcode 给两个 target 都勾选同一 group。
public let APP_GROUP = "group.com.chacca.zbar"

/// Widget 读取的共享 key。
public enum WidgetCacheKey {
    public static let snapshot = "widget.snapshot"          // WidgetSnapshot（JSON）
    public static let lastUpdate = "widget.lastUpdate"      // Double（时间戳）
    public static let currency = "widget.currency"
}

/// Widget 渲染所需的全部数据快照（App 拉好数据后写入共享容器）。
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

    public init(todayCostCny: Double = 0, todayCostUsd: Double = 0,
                todayTokens: Int = 0, todayRequests: Int = 0,
                hour5Pct: Int? = nil, weeklyPct: Int? = nil,
                mcpPct: Int? = nil, mcpUsed: Int? = nil, mcpTotal: Int? = nil,
                weeklyResetMs: Int? = nil, hour5ResetMs: Int? = nil,
                level: String = "", deviceName: String = "",
                updatedAt: Double = 0) {
        self.todayCostCny = todayCostCny
        self.todayCostUsd = todayCostUsd
        self.todayTokens = todayTokens
        self.todayRequests = todayRequests
        self.hour5Pct = hour5Pct
        self.weeklyPct = weeklyPct
        self.mcpPct = mcpPct
        self.mcpUsed = mcpUsed
        self.mcpTotal = mcpTotal
        self.weeklyResetMs = weeklyResetMs
        self.hour5ResetMs = hour5ResetMs
        self.level = level
        self.deviceName = deviceName
        self.updatedAt = updatedAt
    }
}

/// 全局配置中心，ObservableObject 供 SwiftUI 视图订阅。
@MainActor
public final class AppSettings: ObservableObject {
    public static let shared = AppSettings()

    // 用户配置（UserDefaults 标准域）
    @Published public var sync: SyncConfig
    @Published public var pricing: PricingConfig
    @Published public var quotaConfig: QuotaConfig
    @Published public var currency: Currency
    @Published public var peak: PeakConfig

    // 共享容器（Widget 读取）
    private let sharedDefaults: UserDefaults?

    private init() {
        let d = UserDefaults.standard
        self.sync = Self.loadSync(d)
        self.pricing = Self.loadPricing(d)
        self.quotaConfig = Self.loadQuota(d)
        self.currency = Self.loadCurrency(d)
        self.peak = Self.loadPeak(d)
        self.sharedDefaults = UserDefaults(suiteName: APP_GROUP)
    }

    // MARK: - 保存

    public func saveSync() {
        save("sync", sync)
    }
    public func savePricing() {
        save("pricing", pricing)
    }
    public func saveQuotaConfig() {
        save("quotaConfig", quotaConfig)
    }
    public func saveCurrency() {
        UserDefaults.standard.set(currency.rawValue, forKey: "currency")
        sharedDefaults?.set(currency.rawValue, forKey: WidgetCacheKey.currency)
    }
    public func savePeak() {
        save("peak", peak)
    }

    private func save<T: Encodable>(_ key: String, _ value: T) {
        if let data = try? JSONEncoder().encode(value) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }

    // MARK: - Widget 缓存（写入 App Group）

    /// 把 Widget 渲染所需的快照写入共享容器。
    public func writeWidgetSnapshot(_ snap: WidgetSnapshot) {
        guard let sd = sharedDefaults else { return }
        if let data = try? JSONEncoder().encode(snap) {
            sd.set(data, forKey: WidgetCacheKey.snapshot)
            sd.set(Date().timeIntervalSince1970, forKey: WidgetCacheKey.lastUpdate)
        }
    }

    /// 从共享容器读 Widget 快照（Widget 进程也会用这个）。
    public static func readWidgetSnapshot() -> WidgetSnapshot? {
        guard let sd = UserDefaults(suiteName: APP_GROUP) else { return nil }
        guard let data = sd.data(forKey: WidgetCacheKey.snapshot) else { return nil }
        return try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
    }

    // MARK: - 加载（含内置默认值）

    private static func loadSync(_ d: UserDefaults) -> SyncConfig {
        if let s = decode(SyncConfig.self, d, "sync") { return s }
        return SyncConfig(enabled: false, mode: .manual, interval_seconds: 60,
                          server_url: "", device_id: "", device_name: "iPhone",
                          device_token: "", last_uploaded_rowid: 0, last_sync_at: 0)
    }

    private static func loadPricing(_ d: UserDefaults) -> PricingConfig {
        if let p = decode(PricingConfig.self, d, "pricing") { return p }
        // 首次启动用内置默认表
        return loadBuiltinPricingDefaults()
    }

    private static func loadQuota(_ d: UserDefaults) -> QuotaConfig {
        if let q = decode(QuotaConfig.self, d, "quotaConfig") { return q }
        return QuotaConfig()
    }

    private static func loadCurrency(_ d: UserDefaults) -> Currency {
        Currency(rawValue: d.string(forKey: "currency") ?? "cny") ?? .cny
    }

    private static func loadPeak(_ d: UserDefaults) -> PeakConfig {
        if let p = decode(PeakConfig.self, d, "peak") {
            return p
        }
        return PeakConfig(plan_type: nil, zcode_discount: true,
                          enabled: false, segments: [])
    }

    private static func decode<T: Decodable>(_ type: T.Type, _ d: UserDefaults, _ key: String) -> T? {
        guard let data = d.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    /// 从 bundle 加载内置默认价格表（public/pricing-defaults.json 的副本）。
    public static func loadBuiltinPricingDefaults() -> PricingConfig {
        guard let url = Bundle.main.url(forResource: "PricingDefaults", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let parsed = try? JSONDecoder().decode(PricingDefaults.self, from: data) else {
            return PricingConfig()
        }
        return PricingConfig(cny: parsed.cny, usd: parsed.usd)
    }
}

// MARK: - 已配置检查

extension AppSettings {
    /// 同步服务是否已配置（有 server_url + device_token）。
    public var isSyncConfigured: Bool {
        !sync.server_url.isEmpty && !sync.device_token.isEmpty
    }

    /// 额度监控是否已配置（有 token）。
    public var isQuotaConfigured: Bool {
        !quotaConfig.token.trimmingCharacters(in: .whitespaces).isEmpty
    }
}

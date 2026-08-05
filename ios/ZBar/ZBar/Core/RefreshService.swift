//
//  RefreshService.swift
//  ZBar
//
//  数据加载中枢：拉用量 + 额度，合并计费，写 Widget 缓存。
//  UI 订阅它的 @Published，每次刷新都自动更新。
//

import Foundation
import Combine

@MainActor
public final class RefreshService: ObservableObject {
    public static let shared = RefreshService()

    // 状态
    @Published public var loading = false
    @Published public var lastError: String?

    // 数据
    @Published public var todayUsage: RemoteUsage?
    @Published public var todayCost: CostResult?
    @Published public var quota: QuotaResult?
    @Published public var devices: [DeviceInfo] = []
    @Published public var selectedFilter: DeviceFilter = .all
    @Published public var rangePreset: RangePreset = .today

    private let settings = AppSettings.shared
    private var refreshTask: Task<Void, Never>?

    public init() {}

    // MARK: - 公共刷新

    /// 主刷新：用量 + 额度 + 设备列表，刷新完写 Widget 缓存。
    public func refreshAll() {
        refreshTask?.cancel()
        refreshTask = Task { await doRefreshAll() }
    }

    /// 仅刷新额度（额度页主动刷新用）。
    public func refreshQuotaOnly() async {
        guard settings.isQuotaConfigured else {
            self.quota = nil
            return
        }
        do {
            self.quota = try await QuotaClient.shared.query(cfg: settings.quotaConfig)
        } catch {
            self.lastError = error.localizedDescription
        }
        writeWidgetSnapshot()
    }

    // MARK: - 内部

    private func doRefreshAll() async {
        loading = true
        lastError = nil
        defer { loading = false }

        // 1. 没配置同步 → 只拉额度
        guard settings.isSyncConfigured else {
            await refreshQuotaOnly()
            return
        }

        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id

        async let devicesResult: [DeviceInfo]? = try? await APIClient.shared.devices(
            serverURL: server, deviceToken: token)
        async let quotaResult: QuotaResult? = fetchQuotaIfConfigured()

        // 2. 今日用量
        let now = Date()
        let cal = Calendar.current
        let startOfToday = cal.startOfDay(for: now)
        let fromMs = Int(startOfToday.timeIntervalSince1970 * 1000)
        let toMs = Int(now.timeIntervalSince1970 * 1000)

        do {
            let usage = try await APIClient.shared.usage(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs,
                bucket: .hour, filter: selectedFilter, localDeviceId: localId)
            self.todayUsage = usage
            self.todayCost = Billing.computeRemoteCost(usage, pricing: settings.pricing)
        } catch {
            self.lastError = error.localizedDescription
        }

        // 3. 设备列表（失败不阻塞）
        if let devs = await devicesResult {
            self.devices = devs
        }

        // 4. 额度
        self.quota = await quotaResult

        writeWidgetSnapshot()
    }

    private func fetchQuotaIfConfigured() async -> QuotaResult? {
        guard settings.isQuotaConfigured else { return nil }
        do {
            return try await QuotaClient.shared.query(cfg: settings.quotaConfig)
        } catch {
            self.lastError = error.localizedDescription
            return nil
        }
    }

    // MARK: - Widget 缓存

    /// 把当前内存数据打包写入 App Group，供 Widget 渲染。
    /// currency 由 settings.saveCurrency() 单独写共享容器，不放进 snapshot。
    public func writeWidgetSnapshot() {
        let usage = todayUsage
        let cost = todayCost
        let q = quota

        let snap = WidgetSnapshot(
            todayCostCny: cost?.total_cny ?? 0,
            todayCostUsd: cost?.total_usd ?? 0,
            todayTokens: usage?.overall.total_tokens ?? 0,
            todayRequests: usage?.overall.requests ?? 0,
            hour5Pct: q?.hour5?.percentage,
            weeklyPct: q?.weekly?.percentage,
            mcpPct: q?.mcp?.percentage,
            mcpUsed: q?.mcp?.currentValue,
            mcpTotal: q?.mcp?.usage,
            weeklyResetMs: q?.weekly?.nextResetTime,
            hour5ResetMs: q?.hour5?.nextResetTime,
            level: q?.level ?? "",
            deviceName: settings.sync.device_name,
            updatedAt: Date().timeIntervalSince1970)

        settings.writeWidgetSnapshot(snap)
    }

    // MARK: - 设备筛选切换

    public func setFilter(_ f: DeviceFilter) {
        selectedFilter = f
        refreshAll()
    }
}

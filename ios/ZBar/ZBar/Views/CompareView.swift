//
//  CompareView.swift
//  ZBar
//
//  周额度对比：按智谱重置周期展示 weekly 百分比变化 + 实际 token + 折算消耗。
//  快照来自 server /snapshots，token/消耗来自 /period_detail + peak 折算。
//

import SwiftUI
import Charts

struct CompareView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var periods: [WeeklyPeriod] = []
    @State private var tokenBuckets: [WeeklyTokenBucket] = []
    @State private var consumedBuckets: [ConsumedBucket] = []
    @State private var loading = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if !settings.isSyncConfigured {
                        NotConfiguredBanner(message: "尚未连接同步服务，对比需要先注册设备。")
                            .padding(.horizontal)
                    } else {
                        LoadingBar(loading: loading, error: error)

                        if !periods.isEmpty {
                            WeeklyPctChart(periods: periods)
                                .padding(.horizontal)

                            PeriodsTable(periods: periods,
                                         tokens: tokenBuckets,
                                         consumed: consumedBuckets)
                                .padding(.horizontal)
                        }
                    }
                }
                .padding(.vertical)
            }
            .navigationTitle("周额度对比")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { load() } label: { Image(systemName: "arrow.clockwise") }
                }
            }
        }
        .onAppear { load() }
        .onChange(of: refresh.selectedFilter) { _, _ in load() }
    }

    private func load() {
        loading = true
        error = nil
        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id
        let filter = refresh.selectedFilter

        Task {
            do {
                // 取近 90 天的快照来切周期
                let now = Date()
                let fromMs = Int(now.timeIntervalSince1970 * 1000) - 90 * 86_400_000
                let toMs = Int(now.timeIntervalSince1970 * 1000)
                let snaps = await APIClient.shared.snapshots(
                    serverURL: server, deviceToken: token,
                    fromMs: fromMs, toMs: toMs,
                    filter: filter, localDeviceId: localId)
                let localSnaps = snaps.map { $0.toLocal() }
                let parsed = PeriodParser.splitPeriods(localSnaps)

                // 取每个周期的明细折算 token + 消耗
                let periodPairs = parsed.map { ($0.reset_at, $0.end_at) }
                let detail = await APIClient.shared.periodDetail(
                    serverURL: server, deviceToken: token,
                    periods: periodPairs,
                    filter: filter, localDeviceId: localId)

                let tokenB: [WeeklyTokenBucket] = parsed.indices.map { i in
                    let rows = (i < detail.count) ? detail[i].rows : []
                    let total = rows.reduce(0) { $0 + $1.total_tokens }
                    let reqs = rows.count
                    return WeeklyTokenBucket(reset_at: parsed[i].reset_at,
                                              end_at: parsed[i].end_at,
                                              total_tokens: total,
                                              requests: reqs)
                }
                let consumed = Peak.detailToConsumed(detail, cfg: settings.peak)
                let consumedB: [ConsumedBucket] = parsed.indices.map { i in
                    let c = (i < consumed.count) ? consumed[i] : (consumed: 0.0, requests: 0)
                    return ConsumedBucket(reset_at: parsed[i].reset_at,
                                          end_at: parsed[i].end_at,
                                          consumed: c.consumed,
                                          requests: c.requests)
                }
                await MainActor.run {
                    self.periods = parsed
                    self.tokenBuckets = tokenB
                    self.consumedBuckets = consumedB
                    self.loading = false
                }
            } catch {
                await MainActor.run {
                    self.error = error.localizedDescription
                    self.loading = false
                }
            }
        }
    }
}

// MARK: - 周期 weekly 百分比图

private struct WeeklyPctChart: View {
    let periods: [WeeklyPeriod]

    var body: some View {
        Card(title: "每周峰值百分比") {
            Chart(periods) { p in
                BarMark(
                    x: .value("周期", periodLabel(p)),
                    y: .value("百分比", p.pct_peak)
                )
                .foregroundStyle(quotaColor(p.pct_peak).gradient)
            }
            .frame(height: 180)
        }
    }

    private func periodLabel(_ p: WeeklyPeriod) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(p.reset_at) / 1000)
        let fmt = DateFormatter()
        fmt.dateFormat = "MM-dd"
        return fmt.string(from: d)
    }
}

// MARK: - 周期表

private struct PeriodsTable: View {
    let periods: [WeeklyPeriod]
    let tokens: [WeeklyTokenBucket]
    let consumed: [ConsumedBucket]
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Card(title: "周期明细") {
            ForEach(periods) { p in
                let i = periods.firstIndex { $0.reset_at == p.reset_at } ?? 0
                let tok = tokens[safe: i]
                let con = consumed[safe: i]
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(periodHeader(p))
                            .font(.subheadline.weight(.semibold))
                        if p.is_current {
                            Text("当前")
                                .font(.caption2)
                                .padding(.horizontal, 6).padding(.vertical, 2)
                                .background(Capsule().fill(Color.green.opacity(0.2)))
                                .foregroundColor(.green)
                        }
                        Spacer()
                        Text("\(p.pct_end)%")
                            .font(.subheadline.monospacedDigit().weight(.semibold))
                            .foregroundColor(quotaColor(p.pct_end))
                    }
                    HStack(spacing: 12) {
                        if let t = tok {
                            Tag("Token " + Format.tokens(t.total_tokens))
                            Tag("\(t.requests) 次")
                        }
                        if settings.peak.plan_type != nil, let c = con {
                            Tag("消耗 " + Format.tokens(Int(c.consumed)))
                        }
                        Tag("采样 \(p.sample_count)")
                    }
                    .font(.caption2).foregroundColor(.secondary)
                    // 进度范围：start → peak → end
                    HStack(spacing: 4) {
                        Text("起 \(p.pct_start)%")
                        Text("峰 \(p.pct_peak)%")
                        Text("终 \(p.pct_end)%")
                    }
                    .font(.caption2.monospacedDigit())
                    .foregroundColor(.secondary)
                }
                .padding(.vertical, 4)
                Divider()
            }
        }
    }

    private func periodHeader(_ p: WeeklyPeriod) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        let r = Date(timeIntervalSince1970: TimeInterval(p.reset_at) / 1000)
        return f.string(from: r)
    }
}

extension Array {
    subscript(safe i: Int) -> Element? {
        return (0..<count).contains(i) ? self[i] : nil
    }
}

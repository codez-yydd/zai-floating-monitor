//
//  TrendView.swift
//  ZBar
//
//  趋势图：近 7 天 / 30 天，按天或按小时分桶，token + 花费双轴。
//  数据来自 server /usage 的 trend 字段，cost 用 pricing 自算。
//

import SwiftUI
import Charts

struct TrendView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var days: Int = 7
    @State private var points: [TrendPoint] = []
    @State private var loading = false
    @State private var error: String?

    private let dayOptions = [1, 7, 30]

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if !settings.isSyncConfigured {
                        NotConfiguredBanner(message: "尚未连接同步服务，趋势需要先注册设备。")
                            .padding(.horizontal)
                    } else {
                        Picker("范围", selection: $days) {
                            ForEach(dayOptions, id: \.self) { d in
                                Text("\(d == 1 ? "24小时" : "\(d)天")").tag(d)
                            }
                        }
                        .pickerStyle(.segmented)
                        .padding(.horizontal)

                        LoadingBar(loading: loading, error: error)

                        if !points.isEmpty {
                            TrendChartCard(points: points)
                                .padding(.horizontal)
                            TrendTable(points: points)
                                .padding(.horizontal)
                        }
                    }
                }
                .padding(.vertical)
            }
            .navigationTitle("趋势")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { load() } label: { Image(systemName: "arrow.clockwise") }
                }
            }
        }
        .onAppear { load() }
        .onChange(of: days) { _, _ in load() }
        .onChange(of: refresh.selectedFilter) { _, _ in load() }
    }

    private func load() {
        loading = true
        error = nil
        let now = Date()
        let fromMs: Int
        let bucket: TrendBucket
        if days == 1 {
            fromMs = Int(now.timeIntervalSince1970 * 1000) - 86_400_000
            bucket = .hour
        } else {
            fromMs = Int(now.timeIntervalSince1970 * 1000) - days * 86_400_000
            bucket = .day
        }
        let toMs = Int(now.timeIntervalSince1970 * 1000)
        Task {
            do {
                let usage = try await APIClient.shared.usage(
                    serverURL: settings.sync.server_url,
                    deviceToken: settings.sync.device_token,
                    fromMs: fromMs, toMs: toMs, bucket: bucket,
                    filter: refresh.selectedFilter,
                    localDeviceId: settings.sync.device_id)
                let pts = Billing.remoteTrendToLocal(usage,
                                                     pricing: settings.pricing,
                                                     bucket: bucket)
                await MainActor.run {
                    self.points = pts
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

private struct TrendChartCard: View {
    let points: [TrendPoint]
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Card(title: "Token 趋势") {
            Chart(points) { p in
                BarMark(
                    x: .value("时间", p.label),
                    y: .value("Token", p.total_tokens)
                )
                .foregroundStyle(Color.blue.gradient)
            }
            .frame(height: 200)

            if settings.currency == .cny {
                Divider().padding(.vertical, 4)
                Text("花费趋势（¥）").font(.caption.weight(.semibold))
                Chart(points) { p in
                    LineMark(
                        x: .value("时间", p.label),
                        y: .value("花费", p.cost_cny)
                    )
                    .foregroundStyle(.green)
                    PointMark(
                        x: .value("时间", p.label),
                        y: .value("花费", p.cost_cny)
                    )
                    .foregroundStyle(.green)
                }
                .frame(height: 120)
            }
        }
    }
}

private struct TrendTable: View {
    let points: [TrendPoint]
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Card(title: "明细") {
            ForEach(points.reversed()) { p in
                HStack {
                    Text(p.label).font(.subheadline.monospacedDigit())
                    Spacer()
                    Text(Format.tokens(p.total_tokens))
                        .font(.caption.monospacedDigit())
                        .foregroundColor(.secondary)
                    Text("\(p.requests) 次")
                        .font(.caption2).foregroundColor(.secondary)
                    if settings.currency == .cny {
                        Text(Format.cost(p.cost_cny, .cny))
                            .font(.caption.monospacedDigit()).foregroundColor(.green)
                    } else {
                        Text(Format.cost(p.cost_usd, .usd))
                            .font(.caption.monospacedDigit()).foregroundColor(.green)
                    }
                }
                Divider()
            }
        }
    }
}

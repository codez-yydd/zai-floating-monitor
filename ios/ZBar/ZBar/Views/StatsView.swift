//
//  StatsView.swift
//  ZBar
//
//  统计面板：今日 / 7天 / 30天 / 自定义 范围切换，
//  展示 overall 汇总 + 按模型分组 + 花费（用 pricing 自算）。
//

import SwiftUI

struct StatsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var preset: RangePreset = .today
    @State private var customFrom: String = ""
    @State private var customTo: String = ""
    @State private var rangeUsage: RemoteUsage?
    @State private var rangeCost: CostResult?
    @State private var rangeLoading = false
    @State private var rangeError: String?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if !settings.isSyncConfigured {
                        // 未配置同步服务
                        NotConfiguredBanner(
                            message: "尚未连接同步服务，统计需要先注册设备。",
                            actionTitle: nil, action: nil)
                    } else {
                        LoadingBar(loading: rangeLoading, error: rangeError)

                        DeviceFilterBar()
                            .padding(.horizontal)

                        RangePicker(preset: $preset,
                                    customFrom: $customFrom, customTo: $customTo) {
                            loadRange()
                        }
                        .padding(.horizontal)

                        if let usage = rangeUsage {
                            OverallCard(usage: usage, cost: rangeCost)
                            ModelBreakdownCard(usage: usage, cost: rangeCost)
                        }
                    }
                }
                .padding(.vertical)
            }
            .navigationTitle("统计")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        loadRange()
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                }
            }
        }
        .onAppear { loadRange() }
        .onChange(of: refresh.selectedFilter) { _, _ in loadRange() }
    }

    // MARK: - 加载指定范围

    private func loadRange() {
        let (fromMs, toMs) = Format.rangeToMs(preset: preset, now: Date(),
                                               custom: (customFrom, customTo))
        rangeLoading = true
        rangeError = nil
        Task {
            do {
                let usage = try await APIClient.shared.usage(
                    serverURL: settings.sync.server_url,
                    deviceToken: settings.sync.device_token,
                    fromMs: fromMs, toMs: toMs,
                    bucket: (preset == .today || preset == .d1) ? .hour : .day,
                    filter: refresh.selectedFilter,
                    localDeviceId: settings.sync.device_id)
                let cost = Billing.computeRemoteCost(usage, pricing: settings.pricing)
                await MainActor.run {
                    self.rangeUsage = usage
                    self.rangeCost = cost
                    self.rangeLoading = false
                }
            } catch {
                await MainActor.run {
                    self.rangeError = error.localizedDescription
                    self.rangeLoading = false
                }
            }
        }
    }
}

// MARK: - 范围选择器

private struct RangePicker: View {
    @Binding var preset: RangePreset
    @Binding var customFrom: String
    @Binding var customTo: String
    let onApply: () -> Void

    var body: some View {
        VStack(spacing: 10) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack {
                    ForEach(RangePreset.allCases) { p in
                        Button {
                            preset = p
                            if p != .custom { onApply() }
                        } label: {
                            Text(p.displayName)
                                .font(.subheadline.weight(preset == p ? .semibold : .regular))
                                .padding(.horizontal, 12).padding(.vertical, 6)
                                .background(
                                    Capsule().fill(preset == p ? Color.blue.opacity(0.25) : Color.white.opacity(0.05)))
                                .foregroundColor(preset == p ? .blue : .primary)
                        }
                    }
                }
            }
            if preset == .custom {
                HStack {
                    DatePicker("起", selection: Binding(
                        get: { Format.dateFromYMD(customFrom) ?? Date() },
                        set: { customFrom = Format.dateStr(Int($0.timeIntervalSince1970 * 1000)) }),
                        displayedComponents: .date)
                    DatePicker("止", selection: Binding(
                        get: { Format.dateFromYMD(customTo) ?? Date() },
                        set: { customTo = Format.dateStr(Int($0.timeIntervalSince1970 * 1000)) }),
                        displayedComponents: .date)
                    Button("应用", action: onApply)
                        .buttonStyle(.borderedProminent)
                }
                .font(.caption)
            }
        }
    }
}

// MARK: - 设备筛选条

struct DeviceFilterBar: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                FilterChip(title: "全部", selected: refresh.selectedFilter == .all) {
                    refresh.setFilter(.all)
                }
                FilterChip(title: "本机", selected: refresh.selectedFilter == .local) {
                    refresh.setFilter(.local)
                }
                ForEach(refresh.devices) { d in
                    FilterChip(title: d.device_name,
                               selected: refresh.selectedFilter == .specific(d.device_id)) {
                        refresh.setFilter(.specific(d.device_id))
                    }
                }
            }
        }
    }
}

private struct FilterChip: View {
    let title: String
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.caption.weight(selected ? .semibold : .regular))
                .padding(.horizontal, 10).padding(.vertical, 5)
                .background(Capsule().fill(selected ? Color.accentColor.opacity(0.25) : Color.white.opacity(0.05)))
                .foregroundColor(selected ? .accentColor : .primary)
        }
    }
}

// MARK: - 总览卡

private struct OverallCard: View {
    let usage: RemoteUsage
    let cost: CostResult?
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Card(title: "总览") {
            HStack {
                MetricCell(title: "请求数",
                           value: String(usage.overall.requests))
                MetricCell(title: "总 Token",
                           value: Format.tokens(usage.overall.total_tokens))
            }
            Divider()
            HStack {
                MetricCell(title: "输入",
                           value: Format.tokens(usage.overall.input_tokens))
                MetricCell(title: "输出",
                           value: Format.tokens(usage.overall.output_tokens))
            }
            HStack {
                MetricCell(title: "缓存读",
                           value: Format.tokens(usage.overall.cache_read_tokens))
                MetricCell(title: "推理",
                           value: Format.tokens(usage.overall.reasoning_tokens))
            }
            if let c = cost {
                Divider()
                HStack {
                    MetricCell(title: "花费（¥）",
                               value: Format.cost(c.total_cny, .cny),
                               color: .green)
                    MetricCell(title: "花费（$）",
                               value: Format.cost(c.total_usd, .usd),
                               color: .green)
                }
            }
        }
        .padding(.horizontal)
    }
}

// MARK: - 模型明细

private struct ModelBreakdownCard: View {
    let usage: RemoteUsage
    let cost: CostResult?
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Card(title: "按模型") {
            if usage.by_model.isEmpty {
                Text("范围内无数据")
                    .foregroundColor(.secondary)
                    .font(.subheadline)
            } else {
                ForEach(usage.by_model) { m in
                    ModelRow(stat: m,
                             cost: costFor(modelId: m.model_id))
                    Divider()
                }
            }
        }
        .padding(.horizontal)
    }

    private func costFor(modelId: String) -> Double? {
        guard let c = cost else { return nil }
        return settings.currency == .cny
            ? c.per_model_cny.first { $0.model_id == modelId }?.cost
            : c.per_model_usd.first { $0.model_id == modelId }?.cost
    }
}

private struct ModelRow: View {
    let stat: ModelStat
    let cost: Double?
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(stat.model_id)
                    .font(.subheadline.weight(.semibold))
                Spacer()
                if let c = cost {
                    Text(Format.cost(c, settings.currency))
                        .font(.subheadline.monospacedDigit())
                        .foregroundColor(.green)
                }
            }
            HStack(spacing: 12) {
                Tag("请 \(stat.requests)")
                Tag(Format.tokens(stat.total_tokens) + " tok")
                Tag("入 " + Format.tokens(stat.input_tokens))
                Tag("出 " + Format.tokens(stat.output_tokens))
            }
            .font(.caption2)
            .foregroundColor(.secondary)
        }
        .padding(.vertical, 4)
    }
}

private struct Tag: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(Capsule().fill(Color.white.opacity(0.06)))
    }
}

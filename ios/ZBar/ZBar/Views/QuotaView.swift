//
//  QuotaView.swift
//  ZBar
//
//  Coding Plan 额度监控：5 小时窗口 / 每周 / MCP 月度三色进度条 + 重置倒计时。
//  数据直连智谱开放平台，不经过自托管 server。
//

import SwiftUI

struct QuotaView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if !settings.isQuotaConfigured {
                        NotConfiguredBanner(
                            message: "尚未配置 Coding Plan Token，请在设置中填写后查看额度。")
                            .padding(.horizontal)
                    } else if refresh.loading {
                        ProgressView().padding(.top, 40)
                    } else if let q = refresh.quota {
                        LevelHeader(level: q.level)
                            .padding(.horizontal)

                        Card(title: "5 小时窗口") {
                            if let h = q.hour5 {
                                ProgressBar(pct: h.percentage,
                                            label: "滑动窗口用量",
                                            resetMs: h.nextResetTime)
                            } else {
                                Text("暂无 5 小时窗口数据").foregroundColor(.secondary)
                            }
                        }
                        .padding(.horizontal)

                        Card(title: "每周额度") {
                            if let w = q.weekly {
                                ProgressBar(pct: w.percentage,
                                            label: "本周用量",
                                            resetMs: w.nextResetTime)
                            } else {
                                Text("暂无每周额度数据").foregroundColor(.secondary)
                            }
                        }
                        .padding(.horizontal)

                        if let m = q.mcp, m.usage != nil {
                            Card(title: "MCP 月度") {
                                ProgressBar(pct: m.percentage,
                                            label: "月度次数",
                                            resetMs: m.nextResetTime)
                                if let used = m.currentValue, let total = m.usage {
                                    Text("\(used) / \(total) 次")
                                        .font(.caption.monospacedDigit())
                                        .foregroundColor(.secondary)
                                }
                                if let details = m.usageDetails, !details.isEmpty {
                                    Divider()
                                    Text("按工具").font(.caption.weight(.semibold))
                                    ForEach(details, id: \.model_code) { d in
                                        HStack {
                                            Text(d.model_code).font(.caption)
                                            Spacer()
                                            Text("\(d.usage) 次").font(.caption.monospacedDigit())
                                        }
                                        .foregroundColor(.secondary)
                                    }
                                }
                            }
                            .padding(.horizontal)
                        }
                    } else {
                        Text(refresh.lastError ?? "暂无数据")
                            .foregroundColor(.secondary)
                            .padding()
                    }
                }
                .padding(.vertical)
            }
            .navigationTitle("额度监控")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        Task { await refresh.refreshQuotaOnly() }
                    } label: { Image(systemName: "arrow.clockwise") }
                }
            }
        }
    }
}

private struct LevelHeader: View {
    let level: String
    var body: some View {
        HStack {
            Image(systemName: "rosette")
                .foregroundColor(.yellow)
            Text("套餐等级")
                .foregroundColor(.secondary)
            Spacer()
            Text(level.uppercased())
                .font(.headline.monospaced())
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(Capsule().fill(Color.yellow.opacity(0.2)))
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 12).fill(.ultraThinMaterial))
    }
}

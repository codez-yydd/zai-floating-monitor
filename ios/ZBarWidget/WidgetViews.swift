//
//  WidgetViews.swift
//  ZBarWidget
//
//  Widget 视图：主屏（小/中尺寸）+ 锁屏（rectangular/circular/inline）。
//  所有数据从 ZBarEntry.snapshot 读取，不发起网络请求。
//

import WidgetKit
import SwiftUI

// MARK: - 主屏 Widget 视图

struct ZBarHomeWidgetView: View {
    let entry: ZBarEntry
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .systemSmall:
            SmallHomeView(snap: entry.snapshot, currency: entry.currency)
        case .systemMedium:
            MediumHomeView(snap: entry.snapshot, currency: entry.currency)
        default:
            SmallHomeView(snap: entry.snapshot, currency: entry.currency)
        }
    }
}

/// 小尺寸：今日花费 + 今日 Token + 额度百分比
private struct SmallHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("ZBar")
                    .font(.caption2.weight(.bold))
                    .foregroundColor(.secondary)
                Spacer()
                Text(timeAgo)
                    .font(.system(size: 8))
                    .foregroundColor(.secondary)
            }
            Text(costText)
                .font(.system(size: 26, weight: .bold, design: .rounded))
                .foregroundColor(.green)
            Text("\(tokenText) · \(snap.todayRequests) 次")
                .font(.caption2.monospacedDigit())
                .foregroundColor(.secondary)

            Divider()
            if let w = snap.weeklyPct {
                miniBar(label: "周", pct: w)
            }
            if let h = snap.hour5Pct {
                miniBar(label: "5h", pct: h)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
    private var tokenText: String { Format.tokens(snap.todayTokens) }
    private var timeAgo: String {
        let diff = Date().timeIntervalSince1970 - snap.updatedAt
        if diff < 60 { return "刚刚" }
        if diff < 3600 { return "\(Int(diff/60))分前" }
        return "\(Int(diff/3600))时前"
    }

    private func miniBar(label: String, pct: Int) -> some View {
        HStack(spacing: 6) {
            Text(label).font(.system(size: 9)).foregroundColor(.secondary)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 2)
                        .fill(Color.white.opacity(0.1))
                    RoundedRectangle(cornerRadius: 2)
                        .fill(widgetQuotaColor(pct))
                        .frame(width: geo.size.width * CGFloat(min(max(Double(pct)/100,0),1)))
                }
            }
            .frame(height: 4)
            Text("\(pct)%")
                .font(.system(size: 9, design: .rounded))
                .foregroundColor(widgetQuotaColor(pct))
        }
    }
}

/// 中尺寸：左侧大数字 + 右侧额度三档
private struct MediumHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("今日花费")
                    .font(.caption2).foregroundColor(.secondary)
                Text(costText)
                    .font(.system(size: 28, weight: .bold, design: .rounded))
                    .foregroundColor(.green)
                Text("\(tokenText) Token")
                    .font(.caption.monospacedDigit())
                    .foregroundColor(.secondary)
                Text("\(snap.todayRequests) 次请求")
                    .font(.caption2).foregroundColor(.secondary)
                Spacer()
            }
            Divider()
            VStack(alignment: .leading, spacing: 6) {
                if let h = snap.hour5Pct {
                    quotaRow(label: "5h 窗口", pct: h)
                }
                if let w = snap.weeklyPct {
                    quotaRow(label: "每周", pct: w)
                }
                if let m = snap.mcpPct {
                    quotaRow(label: "MCP", pct: m)
                }
                if let r = snap.weeklyResetMs {
                    Text("周重置：" + Format.countdown(to: r))
                        .font(.system(size: 9))
                        .foregroundColor(.secondary)
                }
                Spacer()
            }
        }
        .padding(12)
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
    private var tokenText: String { Format.tokens(snap.todayTokens) }

    private func quotaRow(label: String, pct: Int) -> some View {
        HStack(spacing: 6) {
            Text(label)
                .font(.system(size: 10))
                .foregroundColor(.secondary)
                .frame(width: 38, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(Color.white.opacity(0.1))
                    RoundedRectangle(cornerRadius: 3)
                        .fill(widgetQuotaColor(pct))
                        .frame(width: geo.size.width * CGFloat(min(max(Double(pct)/100,0),1)))
                }
            }
            .frame(height: 6)
            Text("\(pct)%")
                .font(.system(size: 10, design: .rounded).monospacedDigit())
                .foregroundColor(widgetQuotaColor(pct))
                .frame(width: 30, alignment: .trailing)
        }
    }
}

// MARK: - 锁屏 Widget 视图

struct ZBarLockScreenView: View {
    let entry: ZBarEntry
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .accessoryRectangular:
            RectangularView(snap: entry.snapshot, currency: entry.currency)
        case .accessoryCircular:
            CircularView(snap: entry.snapshot)
        case .accessoryInline:
            InlineView(snap: entry.snapshot, currency: entry.currency)
        default:
            RectangularView(snap: entry.snapshot, currency: entry.currency)
        }
    }
}

/// 锁屏长条：花费 + 额度
private struct RectangularView: View {
    let snap: WidgetSnapshot
    let currency: Currency

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("ZBar")
                .font(.caption2.weight(.semibold))
            Text(costLine)
                .font(.headline.monospacedDigit())
            if let w = snap.weeklyPct {
                Text("周 \(w)%")
                    .font(.caption2.monospacedDigit())
            }
            if let h = snap.hour5Pct {
                Text("5h \(h)%")
                    .font(.caption2.monospacedDigit())
            }
        }
    }

    private var costLine: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
}

/// 锁屏圆形：weekly 百分比环
private struct CircularView: View {
    let snap: WidgetSnapshot

    var body: some View {
        ZStack {
            if let w = snap.weeklyPct {
                AccessoryWidgetBackground()
                ProgressView(value: Double(w), total: 100) {
                    Text("周")
                        .font(.system(size: 9))
                } currentValueLabel: {
                    Text("\(w)")
                        .font(.system(size: 14, weight: .bold, design: .rounded).monospacedDigit())
                }
                .progressViewStyle(.circular)
                .tint(widgetQuotaColor(w))
            } else {
                AccessoryWidgetBackground()
                Text("—")
                    .font(.headline)
            }
        }
    }
}

/// 锁屏顶部一行：花费 + token
private struct InlineView: View {
    let snap: WidgetSnapshot
    let currency: Currency

    var body: some View {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        Text("ZBar \(Format.cost(n, currency)) · \(Format.tokens(snap.todayTokens))")
    }
}

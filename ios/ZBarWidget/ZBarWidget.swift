//
//  ZBarWidget.swift
//  ZBarWidget
//
//  WidgetKit 小组件入口。支持锁屏（accessoryRectangular/Circular/Inline）
//  和主屏（systemSmall/Medium）。数据从 App Group 共享容器读取。
//

import WidgetKit
import SwiftUI

@main
struct ZBarWidgetBundle: WidgetBundle {
    var body: some Widget {
        ZBarHomeWidget()      // 主屏/中尺寸
        ZBarLockScreenWidget() // 锁屏
    }
}

// MARK: - 主屏 Widget（systemSmall / systemMedium）

struct ZBarHomeWidget: Widget {
    let kind = "ZBarHomeWidget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: ZBarProvider()) { entry in
            ZBarHomeWidgetView(entry: entry)
                .containerBackgroundForWidget()
        }
        .configurationDisplayName("ZBar 用量")
        .description("今日花费、Token 与额度进度")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

// MARK: - 锁屏 Widget（accessory 家族）

struct ZBarLockScreenWidget: Widget {
    let kind = "ZBarLockWidget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: ZBarProvider()) { entry in
            // 锁屏 accessory family 也必须用 containerBackground，
            // 否则 iOS 17+ 会有运行时警告；系统会自动渲染成单色/透明。
            ZBarLockScreenView(entry: entry)
                .containerBackgroundForWidget()
        }
        .configurationDisplayName("ZBar 额度")
        .description("锁屏快速查看额度与花费")
        .supportedFamilies([.accessoryRectangular, .accessoryCircular, .accessoryInline])
    }
}

// MARK: - Provider

struct ZBarProvider: TimelineProvider {
    func placeholder(in context: Context) -> ZBarEntry {
        ZBarEntry(date: Date(), snapshot: .preview, currency: .cny)
    }

    func getSnapshot(in context: Context, completion: @escaping (ZBarEntry) -> Void) {
        completion(currentEntry())
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<ZBarEntry>) -> Void) {
        let entry = currentEntry()
        // 每 30 分钟由系统调度一次刷新机会（实际频率由系统决定）
        let next = Date().addingTimeInterval(30 * 60)
        completion(Timeline(entries: [entry], policy: .after(next)))
    }

    private func currentEntry() -> ZBarEntry {
        let snap = SharedReader.readSnapshot() ?? WidgetSnapshot.placeholder
        let currency = SharedReader.readCurrency()
        return ZBarEntry(date: Date(), snapshot: snap, currency: currency)
    }
}

// MARK: - Entry

struct ZBarEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot
    let currency: Currency
}

// MARK: - 共享读取（Widget 进程侧）

enum SharedReader {
    static let group = "group.com.chacca.zbar"

    static func readSnapshot() -> WidgetSnapshot? {
        guard let sd = UserDefaults(suiteName: group) else { return nil }
        guard let data = sd.data(forKey: "widget.snapshot") else { return nil }
        return try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
    }

    static func readCurrency() -> Currency {
        guard let sd = UserDefaults(suiteName: group) else { return .cny }
        return Currency(rawValue: sd.string(forKey: "widget.currency") ?? "cny") ?? .cny
    }
}

// MARK: - 小工具扩展

extension View {
    @ViewBuilder
    func containerBackgroundForWidget() -> some View {
        if #available(iOS 17.0, *) {
            self.containerBackground(for: .widget) {
                LinearGradient(colors: [Color.blue.opacity(0.25), Color.purple.opacity(0.15)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
            }
        } else {
            self.background(
                LinearGradient(colors: [Color.blue.opacity(0.25), Color.purple.opacity(0.15)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
            )
        }
    }
}

/// 按百分比选颜色（与 App 端一致）
func widgetQuotaColor(_ pct: Int) -> Color {
    if pct >= 85 { return .red }
    if pct >= 60 { return .orange }
    return .green
}

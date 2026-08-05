//
//  ZBarApp.swift
//  ZBar
//
//  ZCode Token 监控 · iOS 版
//  复用桌面版 ZBar 的同步服务和计费逻辑。
//

import SwiftUI

@main
struct ZBarApp: App {
    @StateObject private var settings = AppSettings.shared
    @StateObject private var refresh = RefreshService.shared

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(settings)
                .environmentObject(refresh)
                .preferredColorScheme(.dark)
                .task {
                    // 启动即拉一次
                    refresh.refreshAll()
                    // 启动写一次 Widget 缓存，避免 Widget 显示旧数据
                    refresh.writeWidgetSnapshot()
                }
        }
    }
}

/// 主导航容器
struct RootView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @State private var tab: Tab = .stats

    enum Tab: Hashable { case stats, quota, trend, compare, settings }

    var body: some View {
        TabView(selection: $tab) {
            StatsView()
                .tabItem { Label("统计", systemImage: "chart.bar.doc.horizontal") }
                .tag(Tab.stats)

            QuotaView()
                .tabItem { Label("额度", systemImage: "gauge.with.dots.needle.67percent") }
                .tag(Tab.quota)

            TrendView()
                .tabItem { Label("趋势", systemImage: "chart.xyaxis.line") }
                .tag(Tab.trend)

            CompareView()
                .tabItem { Label("对比", systemImage: "arrow.left.arrow.right.square") }
                .tag(Tab.compare)

            SettingsView()
                .tabItem { Label("设置", systemImage: "gearshape") }
                .tag(Tab.settings)
        }
        .tint(.blue)
    }
}

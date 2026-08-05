//
//  SharedUI.swift
//  ZBar
//
//  跨视图复用的小组件：三色进度条、卡片、状态指示。
//

import SwiftUI

// MARK: - 额度三色进度条（绿→琥珀→红）

/// 按百分比选颜色：< 60 绿色，60-80 琥珀，> 80 红色。
public func quotaColor(_ pct: Int) -> Color {
    if pct >= 85 { return .red }
    if pct >= 60 { return .orange }
    return .green
}

struct ProgressBar: View {
    let pct: Int
    let label: String
    let resetMs: Int?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(label)
                    .font(.subheadline.weight(.medium))
                Spacer()
                Text("\(pct)%")
                    .font(.subheadline.monospacedDigit().weight(.semibold))
                    .foregroundColor(quotaColor(pct))
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 4)
                        .fill(Color.white.opacity(0.1))
                    RoundedRectangle(cornerRadius: 4)
                        .fill(quotaColor(pct))
                        .frame(width: geo.size.width * CGFloat(min(max(Double(pct) / 100, 0), 1)))
                }
            }
            .frame(height: 8)
            if let r = resetMs {
                Text("重置：" + Format.countdown(to: r))
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }
        }
    }
}

// MARK: - 通用卡片

struct Card<Content: View>: View {
    let title: String?
    let content: Content

    init(title: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let t = title {
                Text(t)
                    .font(.headline)
            }
            content
        }
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: 14)
                .fill(.ultraThinMaterial)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(Color.white.opacity(0.08), lineWidth: 1)
        )
    }
}

// MARK: - 指标格

struct MetricCell: View {
    let title: String
    let value: String
    var subtitle: String? = nil
    var color: Color = .primary

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption)
                .foregroundColor(.secondary)
            Text(value)
                .font(.title3.monospacedDigit().weight(.semibold))
                .foregroundColor(color)
            if let s = subtitle {
                Text(s)
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - 空配置引导

struct NotConfiguredBanner: View {
    let message: String
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.largeTitle)
                .foregroundColor(.orange)
            Text(message)
                .multilineTextAlignment(.center)
                .foregroundColor(.secondary)
            if let act = action, let title = actionTitle {
                Button(title, action: act)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity)
    }
}

// MARK: - 加载/错误条

struct LoadingBar: View {
    let loading: Bool
    let error: String?

    var body: some View {
        VStack(spacing: 0) {
            if loading {
                ProgressView()
                    .controlSize(.small)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 4)
            }
            if let err = error, !err.isEmpty {
                Text(err)
                    .font(.caption2)
                    .foregroundColor(.red)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal)
                    .padding(.bottom, 4)
            }
        }
    }
}

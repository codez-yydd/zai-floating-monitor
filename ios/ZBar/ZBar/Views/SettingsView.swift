//
//  SettingsView.swift
//  ZBar
//
//  设置：同步服务注册、价格编辑、额度 Token、货币、高峰期配置。
//  注册流程复用桌面端的 Master Token → Device Token 流程。
//

import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    var body: some View {
        NavigationStack {
            Form {
                Section("同步服务") {
                    SyncConfigRow()
                }
                Section("Coding Plan 额度") {
                    QuotaConfigRow()
                }
                Section("显示") {
                    CurrencyRow()
                }
                Section {
                    NavigationLink("价格配置") { PricingEditView() }
                    NavigationLink("高峰期折算") { PeakEditView() }
                }
                Section {
                    Button("立即刷新全部数据") {
                        refresh.refreshAll()
                    }
                    .foregroundColor(.accentColor)
                }
                Section("关于") {
                    HStack {
                        Text("版本")
                        Spacer()
                        Text("0.1.0 (iOS)").foregroundColor(.secondary)
                    }
                    Text("桌面版 ZBar 的 iOS 配套应用。用量数据通过自托管同步服务获取，额度数据直连智谱开放平台。")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .navigationTitle("设置")
        }
    }
}

// MARK: - 同步服务配置

private struct SyncConfigRow: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var editing = false
    @State private var serverURL = ""
    @State private var masterToken = ""
    @State private var deviceName = "iPhone"
    @State private var registering = false
    @State private var error: String?
    @State private var showDisconnect = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if settings.isSyncConfigured {
                HStack {
                    Image(systemName: "checkmark.circle.fill").foregroundColor(.green)
                    Text("已连接").font(.subheadline.weight(.medium))
                    Spacer()
                    Text(settings.sync.device_name).foregroundColor(.secondary).font(.caption)
                }
                Text(settings.sync.server_url)
                    .font(.caption.monospaced())
                    .foregroundColor(.secondary)
                    .lineLimit(1)
                Text("设备 ID：\(settings.sync.device_id.prefix(8))…")
                    .font(.caption2).foregroundColor(.secondary)

                HStack {
                    Button("重新注册…") { editing = true }
                    Spacer()
                    Button("断开连接", role: .destructive) { showDisconnect = true }
                }
                .font(.caption)
            } else {
                Text("未连接")
                    .font(.subheadline)
                    .foregroundColor(.orange)
                Button("连接同步服务…") { editing = true }
            }
        }
        .sheet(isPresented: $editing) {
            RegisterSheet(serverURL: $serverURL,
                          masterToken: $masterToken,
                          deviceName: $deviceName,
                          registering: $registering,
                          error: $error,
                          onRegister: doRegister)
                .presentationDetents([.medium])
                .onAppear {
                    serverURL = settings.sync.server_url.isEmpty
                        ? "http://192.168.1.100:3838" : settings.sync.server_url
                    deviceName = settings.sync.device_name.isEmpty ? "iPhone" : settings.sync.device_name
                }
        }
        .confirmationDialog("断开连接后需要重新注册。确定？",
                            isPresented: $showDisconnect) {
            Button("断开", role: .destructive) {
                settings.sync.enabled = false
                settings.sync.server_url = ""
                settings.sync.device_token = ""
                settings.sync.device_id = ""
                settings.saveSync()
            }
        }
    }

    private func doRegister() {
        registering = true
        error = nil
        Task {
            do {
                let resp = try await APIClient.shared.register(
                    serverURL: serverURL,
                    masterToken: masterToken,
                    deviceName: deviceName)
                await MainActor.run {
                    settings.sync.enabled = true
                    settings.sync.server_url = serverURL
                    settings.sync.device_id = resp.device_id
                    settings.sync.device_token = resp.device_token
                    settings.sync.device_name = resp.device_name
                    settings.saveSync()
                    registering = false
                    editing = false
                    refresh.refreshAll()
                }
            } catch {
                await MainActor.run {
                    self.error = error.localizedDescription
                    registering = false
                }
            }
        }
    }
}

private struct RegisterSheet: View {
    @Binding var serverURL: String
    @Binding var masterToken: String
    @Binding var deviceName: String
    @Binding var registering: Bool
    @Binding var error: String?
    let onRegister: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section("服务器") {
                    TextField("http://IP:3838", text: $serverURL)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    SecureField("Master Token", text: $masterToken)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("设备名", text: $deviceName)
                }
                if let err = error {
                    Section { Text(err).foregroundColor(.red).font(.caption) }
                }
            }
            .navigationTitle("连接同步服务")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { error = nil }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(registering ? "注册中…" : "连接并注册") {
                        onRegister()
                    }
                    .disabled(serverURL.isEmpty || masterToken.isEmpty || deviceName.isEmpty || registering)
                }
            }
        }
    }
}

// MARK: - 额度 Token 配置

private struct QuotaConfigRow: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var token = ""
    @State private var endpoint: QuotaEndpoint = .cn

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SecureField("Coding Plan API Token", text: $token)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Picker("端点", selection: $endpoint) {
                Text("🇨🇳 国内 (open.bigmodel.cn)").tag(QuotaEndpoint.cn)
                Text("🌐 国际 (api.z.ai)").tag(QuotaEndpoint.global)
            }
            Button("保存并查询") {
                settings.quotaConfig.token = token
                settings.quotaConfig.endpoint = endpoint
                settings.saveQuotaConfig()
                Task { await refresh.refreshQuotaOnly() }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .disabled(token.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .onAppear {
            token = settings.quotaConfig.token
            endpoint = settings.quotaConfig.endpoint
        }
    }
}

// MARK: - 货币切换

private struct CurrencyRow: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Picker("显示货币", selection: Binding(
            get: { settings.currency },
            set: {
                settings.currency = $0
                settings.saveCurrency()
            })) {
            Text("人民币 ¥").tag(Currency.cny)
            Text("美元 $").tag(Currency.usd)
        }
        .pickerStyle(.segmented)
    }
}

// MARK: - 价格编辑

private struct PricingEditView: View {
    @EnvironmentObject var settings: AppSettings
    @State private var currency: Currency = .cny
    @State private var newModel = ""
    @State private var newInput = ""
    @State private var newOutput = ""
    @State private var newCacheRead = ""

    var body: some View {
        Form {
            Picker("货币", selection: $currency) {
                Text("¥ CNY").tag(Currency.cny)
                Text("$ USD").tag(Currency.usd)
            }
            .pickerStyle(.segmented)

            Section("已配置模型") {
                let map = (currency == .cny) ? settings.pricing.cny : settings.pricing.usd
                if map.isEmpty {
                    Text("尚无配置，下方添加").foregroundColor(.secondary)
                }
                ForEach(map.keys.sorted(), id: \.self) { id in
                    PricingRow(currency: currency, modelId: id) { newPrice in
                        if currency == .cny { settings.pricing.cny[id] = newPrice }
                        else { settings.pricing.usd[id] = newPrice }
                        settings.savePricing()
                    } onDelete: {
                        if currency == .cny { settings.pricing.cny.removeValue(forKey: id) }
                        else { settings.pricing.usd.removeValue(forKey: id) }
                        settings.savePricing()
                    }
                }
            }

            Section("新增模型") {
                TextField("model_id（如 glm-4.6）", text: $newModel)
                    .textInputAutocapitalization(.never)
                HStack {
                    numField("入", $newInput)
                    numField("出", $newOutput)
                    numField("缓存", $newCacheRead)
                }
                Button("添加") {
                    guard let i = Double(newInput), let o = Double(newOutput),
                          let c = Double(newCacheRead), !newModel.isEmpty else { return }
                    let price = ModelPrice(input: i, output: o, cache_read: c)
                    if currency == .cny { settings.pricing.cny[newModel] = price }
                    else { settings.pricing.usd[newModel] = price }
                    settings.savePricing()
                    newModel = ""; newInput = ""; newOutput = ""; newCacheRead = ""
                }
                .disabled(newModel.isEmpty)
            }

            Section {
                Button("重置为内置默认表") {
                    settings.pricing = AppSettings.loadBuiltinPricingDefaults()
                    settings.savePricing()
                }
                .foregroundColor(.orange)
            }
        }
        .navigationTitle("价格配置")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func numField(_ label: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading) {
            Text(label).font(.caption).foregroundColor(.secondary)
            TextField("0", text: text)
                .keyboardType(.decimalPad)
        }
    }
}

private struct PricingRow: View {
    let currency: Currency
    let modelId: String
    let onSave: (ModelPrice) -> Void
    let onDelete: () -> Void

    @State private var input = ""
    @State private var output = ""
    @State private var cacheRead = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(modelId).font(.subheadline.weight(.semibold))
                Spacer()
                Button(role: .destructive, action: onDelete) {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
            }
            HStack {
                numField("入", $input)
                numField("出", $output)
                numField("缓存", $cacheRead)
                Button("存") {
                    guard let i = Double(input), let o = Double(output), let c = Double(cacheRead) else { return }
                    onSave(ModelPrice(input: i, output: o, cache_read: c))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
        }
        .onAppear {
            let map = (currency == .cny)
                ? AppSettings.shared.pricing.cny : AppSettings.shared.pricing.usd
            if let p = map[modelId] {
                input = String(p.input)
                output = String(p.output)
                cacheRead = String(p.cache_read)
            }
        }
    }

    private func numField(_ label: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading) {
            Text(label).font(.caption2).foregroundColor(.secondary)
            TextField("0", text: text)
                .keyboardType(.decimalPad)
                .font(.caption.monospacedDigit())
        }
    }
}

// MARK: - 高峰期配置

private struct PeakEditView: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Form {
            Section("折算开关") {
                Toggle("启用高峰期折算", isOn: Binding(
                    get: { settings.peak.enabled },
                    set: { settings.peak.enabled = $0; settings.savePeak() }))
                Toggle("ZCode 150% 提额优惠", isOn: Binding(
                    get: { settings.peak.zcode_discount },
                    set: { settings.peak.zcode_discount = $0; settings.savePeak() }))
                Picker("订阅类型", selection: Binding(
                    get: { settings.peak.plan_type ?? .v2 },
                    set: { settings.peak.plan_type = $0; settings.savePeak() })) {
                    Text("V2（等效 token）").tag(PlanType.v2)
                    Text("V3（积分）").tag(PlanType.v3)
                }
            }
            Section("时段") {
                if settings.peak.segments.isEmpty {
                    Text("未配置时段（默认全 1.0 倍率）").foregroundColor(.secondary).font(.caption)
                }
                ForEach(settings.peak.segments.indices, id: \.self) { i in
                    Text("\(settings.peak.segments[i].start)–\(settings.peak.segments[i].end) ×\(settings.peak.segments[i].multiplier)")
                        .font(.caption)
                }
                Text("iOS 版暂不支持可视化编辑时段，请保持默认或与桌面端同步。")
                    .font(.caption2).foregroundColor(.secondary)
            }
        }
        .navigationTitle("高峰期折算")
        .navigationBarTitleDisplayMode(.inline)
    }
}

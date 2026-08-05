//
//  APIClient.swift
//  ZBar
//
//  封装自托管同步服务（server/）的所有 HTTP 接口。
//  接口路径、字段名、鉴权方式与 server/app.py 完全一致。
//  iOS 端没有本地 SQLite，本类是用量数据的唯一来源。
//

import Foundation

public enum APIError: LocalizedError {
    case notConfigured
    case http(status: Int, body: String)
    case decode(String)
    case network(String)

    public var errorDescription: String? {
        switch self {
        case .notConfigured: return "尚未配置同步服务，请先在设置中注册设备"
        case .http(let s, let b): return "服务端返回错误（\(s)）：\(b)"
        case .decode(let m): return "数据解析失败：\(m)"
        case .network(let m): return "网络请求失败：\(m)"
        }
    }
}

public actor APIClient {
    public static let shared = APIClient()

    private let session: URLSession
    private let decoder: JSONDecoder

    public init() {
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 15
        cfg.timeoutIntervalForResource = 30
        // 允许 HTTP（自托管服务可能没 HTTPS）
        cfg.waitsForConnectivity = true
        self.session = URLSession(configuration: cfg)
        self.decoder = JSONDecoder()
    }

    // MARK: - 底层请求

    private func request<T: Decodable>(_ url: URL,
                                       method: String = "GET",
                                       body: Data? = nil,
                                       deviceToken: String? = nil,
                                       decodeAs: T.Type) async throws -> T {
        var req = URLRequest(url: url)
        req.httpMethod = method
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let dt = deviceToken, !dt.isEmpty {
            req.setValue("Bearer \(dt)", forHTTPHeaderField: "Authorization")
        }
        if let body = body { req.httpBody = body }

        do {
            let (data, resp) = try await session.data(for: req)
            guard let http = resp as? HTTPURLResponse else {
                throw APIError.network("非 HTTP 响应")
            }
            if !(200...299).contains(http.statusCode) {
                let body = String(data: data, encoding: .utf8) ?? ""
                throw APIError.http(status: http.statusCode, body: body)
            }
            do {
                return try decoder.decode(T.self, from: data)
            } catch {
                throw APIError.decode("\(error)")
            }
        } catch let e as APIError {
            throw e
        } catch {
            throw APIError.network("\(error)")
        }
    }

    // MARK: - 注册设备（对应 POST /register）

    /// 用 Master Token 注册一台新设备，返回 device_id + device_token。
    public func register(serverURL: String,
                         masterToken: String,
                         deviceName: String) async throws -> RegisterResponse {
        let base = sanitizeBase(serverURL)
        let url = try makeURL(base, path: "/register")
        let body = try JSONEncoder().encode([
            "master_token": masterToken,
            "device_name": deviceName,
        ])
        return try await request(url, method: "POST", body: body, decodeAs: RegisterResponse.self)
    }

    // MARK: - 聚合查询（对应 GET /usage）

    /// 查询指定设备集合在时间范围内的 overall + by_model + trend。
    public func usage(serverURL: String,
                     deviceToken: String,
                     fromMs: Int,
                     toMs: Int,
                     bucket: TrendBucket,
                     filter: DeviceFilter = .all,
                     localDeviceId: String = "") async throws -> RemoteUsage {
        let base = sanitizeBase(serverURL)
        var comps: [URLQueryItem] = [
            URLQueryItem(name: "from_ms", value: String(fromMs)),
            URLQueryItem(name: "to_ms", value: String(toMs)),
            URLQueryItem(name: "bucket", value: bucket.rawValue),
        ]
        applyFilter(filter, localDeviceId: localDeviceId, into: &comps)
        let url = try makeURL(base, path: "/usage", query: comps)
        return try await request(url, deviceToken: deviceToken, decodeAs: RemoteUsage.self)
    }

    // MARK: - 额度快照（对应 GET /snapshots）

    public func snapshots(serverURL: String,
                         deviceToken: String,
                         fromMs: Int,
                         toMs: Int,
                         filter: DeviceFilter = .all,
                         localDeviceId: String = "") async throws -> [RemoteSnapshot] {
        let base = sanitizeBase(serverURL)
        var comps: [URLQueryItem] = [
            URLQueryItem(name: "from_ms", value: String(fromMs)),
            URLQueryItem(name: "to_ms", value: String(toMs)),
        ]
        applyFilter(filter, localDeviceId: localDeviceId, into: &comps)
        let url = try makeURL(base, path: "/snapshots", query: comps)
        struct Wrapper: Codable { var snapshots: [RemoteSnapshot] }
        let wrapped = try await request(url, deviceToken: deviceToken, decodeAs: Wrapper.self)
        return wrapped.snapshots
    }

    // MARK: - 周期明细（对应 POST /period_detail）

    public func periodDetail(serverURL: String,
                             deviceToken: String,
                             periods: [(Int, Int)],
                             filter: DeviceFilter = .all,
                             localDeviceId: String = "") async throws -> [RemotePeriodDetail] {
        let base = sanitizeBase(serverURL)
        let url = try makeURL(base, path: "/period_detail")
        var body: [String: Any] = [
            "periods": periods.map { [$0, $1] },
        ]
        applyFilterBody(filter, localDeviceId: localDeviceId, into: &body)
        let data = try JSONSerialization.data(withJSONObject: body)
        struct Wrapper: Codable { var buckets: [RemotePeriodDetail] }
        let wrapped = try await request(url, method: "POST", body: data,
                                        deviceToken: deviceToken, decodeAs: Wrapper.self)
        return wrapped.buckets
    }

    // MARK: - 设备列表（对应 GET /devices）

    public func devices(serverURL: String,
                        deviceToken: String) async throws -> [DeviceInfo] {
        let base = sanitizeBase(serverURL)
        let url = try makeURL(base, path: "/devices")
        return try await request(url, deviceToken: deviceToken, decodeAs: [DeviceInfo].self)
    }

    // MARK: - 清理状态（对应 GET /cleanup/status）

    public func cleanupStatus(serverURL: String,
                              deviceToken: String) async throws -> CleanupStatus {
        let base = sanitizeBase(serverURL)
        let url = try makeURL(base, path: "/cleanup/status")
        return try await request(url, deviceToken: deviceToken, decodeAs: CleanupStatus.self)
    }

    // MARK: - 健康检查（GET /health）

    /// 探测服务端是否可达，返回 true/false（不抛错）。
    public func health(_ serverURL: String) async -> Bool {
        guard let base = try? sanitizeBase(serverURL),
              let url = try? makeURL(base, path: "/health") else { return false }
        var req = URLRequest(url: url)
        req.timeoutInterval = 6
        do {
            let (_, resp) = try await session.data(for: req)
            return (resp as? HTTPURLResponse)?.statusCode == 200
        } catch {
            return false
        }
    }

    // MARK: - 辅助

    /// 去除末尾斜杠，保留 scheme。
    /// 用户可能填 "http://192.168.1.100:3838" 或 "http://192.168.1.100:3838/"
    private func sanitizeBase(_ s: String) throws -> String {
        var t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        while t.hasSuffix("/") { t.removeLast() }
        guard !t.isEmpty else { throw APIError.notConfigured }
        return t
    }

    private func makeURL(_ base: String, path: String, query: [URLQueryItem]? = nil) throws -> URL {
        guard var comps = URLComponents(string: base + path) else {
            throw APIError.network("非法的服务器地址：\(base)")
        }
        comps.queryItems = query
        guard let url = comps.url else {
            throw APIError.network("无法构造 URL：\(base)\(path)")
        }
        return url
    }

    private func applyFilter(_ filter: DeviceFilter,
                             localDeviceId: String,
                             into comps: inout [URLQueryItem]) {
        switch filter {
        case .all:
            break
        case .local:
            // 仅本机：用 devices=localDeviceId
            if !localDeviceId.isEmpty {
                comps.append(URLQueryItem(name: "devices", value: localDeviceId))
            }
        case .specific(let id):
            comps.append(URLQueryItem(name: "devices", value: id))
        }
    }

    private func applyFilterBody(_ filter: DeviceFilter,
                                  localDeviceId: String,
                                  into body: inout [String: Any]) {
        switch filter {
        case .all:
            break
        case .local:
            if !localDeviceId.isEmpty { body["devices"] = localDeviceId }
        case .specific(let id):
            body["devices"] = id
        }
    }
}

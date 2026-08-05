//
//  QuotaClient.swift
//  ZBar
//
//  直连智谱开放平台的额度接口（不经过自托管 server）。
//  翻译自 src-tauri/src/quota.rs::query_quota 的请求 + 解析逻辑。
//

import Foundation

public actor QuotaClient {
    public static let shared = QuotaClient()

    private let session: URLSession
    private let decoder: JSONDecoder

    public init() {
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 12
        cfg.timeoutIntervalForResource = 20
        self.session = URLSession(configuration: cfg)
        self.decoder = JSONDecoder()
    }

    // MARK: - 智谱接口原始结构

    private struct BigModelResponse: Decodable {
        var success: Bool?
        var msg: String?
        var data: BigModelData?
    }

    private struct BigModelData: Decodable {
        var level: String?
        var limits: [QuotaLimit]?
    }

    /// 查询额度（对应 quota.rs::query_quota）。返回解析后的三档结果。
    public func query(cfg: QuotaConfig) async throws -> QuotaResult {
        guard !cfg.token.trimmingCharacters(in: .whitespaces).isEmpty else {
            throw APIError.notConfigured
        }
        let base = cfg.endpoint.base
        guard let url = URL(string: "\(base)/api/monitor/usage/quota/limit") else {
            throw APIError.network("非法的额度接口 URL")
        }
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        req.setValue(cfg.token.trimmingCharacters(in: .whitespaces),
                     forHTTPHeaderField: "Authorization")

        let (data, resp): (Data, URLResponse)
        do {
            (data, resp) = try await session.data(for: req)
        } catch {
            throw APIError.network("请求额度接口失败：\(error)")
        }
        guard let http = resp as? HTTPURLResponse else {
            throw APIError.network("非 HTTP 响应")
        }
        if !(200...299).contains(http.statusCode) {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw APIError.http(status: http.statusCode, body: body)
        }

        let raw: BigModelResponse
        do {
            raw = try decoder.decode(BigModelResponse.self, from: data)
        } catch {
            throw APIError.decode("解析额度响应失败：\(error)")
        }

        guard raw.success == true else {
            throw APIError.http(status: 0,
                                body: raw.msg?.isEmpty == false ? raw.msg! : "额度接口返回失败")
        }
        guard let d = raw.data else {
            throw APIError.decode("额度响应缺少 data 字段")
        }

        let limits = d.limits ?? []

        // MCP 月度额度：type=TIME_LIMIT
        let mcp = limits.first { $0.type == "TIME_LIMIT" }

        // token 额度（TOKENS_LIMIT）
        let tokenLimits = limits.filter { $0.type == "TOKENS_LIMIT" }

        // (unit=3, number=5) = 5 小时；(unit=6, number=1) = 每周
        let hour5 = tokenLimits.first { $0.unit == 3 && $0.number == 5 }
            ?? tokenLimits.first { $0.unit == 3 }
            ?? tokenLimits.min { a, b in
                rankWindow(a) < rankWindow(b)
            }
        let weekly = tokenLimits.first { $0.unit == 6 && $0.number == 1 }
            ?? tokenLimits.first { $0.unit == 6 }

        return QuotaResult(level: d.level ?? "",
                           hour5: hour5,
                           weekly: weekly,
                           mcp: mcp)
    }

    /// 窗口排序兜底：按 (unit 权重, number) 排，5 小时窗口优先。
    private func rankWindow(_ l: QuotaLimit) -> Int {
        // unit=3（小时）优先于 unit=6（周）
        let u = l.unit ?? 0
        let n = l.number ?? 0
        return u * 1000 + n
    }
}

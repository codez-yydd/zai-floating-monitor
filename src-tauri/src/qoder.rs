//! Qoder（大模型积分订阅）额度查询模块。
//!
//! 凭证型：kind=cookie 的 secret 是用户从浏览器复制的 Cookie 请求头（或整段
//! Copy as cURL 粘贴，由 cookie_util::normalize_cookie_secret 归一），按
//! Chrome 浏览器仿真头请求用量接口。host 按凭证 region 分流：
//! - `region == Some("global")` → https://qoder.com（国际站）
//! - 其余（None 或 "cn"）→ https://qoder.com.cn（中国站，默认）
//!
//! 数据来源：GET {host}/api/v2/me/usages/big_model_credits，头为
//! chrome_like_headers 产物 + `X-Requested-With: XMLHttpRequest` +
//! `Bx-V: 2.5.35`（Origin 用所选站点根，Referer 用 {host}/account/usage）。
//!
//! 响应字段 camelCase/snake_case 双兼容（每对都试）：
//! `{"totalQuota":{"quotaSummary":{"usedValue":30,"limitValue":100,
//!   "remainingValue":70,"usagePercentage":30,"unit":"credits"}},
//!  "sharedQuota":{...同构...},"nextResetAt":"2030-10-27T00:00:00Z"}`
//! - 顶层 sharedQuota 存在时与 totalQuota 的 used/limit/remaining 逐项相加；
//!   两项 usage_percentage 不可直接相加，合并池按合并后 used/limit 重算；
//! - nextResetAt|next_reset_at 支持 ISO-8601 或 epoch 秒/毫秒（自适应）。
//!
//! 窗口：单积分窗 key="credits" title="大模型积分"，usedPercent 优先服务端
//! usage_percentage（缺失时 used/limit*100），used/total 原始数值带上。
//! 401/403 → expired「会话已失效，请重新登录 qoder.com 后更新 Cookie」；
//! 缺 quotaSummary 结构 → error「响应格式无法解析」。
//!
//! 工程纪律（对齐 minimax.rs）：网络 ureq 同步 + 15s 超时 + resolve_proxy，
//! 调用方 spawn_blocking；解析纯函数可单测；错误消息中文且不含 secret；
//! Cookie 值不进任何日志。

use crate::cookie_util::{chrome_like_headers, normalize_cookie_secret, parse_time_flexible};
use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, get_any, now_ms, num_any, quota_http_agent, ProviderQuotaEntry,
    ProviderQuotaWindow,
};

/// region → 站点根 URL（纯函数，便于单测）：global 走国际站 qoder.com，
/// 其余（None/"cn"/未知值）默认中国站 qoder.com.cn。
fn host_for_region(region: Option<&str>) -> &'static str {
    if region == Some("global") {
        "https://qoder.com"
    } else {
        "https://qoder.com.cn"
    }
}

/// 逐凭证查询 Qoder 大模型积分（串行；单凭证失败产出 error/expired 条目，
/// 不阻塞其他凭证）。只消费 kind=cookie 的凭证，由 provider_quota 骨架分发。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .filter(|cred| cred.kind == "cookie")
        .map(|cred| {
            // Cookie 内容支持裸串 / 整段 cURL 粘贴，查询时归一（保存原样）
            let cookie = normalize_cookie_secret(&cred.secret);
            let raw = if cookie.is_empty() {
                Err("未能从粘贴内容中解析出 Cookie，请重新复制请求头或 cURL 命令".to_string())
            } else {
                fetch_credits_raw(&agent, host_for_region(cred.region.as_deref()), &cookie)
            };
            entry_from_raw(&cred.id, &cred.label, &raw)
        })
        .collect()
}

/// 单凭证查询（网络层）：GET {host}/api/v2/me/usages/big_model_credits，
/// 浏览器仿真头 + XHR 标记。返回展平的 (HTTP 状态码, 响应体)；网络层彻底
/// 失败返回 Err（中文原因，不含 secret）。解析交给 entry_from_raw 纯函数。
fn fetch_credits_raw(
    agent: &ureq::Agent,
    host: &str,
    cookie: &str,
) -> Result<(u16, Option<String>), String> {
    let mut req = agent.get(&format!("{host}/api/v2/me/usages/big_model_credits"));
    for (name, value) in chrome_like_headers(cookie, host, &format!("{host}/account/usage")) {
        req = req.set(&name, &value);
    }
    let resp = req
        .set("X-Requested-With", "XMLHttpRequest")
        .set("Bx-V", "2.5.35")
        .call();
    flatten_response(resp).map_err(|e| format!("Qoder 额度{e}"))
}

/// quotaSummary 的内存形态（双兼容键归一后）。
struct SummaryFields {
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    percentage: Option<f64>,
    unit: Option<String>,
}

/// 从 quota 对象（totalQuota / sharedQuota）取 quotaSummary 字段组
///（纯函数）：quotaSummary|quota_summary 缺失或非对象 → None。
fn summary_fields(quota: &serde_json::Value) -> Option<SummaryFields> {
    let summary = get_any(quota, &["quotaSummary", "quota_summary"])
        .filter(|s| s.is_object())?;
    Some(SummaryFields {
        used: num_any(summary, &["usedValue", "used_value"]),
        limit: num_any(summary, &["limitValue", "limit_value"]),
        remaining: num_any(summary, &["remainingValue", "remaining_value"]),
        percentage: num_any(summary, &["usagePercentage", "usage_percentage"]),
        unit: get_any(summary, &["unit"])
            .and_then(|u| u.as_str())
            .map(str::to_string),
    })
}

/// Option 数值逐项相加（任一侧有值即保留；双侧缺失保持 None）。
fn add_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// 已用/总量 → 百分比（纯函数）：total 缺失或 ≤0 → None（前端只展示
/// used/total）；结果 clamp 到 0-100。
fn pct_of(used: Option<f64>, total: Option<f64>) -> Option<f64> {
    match (used, total) {
        (Some(u), Some(t)) if t > 0.0 => Some(((u / t) * 100.0).clamp(0.0, 100.0)),
        _ => None,
    }
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败(error) > 缺 totalQuota/quotaSummary 结构(error「响应格式
/// 无法解析」) > 成功(ok + 单积分窗)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), String>,
) -> ProviderQuotaEntry {
    // 失败条目构造（windows 恒空；message 承载原因）
    let fail = |status: &str, message: String| ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message),
        updated_at: now_ms(),
    };

    let Ok((http_status, body)) = raw else {
        return fail("error", format!("Qoder 额度{}", raw.as_ref().unwrap_err()));
    };
    // 会话被服务端拒绝：Cookie 失效（凭证卡显示「已过期」徽章）
    if *http_status == 401 || *http_status == 403 {
        return fail(
            "expired",
            "会话已失效，请重新登录 qoder.com 后更新 Cookie".to_string(),
        );
    }
    if *http_status != 200 {
        return fail("error", format!("Qoder 额度查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref() else {
        return fail("error", "Qoder 额度响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "Qoder 额度响应解析失败".to_string());
    };
    let Some(total_quota) = get_any(&v, &["totalQuota", "total_quota"]).filter(|q| q.is_object())
    else {
        return fail("error", "响应格式无法解析".to_string());
    };
    let Some(total) = summary_fields(total_quota) else {
        return fail("error", "响应格式无法解析".to_string());
    };
    let shared = get_any(&v, &["sharedQuota", "shared_quota"])
        .filter(|q| q.is_object())
        .and_then(summary_fields);

    // sharedQuota 存在时逐项相加合并（unit 取主池，缺失回退共享池）
    let mut used = total.used;
    let mut limit = total.limit;
    let mut remaining = total.remaining;
    let mut unit = total.unit;
    if let Some(s) = &shared {
        used = add_opt(used, s.used);
        limit = add_opt(limit, s.limit);
        remaining = add_opt(remaining, s.remaining);
        unit = unit.or_else(|| s.unit.clone());
    }
    // usedValue 缺失时按 limit - remaining 回退（合并后同口径）
    let used = used.or_else(|| match (limit, remaining) {
        (Some(l), Some(r)) => Some((l - r).max(0.0)),
        _ => None,
    });
    // 百分比：单池优先服务端 usage_percentage（缺失 used/limit 回退）；
    // 合并池两项百分比不可直接相加，按合并后 used/limit 重算（limit 不可用
    // 时不给百分比，前端只展示 used/total）
    let used_percent = if shared.is_some() {
        pct_of(used, limit)
    } else {
        total.percentage.or_else(|| pct_of(used, limit))
    };

    // 重置时间：顶层优先，totalQuota / quotaSummary 内也容忍（ISO-8601 或
    // epoch 秒/毫秒自适应）
    let resets_at = get_any(&v, &["nextResetAt", "next_reset_at"])
        .or_else(|| get_any(total_quota, &["nextResetAt", "next_reset_at"]))
        .and_then(parse_time_flexible);

    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows: vec![ProviderQuotaWindow {
            key: "credits".to_string(),
            title: "大模型积分".to_string(),
            used_percent,
            used,
            total: limit,
            unit: Some(unit.unwrap_or_else(|| "积分".to_string())),
            resets_at,
        }],
        balance: None,
        plan_name: None,
        message: None,
        updated_at: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "qd-1";
    const LABEL: &str = "主账号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// camelCase 样例：成功路径（服务端百分比、unit、ISO-8601 重置时间）。
    #[test]
    fn parses_camel_case_sample() {
        let raw = ok_raw(
            r#"{"totalQuota":{"quotaSummary":{"usedValue":30,"limitValue":100,
               "remainingValue":70,"usagePercentage":30,"unit":"credits"}},
               "nextResetAt":"2030-10-27T05:06:07Z"}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert_eq!(entry.windows.len(), 1);
        let w = &entry.windows[0];
        assert_eq!(w.key, "credits");
        assert_eq!(w.title, "大模型积分");
        assert_eq!(w.used_percent, Some(30.0)); // 服务端 usage_percentage 优先
        assert_eq!(w.used, Some(30.0));
        assert_eq!(w.total, Some(100.0));
        assert_eq!(w.unit.as_deref(), Some("credits"));
        assert_eq!(w.resets_at, Some(1_919_307_967_000)); // ISO-8601 → ms
    }

    /// snake_case 样例：字段名全换蛇形仍可解析；缺失 usage_percentage 时
    /// 按 used/limit*100 回退。
    #[test]
    fn parses_snake_case_and_percent_fallback() {
        let raw = ok_raw(
            r#"{"total_quota":{"quota_summary":{"used_value":25,"limit_value":200,
               "remaining_value":175}}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        let w = &entry.windows[0];
        assert_eq!(w.used_percent, Some(12.5)); // 25/200*100 回退
        assert_eq!(w.used, Some(25.0));
        assert_eq!(w.total, Some(200.0));
        // unit 缺失 → 默认「积分」；重置时间缺失 → None
        assert_eq!(w.unit.as_deref(), Some("积分"));
        assert_eq!(w.resets_at, None);
    }

    /// sharedQuota 合并：used/limit/remaining 逐项相加；百分比按合并池重算
    ///（30/100 与 50/100 合并 → 80/200 = 40%，而非 30+50=80%）。
    #[test]
    fn shared_quota_merges_and_recomputes_percent() {
        let raw = ok_raw(
            r#"{"totalQuota":{"quotaSummary":{"usedValue":30,"limitValue":100,
               "remainingValue":70,"usagePercentage":30,"unit":"credits"}},
               "sharedQuota":{"quotaSummary":{"usedValue":50,"limitValue":100,
               "remainingValue":50,"usagePercentage":50}},
               "next_reset_at":1919289600}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        let w = &entry.windows[0];
        assert_eq!(w.used, Some(80.0)); // 30 + 50
        assert_eq!(w.total, Some(200.0)); // 100 + 100
        assert_eq!(w.used_percent, Some(40.0)); // 80/200 重算
        assert_eq!(w.resets_at, Some(1_919_289_600_000)); // epoch 秒 → 毫秒
    }

    /// HTTP 401/403 → expired「会话已失效，请重新登录 qoder.com 后更新
    /// Cookie」（假 Cookie 手测链路的预期分支）。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some(r#"{"detail":"unauthorized"}"#.to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(
                entry.message.as_deref(),
                Some("会话已失效，请重新登录 qoder.com 后更新 Cookie")
            );
            assert!(entry.windows.is_empty());
            // 错误消息不得回显响应体（可能含敏感信息）
        }
    }

    /// 缺 totalQuota / quotaSummary 结构 → error「响应格式无法解析」。
    #[test]
    fn missing_summary_structure_maps_to_error() {
        for body in [
            r#"{"detail":"Not found."}"#,
            r#"{"totalQuota":{"other":1}}"#,
            r#"{}"#,
        ] {
            let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(body));
            assert_eq!(entry.status, "error", "body={body}");
            assert_eq!(entry.message.as_deref(), Some("响应格式无法解析"));
        }
    }

    /// 网络层失败 / 非 200 非 401/403 → error（原因透传，不含 secret）。
    #[test]
    fn network_and_server_failures_map_to_error() {
        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));

        let raw = Ok((500, Some("internal error".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
    }

    /// region → 站点分流：global 国际站 qoder.com，其余默认中国站。
    #[test]
    fn region_maps_to_host() {
        assert_eq!(host_for_region(Some("global")), "https://qoder.com");
        assert_eq!(host_for_region(Some("cn")), "https://qoder.com.cn");
        assert_eq!(host_for_region(None), "https://qoder.com.cn");
        assert_eq!(host_for_region(Some("weird")), "https://qoder.com.cn");
    }

    /// 无法解析出 Cookie 的空串入参在 fetch 层即产出 error（不发起请求），
    /// 归一逻辑本身由 cookie_util 单测覆盖；这里验证非 cookie 凭证被过滤。
    #[test]
    fn non_cookie_credentials_are_skipped() {
        let snapshots = [
            CredentialQuerySnapshot {
                id: "a".into(),
                label: "apiKey 条目".into(),
                kind: "apiKey".into(),
                secret: "sk-x".into(),
                region: None,
            },
            CredentialQuerySnapshot {
                id: "b".into(),
                label: "cookie 条目".into(),
                kind: "cookie".into(),
                // 无法解析的 cURL → 该条目产出 error，而非 panic/跳过
                secret: "curl 'https://qoder.com' -H 'User-Agent: x'".into(),
                region: None,
            },
        ];
        let entries = fetch_quota_entries(&snapshots);
        assert_eq!(entries.len(), 1, "apiKey 凭证不应被 cookie 型 provider 消费");
        assert_eq!(entries[0].credential_id, "b");
        assert_eq!(entries[0].status, "error");
        assert!(entries[0].message.as_ref().unwrap().contains("解析出 Cookie"));
    }
}

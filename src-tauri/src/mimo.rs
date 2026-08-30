//! 小米 MiMo 开放平台额度查询模块。
//!
//! 凭证型：kind=cookie 的 secret 是用户从浏览器复制的 Cookie 请求头（或整段
//! Copy as cURL 粘贴，由 cookie_util::normalize_cookie_secret 归一）。必需
//! cookie：`api-platform_serviceToken` + `userId`（对齐 CodexBar
//! MiMoCookieHeader.requiredCookieNames；其余域内 cookie 一并携带）。
//!
//! 端点（对齐 CodexBar MiMoUsageFetcher，单一域名无区域之分）：
//! - 余额（必需成功）：GET `{BASE}/balance`
//! - Token 套餐详情（失败静默）：GET `{BASE}/tokenPlan/detail`
//! - Token 套餐用量（失败静默）：GET `{BASE}/tokenPlan/usage`
//!   BASE = https://platform.xiaomimimo.com/api/v1
//! 请求头（CodexBar fetchAuthenticated 照抄口径）：浏览器仿真头
//! （chrome_like_headers：Cookie / Accept / Accept-Language / Chrome UA /
//! Origin / Referer，Referer 用 #/console/balance 页）+ `x-timeZone`。
//!
//! 响应信封 `{code: 0, message, data}`：
//! - balance：`data{balance:"12.34"(字符串数字), currency:"CNY",
//!   cashBalance, giftBalance}`（paid/granted 拆分，映射 toppedUp/granted）；
//! - tokenPlan/detail：`data{planCode, currentPeriodEnd:"yyyy-MM-dd HH:mm:ss"
//!   (UTC), expired}`（planCode → plan_name，periodEnd → 窗口重置时间）；
//! - tokenPlan/usage：`data{monthUsage:{percent(0-1), items:[{name, used,
//!   limit, percent(0-1)}]}}` 取 items[0] →「当前积分」窗（percent 0-1，
//!   usedPercent=percent*100）。
//!
//! 错误映射（对齐 CodexBar）：HTTP 401/403 → expired；响应体为 HTML（会话
//! 过期被重定向到登录页，ureq 自动跟随 3xx 后的落点）→ expired；信封
//! code 401/403 → expired；其他非 0 → error（带 message）。
//!
//! 工程纪律（对齐 qoder.rs）：网络 ureq 同步 + 15s 超时 + resolve_proxy，
//! 调用方 spawn_blocking；解析纯函数可单测；错误消息中文且不含 secret；
//! Cookie 值不进任何日志。

use crate::cookie_util::{chrome_like_headers, normalize_cookie_secret, parse_time_flexible};
use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, get_any, now_ms, num_any, quota_http_agent, ProviderQuotaBalance,
    ProviderQuotaEntry, ProviderQuotaWindow,
};

/// API 根（单一域名，无区域之分；region 字段被有意忽略）。
const BASE: &str = "https://platform.xiaomimimo.com/api/v1";

/// 必需 cookie 名（CodexBar MiMoCookieHeader.requiredCookieNames）。
const REQUIRED_COOKIES: [&str; 2] = ["api-platform_serviceToken", "userId"];

// ============================================================
// 网络层（ureq 同步；调用方 spawn_blocking）
// ============================================================

/// 单凭证三端点的抓取结果（balance 必需，detail/usage 失败静默为 None）。
struct QuotaBundle {
    balance: Result<(u16, Option<String>), String>,
    detail: Option<(u16, Option<String>)>,
    usage: Option<(u16, Option<String>)>,
}

/// 逐凭证查询 MiMo 余额与 Token 套餐用量（串行；单凭证失败产出 error/expired
/// 条目，不阻塞其他凭证）。只消费 kind=cookie 的凭证，由 provider_quota 骨架分发。
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
            let bundle = if !has_required_cookies(&cookie) {
                QuotaBundle {
                    balance: Err("缺少必需 Cookie（api-platform_serviceToken / userId）".to_string()),
                    detail: None,
                    usage: None,
                }
            } else {
                fetch_bundle(&agent, &cookie)
            };
            entry_from_bundle(&cred.id, &cred.label, &bundle)
        })
        .collect()
}

/// 抓取单凭证的三端点（balance 失败保留原始 Result 供状态判定；
/// detail/usage 对齐 CodexBar 的 `try?` 语义，任何失败静默为 None）。
fn fetch_bundle(agent: &ureq::Agent, cookie: &str) -> QuotaBundle {
    QuotaBundle {
        balance: fetch_api_raw(agent, "balance", cookie),
        detail: fetch_api_raw(agent, "tokenPlan/detail", cookie).ok(),
        usage: fetch_api_raw(agent, "tokenPlan/usage", cookie).ok(),
    }
}

/// 单端点请求（网络层）：GET `{BASE}/{path}`，头为浏览器仿真 + x-timeZone。
/// 返回展平的 (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，
/// 不含 secret）。解析交给 entry_from_bundle 纯函数。
fn fetch_api_raw(
    agent: &ureq::Agent,
    path: &str,
    cookie: &str,
) -> Result<(u16, Option<String>), String> {
    let mut req = agent.get(&format!("{BASE}/{path}"));
    for (name, value) in chrome_like_headers(
        cookie,
        "https://platform.xiaomimimo.com",
        "https://platform.xiaomimimo.com/#/console/balance",
    ) {
        req = req.set(&name, &value);
    }
    let resp = req.set("x-timeZone", "UTC+01:00").call();
    flatten_response(resp).map_err(|e| format!("MiMo 额度{e}"))
}

// ============================================================
// 解析纯函数（网络无关，单测直接构造输入）
// ============================================================

/// 归一后的 cookie 串是否含全部必需 cookie 名（纯函数；顺序无关，
/// 其余 cookie 共存不受影响）。
fn has_required_cookies(cookie: &str) -> bool {
    REQUIRED_COOKIES.iter().all(|required| {
        cookie.split(';').any(|part| {
            let name = part.split('=').next().unwrap_or("").trim();
            name.eq_ignore_ascii_case(required)
        })
    })
}

/// 信封语义判定（纯函数）：`{code, message}` 中 code 非 0 时的条目状态与
/// 原因 —— 401/403 → expired「会话已失效…」；其他 → error（带 message）。
fn envelope_failure(code: f64, message: Option<&str>) -> (String, String) {
    if code == 401.0 || code == 403.0 {
        return (
            "expired".to_string(),
            "会话已失效，请重新登录 platform.xiaomimimo.com 后更新 Cookie".to_string(),
        );
    }
    let reason = match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None => format!("code {code}"),
    };
    ("error".to_string(), format!("MiMo 平台返回错误: {reason}"))
}

/// percent（0-1）→ 已用百分比：percent*100，clamp 0-100，保留两位小数
///（消除浮点尾差，展示口径）；越出 0-1 视为脏值（None）。
fn used_percent_from_ratio(ratio: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&ratio) {
        return None;
    }
    Some(((ratio * 100.0).clamp(0.0, 100.0) * 100.0).round() / 100.0)
}

/// 解析单凭证抓取结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：缺必需 cookie/网络失败(error) > 401/403(expired) > 非 200
/// (error) > HTML 登录页(expired，会话过期重定向落点) > body 解析失败(error)
/// > 信封非 0（401/403 → expired，其余 → error）> 缺余额数据(error) >
/// 成功(ok + 余额 + 可选 Token 套餐窗)。
fn entry_from_bundle(
    cred_id: &str,
    label: &str,
    bundle: &QuotaBundle,
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

    let Ok((http_status, body)) = &bundle.balance else {
        return fail("error", format!("MiMo 额度{}", bundle.balance.as_ref().unwrap_err()));
    };
    // 会话被服务端拒绝：Cookie 失效（凭证卡显示「已过期」徽章）
    if *http_status == 401 || *http_status == 403 {
        return fail(
            "expired",
            "会话已失效，请重新登录 platform.xiaomimimo.com 后更新 Cookie".to_string(),
        );
    }
    if *http_status != 200 {
        return fail("error", format!("MiMo 额度查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref().filter(|b| !b.trim().is_empty()) else {
        return fail("error", "MiMo 额度响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        // 会话过期时 API 会 3xx 重定向到登录页（ureq 自动跟随），落点为
        // HTML——JSON 解析必失败，按会话失效处理（对齐 CodexBar 3xx →
        // loginRequired 的语义）
        if body.trim_start().starts_with('<') {
            return fail(
                "expired",
                "会话已失效，请重新登录 platform.xiaomimimo.com 后更新 Cookie".to_string(),
            );
        }
        return fail("error", "MiMo 额度响应解析失败".to_string());
    };
    // 业务信封：code != 0 按语义分流
    let code = num_any(&v, &["code"]).unwrap_or(0.0);
    if code != 0.0 {
        let message = get_any(&v, &["message"]).and_then(|m| m.as_str());
        let (status, reason) = envelope_failure(code, message);
        return fail(&status, reason);
    }
    // 余额必需字段：balance（字符串数字）+ currency
    let Some(data) = get_any(&v, &["data"]).filter(|d| d.is_object()) else {
        return fail("error", "MiMo 额度响应缺少余额数据".to_string());
    };
    let Some(amount) = num_any(data, &["balance"]) else {
        return fail("error", "MiMo 额度响应缺少余额数据".to_string());
    };
    let Some(currency) = get_any(data, &["currency"])
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
    else {
        return fail("error", "MiMo 额度响应缺少余额数据".to_string());
    };
    let balance = ProviderQuotaBalance {
        amount,
        currency: currency.clone(),
        // paid/granted 拆分：cashBalance → toppedUp、giftBalance → granted
        granted: num_any(data, &["giftBalance"]),
        topped_up: num_any(data, &["cashBalance"]),
    };

    // Token 套餐（非致命）：detail 给 plan 名与周期结束，usage 给当前积分窗
    let (plan_name, period_end) = bundle
        .detail
        .as_ref()
        .and_then(|(status, body)| parse_token_plan_detail(*status, body.as_deref()))
        .unwrap_or((None, None));

    let usage_window = bundle.usage.as_ref().and_then(|(status, body)| {
        parse_token_plan_usage(*status, body.as_deref(), period_end)
    });

    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows: usage_window.into_iter().collect(),
        balance: Some(balance),
        plan_name,
        message: None,
        updated_at: now_ms(),
    }
}

/// tokenPlan/detail 解析（纯函数）：200 + code==0 时取
/// (planCode, currentPeriodEnd)；其余（非 200 / 信封错误 / 结构变化）→ None
///（详情非致命，静默降级）。
fn parse_token_plan_detail(
    http_status: u16,
    body: Option<&str>,
) -> Option<(Option<String>, Option<i64>)> {
    if http_status != 200 {
        return None;
    }
    let v = serde_json::from_str::<serde_json::Value>(body?.trim()).ok()?;
    if num_any(&v, &["code"]).unwrap_or(1.0) != 0.0 {
        return None;
    }
    let data = get_any(&v, &["data"]).filter(|d| d.is_object())?;
    let plan_code = get_any(data, &["planCode", "plan_code"])
        .and_then(|p| p.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    // currentPeriodEnd："yyyy-MM-dd HH:mm:ss"（UTC 解释）等，弹性解析
    let period_end = get_any(data, &["currentPeriodEnd", "current_period_end"])
        .and_then(parse_time_flexible);
    Some((plan_code, period_end))
}

/// tokenPlan/usage 解析（纯函数）：200 + code==0 时取 monthUsage.items[0]
/// 的 used/limit/percent（0-1）→「当前积分」窗；items 取不到时回退
/// monthUsage.percent（仅百分比）。其余情况 → None（用量非致命）。
fn parse_token_plan_usage(
    http_status: u16,
    body: Option<&str>,
    resets_at: Option<i64>,
) -> Option<ProviderQuotaWindow> {
    if http_status != 200 {
        return None;
    }
    let v = serde_json::from_str::<serde_json::Value>(body?.trim()).ok()?;
    if num_any(&v, &["code"]).unwrap_or(1.0) != 0.0 {
        return None;
    }
    let month = get_any(&v, &["data"])
        .and_then(|d| get_any(d, &["monthUsage", "month_usage"]))?;
    let item = get_any(month, &["items"])
        .and_then(|i| i.as_array())
        .and_then(|items| items.first());
    let used = item.and_then(|it| num_any(it, &["used"]));
    let total = item.and_then(|it| num_any(it, &["limit"]));
    // percent：items[0] 优先，缺失时 monthUsage.percent 回退（均 0-1 口径）
    let ratio = item
        .and_then(|it| num_any(it, &["percent"]))
        .or_else(|| num_any(month, &["percent"]));
    if used.is_none() && total.is_none() && ratio.is_none() {
        return None;
    }
    Some(ProviderQuotaWindow {
        key: "credits".to_string(),
        title: "当前积分".to_string(),
        used_percent: ratio.and_then(used_percent_from_ratio),
        used,
        total,
        unit: Some("积分".to_string()),
        resets_at,
    })
}

// ============================================================
// 单元测试（纯函数，不联网）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "mimo-1";
    const LABEL: &str = "MiMo 主号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// detail/usage 用的成功元组形态（已剥 Result，与 QuotaBundle 字段对齐）。
    fn ok_tuple(body: &str) -> (u16, Option<String>) {
        (200, Some(body.to_string()))
    }

    /// 余额 + Token 套餐样例（对齐 CodexBar 解码结构：字符串数字、0-1
    /// percent、`yyyy-MM-dd HH:mm:ss` UTC 周期结束）。
    #[test]
    fn parses_balance_and_token_plan() {
        let bundle = QuotaBundle {
            balance: ok_raw(
                r#"{"code":0,"message":"","data":{"balance":"12.34","currency":"CNY",
                   "cashBalance":"10.00","giftBalance":"2.34"}}"#,
            ),
            detail: Some(ok_tuple(
                r#"{"code":0,"message":"","data":{"planCode":"mimo_pro",
                   "currentPeriodEnd":"2030-10-27 05:06:07","expired":false}}"#,
            )),
            usage: Some(ok_tuple(
                r#"{"code":0,"message":"","data":{"monthUsage":{"percent":0.55,
                   "items":[{"name":"模型调用","used":450,"limit":1000,"percent":0.45}]}}}"#,
            )),
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);

        // 余额：amount/currency + paid/granted 拆分（cashBalance→toppedUp）
        let balance = entry.balance.expect("余额必须产出");
        assert_eq!(balance.amount, 12.34);
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.topped_up, Some(10.0));
        assert_eq!(balance.granted, Some(2.34));

        // Token 套餐窗：items[0] percent 0-1 → 45%
        assert_eq!(entry.windows.len(), 1);
        let w = &entry.windows[0];
        assert_eq!(w.key, "credits");
        assert_eq!(w.title, "当前积分");
        assert_eq!(w.used_percent, Some(45.0));
        assert_eq!(w.used, Some(450.0));
        assert_eq!(w.total, Some(1000.0));
        assert_eq!(w.unit.as_deref(), Some("积分"));
        // "2030-10-27 05:06:07"（无时区按 UTC）→ 毫秒
        assert_eq!(w.resets_at, Some(1_919_307_967_000));

        // planCode → plan_name
        assert_eq!(entry.plan_name.as_deref(), Some("mimo_pro"));
    }

    /// 缺必需 cookie（归一后无 serviceToken 或无 userId）→ error，不发请求。
    #[test]
    fn missing_required_cookies_maps_to_error() {
        let bundle = QuotaBundle {
            balance: Err("缺少必需 Cookie（api-platform_serviceToken / userId）".to_string()),
            detail: None,
            usage: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        let msg = entry.message.as_deref().unwrap_or("");
        assert!(msg.contains("api-platform_serviceToken"));
        assert!(msg.contains("userId"));

        // has_required_cookies 各形态
        assert!(has_required_cookies(
            "userId=42; api-platform_serviceToken=tok; theme=dark"
        ));
        assert!(has_required_cookies("api-platform_serviceToken=tok; userId=42"));
        assert!(!has_required_cookies("userId=42; theme=dark"));
        assert!(!has_required_cookies("api-platform_serviceToken=tok"));
        assert!(!has_required_cookies(""));
    }

    /// HTTP 401/403 与信封 code 401/403 → expired（假 Cookie 手测链路的
    /// 预期分支），文案含重新登录指引且不回显 Cookie。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let bundle = QuotaBundle {
                balance: Ok((status, Some(r#"{"detail":"unauthorized"}"#.to_string()))),
                detail: None,
                usage: None,
            };
            let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(
                entry.message.as_deref(),
                Some("会话已失效，请重新登录 platform.xiaomimimo.com 后更新 Cookie")
            );
            assert!(entry.windows.is_empty());
        }
        // 信封 code 形态
        for code in [401.0, 403.0] {
            let body = serde_json::json!({ "code": code, "message": "login required" })
                .to_string();
            let bundle = QuotaBundle {
                balance: ok_raw(&body),
                detail: None,
                usage: None,
            };
            let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
            assert_eq!(entry.status, "expired", "code {code} 应判定为 expired");
        }
    }

    /// 字段弹性：数字/字符串数字余额、可选拆分缺省、items[0] percent 缺失
    /// 回退 monthUsage.percent、detail/usage 请求失败静默（仅余额条目 ok）。
    #[test]
    fn flexible_fields_and_silent_detail_usage() {
        // balance 数字形态 + 拆分缺省 + usage 仅 monthUsage.percent 回退
        let bundle = QuotaBundle {
            balance: ok_raw(
                r#"{"code":0,"message":"","data":{"balance":88.5,"currency":"CNY"}}"#,
            ),
            detail: None, // 详情请求失败 → 静默
            usage: Some(ok_tuple(
                r#"{"code":0,"message":"","data":{"monthUsage":{"percent":0.3,"items":[]}}}"#,
            )),
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        let balance = entry.balance.expect("余额必须产出");
        assert_eq!(balance.amount, 88.5);
        assert_eq!(balance.topped_up, None);
        assert_eq!(balance.granted, None);
        assert_eq!(entry.plan_name, None);
        assert_eq!(entry.windows.len(), 1);
        assert_eq!(entry.windows[0].used_percent, Some(30.0));
        assert_eq!(entry.windows[0].used, None); // items 空 → 无原始计数
        assert_eq!(entry.windows[0].resets_at, None);

        // usage 请求也失败 → 仅余额条目，无窗口，仍 ok
        let bundle = QuotaBundle {
            balance: ok_raw(
                r#"{"code":0,"message":"","data":{"balance":"1.00","currency":"CNY"}}"#,
            ),
            detail: None,
            usage: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert!(entry.windows.is_empty());
        assert!(entry.balance.is_some());
    }

    /// 会话过期重定向落点（HTML）→ expired；其他信封非 0 → error 带 message。
    #[test]
    fn html_redirect_maps_expired_and_other_codes_error() {
        let bundle = QuotaBundle {
            balance: ok_raw("<!DOCTYPE html><html>login page</html>"),
            detail: None,
            usage: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "expired");
        assert_eq!(
            entry.message.as_deref(),
            Some("会话已失效，请重新登录 platform.xiaomimimo.com 后更新 Cookie")
        );

        let bundle = QuotaBundle {
            balance: ok_raw(r#"{"code":1001,"message":"quota query failed"}"#),
            detail: None,
            usage: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        let msg = entry.message.expect("error 条目必须有原因");
        assert!(msg.contains("quota query failed"), "消息应带 message: {msg}");

        // 无 message 的非 0 code → 带 code 的 error
        let bundle = QuotaBundle {
            balance: ok_raw(r#"{"code":500}"#),
            detail: None,
            usage: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
    }

    /// 网络层失败 / 非 200 非 401/403 / 坏 JSON / 缺余额字段 → error。
    #[test]
    fn network_and_structure_failures_map_to_error() {
        let bundle = QuotaBundle {
            balance: Err("网络错误或服务不可用: connection timed out".to_string()),
            detail: None,
            usage: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));

        for (status, body_text) in [
            (500u16, "internal".to_string()),
            (200, "not json".to_string()),
            (200, r#"{"code":0,"data":{}}"#.to_string()),          // 缺 balance
            (200, r#"{"code":0,"data":{"balance":"1"}}"#.to_string()), // 缺 currency
        ] {
            let case = format!("case ({status}, {body_text}) 应判定 error");
            let bundle = QuotaBundle {
                balance: Ok((status, Some(body_text))),
                detail: None,
                usage: None,
            };
            let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
            assert_eq!(entry.status, "error", "{case}");
        }
    }

    /// 非非 cookie 凭证被过滤；region 不参与任何分流（单一域名，模块不读
    /// region 字段——不同 region 值的凭证走同一 BASE，不产生分支差异）。
    #[test]
    fn non_cookie_credentials_are_skipped_and_region_ignored() {
        let snapshots = [
            CredentialQuerySnapshot {
                id: "a".into(),
                label: "apiKey 条目".into(),
                kind: "apiKey".into(),
                secret: "sk-x".into(),
                region: Some("global".into()),
            },
            CredentialQuerySnapshot {
                id: "b".into(),
                label: "cookie 条目".into(),
                kind: "cookie".into(),
                // 缺必需 cookie → 该条目产出 error（不发请求），而非 panic
                secret: "theme=dark; userId=42".into(),
                region: Some("cn".into()),
            },
        ];
        let entries = fetch_quota_entries(&snapshots);
        assert_eq!(entries.len(), 1, "apiKey 凭证不应被 cookie 型 provider 消费");
        assert_eq!(entries[0].credential_id, "b");
        assert_eq!(entries[0].status, "error");
        assert!(entries[0]
            .message
            .as_ref()
            .unwrap()
            .contains("api-platform_serviceToken"));
    }
}

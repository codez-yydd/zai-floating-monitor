//! StepFun（阶跃星辰）额度查询模块。
//!
//! 手动 token 模式（不做账号密码登录流）：kind=token 凭证的 secret 是用户
//! 从浏览器 DevTools 复制的 Oasis-Token（platform.stepfun.com 登录态 JWT）。
//!
//! 数据来源（POST，JSON 体 `{}`）：
//! - 用量：`{BASE}/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit`
//! - 套餐名：`{BASE}/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus`
//!   取 `subscription.name` 作 plan_name；失败静默（非致命）。
//!
//! Oasis-Webid 绑定：请求需带 `Oasis-Webid`（header 或 cookie，与 token 的
//! JWT `device_id` claim 匹配）。从 token JWT payload（base64url 中段，不验
//! 签）解出 `device_id`，同时以 header `Oasis-Webid: <device_id>` 和 cookie
//! 形式带上；解不出时不带（多数情况服务端仍放行；401 时错误消息提示 token
//! 可能需要连 Oasis-Webid 一起复制）。
//!
//! 用量响应两形态（键名 snake_case，camelCase 别名兜底）：
//! - 速率套餐（`plan_family` 缺失或 ≠2，或速率字段有非零值）：
//!   `five_hour_usage_left_rate` / `weekly_usage_left_rate`（0-1 剩余率，
//!   usedPercent=(1-left)*100）+ `*_usage_reset_time`（字符串/整数自适应）
//!   → 双窗口 hour5「5小时窗口」/ weekly「本周」；
//! - 积分套餐（`plan_family==2` 且速率字段为 0/缺失）：
//!   `plan_credit_rate_limit.subscription_credit_left_rate` +
//!   `subscription_credit_reset_time` → sub_credits「订阅积分」；
//!   `topup_credit_left_rate` → topup_credits「充值积分」（订阅/充值积分
//!   百分比不可相加，分别成窗）；`credit_buckets[]`
//!   （`{credit_total, credit_residual, expire_at, next_reset_at}`）存在时把
//!   buckets 汇总剩余作订阅积分窗的 used/total 原始值。
//!
//! 错误映射：401/403 → expired「Oasis-Token 无效或已过期…」；其他错误 → error。
//!
//! 工程纪律（对齐 alibaba.rs / minimax.rs）：网络 ureq 同步 + 15s 超时 +
//! resolve_proxy，调用方 spawn_blocking；解析纯函数与网络分离，单测不联网；
//! 错误消息中文且不含 secret；token 不进任何日志。

use crate::cookie_util::parse_time_flexible;
use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, get_any, now_ms, quota_http_agent, ProviderQuotaEntry, ProviderQuotaWindow,
};

/// 站点根（Origin / Referer / 错误文案共用）。
const BASE: &str = "https://platform.stepfun.com";

/// 用量查询端点（速率 / 积分两形态同一端点，按 plan_family 分流）。
const RATE_LIMIT_PATH: &str = "/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit";
/// 套餐状态端点（subscription.name → plan_name；失败静默）。
const PLAN_STATUS_PATH: &str = "/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus";

// ============================================================
// token 归一与 JWT device_id 解码（纯函数）
// ============================================================

/// 剥最外层成对引号（可叠多层）；非成对不动。
fn strip_wrapping_quotes(s: &str) -> &str {
    let mut s = s.trim();
    while s.len() >= 2 {
        let b = s.as_bytes();
        let paired = (b[0] == b'\'' && b[s.len() - 1] == b'\'')
            || (b[0] == b'"' && b[s.len() - 1] == b'"');
        if !paired {
            break;
        }
        s = s[1..s.len() - 1].trim();
    }
    s
}

/// 单段（无分号）token 提取：`Oasis-Token=xxx` / `Cookie: Oasis-Token=xxx`
/// / `Oasis-Token: xxx` 取值；裸 JWT 原样返回。
fn extract_token_single(segment: &str) -> String {
    let s = strip_wrapping_quotes(segment);
    // 键值形态：`Oasis-Token=xxx`（容忍 `Cookie: Oasis-Token=xxx` 前缀）
    if let Some(eq) = s.find('=') {
        let head = s[..eq].trim().to_ascii_lowercase();
        if head == "oasis-token" || head.ends_with("oasis-token") {
            return strip_wrapping_quotes(&s[eq + 1..]).to_string();
        }
    }
    // 头名冒号形态：`Oasis-Token: xxx`（裸 JWT 不含冒号，不影响原样返回）
    if let Some(colon) = s.find(':') {
        if s[..colon].trim().eq_ignore_ascii_case("oasis-token") {
            return strip_wrapping_quotes(&s[colon + 1..]).to_string();
        }
    }
    s.to_string()
}

/// 从用户粘贴内容提取 Oasis-Token 值（纯函数）：裸 JWT 原样；`Oasis-Token=xxx`
/// 形态取值；粘贴整串 cookie（含分号）时逐段找 Oasis-Token 段。解析不出
/// 非空值返回空串（调用方产出 error 提示重新复制）。
fn extract_token(secret: &str) -> String {
    let s = strip_wrapping_quotes(secret);
    if s.contains(';') {
        // 整串 cookie：只认 Oasis-Token 段，其余（Oasis-Webid 等）不混入
        for part in s.split(';') {
            let token = extract_token_single(part);
            if !token.trim().is_empty() && token.contains('.') {
                return token;
            }
        }
        return String::new();
    }
    extract_token_single(s)
}

/// base64url 解码（JWT payload 用；兼容带/不带 padding）。
fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let trimmed = input.trim();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(trimmed))
        .ok()
}

/// 从 Oasis-Token（JWT）payload 解出 `device_id` claim（Oasis-Webid 绑定用；
/// 不验签，仅作请求绑定）。三段式 JWT 损坏 / 缺 claim 返回 None。
fn device_id_from_token(token: &str) -> Option<String> {
    let payload = token.trim().split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    let v = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let id = v.get("device_id")?;
    // 常规为字符串；数字形态也容忍（转字符串携带）
    match id {
        serde_json::Value::String(s) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

// ============================================================
// 网络层（ureq 同步；调用方 spawn_blocking）
// ============================================================

/// 逐凭证查询 StepFun 套餐额度（串行；单凭证失败产出 error/expired 条目，
/// 不阻塞其他凭证）。只消费 kind=token 的凭证，由 provider_quota 骨架分发。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .filter(|cred| cred.kind == "token")
        .map(|cred| {
            let token = extract_token(&cred.secret);
            // JWT 形态粗校验（header.payload.signature 至少两个点）：解析不出
            // 有效形态时不发请求直接报错（也保证单测离线）
            let usable = !token.is_empty() && token.matches('.').count() >= 2;
            let (raw, plan_name) = if !usable {
                (
                    Err("未能从粘贴内容中解析出 Oasis-Token，请复制 Cookie 里 Oasis-Token 的值".to_string()),
                    None,
                )
            } else {
                let device_id = device_id_from_token(&token);
                let raw = fetch_post_raw(&agent, RATE_LIMIT_PATH, &token, device_id.as_deref());
                // 套餐名非致命：请求或解析失败静默为 None，不影响用量条目
                let plan_raw =
                    fetch_post_raw(&agent, PLAN_STATUS_PATH, &token, device_id.as_deref());
                (raw, plan_name_from_raw(&plan_raw))
            };
            entry_from_raw(&cred.id, &cred.label, &raw, plan_name)
        })
        .collect()
}

/// 单次 POST 请求（网络层）：`{BASE}{path}`，体 `{}`，头为浏览器仿真
/// （Chrome UA / Accept / Origin / Referer）+ `Cookie: Oasis-Token=...`
/// （可解出 device_id 时附带 `Oasis-Webid` cookie）+ `Oasis-Webid` header。
/// 返回展平的 (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，
/// 不含 secret）。
fn fetch_post_raw(
    agent: &ureq::Agent,
    path: &str,
    token: &str,
    device_id: Option<&str>,
) -> Result<(u16, Option<String>), String> {
    let cookie = match device_id {
        Some(id) => format!("Oasis-Token={token}; Oasis-Webid={id}"),
        None => format!("Oasis-Token={token}"),
    };
    let mut req = agent
        .post(&format!("{BASE}{path}"))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json, text/plain, */*")
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36")
        .set("Origin", BASE)
        .set("Referer", &format!("{BASE}/"))
        .set("Cookie", &cookie);
    if let Some(id) = device_id {
        req = req.set("Oasis-Webid", id);
    }
    let resp = req.send_string("{}");
    flatten_response(resp).map_err(|e| format!("StepFun 额度{e}"))
}

/// 套餐名解析（纯函数，网络无关）：GetStepPlanStatus 200 时深度找
/// `subscription` 对象取 `name`；非 200 / 坏 JSON / 结构变化一律 None
/// （套餐名非致命，静默降级）。
fn plan_name_from_raw(raw: &Result<(u16, Option<String>), String>) -> Option<String> {
    let Ok((200, Some(body))) = raw else {
        return None;
    };
    let v = serde_json::from_str::<serde_json::Value>(body.trim()).ok()?;
    let subscription = find_first_obj_with_keys(&v, &["subscription"])?;
    subscription
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// 深度优先找第一个「对象值且含任一目标键」的值（对齐 alibaba.rs 同名函数
/// 的先本层后递归策略；本地实现避免跨模块导出私有工具）。
fn find_first_obj_with_keys<'a>(
    v: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(val) = map.get(*key) {
                    if val.is_object() {
                        return Some(val);
                    }
                }
            }
            map.values().find_map(|val| find_first_obj_with_keys(val, keys))
        }
        serde_json::Value::Array(items) => {
            items.iter().find_map(|it| find_first_obj_with_keys(it, keys))
        }
        _ => None,
    }
}

// ============================================================
// 解析纯函数（网络无关，单测直接构造输入）
// ============================================================

/// 剩余率（0-1）→ 已用百分比：usedPercent=(1-left)*100，clamp 0-100；
/// 结果保留两位小数（消除浮点尾差，展示口径）；left 越出 0-1 视为脏值（None）。
fn used_percent_from_left(left: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&left) {
        return None;
    }
    Some((((1.0 - left) * 100.0).clamp(0.0, 100.0) * 100.0).round() / 100.0)
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败(error) > 缺配额数据(error) > 成功(ok + 窗口组)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), String>,
    plan_name: Option<String>,
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
        return fail("error", format!("StepFun 额度{}", raw.as_ref().unwrap_err()));
    };
    // Oasis-Token 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）。
    // JWT 解不出 device_id（未带 Oasis-Webid）时服务端也可能 401，提示里
    // 带上连 Oasis-Webid 一起核对的指引。
    if *http_status == 401 || *http_status == 403 {
        return fail(
            "expired",
            "Oasis-Token 无效或已过期，请重新登录 platform.stepfun.com 后从请求头复制新 Token（如仍失败，请连 Cookie 中的 Oasis-Webid 一起核对）".to_string(),
        );
    }
    if *http_status != 200 {
        return fail("error", format!("StepFun 额度查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref().filter(|b| !b.trim().is_empty()) else {
        return fail("error", "StepFun 额度响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "StepFun 额度响应解析失败".to_string());
    };

    let windows = parse_windows(&v);
    if windows.is_empty() {
        return fail("error", "StepFun 额度响应缺少配额数据".to_string());
    }

    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name,
        message: None,
        updated_at: now_ms(),
    }
}

/// 从用量响应提取窗口组（纯函数）：速率套餐（plan_family 缺失/≠2，或速率
/// 字段有非零值）优先；plan_family==2 且速率字段全 0/缺失时按积分套餐解析
/// （订阅积分与充值积分百分比不可相加，分别成窗）。无任何窗口返回空数组
/// （由调用方报 error）。
fn parse_windows(v: &serde_json::Value) -> Vec<ProviderQuotaWindow> {
    let plan_family = crate::provider_quota::num_any(v, &["plan_family", "planFamily"]);
    let five_left =
        crate::provider_quota::num_any(v, &["five_hour_usage_left_rate", "fiveHourUsageLeftRate"]);
    let weekly_left =
        crate::provider_quota::num_any(v, &["weekly_usage_left_rate", "weeklyUsageLeftRate"]);
    let rate_available = five_left.map(|l| l != 0.0).unwrap_or(false)
        || weekly_left.map(|l| l != 0.0).unwrap_or(false);
    let is_credit_plan = plan_family == Some(2.0) && !rate_available;
    if is_credit_plan {
        return parse_credit_windows(v);
    }
    parse_rate_windows(v, five_left, weekly_left)
}

/// 速率套餐双窗口（纯函数）：left_rate 任一有值才产出对应窗口；
/// usedPercent=(1-left)*100；reset 时间字符串/整数自适应（无原始 used/total）。
fn parse_rate_windows(
    v: &serde_json::Value,
    five_left: Option<f64>,
    weekly_left: Option<f64>,
) -> Vec<ProviderQuotaWindow> {
    let specs: [(&str, &str, Option<f64>, &[&str]); 2] = [
        (
            "hour5",
            "5小时窗口",
            five_left,
            &["five_hour_usage_reset_time", "fiveHourUsageResetTime"],
        ),
        (
            "weekly",
            "本周",
            weekly_left,
            &["weekly_usage_reset_time", "weeklyUsageResetTime"],
        ),
    ];
    specs
        .iter()
        .filter_map(|(key, title, left, reset_keys)| {
            let left = (*left)?;
            Some(ProviderQuotaWindow {
                key: (*key).to_string(),
                title: (*title).to_string(),
                used_percent: used_percent_from_left(left),
                used: None,
                total: None,
                unit: None,
                resets_at: get_any(v, reset_keys).and_then(parse_time_flexible),
            })
        })
        .collect()
}

/// 积分套餐双窗口（纯函数）：
/// - sub_credits「订阅积分」：subscription_credit_left_rate 反推百分比；
///   credit_buckets[] 存在时汇总 residual/total 作 used/total 原始值
///   （used = Σtotal - Σresidual）；reset 优先 subscription_credit_reset_time，
///   缺失时 buckets 内 next_reset_at 兜底；
/// - topup_credits「充值积分」：topup_credit_left_rate 反推百分比。
fn parse_credit_windows(v: &serde_json::Value) -> Vec<ProviderQuotaWindow> {
    let credit = get_any(v, &["plan_credit_rate_limit", "planCreditRateLimit"])
        .filter(|c| c.is_object());
    let mut windows = Vec::new();

    // 订阅积分：left_rate 与 buckets 任一有值即产出
    let sub_left = credit.and_then(|c| {
        crate::provider_quota::num_any(c, &["subscription_credit_left_rate", "subscriptionCreditLeftRate"])
    });
    let buckets = credit
        .and_then(|c| get_any(c, &["credit_buckets", "creditBuckets"]))
        .and_then(|b| b.as_array());
    let (sum_total, sum_residual): (Option<f64>, Option<f64>) = match buckets {
        Some(items) if !items.is_empty() => {
            let total: f64 = items
                .iter()
                .filter_map(|b| crate::provider_quota::num_any(b, &["credit_total", "creditTotal"]))
                .sum();
            let residual: f64 = items
                .iter()
                .filter_map(|b| {
                    crate::provider_quota::num_any(b, &["credit_residual", "creditResidual"])
                })
                .sum();
            (Some(total), Some(residual))
        }
        _ => (None, None),
    };
    if sub_left.is_some() || sum_total.is_some() {
        let sub_reset = credit
            .and_then(|c| {
                get_any(c, &["subscription_credit_reset_time", "subscriptionCreditResetTime"])
            })
            .and_then(parse_time_flexible)
            .or_else(|| {
                buckets
                    .and_then(|items| {
                        items
                            .iter()
                            .find_map(|b| get_any(b, &["next_reset_at", "nextResetAt"]))
                    })
                    .and_then(parse_time_flexible)
            });
        let used = match (sum_total, sum_residual) {
            (Some(t), Some(r)) => Some((t - r).max(0.0)),
            _ => None,
        };
        windows.push(ProviderQuotaWindow {
            key: "sub_credits".to_string(),
            title: "订阅积分".to_string(),
            used_percent: sub_left.and_then(used_percent_from_left),
            used,
            total: sum_total,
            unit: Some("积分".to_string()),
            resets_at: sub_reset,
        });
    }

    // 充值积分：left_rate 有值才产出（与订阅积分不可相加，独立成窗）
    let topup_left = credit.and_then(|c| {
        crate::provider_quota::num_any(c, &["topup_credit_left_rate", "topupCreditLeftRate"])
    });
    if let Some(left) = topup_left {
        windows.push(ProviderQuotaWindow {
            key: "topup_credits".to_string(),
            title: "充值积分".to_string(),
            used_percent: used_percent_from_left(left),
            used: None,
            total: None,
            unit: Some("积分".to_string()),
            resets_at: None,
        });
    }
    windows
}

// ============================================================
// 单元测试（纯函数，不联网）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "sf-1";
    const LABEL: &str = "StepFun 主号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 构造测试用 JWT：header.payload.signature（payload 为 base64url JSON）。
    fn make_jwt(payload_json: &str) -> String {
        use base64::Engine;
        let header =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{\"alg\":\"HS256\"}");
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header}.{payload}.sig")
    }

    /// 速率套餐样例：双窗口 + 剩余率反推百分比（0.7 → 30%）+ reset 字符串。
    #[test]
    fn parses_rate_plan_dual_windows() {
        let raw = ok_raw(
            r#"{"plan_family":1,
               "five_hour_usage_left_rate":0.7,
               "five_hour_usage_reset_time":"2030-10-27 05:06:07",
               "weekly_usage_left_rate":0.24,
               "weekly_usage_reset_time":1919307967}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert_eq!(entry.windows.len(), 2);

        let hour5 = &entry.windows[0];
        assert_eq!(hour5.key, "hour5");
        assert_eq!(hour5.title, "5小时窗口");
        assert_eq!(hour5.used_percent, Some(30.0)); // (1-0.7)*100（两位小数归一）
        assert_eq!(hour5.used, None); // 速率套餐无原始 used/total
        assert_eq!(hour5.total, None);
        assert_eq!(hour5.resets_at, Some(1_919_307_967_000)); // 无时区按 UTC

        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.title, "本周");
        assert_eq!(weekly.used_percent, Some(76.0)); // (1-0.24)*100
        assert_eq!(weekly.resets_at, Some(1_919_307_967_000)); // 整数秒 → 毫秒
    }

    /// 积分套餐样例：plan_family==2 且速率字段为 0 → sub/topup 双窗，
    /// buckets 汇总作订阅积分窗 used/total 原始值。
    #[test]
    fn parses_credit_plan_with_buckets() {
        let raw = ok_raw(
            r#"{"plan_family":2,
               "five_hour_usage_left_rate":0,
               "weekly_usage_left_rate":0,
               "plan_credit_rate_limit":{
                 "subscription_credit_left_rate":0.55,
                 "subscription_credit_reset_time":"2030-11-01T00:00:00Z",
                 "topup_credit_left_rate":0.8,
                 "credit_buckets":[
                   {"credit_total":1000,"credit_residual":600,"expire_at":"2030-12-01","next_reset_at":1919307967},
                   {"credit_total":500,"credit_residual":250,"expire_at":"2031-01-01","next_reset_at":1919307967}
                 ]
               }}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, Some("Max".to_string()));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.plan_name.as_deref(), Some("Max"));
        assert_eq!(entry.windows.len(), 2);

        let sub = &entry.windows[0];
        assert_eq!(sub.key, "sub_credits");
        assert_eq!(sub.title, "订阅积分");
        assert_eq!(sub.used_percent, Some(45.0)); // (1-0.55)*100
        assert_eq!(sub.total, Some(1500.0)); // 1000 + 500
        assert_eq!(sub.used, Some(650.0)); // (1000-600) + (500-250)
        assert_eq!(sub.unit.as_deref(), Some("积分"));
        assert_eq!(sub.resets_at, Some(1_919_721_600_000)); // ISO → 毫秒

        let topup = &entry.windows[1];
        assert_eq!(topup.key, "topup_credits");
        assert_eq!(topup.title, "充值积分");
        assert_eq!(topup.used_percent, Some(20.0)); // (1-0.8)*100
        assert_eq!(topup.used, None); // 充值积分无原始计数
        assert_eq!(topup.total, None);
    }

    /// 剩余率反推边界：0 → 100%、1 → 0%、越界脏值 → 无百分比。
    #[test]
    fn left_rate_clamps_and_guards() {
        assert_eq!(used_percent_from_left(0.0), Some(100.0));
        assert_eq!(used_percent_from_left(1.0), Some(0.0));
        assert_eq!(used_percent_from_left(0.75), Some(25.0));
        assert_eq!(used_percent_from_left(-0.1), None);
        assert_eq!(used_percent_from_left(1.5), None);
    }

    /// JWT device_id 解码：标准三段式解出 claim；损坏 / 缺 claim → None；
    /// 数字形态容忍。
    #[test]
    fn jwt_device_id_decodes() {
        let token = make_jwt(r#"{"device_id":"web-abc123","exp":1900000000}"#);
        assert_eq!(device_id_from_token(&token).as_deref(), Some("web-abc123"));

        // 数字形态
        let token = make_jwt(r#"{"device_id":12345}"#);
        assert_eq!(device_id_from_token(&token).as_deref(), Some("12345"));

        // 非 JWT / 段数不足 / 坏 base64 / 缺 claim
        assert_eq!(device_id_from_token("not-a-jwt"), None);
        assert_eq!(device_id_from_token("a.b"), None);
        let no_claim = make_jwt(r#"{"sub":"u1"}"#);
        assert_eq!(device_id_from_token(&no_claim), None);
        assert_eq!(device_id_from_token("x.!!!bad!base64!.y"), None);
    }

    /// reset 时间字段字符串/整数自适应；脏值 → 窗口照常产出但无重置时间。
    #[test]
    fn reset_time_accepts_string_and_int() {
        let raw = ok_raw(
            r#"{"five_hour_usage_left_rate":0.5,
               "five_hour_usage_reset_time":"2030-10-27T05:06:07Z",
               "weekly_usage_left_rate":0.5,
               "weekly_usage_reset_time":"not-a-time"}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows[0].resets_at, Some(1_919_307_967_000));
        assert_eq!(entry.windows[1].resets_at, None);
    }

    /// HTTP 401/403 → expired（假 token 手测链路的预期分支），文案含重登指引
    /// 且不含 secret；其他非 200 → error；网络失败 → error 原因透传。
    #[test]
    fn unauthorized_maps_to_expired_and_others_to_error() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some(r#"{"message":"unauthorized"}"#.to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            let msg = entry.message.as_deref().unwrap_or("");
            assert!(msg.contains("Oasis-Token"), "文案应指向 Oasis-Token: {msg}");
            assert!(msg.contains("platform.stepfun.com"));
            assert!(msg.contains("Oasis-Webid"), "401 文案应提示核对 Oasis-Webid");
            assert!(!msg.contains("eyJ"), "错误消息不得含 token 片段");
            assert!(entry.windows.is_empty());
        }

        let raw = Ok((500, Some("internal".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));

        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));
    }

    /// GetStepPlanStatus 失败容错：plan_name=None 时用量条目照常 ok；
    /// 套餐名解析只在 200 + subscription.name 时产出。
    #[test]
    fn plan_status_failure_is_tolerated() {
        let raw = ok_raw(r#"{"five_hour_usage_left_rate":0.9}"#);
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
        assert_eq!(entry.status, "ok");
        assert!(entry.plan_name.is_none());

        // plan_name_from_raw 的容错分支：非 200 / 坏 JSON / 缺 subscription
        let bad: Result<(u16, Option<String>), String> = Ok((401, None));
        assert_eq!(plan_name_from_raw(&bad), None);
        let bad = ok_raw("not json");
        assert_eq!(plan_name_from_raw(&bad), None);
        let bad = ok_raw(r#"{"data":{}}"#);
        assert_eq!(plan_name_from_raw(&bad), None);
        // 成功形态（data 包裹也能深度命中）
        let good = ok_raw(r#"{"data":{"subscription":{"name":"旗舰版"}}}"#);
        assert_eq!(plan_name_from_raw(&good).as_deref(), Some("旗舰版"));
    }

    /// 响应缺配额数据（{} / 积分套餐无任何可用字段）→ error。
    #[test]
    fn missing_quota_maps_to_error() {
        for body in ["{}", r#"{"plan_family":2}"#] {
            let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(body), None);
            assert_eq!(entry.status, "error", "body={body}");
            assert!(entry.message.unwrap().contains("缺少配额数据"));
        }
    }

    /// plan_family==2 但速率字段有非零值 → 仍按速率套餐展示（有可用速率
    /// 优先）；plan_family 缺失 → 走速率路径，全键缺失则 error。
    #[test]
    fn credit_plan_detection_edge_cases() {
        // 速率字段非 0 → 速率窗口
        let raw = ok_raw(r#"{"plan_family":2,"five_hour_usage_left_rate":0.5}"#);
        let entry = entry_from_raw(CRED_ID, LABEL, &raw, None);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows[0].key, "hour5");

        // plan_family 缺失 → 速率套餐路径（缺字段则空 → error）
        let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw("{}"), None);
        assert_eq!(entry.status, "error");
    }

    /// token 提取：裸 JWT 原样；`Oasis-Token=xxx` / `Oasis-Token: xxx` /
    /// `Cookie: Oasis-Token=xxx` / 引号包裹 / 整串 cookie 取 Oasis-Token 段。
    #[test]
    fn token_extraction_forms() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJkZXZpY2VfaWQiOiJhYmMifQ.sig";
        assert_eq!(extract_token(jwt), jwt);
        assert_eq!(extract_token("Oasis-Token=abc.def.gh"), "abc.def.gh");
        assert_eq!(extract_token("oasis-token: abc.def.gh"), "abc.def.gh");
        assert_eq!(extract_token("Cookie: Oasis-Token=abc.def.gh"), "abc.def.gh");
        assert_eq!(extract_token(" \"a.b.c\" "), "a.b.c");
        // 整串 cookie（含 Oasis-Webid）：只取 Oasis-Token 段
        assert_eq!(
            extract_token("Oasis-Webid=web-1; Oasis-Token=abc.def.gh; theme=dark"),
            "abc.def.gh"
        );
        assert_eq!(extract_token("   "), "");
    }

    /// 非 token 凭证被过滤，不产生条目；空 token 凭证产出 error 而非跳过。
    #[test]
    fn non_token_credentials_are_skipped() {
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
                label: "token 条目".into(),
                kind: "token".into(),
                // 无 '.' 的残段 → 解析不出有效 token → 该条目产出 error
                secret: "theme=dark".into(),
                region: None,
            },
        ];
        let entries = fetch_quota_entries(&snapshots);
        assert_eq!(entries.len(), 1, "apiKey 凭证不应被 token 型 provider 消费");
        assert_eq!(entries[0].credential_id, "b");
        assert_eq!(entries[0].status, "error");
        assert!(entries[0].message.as_ref().unwrap().contains("Oasis-Token"));
    }
}

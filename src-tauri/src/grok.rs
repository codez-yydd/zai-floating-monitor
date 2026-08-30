//! Grok CLI（xAI）额度查询模块。
//!
//! 混合凭证型：本地 `~/.grok/auth.json`（grok login 的 OIDC/会话登录态）
//! 优先，同时并入凭证体系里手动添加的 kind=token 条目（用户从 auth.json
//! 复制 key 或其他来源），本地与手动两路合并查询、按 secret 去重。
//! `GROK_HOME` 可覆盖根目录（与 CodexBar 同名环境变量，测试/便携场景用）。
//!
//! auth.json 结构：顶层键为 OIDC scope URL，优先 `https://auth.x.ai::<id>`
//! 前缀键（SuperGrok 订阅），回退 `https://accounts.x.ai/sign-in`（旧会话）；
//! 条目 `{key, refresh_token, expires_at, auth_mode, email, ...}`。只读不写：
//! `expires_at` 已过 → expired 条目提示重跑 `grok login`（与 CodexBar 一致，
//! 不主动刷新）。
//!
//! 查询端点（grok CLI 自带的 cli-chat-proxy）：
//! - `GET /v1/billing?format=credits`：金额为 `{"val": <美分整数>}` 结构，
//!   usedPercent = totalUsed.val / monthlyLimit.val * 100（limit 为 0/null 时
//!   只显示已用金额不算百分比），resets_at = billingPeriodEnd；
//! - `GET /v1/settings`（3s 超时，失败静默）：subscription_tier_display 作
//!   plan_name（如 SuperGrok / SuperGrok Heavy）。
//!
//! 工程纪律（对齐 provider_quota.rs / moonshot.rs）：
//! - 网络：ureq 同步请求 + 15s（settings 3s）超时 + codex::resolve_proxy，
//!   调用方 spawn_blocking；
//! - 安全：key 只在 Rust 内部构造鉴权头；错误消息中文且不含 key 片段。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, now_ms, parse_flexible_f64, quota_http_agent, quota_http_agent_timeout,
    ProviderQuotaEntry, ProviderQuotaWindow,
};
use std::path::PathBuf;

/// 额度端点（grok CLI 的 cli-chat-proxy；format=credits 返回美分结构）。
const BILLING_ENDPOINT: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// 订阅信息端点（plan_name 用；失败静默不阻断额度展示）。
const SETTINGS_ENDPOINT: &str = "https://cli-chat-proxy.grok.com/v1/settings";
/// settings 请求单独的短超时（非关键数据，慢网时不能拖住整轮查询）。
const SETTINGS_TIMEOUT_SECS: u64 = 3;

/// SuperGrok 订阅的 OIDC scope 键前缀（优先选用）。
const OIDC_SCOPE_PREFIX: &str = "https://auth.x.ai::";
/// 旧版会话登录的 scope 键（回退选用）。
const LEGACY_SESSION_SCOPE: &str = "https://accounts.x.ai/sign-in";

/// Grok 配置根目录（GROK_HOME 优先，其次 ~/.grok/）。
fn grok_home() -> PathBuf {
    if let Ok(home) = std::env::var("GROK_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
}

/// 本地登录态是否可用（provider_credentials::has_credentials 的 OR 特判用）：
/// auth.json 存在即视为已装/已登录 grok CLI。
pub(crate) fn has_local_data() -> bool {
    grok_home().join("auth.json").exists()
}

// ============================================================
// 纯函数：auth.json 解析 / 时间 / 额度映射（单测不联网）
// ============================================================

/// 本地 auth.json 条目的内存形态（只取查询需要的字段）。
#[derive(Debug, Clone, PartialEq)]
struct LocalAuth {
    /// Bearer token（auth.json 条目的 key 字段）
    key: String,
    email: Option<String>,
    /// epoch 毫秒（缺失为 None，视为未过期）
    expires_at_ms: Option<i64>,
}

/// 是否已过期（纯函数，now 注入）：expires_at 缺失 → 未过期（无法判定时
/// 放行，由服务端 401 兜底）。
fn is_expired(expires_at_ms: Option<i64>, now: i64) -> bool {
    matches!(expires_at_ms, Some(at) if at <= now)
}

/// 从 auth.json 内容选登录条目（纯函数）：OIDC scope 前缀键优先，
/// 旧会话 scope 回退；条目必须带非空 key（残缺的 OIDC 记录不能挤掉健康的
/// 会话条目，对齐 CodexBar selectPreferredEntry）。无可用条目 → None。
fn select_local_auth(content: &str) -> Option<LocalAuth> {
    let v = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let root = v.as_object()?;
    let mut oidc: Option<&serde_json::Value> = None;
    let mut legacy: Option<&serde_json::Value> = None;
    for (scope, entry) in root {
        if !entry
            .get("key")
            .and_then(|k| k.as_str())
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false)
        {
            continue;
        }
        if scope.starts_with(OIDC_SCOPE_PREFIX) {
            // 同类多条时保留首个出现的（auth.json 实际只会有一个 OIDC 键）
            oidc = oidc.or(Some(entry));
        } else if scope == LEGACY_SESSION_SCOPE || scope.contains("/sign-in") {
            legacy = legacy.or(Some(entry));
        }
    }
    let entry = oidc.or(legacy)?;
    let non_empty = |field: &str| {
        entry
            .get(field)
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(LocalAuth {
        key: non_empty("key")?,
        email: non_empty("email"),
        expires_at_ms: entry.get("expires_at").and_then(parse_time_ms),
    })
}

/// 灵活时间解析（纯函数）：ISO-8601 字符串（含/不含毫秒、带偏移量）或
/// epoch 数字（秒/毫秒自适应）。脏值 → None。
fn parse_time_ms(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => epoch_to_ms(n.as_f64()?),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis())
                .or_else(|| s.parse::<f64>().ok().and_then(epoch_to_ms))
        }
        _ => None,
    }
}

/// epoch 秒/毫秒自适应：>=1e12 视为毫秒；1e9..1e12 视为秒；其余视为脏值。
fn epoch_to_ms(n: f64) -> Option<i64> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    if n >= 1e12 {
        Some(n as i64)
    } else if n >= 1e9 {
        Some((n * 1000.0) as i64)
    } else {
        None
    }
}

/// billing 响应 → 月度窗口（纯函数）：
/// - used = totalUsed 美元；total = monthlyLimit 美元（limit 为 0/缺失 → None）；
/// - usedPercent = totalUsed / monthlyLimit * 100（clamp 0-100；limit 无效时
///   None，前端只展示已用金额不算百分比）；
/// - resets_at = billingPeriodEnd（ISO 字符串 / epoch 秒或毫秒自适应）。
fn build_monthly_window(billing: &serde_json::Value) -> Option<ProviderQuotaWindow> {
    let usage = billing.get("usage")?;
    // 百分比按美分原值计算（分子分母同单位）；展示金额再除 100 换美元
    let used_cents = usage
        .get("totalUsed")
        .and_then(|u| u.get("val"))
        .and_then(parse_flexible_f64)?;
    let limit_cents = billing
        .get("monthlyLimit")
        .and_then(|l| l.get("val"))
        .and_then(parse_flexible_f64);
    let used_percent = limit_cents
        .filter(|l| *l > 0.0)
        .map(|l| (used_cents / l * 100.0).clamp(0.0, 100.0));
    let resets_at = billing
        .get("billingCycle")
        .and_then(|c| c.get("billingPeriodEnd"))
        .and_then(parse_time_ms);
    Some(ProviderQuotaWindow {
        key: "monthly".to_string(),
        title: "本月额度".to_string(),
        used_percent,
        used: Some(used_cents / 100.0),
        // limit 为 0（不限量）/缺失时不给 total，避免前端算出错误百分比
        total: limit_cents.filter(|l| *l > 0.0).map(|l| l / 100.0),
        unit: Some("$".to_string()),
        resets_at,
    })
}

/// settings 响应 → 订阅名（纯函数；失败/缺失 → None，不阻断额度展示）。
fn parse_tier_name(settings: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(settings).ok()?;
    v.get("subscription_tier_display")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// 单条凭证的查询结果 → 展示条目（纯函数，单测直接构造输入）。
/// 分支：网络失败(error) > 401/403(expired) > 非 200(error) > 解析失败
/// (error) > 成功(ok + 月度窗口)。
fn entry_from_billing_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), String>,
    plan_name: Option<String>,
) -> ProviderQuotaEntry {
    let fail = |status: &str, message: String| ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: plan_name.clone(),
        message: Some(message),
        updated_at: now_ms(),
    };
    let Ok((http_status, body)) = raw else {
        return fail("error", format!("Grok 额度{}", raw.as_ref().unwrap_err()));
    };
    // Key 被服务端拒绝：视为凭证过期（提示重登，消息不含 key 片段）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", "Token 无效或已过期，请重新 grok login".to_string());
    }
    if *http_status != 200 {
        return fail("error", format!("Grok 额度查询失败（HTTP {http_status}）"));
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body.as_deref().unwrap_or_default())
    else {
        return fail("error", "Grok 额度响应解析失败".to_string());
    };
    let Some(window) = build_monthly_window(&v) else {
        return fail("error", "Grok 额度响应缺少用量数据".to_string());
    };
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows: vec![window],
        balance: None,
        plan_name,
        message: None,
        updated_at: now_ms(),
    }
}

// ============================================================
// 网络层（ureq 同步；调用方 spawn_blocking）
// ============================================================

/// GET /v1/billing?format=credits（Bearer + CLI 代理标识头）。
fn fetch_billing_raw(agent: &ureq::Agent, key: &str) -> Result<(u16, Option<String>), String> {
    let resp = agent
        .get(BILLING_ENDPOINT)
        .set("Authorization", &format!("Bearer {key}"))
        .set("x-xai-token-auth", "xai-grok-cli")
        .set("Accept", "application/json")
        .call();
    flatten_response(resp).map_err(|e| format!("Grok 额度{e}"))
}

/// GET /v1/settings 取订阅名（3s 短超时；任何失败静默 → None，
/// 不阻断额度展示）。
fn fetch_tier_name(agent: &ureq::Agent, key: &str) -> Option<String> {
    let resp = agent
        .get(SETTINGS_ENDPOINT)
        .set("Authorization", &format!("Bearer {key}"))
        .set("x-xai-token-auth", "xai-grok-cli")
        .set("Accept", "application/json")
        .call();
    let (status, body) = flatten_response(resp).ok()?;
    if status != 200 {
        return None;
    }
    parse_tier_name(&body?)
}

// ============================================================
// 主入口（provider_quota 的 "grok" match 分支调用）
// ============================================================

/// 本地条目标签：「Grok CLI 账号」+ email 后缀（有就拼）。
fn local_label(email: Option<&str>) -> String {
    match email.filter(|e| !e.trim().is_empty()) {
        Some(email) => format!("Grok CLI 账号（{email}）"),
        None => "Grok CLI 账号".to_string(),
    }
}

/// 产出一个本地型失败/提示条目（credential_id="local"）。
fn local_entry(status: &str, label: &str, message: String) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: label.to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message),
        updated_at: now_ms(),
    }
}

/// 查询单条凭证并产出条目（billing 成功才附带 settings 订阅名；
/// 已过期的本地登录态不发起请求，直接给 expired 提示）。
fn query_entry(
    agent: &ureq::Agent,
    settings_agent: &ureq::Agent,
    cred_id: &str,
    label: &str,
    key: &str,
    expires_at_ms: Option<i64>,
) -> ProviderQuotaEntry {
    if is_expired(expires_at_ms, now_ms()) {
        return local_expired_entry(cred_id, label);
    }
    let raw = fetch_billing_raw(agent, key);
    // flatten_response 会把 4xx/5xx 也展平为 Ok((status, body))，is_ok() 在
    // 401/500 时同样为真；仅 billing 200 才请求 settings 订阅名，避免用失效
    // key 多打一次 3s 超时的 settings 请求。订阅名是非关键增强，失败静默
    // 降级为 None。
    if matches!(&raw, Ok((200, _))) {
        let tier = fetch_tier_name(settings_agent, key);
        return entry_from_billing_raw(cred_id, label, &raw, tier);
    }
    entry_from_billing_raw(cred_id, label, &raw, None)
}

/// 过期/无效条目（本地过期与 401/403 的提示语义一致）。
fn local_expired_entry(cred_id: &str, label: &str) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "expired".to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some("Token 已过期，请重新运行 grok login".to_string()),
        updated_at: now_ms(),
    }
}

/// 查询 Grok 额度：本地 auth.json 条目 + 手动凭证条目合并（串行；单条失败
/// 产出 error/expired 条目，不阻塞其他条目）。home 注入版，单测可指向临时
/// 目录；有效登录态会发起网络请求，单测只覆盖离线分支（本地过期/无数据）。
/// - 本地条目：credential_id="local"，label 带 email 后缀；
/// - 手动条目：去重（secret 与本地 key 相同的跳过），无本地过期信息，
///   有效性由服务端 401/403 判定。
fn fetch_entries_from(
    home: &std::path::Path,
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    let settings_agent = quota_http_agent_timeout(SETTINGS_TIMEOUT_SECS);
    let mut entries = Vec::new();
    let mut local_key: Option<String> = None;

    // 第一路：本地 auth.json（文件存在才有条目；文件存在但无可用 key →
    // pending 提示重跑 grok login，与 presence 口径一致）
    let auth_path = home.join("auth.json");
    if auth_path.exists() {
        match std::fs::read_to_string(&auth_path)
            .ok()
            .and_then(|content| select_local_auth(&content))
        {
            Some(local) => {
                local_key = Some(local.key.clone());
                let label = local_label(local.email.as_deref());
                entries.push(query_entry(
                    &agent,
                    &settings_agent,
                    "local",
                    &label,
                    &local.key,
                    local.expires_at_ms,
                ));
            }
            None => {
                entries.push(local_entry(
                    "pending",
                    "Grok CLI 账号",
                    "Grok 登录态不可用，请在终端重新运行 grok login".to_string(),
                ));
            }
        }
    }

    // 第二路：凭证体系手动条目（本地+手动合并，secret 与本地 key 重复的跳过）
    for cred in snapshots {
        let secret = cred.secret.trim();
        if secret.is_empty() || local_key.as_deref() == Some(secret) {
            continue;
        }
        entries.push(query_entry(
            &agent,
            &settings_agent,
            &cred.id,
            &cred.label,
            secret,
            None,
        ));
    }
    entries
}

/// 查询入口（provider_quota 的 "grok" match 分支调用）：以 ~/.grok 为根。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    fetch_entries_from(&grok_home(), snapshots)
}

// ============================================================
// 单元测试（纯函数，不联网、不碰真实 ~/.grok）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    /// 美分换算与百分比：totalUsed/monthlyLimit → used/total 美元 + 百分比，
    /// resets_at 秒/毫秒自适应。
    #[test]
    fn cents_conversion_and_percent() {
        let billing = serde_json::json!({
            "billingCycle": {
                "billingPeriodStart": "2026-08-01T00:00:00Z",
                "billingPeriodEnd": "2026-09-01T00:00:00Z"
            },
            "monthlyLimit": { "val": 15000 },
            "onDemandCap": { "val": 300000 },
            "usage": {
                "includedUsed": { "val": 2500 },
                "onDemandUsed": { "val": 500 },
                "totalUsed": { "val": 3000 }
            }
        });
        let w = build_monthly_window(&billing).expect("应产出窗口");
        assert_eq!(w.key, "monthly");
        assert_eq!(w.title, "本月额度");
        assert_eq!(w.used, Some(30.0)); // 3000 美分 → $30
        assert_eq!(w.total, Some(150.0)); // 15000 美分 → $150
        assert_eq!(w.unit.as_deref(), Some("$"));
        assert!((w.used_percent.unwrap() - 20.0).abs() < 1e-9); // 30/150
        assert_eq!(w.resets_at, parse_time_ms(&serde_json::json!("2026-09-01T00:00:00Z")));

        // epoch 毫秒形态的 billingPeriodEnd
        let billing = serde_json::json!({
            "billingCycle": { "billingPeriodEnd": 1783000000000i64 },
            "monthlyLimit": { "val": 100 },
            "usage": { "totalUsed": { "val": 10 } }
        });
        assert_eq!(build_monthly_window(&billing).unwrap().resets_at, Some(1_783_000_000_000));
        // epoch 秒形态 → 自动 ×1000
        let billing = serde_json::json!({
            "billingCycle": { "billingPeriodEnd": 1783000000u64 },
            "monthlyLimit": { "val": 100 },
            "usage": { "totalUsed": { "val": 10 } }
        });
        assert_eq!(
            build_monthly_window(&billing).unwrap().resets_at,
            Some(1_783_000_000_000)
        );
    }

    /// monthlyLimit=0（不限量）/null/缺失：只显示已用金额，不算百分比、不给 total。
    #[test]
    fn zero_or_missing_limit_omits_percent() {
        for limit in [serde_json::json!({ "val": 0 }), serde_json::json!({ "val": null }), serde_json::json!(null)] {
            let billing = serde_json::json!({
                "billingCycle": { "billingPeriodEnd": "2026-09-01T00:00:00Z" },
                "monthlyLimit": limit,
                "usage": { "totalUsed": { "val": 1234 } }
            });
            let w = build_monthly_window(&billing).expect("窗口仍应产出");
            assert_eq!(w.used, Some(12.34));
            assert_eq!(w.used_percent, None, "limit 无效时不应算百分比");
            assert_eq!(w.total, None, "limit 无效时不应给 total");
        }
        // 超限（used > limit）按 100% 展示，不炸进度条
        let billing = serde_json::json!({
            "monthlyLimit": { "val": 1000 },
            "usage": { "totalUsed": { "val": 5000 } }
        });
        assert_eq!(build_monthly_window(&billing).unwrap().used_percent, Some(100.0));
        // 缺 totalUsed → 无窗口（entry 层转 error）
        assert!(build_monthly_window(&serde_json::json!({ "monthlyLimit": { "val": 1 } })).is_none());
    }

    /// 过期判断：expires_at 缺失视为未过期；等于/早于 now 判过期。
    #[test]
    fn expiry_judgement() {
        assert!(!is_expired(None, NOW));
        assert!(!is_expired(Some(NOW + 1), NOW));
        assert!(is_expired(Some(NOW), NOW));
        assert!(is_expired(Some(NOW - 1), NOW));
    }

    /// scope 键优先级：OIDC（auth.x.ai::）优先于旧会话（accounts.x.ai/sign-in）；
    /// 残缺 OIDC（key 空）不能挤掉健康会话条目；两路都没有 → None。
    #[test]
    fn scope_key_priority() {
        let both = serde_json::json!({
            "https://accounts.x.ai/sign-in": { "key": "legacy-key", "email": "a@x.com" },
            "https://auth.x.ai::client-1": { "key": "oidc-key", "email": "b@x.com",
                "expires_at": "2027-01-01T00:00:00Z" }
        })
        .to_string();
        let local = select_local_auth(&both).expect("应有可用条目");
        assert_eq!(local.key, "oidc-key"); // OIDC 优先
        assert_eq!(local.email.as_deref(), Some("b@x.com"));
        assert_eq!(local.expires_at_ms, Some(1_798_761_600_000)); // 2027-01-01T00:00:00Z

        // OIDC 条目 key 为空 → 回退 legacy
        let broken_oidc = serde_json::json!({
            "https://auth.x.ai::client-1": { "key": "  " },
            "https://accounts.x.ai/sign-in": { "key": "legacy-key" }
        })
        .to_string();
        assert_eq!(select_local_auth(&broken_oidc).unwrap().key, "legacy-key");

        // 仅其他未知 scope（无 key 或非 sign-in）→ None
        assert_eq!(select_local_auth(r#"{"https://other.scope": {"key": "k"}}"#), None);
        // 坏 JSON / 顶层数组 → None
        assert_eq!(select_local_auth("not json"), None);
        assert_eq!(select_local_auth("[1]"), None);
    }

    /// 401/403 → expired「Token 无效或已过期」；网络失败/非 200/解析失败 → error。
    #[test]
    fn billing_status_mapping() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some("denied".to_string())));
            let entry = entry_from_billing_raw("local", "Grok CLI 账号", &raw, None);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(entry.message.as_deref(), Some("Token 无效或已过期，请重新 grok login"));
            assert!(entry.windows.is_empty());
        }
        // 网络层失败 → error
        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: timeout".into());
        let entry = entry_from_billing_raw("abc", "手动", &raw, None);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误"));
        // 500 → error 带状态码
        let entry = entry_from_billing_raw("abc", "手动", &Ok((500, Some("oops".into()))), None);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
        // 200 坏 JSON / 缺用量 → error
        let entry = entry_from_billing_raw("abc", "手动", &Ok((200, Some("not json".into()))), None);
        assert_eq!(entry.status, "error");
        let entry = entry_from_billing_raw("abc", "手动", &Ok((200, Some("{}".into()))), None);
        assert_eq!(entry.status, "error");
    }

    /// 成功路径：窗口 + plan_name（settings 订阅名）透传；label 由调用方拼好。
    #[test]
    fn ok_entry_with_tier() {
        let body = serde_json::json!({
            "billingCycle": { "billingPeriodEnd": "2026-09-01T00:00:00Z" },
            "monthlyLimit": { "val": 30000 },
            "usage": { "totalUsed": { "val": 15000 } }
        })
        .to_string();
        let entry = entry_from_billing_raw(
            "abc-1",
            "Grok CLI 账号（a@x.com）",
            &Ok((200, Some(body))),
            Some("SuperGrok".to_string()),
        );
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, "abc-1");
        assert_eq!(entry.label, "Grok CLI 账号（a@x.com）");
        assert_eq!(entry.plan_name.as_deref(), Some("SuperGrok"));
        assert_eq!(entry.windows.len(), 1);
        assert!((entry.windows[0].used_percent.unwrap() - 50.0).abs() < 1e-9);
    }

    /// settings 解析容错：正常取 subscription_tier_display；坏 JSON / 缺字段 /
    /// 空串 → None（不阻断额度展示）。
    #[test]
    fn settings_tier_tolerance() {
        assert_eq!(
            parse_tier_name(r#"{"subscription_tier_display":"SuperGrok Heavy"}"#)
                .as_deref(),
            Some("SuperGrok Heavy")
        );
        assert_eq!(parse_tier_name("not json"), None);
        assert_eq!(parse_tier_name(r#"{}"#), None);
        assert_eq!(parse_tier_name(r#"{"subscription_tier_display":"  "}"#), None);
        assert_eq!(parse_tier_name(r#"{"other": "x"}"#), None);
    }

    /// 时间解析：ISO（含毫秒/偏移）与 epoch 秒/毫秒互认，脏值拒绝。
    #[test]
    fn time_parsing_flexibility() {
        assert_eq!(parse_time_ms(&serde_json::json!("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(
            parse_time_ms(&serde_json::json!("1970-01-01T08:00:00.500+08:00")),
            Some(500)
        );
        assert_eq!(parse_time_ms(&serde_json::json!(1783000000u64)), Some(1_783_000_000_000));
        assert_eq!(parse_time_ms(&serde_json::json!(1783000000000i64)), Some(1_783_000_000_000));
        assert_eq!(parse_time_ms(&serde_json::json!("1783000000")), Some(1_783_000_000_000));
        // 脏值：过小数字 / 负数 / 空串 / 布尔 / 无法解析字符串
        assert_eq!(parse_time_ms(&serde_json::json!(100)), None);
        assert_eq!(parse_time_ms(&serde_json::json!(-5)), None);
        assert_eq!(parse_time_ms(&serde_json::json!("")), None);
        assert_eq!(parse_time_ms(&serde_json::json!(true)), None);
        assert_eq!(parse_time_ms(&serde_json::json!("garbage")), None);
    }

    /// 本地条目标签：有 email 拼后缀，无 email 保持原样。
    #[test]
    fn local_label_building() {
        assert_eq!(local_label(Some("a@x.com")), "Grok CLI 账号（a@x.com）");
        assert_eq!(local_label(Some("  ")), "Grok CLI 账号");
        assert_eq!(local_label(None), "Grok CLI 账号");
    }

    /// 离线闭环（临时目录，不联网）：无 auth.json 且无手动凭证 → 空数组
    /// （tab 不出现）；本地登录态已过期 → expired 条目且不发起请求；
    /// auth.json 存在但无可用条目 → pending 提示。
    #[test]
    fn offline_presence_flow() {
        let tmp = std::env::temp_dir().join(format!("zbar-grok-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let auth = tmp.join("auth.json");

        // 无数据 → 空数组
        assert!(fetch_entries_from(&tmp, &[]).is_empty());

        // 本地已过期（expires_at=1100000000 秒 → 2004-11，早于现在且能被
        // parse_time_ms 按秒解析；注意不能用 1000 / 946684800 这类过小数值
        // ——低于 parse_time_ms 的 1e9 秒下限会被判脏数据视作未过期，导致
        // 用例意外走到真实网络请求路径）
        // → expired，不发起网络请求
        std::fs::write(
            &auth,
            r#"{"https://auth.x.ai::c1": {"key": "k-expired",
                "email": "a@x.com", "expires_at": 1100000000}}"#,
        )
        .unwrap();
        let entries = fetch_entries_from(&tmp, &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].credential_id, "local");
        assert_eq!(entries[0].label, "Grok CLI 账号（a@x.com）");
        assert_eq!(entries[0].status, "expired");
        assert!(entries[0].message.as_deref().unwrap().contains("grok login"));

        // 手动凭证与过期本地 key 重复 → 去重后仅本地条目（同样不发请求）
        let dup = vec![CredentialQuerySnapshot {
            id: "manual-1".into(),
            label: "手动".into(),
            kind: "token".into(),
            secret: "k-expired".into(),
            region: None,
        }];
        assert_eq!(fetch_entries_from(&tmp, &dup).len(), 1);

        // auth.json 存在但无可用条目 → pending 提示
        std::fs::write(&auth, r#"{"https://auth.x.ai::c1": {"key": " "}}"#).unwrap();
        let entries = fetch_entries_from(&tmp, &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "pending");
        assert!(entries[0].message.as_deref().unwrap().contains("grok login"));

        std::fs::remove_dir_all(&tmp).ok();
    }
}

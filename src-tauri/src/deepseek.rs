//! DeepSeek 开放平台余额查询模块。
//!
//! 数据来源：GET https://api.deepseek.com/user/balance（Bearer API Key 鉴权）。
//! DeepSeek 无国内/国际站之分，凭证 region 忽略。
//!
//! 响应样例（余额三项均为字符串数字）：
//! `{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":
//!   "110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}`
//!
//! 映射：total_balance → balance.amount；granted_balance → granted（赠送）；
//! topped_up_balance → topped_up（充值）；currency 直接取服务端值。多条
//! balance_infos 时优先取 currency=="USD" 的，否则取第一条。
//! 401/403 → status="expired"；is_available=false 且余额非 0 → 仍为 ok，
//! 但 message 提示「余额暂不可用于 API 调用」。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, now_ms, parse_flexible_f64, quota_http_agent, ProviderQuotaBalance,
    ProviderQuotaEntry,
};

/// 余额接口基址（无区域之分）。
const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

/// 逐凭证查询 DeepSeek 余额（串行；单凭证失败产出 error/expired 条目，
/// 不阻塞其他凭证）。由 provider_quota 骨架分发调用。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .map(|cred| {
            let raw = fetch_balance_raw(&agent, cred);
            entry_from_raw(&cred.id, &cred.label, &raw)
        })
        .collect()
}

/// 单凭证余额查询（网络层）：GET https://api.deepseek.com/user/balance。
/// 返回展平的 (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，
/// 不含 secret）。解析交给 entry_from_raw 纯函数（单测不联网）。
fn fetch_balance_raw(
    agent: &ureq::Agent,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let resp = agent
        .get(BALANCE_URL)
        .set("Authorization", &format!("Bearer {}", cred.secret))
        .set("Accept", "application/json")
        .call();
    flatten_response(resp).map_err(|e| format!("DeepSeek 余额{e}"))
}

/// 从 balance_infos 中挑展示条目（纯函数，便于单测）：优先 currency=="USD"
/// 的，否则第一条；缺失/为空返回 None（由调用方报 error）。
fn pick_balance_info(v: &serde_json::Value) -> Option<&serde_json::Value> {
    let infos = v.get("balance_infos")?.as_array()?;
    infos
        .iter()
        .find(|i| i.get("currency").and_then(|c| c.as_str()) == Some("USD"))
        .or_else(|| infos.first())
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败/缺 balance_infos(error) > 成功(ok + 余额，is_available=false
/// 且余额非 0 时附加提示消息)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), String>,
) -> ProviderQuotaEntry {
    // 失败条目构造（windows 恒空、无余额；message 承载原因）
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
        return fail("error", format!("DeepSeek 余额{}", raw.as_ref().unwrap_err()));
    };
    // Key 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", "API Key 无效或已过期".to_string());
    }
    if *http_status != 200 {
        return fail("error", format!("DeepSeek 余额查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref() else {
        return fail("error", "DeepSeek 余额响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "DeepSeek 余额响应解析失败".to_string());
    };
    let Some(info) = pick_balance_info(&v) else {
        return fail("error", "DeepSeek 余额响应缺少 balance_infos".to_string());
    };
    let Some(amount) = info.get("total_balance").and_then(parse_flexible_f64) else {
        return fail("error", "DeepSeek 余额响应缺少 total_balance".to_string());
    };
    // is_available=false 且余额非 0：账户异常冻结等场景，余额仍展示但提示
    // 暂不可用（status 保持 ok，不算凭证失效）；余额为 0 时不提示（新账号常态）
    let unavailable = v.get("is_available").and_then(|b| b.as_bool()) == Some(false);
    let message = if unavailable && amount != 0.0 {
        Some("余额暂不可用于 API 调用".to_string())
    } else {
        None
    };
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows: vec![],
        balance: Some(ProviderQuotaBalance {
            amount,
            // 币种直接取服务端值（"CNY" / "USD"）
            currency: info
                .get("currency")
                .and_then(|c| c.as_str())
                .unwrap_or("CNY")
                .to_string(),
            granted: info.get("granted_balance").and_then(parse_flexible_f64),
            topped_up: info.get("topped_up_balance").and_then(parse_flexible_f64),
        }),
        plan_name: None,
        message,
        updated_at: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "ds-1";
    const LABEL: &str = "充值账号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 成功路径（样例响应）：金额/赠送/充值解析 + 币种取服务端值。
    #[test]
    fn parses_sample_balance() {
        let raw = ok_raw(
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY",
               "total_balance":"110.00","granted_balance":"10.00",
               "topped_up_balance":"100.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert!(entry.windows.is_empty());
        assert!(entry.message.is_none());
        let balance = entry.balance.expect("成功条目必须有余额");
        assert_eq!(balance.amount, 110.0);
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.granted, Some(10.0));
        assert_eq!(balance.topped_up, Some(100.0));
    }

    /// 多条 balance_infos：优先 USD，否则第一条（保持缺 USD 时的现状）。
    #[test]
    fn prefers_usd_balance_info() {
        // 有 USD 条目：取 USD
        let raw = ok_raw(
            r#"{"is_available":true,"balance_infos":[
               {"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},
               {"currency":"USD","total_balance":"15.00","granted_balance":"1.00","topped_up_balance":"14.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        let balance = entry.balance.expect("余额存在");
        assert_eq!(balance.currency, "USD");
        assert_eq!(balance.amount, 15.0);
        assert_eq!(balance.topped_up, Some(14.0));

        // 无 USD 条目：回退第一条
        let raw = ok_raw(
            r#"{"is_available":true,"balance_infos":[
               {"currency":"CNY","total_balance":"88.00","granted_balance":"8.00","topped_up_balance":"80.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.balance.expect("余额存在").amount, 88.0);
    }

    /// is_available=false 且余额非 0 → status=ok + 提示消息；余额为 0 不提示。
    #[test]
    fn unavailable_balance_keeps_ok_with_message() {
        let raw = ok_raw(
            r#"{"is_available":false,"balance_infos":[{"currency":"CNY",
               "total_balance":"50.00","granted_balance":"0.00",
               "topped_up_balance":"50.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(
            entry.message.as_deref(),
            Some("余额暂不可用于 API 调用")
        );
        assert_eq!(entry.balance.expect("余额仍应展示").amount, 50.0);

        // 余额为 0 + is_available=false（新账号常态）：不提示
        let raw = ok_raw(
            r#"{"is_available":false,"balance_infos":[{"currency":"CNY",
               "total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert!(entry.message.is_none());
    }

    /// 401/403 → expired「API Key 无效或已过期」（假 Key 手测链路的预期分支）。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some("invalid key".to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(entry.message.as_deref(), Some("API Key 无效或已过期"));
            assert!(entry.balance.is_none());
        }
    }

    /// 缺 balance_infos / 非 JSON body → error；网络失败 → error。
    #[test]
    fn malformed_responses_map_to_error() {
        let raw = ok_raw(r#"{"is_available":true}"#);
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("balance_infos"));

        let raw = Ok((200, Some("not json".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("解析失败"));

        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));
    }
}

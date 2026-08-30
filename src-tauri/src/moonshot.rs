//! Moonshot（月之暗面开放平台）余额查询模块。
//!
//! 数据来源：GET {host}/v1/users/me/balance（Bearer API Key 鉴权），余额
//! 四项均为字符串数字形态。host 按凭证 region 分流：
//! - `region == Some("global")` → https://api.moonshot.ai（国际站，USD）
//! - 其余（None 或 "cn"）→ https://api.moonshot.cn（国内站，CNY）
//!   （region 缺省按国内站处理——本项目面向国内用户）
//!
//! 响应样例：
//! `{"code":0,"data":{"available_balance":"110.00","cash_balance":"100.00",
//!   "voucher_balance":"10.00","deduction_balance":"0.00"},"smsg":""}`
//!
//! 映射：available_balance → balance.amount；voucher_balance → granted
//! （赠送）；cash_balance → topped_up（充值）；余额拆分前端已有展示，
//! message 留空。401/403 → status="expired"（API Key 无效或已过期）。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, now_ms, parse_flexible_f64, quota_http_agent, ProviderQuotaBalance,
    ProviderQuotaEntry,
};

/// region → API 主机（纯函数，便于单测）：global 走国际站，其余（None/"cn"/
/// 未知值）默认国内站。
fn host_for_region(region: Option<&str>) -> &'static str {
    if region == Some("global") {
        "https://api.moonshot.ai"
    } else {
        "https://api.moonshot.cn"
    }
}

/// region → 余额币种：国际站 USD，国内站 CNY。
fn currency_for_region(region: Option<&str>) -> &'static str {
    if region == Some("global") {
        "USD"
    } else {
        "CNY"
    }
}

/// 401/403 的过期文案（纯函数，便于单测）：region 为默认（None/"cn"，走
/// 国内站端点）时附加区域切换提示——国际站 Key 误填默认区域是最常见的
/// 「假过期」场景，引导用户在编辑弹层把区域切换为国际站；global 不附加
///（区域无错配可能）。
fn expired_message(region: Option<&str>) -> String {
    if region == Some("global") {
        "API Key 无效或已过期".to_string()
    } else {
        "API Key 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）".to_string()
    }
}

/// 逐凭证查询 Moonshot 余额（串行；单凭证失败产出 error/expired 条目，
/// 不阻塞其他凭证）。由 provider_quota 骨架分发调用。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .map(|cred| {
            let raw = fetch_balance_raw(&agent, cred);
            entry_from_raw(&cred.id, &cred.label, cred.region.as_deref(), &raw)
        })
        .collect()
}

/// 单凭证余额查询（网络层）：GET {host}/v1/users/me/balance。
/// 返回展平的 (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，
/// 不含 secret）。解析交给 entry_from_raw 纯函数（单测不联网）。
fn fetch_balance_raw(
    agent: &ureq::Agent,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let host = host_for_region(cred.region.as_deref());
    let resp = agent
        .get(&format!("{host}/v1/users/me/balance"))
        .set("Authorization", &format!("Bearer {}", cred.secret))
        .set("Accept", "application/json")
        .call();
    flatten_response(resp).map_err(|e| format!("Moonshot 余额{e}"))
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败/code!=0(error) > 成功(ok + 余额)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    region: Option<&str>,
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
        return fail("error", format!("Moonshot 余额{}", raw.as_ref().unwrap_err()));
    };
    // Key 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）；
    // 默认区域附加国际站切换提示（见 expired_message）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", expired_message(region));
    }
    if *http_status != 200 {
        return fail("error", format!("Moonshot 余额查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref() else {
        return fail("error", "Moonshot 余额响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "Moonshot 余额响应解析失败".to_string());
    };
    // 业务错误码：code != 0 时 smsg 承载平台侧原因（如 Key 被禁用）
    let code = v
        .get("code")
        .and_then(parse_flexible_f64)
        .unwrap_or(0.0);
    if code != 0.0 {
        let smsg = v
            .get("smsg")
            .and_then(|s| s.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("未知错误");
        return fail("error", format!("Moonshot 平台返回错误: {smsg}"));
    }
    let Some(data) = v.get("data").filter(|d| d.is_object()) else {
        return fail("error", "Moonshot 余额响应缺少 data 字段".to_string());
    };
    let Some(amount) = data
        .get("available_balance")
        .and_then(parse_flexible_f64)
    else {
        return fail("error", "Moonshot 余额响应缺少 available_balance".to_string());
    };
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows: vec![],
        balance: Some(ProviderQuotaBalance {
            amount,
            currency: currency_for_region(region).to_string(),
            granted: data.get("voucher_balance").and_then(parse_flexible_f64),
            topped_up: data.get("cash_balance").and_then(parse_flexible_f64),
        }),
        plan_name: None,
        // 余额拆分（赠送/充值）已由 balance 字段承载，message 留空
        message: None,
        updated_at: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "abc-1";
    const LABEL: &str = "主账号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 成功路径（样例响应）：金额/赠送/充值解析 + 国内站默认 CNY。
    #[test]
    fn parses_sample_balance_cn_default() {
        let raw = ok_raw(
            r#"{"code":0,"data":{"available_balance":"110.00","cash_balance":"100.00",
               "voucher_balance":"10.00","deduction_balance":"0.00"},"smsg":""}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert!(entry.windows.is_empty());
        let balance = entry.balance.expect("成功条目必须有余额");
        assert_eq!(balance.amount, 110.0);
        // region None 默认国内站 → CNY
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.granted, Some(10.0));
        assert_eq!(balance.topped_up, Some(100.0));
    }

    /// 国际站 region → api.moonshot.ai + USD 币种。
    #[test]
    fn global_region_maps_to_usd_host() {
        assert_eq!(host_for_region(Some("global")), "https://api.moonshot.ai");
        assert_eq!(host_for_region(Some("cn")), "https://api.moonshot.cn");
        assert_eq!(host_for_region(None), "https://api.moonshot.cn");
        assert_eq!(host_for_region(Some("weird")), "https://api.moonshot.cn");

        let raw = ok_raw(
            r#"{"code":0,"data":{"available_balance":"12.5","cash_balance":"12.5",
               "voucher_balance":"0","deduction_balance":"0"},"smsg":""}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, Some("global"), &raw);
        let balance = entry.balance.expect("余额存在");
        assert_eq!(balance.currency, "USD");
        assert_eq!(balance.amount, 12.5);
        assert_eq!(balance.granted, Some(0.0));
    }

    /// 业务错误：code != 0 → error，消息带 smsg。
    #[test]
    fn nonzero_code_maps_to_error_with_smsg() {
        let raw = ok_raw(
            r#"{"code":1002,"data":null,"smsg":"该密钥已禁用"}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.balance.is_none());
        let message = entry.message.expect("error 条目必须有原因");
        assert!(message.contains("该密钥已禁用"), "消息应带 smsg: {message}");
        // 错误消息永远不含 secret（此处连 secret 都传不进来，防回归断言形态）
        assert!(!message.contains("sk-"));
    }

    /// 401/403 → expired（假 Key 手测链路的预期分支）；默认区域（None）
    /// 的过期文案带国际站切换提示（见 expired_message_region_hint）。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some(r#"{"error":"invalid"}"#.to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            let message = entry.message.expect("expired 条目必须有原因");
            assert!(
                message.starts_with("API Key 无效或已过期"),
                "消息应以基础文案开头: {message}"
            );
            assert!(entry.balance.is_none());
        }
    }

    /// 过期文案按 region 区分（P2 区域一致性提示）：默认区域（None/"cn"，
    /// 走国内站端点）附加「切换国际站」提示；global 不附加（无区域错配）。
    #[test]
    fn expired_message_region_hint() {
        assert_eq!(expired_message(Some("global")), "API Key 无效或已过期");
        assert_eq!(
            expired_message(None),
            "API Key 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）"
        );
        assert_eq!(expired_message(Some("cn")), expired_message(None));

        // 端到端：401 + 默认区域 → expired 条目消息带提示；global → 不带
        let raw = Ok((401u16, Some(r#"{"error":"invalid"}"#.to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert!(entry.message.unwrap().contains("国际站"));
        let entry = entry_from_raw(CRED_ID, LABEL, Some("global"), &raw);
        assert_eq!(entry.message.as_deref(), Some("API Key 无效或已过期"));
    }

    /// 网络层失败（超时/DNS 等）→ error，原因透传（不含 secret）。
    #[test]
    fn network_failure_maps_to_error() {
        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        let message = entry.message.expect("网络失败必须有原因");
        assert!(message.contains("网络错误或服务不可用"));
    }

    /// 非 200 且非 401/403（如 500/429）→ error 带状态码。
    #[test]
    fn server_error_maps_to_error_with_status() {
        let raw = Ok((500, Some("internal error".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
    }
}

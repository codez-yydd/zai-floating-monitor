//! MiniMax（Coding Plan 订阅）额度查询模块。
//!
//! 数据来源：GET https://api.{host}/v1/token_plan/remains（Bearer Coding Plan
//! Token 鉴权，`sk-cp-` 前缀）。host 按凭证 region 分流：
//! - `region == Some("global")` → api.minimax.io（国际站）
//! - 其余（None 或 "cn"）→ api.minimaxi.com（国内站，默认）
//!
//! 新版端点 404/501（部分站点未部署）时回退旧版开放平台路径
//! `/v1/api/openplatform/coding_plan/remains`；401/403 属凭证错误，
//! 直接判定 expired 不做回退。
//!
//! 响应结构（信封）：
//! `{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
//!   {"model_name":"MiniMax-M2","current_interval_total_count":1000,
//!    "current_interval_usage_count":300,"current_interval_remaining_percent":70,
//!    "start_time":1730000000,"end_time":1730018000,
//!    "current_weekly_total_count":5000,"current_weekly_usage_count":1200,
//!    "current_weekly_remaining_percent":76,"weekly_start_time":1729958400,
//!    "remains_time":7200}]}}`
//!
//! 语义陷阱（CodexBar 源码注释明确，务必保持）：
//! - `current_interval_usage_count` 字段名带 usage 但实际语义是【剩余量】，
//!   不得当作已用量；已用量 = total - usage_count；
//! - 百分比一律以 `*_remaining_percent` 为准：usedPercent = 100 - 剩余百分比；
//!   缺失时用 (total - usage_count)/total 计数回退；
//! - `start_time/end_time/remains_time` 是 epoch 秒或毫秒（>10^12 判毫秒）
//!   自适应；窗口重置时间 = end_time（或 now + remains_time 兜底）。
//!
//! 窗口映射：model_remains 多条时取剩余百分比最低（最接近耗尽）的一条做
//! 主窗口，其余忽略；primary key="interval"（title「当前窗口」，5h 由
//! end-start 可推断但 title 固定），secondary key="weekly"（title「本周」；
//! title 为硬编码中文，前端 QuotaEntryCard 按 window.key 做 i18n 映射）。
//! status_code==1004 或 status_msg 含 "cookie"/"login" 或 HTTP 401/403 →
//! expired（默认区域文案附加国际站切换提示，见 expired_message）；
//! 其他非 0 → error（带 status_msg）。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, now_ms, parse_flexible_f64, quota_http_agent, ProviderQuotaEntry,
    ProviderQuotaWindow,
};

/// 新版端点路径（优先）。
const PRIMARY_PATH: &str = "/v1/token_plan/remains";
/// 旧版开放平台端点路径（新版 404/501 时回退）。
const FALLBACK_PATH: &str = "/v1/api/openplatform/coding_plan/remains";

/// region → API 主机域名（纯函数，便于单测）：global 走国际站 minimax.io，
/// 其余（None/"cn"/未知值）默认国内站 minimaxi.com。
fn host_for_region(region: Option<&str>) -> &'static str {
    if region == Some("global") {
        "api.minimax.io"
    } else {
        "api.minimaxi.com"
    }
}

/// 401/403 与登录态失效共用的过期文案（纯函数，便于单测）：region 为默认
///（None/"cn"，走国内站端点）时附加区域切换提示——国际站 Key 误填默认区域
/// 是最常见的「假过期」场景，引导用户在编辑弹层把区域切换为国际站；
/// global 不附加（区域无错配可能）。
fn expired_message(region: Option<&str>) -> String {
    if region == Some("global") {
        "Coding Plan Token 无效或已过期".to_string()
    } else {
        "Coding Plan Token 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）".to_string()
    }
}

/// 端点缺失形态（404 Not Found / 501 Not Implemented）→ 尝试回退旧端点；
/// 401/403 属凭证错误（Key 无效），网络层彻底失败也不回退（避免双倍超时）。
fn should_fallback(raw: &Result<(u16, Option<String>), String>) -> bool {
    matches!(raw, Ok((404, _)) | Ok((501, _)))
}

/// 逐凭证查询 MiniMax Coding Plan 额度（串行；单凭证失败产出 error/expired
/// 条目，不阻塞其他凭证）。由 provider_quota 骨架分发调用。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .map(|cred| {
            let base = host_for_region(cred.region.as_deref());
            let raw = fetch_remains_with_fallback(&agent, base, cred);
            entry_from_raw(&cred.id, &cred.label, cred.region.as_deref(), &raw)
        })
        .collect()
}

/// 单次请求（网络层）：GET https://{host}{path}，头含 Bearer Token、
/// accept: application/json 与 MM-API-Source: ZBar。返回展平的
/// (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，不含 secret）。
fn fetch_remains_raw(
    agent: &ureq::Agent,
    host: &str,
    path: &str,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let resp = agent
        .get(&format!("https://{host}{path}"))
        .set("Authorization", &format!("Bearer {}", cred.secret))
        .set("accept", "application/json")
        .set("MM-API-Source", "ZBar")
        .call();
    flatten_response(resp).map_err(|e| format!("MiniMax 额度{e}"))
}

/// 带回退的查询：主端点返回 404/501（站点未部署新版端点）时改用旧版
/// 开放平台路径重试一次；其余结果原样返回。
fn fetch_remains_with_fallback(
    agent: &ureq::Agent,
    host: &str,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let primary = fetch_remains_raw(agent, host, PRIMARY_PATH, cred);
    if should_fallback(&primary) {
        return fetch_remains_raw(agent, host, FALLBACK_PATH, cred);
    }
    primary
}

/// epoch 秒/毫秒自适应（纯函数）：>10^12 视为毫秒，否则按秒 ×1000，
/// 统一归一为毫秒时间戳（前端 resetsAt/展示均为 ms 口径）。
fn epoch_to_ms(raw: f64) -> i64 {
    if raw > 1_000_000_000_000.0 {
        raw as i64
    } else {
        (raw * 1000.0) as i64
    }
}

/// 从单条 model_remains 中取「剩余百分比」：优先 remaining_percent 字段，
/// 缺失时按计数回退 (usage_count)/total（usage_count 语义是剩余量）；
/// 两者皆缺 → None（选主时视为最不紧急）。
fn remaining_pct_of(model: &serde_json::Value) -> Option<f64> {
    if let Some(pct) = model
        .get("current_interval_remaining_percent")
        .and_then(parse_flexible_f64)
    {
        return Some(pct);
    }
    let total = model
        .get("current_interval_total_count")
        .and_then(parse_flexible_f64)?;
    let remaining = model
        .get("current_interval_usage_count")
        .and_then(parse_flexible_f64)?;
    if total <= 0.0 {
        return None;
    }
    Some(remaining / total * 100.0)
}

/// model_remains 多条时选「剩余百分比最低」的一条做主窗口（最接近耗尽的
/// 模型最值得关注）；百分比拿不到的条目视为 +∞（最不紧急）；全拿不到回退
/// 第一条。空数组返回 None（由调用方报 error）。
fn pick_primary_model(models: &[serde_json::Value]) -> Option<&serde_json::Value> {
    let first = models.first()?;
    Some(
        models
            .iter()
            .min_by(|a, b| {
                let pa = remaining_pct_of(a).unwrap_or(f64::INFINITY);
                let pb = remaining_pct_of(b).unwrap_or(f64::INFINITY);
                pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(first),
    )
}

/// 已用百分比（纯函数）：优先 100 - remaining_percent；百分比缺失时按
/// (total - usage_count)/total 计数回退（usage_count 语义是剩余量）；
/// 两者都拿不到 → None（前端只展示 used/total）。结果 clamp 到 0-100。
fn used_percent(
    remaining_percent: Option<f64>,
    total: Option<f64>,
    remaining_count: Option<f64>,
) -> Option<f64> {
    if let Some(pct) = remaining_percent {
        return Some((100.0 - pct).clamp(0.0, 100.0));
    }
    let total = total?;
    let remaining = remaining_count?;
    if total <= 0.0 {
        return None;
    }
    Some(((total - remaining) / total * 100.0).clamp(0.0, 100.0))
}

/// 构造单个用量窗口：任一核心字段缺失超过半数时由调用方决定是否跳过；
/// 此处只做「已用量 = total - 剩余量」的换算与 clamp，unit 统一「次」。
fn build_window(
    key: &str,
    title: &str,
    total: Option<f64>,
    remaining_count: Option<f64>,
    remaining_percent: Option<f64>,
    resets_at: Option<i64>,
) -> ProviderQuotaWindow {
    let used = match (total, remaining_count) {
        (Some(total), Some(remaining)) => Some((total - remaining).max(0.0)),
        _ => None,
    };
    ProviderQuotaWindow {
        key: key.to_string(),
        title: title.to_string(),
        used_percent: used_percent(remaining_percent, total, remaining_count),
        used,
        total,
        unit: Some("次".to_string()),
        resets_at,
    }
}

/// 字段是否有值（窗口裁剪用）：JSON null / 缺失都算无值。
fn has_field(model: &serde_json::Value, key: &str) -> bool {
    model.get(key).map(|v| !v.is_null()).unwrap_or(false)
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败(error) > base_resp.status_code != 0（1004/含 cookie|login
/// → expired，其余 → error）> 缺 model_remains(error) > 成功(ok + 双窗口)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    region: Option<&str>,
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
        return fail("error", format!("MiniMax 额度{}", raw.as_ref().unwrap_err()));
    };
    // Token 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）；
    // 默认区域附加国际站切换提示（见 expired_message）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", expired_message(region));
    }
    if *http_status != 200 {
        return fail("error", format!("MiniMax 额度查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref() else {
        return fail("error", "MiniMax 额度响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "MiniMax 额度响应解析失败".to_string());
    };
    // 业务信封：base_resp.status_code != 0 时按语义分流
    let status_code = v
        .get("base_resp")
        .and_then(|b| b.get("status_code"))
        .and_then(parse_flexible_f64)
        .unwrap_or(0.0);
    if status_code != 0.0 {
        let status_msg = v
            .get("base_resp")
            .and_then(|b| b.get("status_msg"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        // 1004 为平台约定的登录态失效码；部分站点把原因写在 status_msg
        // （含 cookie/login 字样），两者都归为 Token 过期（默认区域附加
        // 国际站切换提示，见 expired_message）
        if status_code == 1004.0
            || status_msg.to_lowercase().contains("cookie")
            || status_msg.to_lowercase().contains("login")
        {
            return fail("expired", expired_message(region));
        }
        let reason = if status_msg.trim().is_empty() {
            format!("status_code {status_code}")
        } else {
            status_msg.to_string()
        };
        return fail("error", format!("MiniMax 平台返回错误: {reason}"));
    }
    let Some(models) = v
        .get("data")
        .and_then(|d| d.get("model_remains"))
        .and_then(|m| m.as_array())
        .filter(|m| !m.is_empty())
    else {
        return fail("error", "MiniMax 额度响应缺少 model_remains".to_string());
    };
    let Some(model) = pick_primary_model(models) else {
        return fail("error", "MiniMax 额度响应缺少 model_remains".to_string());
    };

    // 窗口重置时间：end_time 优先；缺失时 now + remains_time 兜底
    // （remains_time 同样秒/毫秒自适应）。周窗按 weekly_start_time + 7 天估算。
    let interval_reset = model
        .get("end_time")
        .and_then(parse_flexible_f64)
        .map(epoch_to_ms)
        .or_else(|| {
            model
                .get("remains_time")
                .and_then(parse_flexible_f64)
                .map(|rt| now_ms() + epoch_to_ms(rt))
        });
    let weekly_reset = model
        .get("weekly_start_time")
        .and_then(parse_flexible_f64)
        .map(|ws| epoch_to_ms(ws) + 7 * 86_400_000);

    let mut windows = Vec::new();
    // 会话窗口（主窗）：interval 字段任一有值才产出
    if has_field(model, "current_interval_total_count")
        || has_field(model, "current_interval_usage_count")
        || has_field(model, "current_interval_remaining_percent")
    {
        windows.push(build_window(
            "interval",
            "当前窗口",
            model
                .get("current_interval_total_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_interval_usage_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_interval_remaining_percent")
                .and_then(parse_flexible_f64),
            interval_reset,
        ));
    }
    // 周窗（副窗）：weekly 字段任一有值才产出
    if has_field(model, "current_weekly_total_count")
        || has_field(model, "current_weekly_usage_count")
        || has_field(model, "current_weekly_remaining_percent")
    {
        windows.push(build_window(
            "weekly",
            "本周",
            model
                .get("current_weekly_total_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_weekly_usage_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_weekly_remaining_percent")
                .and_then(parse_flexible_f64),
            weekly_reset,
        ));
    }

    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name: None,
        message: None,
        updated_at: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "mm-1";
    const LABEL: &str = "Coding Plan";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 任务给定的样例响应（成功路径：双窗口 + 百分比反推 + 秒级时间戳）。
    const SAMPLE_BODY: &str = r#"{
        "base_resp": {"status_code": 0, "status_msg": ""},
        "data": {
            "model_remains": [
                {
                    "model_name": "MiniMax-M2",
                    "current_interval_total_count": 1000,
                    "current_interval_usage_count": 300,
                    "current_interval_remaining_percent": 70,
                    "start_time": 1730000000,
                    "end_time": 1730018000,
                    "current_weekly_total_count": 5000,
                    "current_weekly_usage_count": 1200,
                    "current_weekly_remaining_percent": 76,
                    "weekly_start_time": 1729958400,
                    "remains_time": 7200
                }
            ]
        }
    }"#;

    /// 成功路径：ok 状态、双窗口、百分比以 remaining_percent 反推、
    /// usage_count（剩余语义）换算已用量、重置时间秒 → 毫秒。
    #[test]
    fn parses_sample_dual_windows() {
        let entry = entry_from_raw(CRED_ID, LABEL, None, &ok_raw(SAMPLE_BODY));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert_eq!(entry.windows.len(), 2);

        // 主窗：interval（当前窗口）——usage_count=300 是剩余量，已用 = 700
        let interval = &entry.windows[0];
        assert_eq!(interval.key, "interval");
        assert_eq!(interval.title, "当前窗口");
        assert_eq!(interval.used_percent, Some(30.0)); // 100 - 70
        assert_eq!(interval.used, Some(700.0)); // 1000 - 300（剩余）
        assert_eq!(interval.total, Some(1000.0));
        assert_eq!(interval.unit.as_deref(), Some("次"));
        assert_eq!(interval.resets_at, Some(1_730_018_000_000)); // 秒 → 毫秒

        // 副窗：weekly（本周）
        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.title, "本周");
        assert_eq!(weekly.used_percent, Some(24.0)); // 100 - 76
        assert_eq!(weekly.used, Some(3800.0)); // 5000 - 1200（剩余）
        // weekly_start_time + 7 天（估算）
        assert_eq!(weekly.resets_at, Some(1_729_958_400_000 + 7 * 86_400_000));
    }

    /// status_code == 1004 → expired（默认区域文案带国际站切换提示）。
    #[test]
    fn status_code_1004_maps_to_expired() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":1004,"status_msg":"invalid token"},"data":null}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "expired");
        assert_eq!(entry.windows.len(), 0);
        assert_eq!(entry.message.as_deref(), Some(expired_message(None).as_str()));
    }

    /// status_msg 含 cookie / login（不区分大小写）→ expired。
    #[test]
    fn login_keyword_in_status_msg_maps_to_expired() {
        for msg in ["Please login first", "COOKIE 失效", "login required"] {
            let body = serde_json::json!({
                "base_resp": {"status_code": 1001, "status_msg": msg},
                "data": null
            })
            .to_string();
            let entry = entry_from_raw(CRED_ID, LABEL, None, &ok_raw(&body));
            assert_eq!(entry.status, "expired", "status_msg={msg} 应判定 expired");
            assert_eq!(
                entry.message.as_deref(),
                Some(expired_message(None).as_str())
            );
        }
    }

    /// 其他非 0 status_code → error（带 status_msg）。
    #[test]
    fn other_status_code_maps_to_error() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":1008,"status_msg":"quota exceeded"},"data":null}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        let message = entry.message.expect("error 条目必须有原因");
        assert!(message.contains("quota exceeded"), "消息应带 status_msg: {message}");
        assert!(!message.contains("sk-"), "错误消息不得含 secret 片段");
    }

    /// remaining_percent 缺失 → 按 (total - usage_count)/total 计数回退
    /// （usage_count 语义是剩余量：300 剩余 / 1000 总量 → 已用 70%）。
    #[test]
    fn missing_remaining_percent_falls_back_to_counts() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
               {"model_name":"MiniMax-M2",
                "current_interval_total_count":1000,
                "current_interval_usage_count":300}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows.len(), 1); // weekly 字段全缺 → 周窗不产出
        let interval = &entry.windows[0];
        assert_eq!(interval.used_percent, Some(70.0));
        assert_eq!(interval.used, Some(700.0));
        // end_time / remains_time 均缺 → 无重置时间
        assert_eq!(interval.resets_at, None);
    }

    /// 时间戳秒/毫秒自适应（>10^12 判毫秒）。
    #[test]
    fn epoch_seconds_and_ms_adaptive() {
        assert_eq!(epoch_to_ms(1_730_018_000.0), 1_730_018_000_000);
        assert_eq!(epoch_to_ms(1_730_018_000_000.0), 1_730_018_000_000);

        // end_time 直接给毫秒 → 原样保留
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
               {"model_name":"M","current_interval_total_count":100,
                "current_interval_usage_count":10,
                "current_interval_remaining_percent":90,
                "end_time":1730018000000}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.windows[0].resets_at, Some(1_730_018_000_000));
    }

    /// HTTP 401/403 → expired（假 Key 手测链路的预期分支，不做端点回退）；
    /// 默认区域文案带国际站切换提示（精确断言见 expired_message_region_hint）。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some(r#"{"error":"invalid"}"#.to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(
                entry.message.as_deref(),
                Some(expired_message(None).as_str())
            );
            assert!(entry.windows.is_empty());
        }
    }

    /// 过期文案按 region 区分（P2 区域一致性提示）：默认区域（None/"cn"，
    /// 走国内站端点）附加「切换国际站」提示；global 不附加（无区域错配）。
    /// 消息不含 secret 片段。
    #[test]
    fn expired_message_region_hint() {
        assert_eq!(
            expired_message(Some("global")),
            "Coding Plan Token 无效或已过期"
        );
        assert_eq!(
            expired_message(None),
            "Coding Plan Token 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）"
        );
        assert_eq!(expired_message(Some("cn")), expired_message(None));

        // 端到端：401 + 默认区域 → expired 条目消息带提示；global → 不带
        let raw = Ok((401u16, Some(r#"{"error":"invalid"}"#.to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "expired");
        let message = entry.message.expect("expired 条目必须有原因");
        assert!(message.contains("国际站"), "默认区域应带区域提示: {message}");
        assert!(!message.contains("sk-"), "错误消息不得含 secret 片段");
        let entry = entry_from_raw(CRED_ID, LABEL, Some("global"), &raw);
        assert_eq!(
            entry.message.as_deref(),
            Some("Coding Plan Token 无效或已过期")
        );
    }

    /// 网络层失败 → error，原因透传；非 200 非 401/403 → error 带状态码。
    #[test]
    fn network_and_server_failures_map_to_error() {
        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));

        let raw = Ok((500, Some("internal error".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
    }

    /// 回退判定：仅端点缺失形态（404/501）回退；401/403/200/网络错误不回退。
    #[test]
    fn fallback_only_on_missing_endpoint() {
        assert!(should_fallback(&Ok((404, None))));
        assert!(should_fallback(&Ok((501, None))));
        assert!(!should_fallback(&Ok((401, None)))); // 凭证错误不回退
        assert!(!should_fallback(&Ok((200, None))));
        assert!(!should_fallback(&Ok((500, None))));
        assert!(!should_fallback(&Err("网络错误".to_string()))); // 避免双倍超时
    }

    /// region → 主机分流：global 国际站，其余默认国内站。
    #[test]
    fn region_maps_to_host() {
        assert_eq!(host_for_region(Some("global")), "api.minimax.io");
        assert_eq!(host_for_region(Some("cn")), "api.minimaxi.com");
        assert_eq!(host_for_region(None), "api.minimaxi.com");
        assert_eq!(host_for_region(Some("weird")), "api.minimaxi.com");
    }

    /// 多条 model_remains：取剩余百分比最低（最接近耗尽）的一条做主窗口。
    #[test]
    fn multiple_models_picks_lowest_remaining() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
               {"model_name":"MiniMax-M2","current_interval_total_count":1000,
                "current_interval_usage_count":700,"current_interval_remaining_percent":70},
               {"model_name":"MiniMax-Text01","current_interval_total_count":100,
                "current_interval_usage_count":10,"current_interval_remaining_percent":10}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        // 两条都只有 interval 字段 → 只产出会话主窗
        assert_eq!(entry.windows.len(), 1);
        // 选中剩余 10% 的那条：已用 90%
        assert_eq!(entry.windows[0].used_percent, Some(90.0));
        assert_eq!(entry.windows[0].used, Some(90.0));
        assert_eq!(entry.windows[0].total, Some(100.0));
    }

    /// 全部字段缺失（model_remains 为空对象）→ 无窗口产出，状态仍 ok；
    /// model_remains 缺失 / 空 → error。
    #[test]
    fn empty_or_missing_model_remains() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("model_remains"));

        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[{}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        assert!(entry.windows.is_empty());
    }
}

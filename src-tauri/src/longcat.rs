//! LongCat（Token 配额 + 加油包）额度查询模块。
//!
//! 凭证型：kind=cookie 的 secret 是用户从浏览器复制的 Cookie 请求头（或整段
//! Copy as cURL 粘贴，由 cookie_util::normalize_cookie_secret 归一），host
//! 固定 https://longcat.chat，头统一为 chrome_like_headers 产物（Origin 用
//! 站点根，Referer 用 https://longcat.chat/platform/usage）。
//!
//! 三步 GET：
//! 1. `/api/v1/user-current`：会话探测 + 账号名（`data.name` 或
//!    `data.nickName`）作 label 后缀。⚠ 该响应含敏感 token/手机号，解析时
//!    只取账号名，响应体绝不进任何日志、debug 输出或错误消息；
//! 2. `/api/lc-platform/v1/tokenUsage`：主配额，`data.usage.{totalToken,
//!    usedToken, availableToken}`（data.usage 缺失时退一层直接读顶层 usage）；
//! 3. `/api/lc-platform/v1/pending-fuel-packages`（可选）：`data.{totalQuota,
//!    list[].{availableToken, expireTime}}`，取 list 汇总剩余与最近到期；
//!    失败仅降级不报错。
//!
//! 信封 `{code, message, data}`：`code==0||code==200` 成功；HTTP 401/403 或
//! code 401/403 → expired「请重新登录 longcat.chat」；其他非 0 → error
//! （取 message|msg）。expireTime 兼容 epoch 秒/毫秒、ISO-8601、
//! `yyyy-MM-dd HH:mm:ss`（cookie_util::parse_time_flexible 自适应）。
//!
//! 窗口：主窗 key="quota" title="Token 配额"（used/total 原始数值 +
//! usedPercent=used/total*100，total 为 0 时无百分比）；副窗 key="fuel"
//! title="加油包"（汇总剩余 availableToken + 最近 expireTime 作 resets_at；
//! 无加油包数据时省略副窗）。
//!
//! 工程纪律（对齐 minimax.rs / qoder.rs）：网络 ureq 同步 + 15s 超时 +
//! resolve_proxy，调用方 spawn_blocking；解析纯函数可单测；错误消息中文
//! 且不含 secret；Cookie 值与用户敏感字段不进任何日志。

use crate::cookie_util::{chrome_like_headers, normalize_cookie_secret, parse_time_flexible};
use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, get_any, now_ms, num_any, quota_http_agent, ProviderQuotaEntry,
    ProviderQuotaWindow,
};

/// 站点根（LongCat 无国内/国际站之分，凭证 region 忽略）。
const HOST: &str = "https://longcat.chat";
/// 用量页路径（Referer 用）。
const USAGE_PAGE: &str = "https://longcat.chat/platform/usage";

/// 逐凭证查询 LongCat（串行；单凭证失败产出 error/expired 条目，不阻塞
/// 其他凭证）。只消费 kind=cookie 的凭证，由 provider_quota 骨架分发。
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
            if cookie.is_empty() {
                return fail_entry(
                    &cred.id,
                    &cred.label,
                    false,
                    "未能从粘贴内容中解析出 Cookie，请重新复制请求头或 cURL 命令",
                );
            }
            let steps = StepRaw {
                user: fetch_user_raw(&agent, &cookie),
                usage: fetch_usage_raw(&agent, &cookie),
                fuel: fetch_fuel_raw(&agent, &cookie),
            };
            entry_from_raws(&cred.id, &cred.label, &steps)
        })
        .collect()
}

// ============================================================
// 网络层（三步 GET；解析交给 entry_from_raws 纯函数）
// ============================================================

/// 单步请求的展平结果（HTTP 状态码 + 响应体；网络层失败为 Err）。
type StepResult = Result<(u16, Option<String>), String>;

/// 带统一浏览器仿真头发起 GET（Origin=站点根，Referer=用量页）。
fn get_with_headers(agent: &ureq::Agent, path: &str, cookie: &str) -> Result<ureq::Response, ureq::Error> {
    let mut req = agent.get(&format!("{HOST}{path}"));
    for (name, value) in chrome_like_headers(cookie, HOST, USAGE_PAGE) {
        req = req.set(&name, &value);
    }
    req.call()
}

/// 第 1 步：会话探测 + 账号名。响应含敏感信息，错误只保留状态码语义，
/// 绝不携带响应体。
fn fetch_user_raw(agent: &ureq::Agent, cookie: &str) -> StepResult {
    let resp = get_with_headers(agent, "/api/v1/user-current", cookie);
    flatten_response(resp).map_err(|e| format!("LongCat 会话校验{e}"))
}

/// 第 2 步：主配额（tokenUsage）。
fn fetch_usage_raw(agent: &ureq::Agent, cookie: &str) -> StepResult {
    let resp = get_with_headers(agent, "/api/lc-platform/v1/tokenUsage", cookie);
    flatten_response(resp).map_err(|e| format!("LongCat 用量查询{e}"))
}

/// 第 3 步：加油包（可选，失败由调用方降级）。
fn fetch_fuel_raw(agent: &ureq::Agent, cookie: &str) -> StepResult {
    let resp = get_with_headers(agent, "/api/lc-platform/v1/pending-fuel-packages", cookie);
    flatten_response(resp).map_err(|e| format!("LongCat 加油包查询{e}"))
}

// ============================================================
// 解析纯函数（单测直接构造输入，不联网）
// ============================================================

/// 三步响应的内存形态（测试直接构造）。
struct StepRaw {
    user: StepResult,
    usage: StepResult,
    fuel: StepResult,
}

/// 单步失败（expired 与原因消息；消息中文且不含响应体/secret）。
struct StepError {
    expired: bool,
    message: String,
}

/// 信封解析（纯函数）：网络失败透传；HTTP 401/403 或 code 401/403 →
/// expired；HTTP 非 200 → error 带状态码；body 缺失/解析失败 → error；
/// code 非 0 且非 200 → error（message|msg）；其余返回整体 JSON。
fn parse_envelope(raw: &StepResult, label: &str) -> Result<serde_json::Value, StepError> {
    let expired = || StepError {
        expired: true,
        message: "请重新登录 longcat.chat".to_string(),
    };
    let Ok((http_status, body)) = raw else {
        return Err(StepError {
            expired: false,
            message: raw.as_ref().unwrap_err().clone(),
        });
    };
    if *http_status == 401 || *http_status == 403 {
        return Err(expired());
    }
    if *http_status != 200 {
        return Err(StepError {
            expired: false,
            message: format!("{label}失败（HTTP {http_status}）"),
        });
    }
    let Some(body) = body.as_deref() else {
        return Err(StepError {
            expired: false,
            message: format!("{label}响应解析失败"),
        });
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Err(StepError {
            expired: false,
            message: format!("{label}响应解析失败"),
        });
    };
    let code = num_any(&v, &["code"]).unwrap_or(0.0);
    if code == 401.0 || code == 403.0 {
        return Err(expired());
    }
    if code != 0.0 && code != 200.0 {
        let msg = ["message", "msg"]
            .iter()
            .find_map(|k| v.get(*k).and_then(|m| m.as_str()))
            .filter(|m| !m.trim().is_empty())
            .unwrap_or("未知错误");
        return Err(StepError {
            expired: false,
            message: format!("{label}平台返回错误: {msg}"),
        });
    }
    Ok(v)
}

/// 从信封取 data 对象。
fn data_of(v: &serde_json::Value) -> Option<&serde_json::Value> {
    get_any(v, &["data"]).filter(|d| d.is_object())
}

/// 会话探测的账号名（`data.name` 或 `data.nickName`，仅展示用）。
fn account_name(v: &serde_json::Value) -> Option<String> {
    data_of(v)
        .and_then(|d| get_any(d, &["name", "nickName"]))
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// usage 对象：优先 data.usage，缺失时退一层直接读顶层 usage。
fn usage_of(v: &serde_json::Value) -> Option<&serde_json::Value> {
    data_of(v)
        .and_then(|d| get_any(d, &["usage"]))
        .filter(|u| u.is_object())
        .or_else(|| get_any(v, &["usage"]).filter(|u| u.is_object()))
}

/// 加油包汇总：list 各项 availableToken 求和 + 最近（最早）expireTime。
/// list 缺失/为空 → None（无加油包数据，副窗省略）。
fn fuel_summary(v: &serde_json::Value) -> Option<(f64, Option<i64>)> {
    let list = data_of(v)?
        .get("list")?
        .as_array()
        .filter(|l| !l.is_empty())?;
    let mut remaining = 0.0;
    let mut earliest: Option<i64> = None;
    for item in list {
        if let Some(token) = num_any(item, &["availableToken", "available_token"]) {
            remaining += token;
        }
        // 三格式自适应解析；解析失败的条目跳过到期时间（不拖垮汇总）
        if let Some(at) = get_any(item, &["expireTime", "expire_time"]).and_then(parse_time_flexible) {
            earliest = Some(earliest.map_or(at, |cur| cur.min(at)));
        }
    }
    Some((remaining, earliest))
}

/// 已用/总量 → 百分比：total 缺失或 ≤0 → None（total 为 0 时无百分比）。
fn pct_of(used: Option<f64>, total: Option<f64>) -> Option<f64> {
    match (used, total) {
        (Some(u), Some(t)) if t > 0.0 => Some(((u / t) * 100.0).clamp(0.0, 100.0)),
        _ => None,
    }
}

/// 失败条目构造（windows 恒空；message 承载原因；expired 标记决定状态）。
fn fail_entry(cred_id: &str, label: &str, expired: bool, message: &str) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: if expired { "expired" } else { "error" }.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message.to_string()),
        updated_at: now_ms(),
    }
}

/// 解析三步结果 → 展示条目（纯函数）。
/// 主链路：会话探测失败即整体失败（expired/error）→ 主配额缺失失败 →
/// 加油包失败仅降级（省略副窗）。账号名存在时作 label 后缀展示。
fn entry_from_raws(cred_id: &str, label: &str, steps: &StepRaw) -> ProviderQuotaEntry {
    // 1. 会话探测（含敏感信息，任何失败都不回显响应内容）
    let user = match parse_envelope(&steps.user, "LongCat 会话校验") {
        Ok(v) => v,
        Err(e) => return fail_entry(cred_id, label, e.expired, &e.message),
    };
    let label = account_name(&user).map_or_else(
        || label.to_string(),
        |name| format!("{label} · {name}"),
    );

    // 2. 主配额
    let usage_v = match parse_envelope(&steps.usage, "LongCat 用量查询") {
        Ok(v) => v,
        Err(e) => return fail_entry(cred_id, &label, e.expired, &e.message),
    };
    let Some(usage) = usage_of(&usage_v) else {
        return fail_entry(cred_id, &label, false, "LongCat 响应缺少 usage 数据");
    };
    let total = num_any(usage, &["totalToken", "total_token"]);
    let used = num_any(usage, &["usedToken", "used_token"])
        // usedToken 缺失时按 total - available 回退
        .or_else(|| {
            Some(
                num_any(usage, &["totalToken", "total_token"])?
                    - num_any(usage, &["availableToken", "available_token"])?,
            )
        });
    if total.is_none() && used.is_none() {
        return fail_entry(cred_id, &label, false, "LongCat 响应缺少 usage 数据");
    }

    // 3. 加油包（可选：任何失败仅省略副窗，不影响主窗）
    let fuel = parse_envelope(&steps.fuel, "LongCat 加油包查询")
        .ok()
        .and_then(|v| fuel_summary(&v));

    let mut windows = vec![ProviderQuotaWindow {
        key: "quota".to_string(),
        title: "Token 配额".to_string(),
        used_percent: pct_of(used, total),
        used,
        total,
        unit: None,
        resets_at: None,
    }];
    if let Some((remaining, earliest)) = fuel {
        windows.push(ProviderQuotaWindow {
            key: "fuel".to_string(),
            title: "加油包".to_string(),
            used_percent: None,
            used: Some(remaining),
            total: None,
            unit: None,
            resets_at: earliest,
        });
    }

    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label,
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

    const CRED_ID: &str = "lc-1";

    fn ok_raw(body: &str) -> StepResult {
        Ok((200, Some(body.to_string())))
    }

    fn steps(user: &str, usage: &str, fuel: StepResult) -> StepRaw {
        StepRaw {
            user: ok_raw(user),
            usage: ok_raw(usage),
            fuel,
        }
    }

    /// 全链路成功：账号名作 label 后缀；data.usage 主窗；加油包汇总剩余 +
    /// 最早到期；usedPercent = used/total*100。
    #[test]
    fn parses_full_success_flow() {
        let s = steps(
            r#"{"code":0,"data":{"name":"Alice","token":"sensitive-do-not-log"}}"#,
            r#"{"code":0,"data":{"usage":{"totalToken":1000000,"usedToken":300000,
               "availableToken":700000}}}"#,
            ok_raw(
                r#"{"code":0,"data":{"totalQuota":500000,"list":[
                   {"availableToken":100000,"expireTime":1730000000},
                   {"availableToken":50000,"expireTime":1729900000}]}}"#,
            ),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "ok");
        // 账号名后缀（token 等敏感字段不进 label/消息）
        assert_eq!(entry.label, "主账号 · Alice");
        assert_eq!(entry.windows.len(), 2);

        let quota = &entry.windows[0];
        assert_eq!(quota.key, "quota");
        assert_eq!(quota.title, "Token 配额");
        assert_eq!(quota.used, Some(300000.0));
        assert_eq!(quota.total, Some(1000000.0));
        assert_eq!(quota.used_percent, Some(30.0));
        assert_eq!(quota.resets_at, None);

        let fuel = &entry.windows[1];
        assert_eq!(fuel.key, "fuel");
        assert_eq!(fuel.title, "加油包");
        assert_eq!(fuel.used, Some(150000.0)); // 100000 + 50000
        // 最近（最早）到期：1729900000 秒 → 毫秒
        assert_eq!(fuel.resets_at, Some(1_729_900_000_000));
    }

    /// 会话失效：HTTP 401/403 与信封 code 401/403 都 → expired
    /// 「请重新登录 longcat.chat」。
    #[test]
    fn unauthorized_maps_to_expired() {
        let usage_ok = r#"{"code":0,"data":{"usage":{"totalToken":10,"usedToken":1}}}"#;
        for user in [
            // HTTP 401（响应体可能是 HTML 登录页，不回显）
            r#"unauthorized page"#,
            // 信封 code 401
            r#"{"code":401,"message":"login required","data":null}"#,
        ] {
            let raw_user: StepResult = if user.starts_with('{') {
                ok_raw(user)
            } else {
                Ok((401, Some(user.to_string())))
            };
            let s = StepRaw {
                user: raw_user,
                usage: ok_raw(usage_ok),
                fuel: Err("未发起".to_string()),
            };
            let entry = entry_from_raws(CRED_ID, "主账号", &s);
            assert_eq!(entry.status, "expired");
            assert_eq!(entry.message.as_deref(), Some("请重新登录 longcat.chat"));
            assert!(entry.windows.is_empty());
            // 敏感响应体不得进入任何消息
            assert!(!entry.message.as_deref().unwrap_or("").contains("sensitive"));
        }
    }

    /// usage 缺一层：data.usage 缺失时退读顶层 usage；字段蛇形双兼容。
    #[test]
    fn usage_falls_back_to_root_level() {
        let s = steps(
            r#"{"code":0,"data":{"nickName":"Bob"}}"#,
            r#"{"code":0,"usage":{"total_token":500,"used_token":100,"available_token":400}}"#,
            Err("未发起".to_string()),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "ok");
        // name 缺失时回退 nickName
        assert_eq!(entry.label, "主账号 · Bob");
        assert_eq!(entry.windows.len(), 1);
        assert_eq!(entry.windows[0].used, Some(100.0));
        assert_eq!(entry.windows[0].total, Some(500.0));
        assert_eq!(entry.windows[0].used_percent, Some(20.0));
    }

    /// expireTime 三格式兼容：epoch 秒/毫秒、ISO-8601、yyyy-MM-dd HH:mm:ss，
    /// 副窗 resets_at 取最早一条。
    #[test]
    fn expire_time_formats_adaptive() {
        let s = steps(
            r#"{"code":0,"data":{"name":"C"}}"#,
            r#"{"code":0,"data":{"usage":{"totalToken":10,"usedToken":1}}}"#,
            ok_raw(
                r#"{"code":0,"data":{"list":[
                   {"availableToken":1,"expireTime":"2030-10-27T05:06:07Z"},
                   {"availableToken":2,"expireTime":1730000000},
                   {"availableToken":4,"expireTime":"1730000000000"},
                   {"availableToken":8,"expireTime":"2030-10-28 05:06:07"}]}}"#,
            ),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "ok");
        let fuel = &entry.windows[1];
        assert_eq!(fuel.used, Some(15.0));
        // 最早：1730000000 秒 → 1_730_000_000_000 ms
        assert_eq!(fuel.resets_at, Some(1_730_000_000_000));
    }

    /// 加油包降级：空 list / 接口失败 / 401 都只省略副窗，主窗照常 ok；
    /// code=200 变体同样视为成功。
    #[test]
    fn fuel_degrades_and_code_200_is_success() {
        // 空 list → 无副窗
        let s = steps(
            r#"{"code":200,"data":{"name":"D"}}"#,
            r#"{"code":200,"data":{"usage":{"totalToken":10,"usedToken":1}}}"#,
            ok_raw(r#"{"code":200,"data":{"list":[]}}"#),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "ok", "code=200 信封应视为成功");
        assert_eq!(entry.windows.len(), 1);

        // 加油包网络失败 / 401 → 降级不报错
        for fuel in [
            Err("LongCat 加油包查询网络错误或服务不可用: timeout".to_string()) as StepResult,
            Ok((401, Some("login".to_string()))),
        ] {
            let s = steps(
                r#"{"code":0,"data":{"name":"D"}}"#,
                r#"{"code":0,"data":{"usage":{"totalToken":10,"usedToken":1}}}"#,
                fuel,
            );
            let entry = entry_from_raws(CRED_ID, "主账号", &s);
            assert_eq!(entry.status, "ok");
            assert_eq!(entry.windows.len(), 1, "加油包失败仅省略副窗");
            assert_eq!(entry.windows[0].key, "quota");
        }
    }

    /// 其他非 0 code → error（message|msg）；usage 对象整体缺失 → error；
    /// total 为 0 时无百分比。
    #[test]
    fn error_branches_and_zero_total() {
        // 用量接口业务错误
        let s = steps(
            r#"{"code":0,"data":{"name":"E"}}"#,
            r#"{"code":500,"msg":"internal failure","data":null}"#,
            Err("未发起".to_string()),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("internal failure"));

        // usage 对象缺失
        let s = steps(
            r#"{"code":0,"data":{"name":"E"}}"#,
            r#"{"code":0,"data":{"other":1}}"#,
            Err("未发起".to_string()),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "error");
        assert_eq!(entry.message.as_deref(), Some("LongCat 响应缺少 usage 数据"));

        // total=0：无百分比，仍展示 used/total 原始值
        let s = steps(
            r#"{"code":0,"data":{}}"#,
            r#"{"code":0,"data":{"usage":{"totalToken":0,"usedToken":0}}}"#,
            Err("未发起".to_string()),
        );
        let entry = entry_from_raws(CRED_ID, "主账号", &s);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows[0].used_percent, None);
        assert_eq!(entry.windows[0].total, Some(0.0));
    }

    /// Cookie 无法解析 → 不发起任何请求直接产出 error 条目；
    /// 非 cookie 凭证被过滤。
    #[test]
    fn unparseable_cookie_fails_fast_and_filters_kinds() {
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
                secret: "curl 'https://longcat.chat' -H 'User-Agent: x'".into(),
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

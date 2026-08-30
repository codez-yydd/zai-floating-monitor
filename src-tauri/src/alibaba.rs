//! 阿里 Coding Plan（通义灵码 / 百炼）额度查询模块。
//!
//! 数据来源：`POST {host}/data/api.json?action=zeldaEasy.broadscope-bailian
//! .codingPlan.queryCodingPlanInstanceInfoV2&product=broadscope-bailian&api=
//! queryCodingPlanInstanceInfoV2&currentRegionId=<id>`，Bearer DashScope
//! API Key 鉴权（同时带 x-api-key / X-DashScope-API-Key 双兼容头）。host 按
//! 凭证 region 分流（对齐 CodexBar AlibabaCodingPlanAPIRegion）：
//! - `region == Some("global")` → modelstudio.console.alibabacloud.com（国际站）
//! - 其余（None 或 "cn"）→ bailian.console.aliyun.com（中国站，默认）
//!
//! 请求体（CodexBar queryCodingPlanAPIRequestBody 照抄，文档未写请求体）：
//! `{"queryCodingPlanInstanceInfoRequest":{"commodityCode":"sfm_codingplan_public_cn|intl"}}`
//!
//! 响应信封（阿里 console 惯例，键名可能 camelCase/snake_case 双形态，且
//! data 字段可能是「JSON 字符串再包一层」，参考 CodexBar expandedJSON 展开）：
//! `{data:{"codingPlanInstanceInfos":[{planName, instanceName, packageName,
//!   status, codingPlanQuotaInfo:{...}}], "codingPlanQuotaInfo":{...}}}`
//! 配额三窗口（usedPercent = used/total*100）：
//! - 5h：per5HourUsedQuota / per5HourTotalQuota / per5HourQuotaNextRefreshTime
//!   （兼容 perFiveHour* 别名）
//! - 周：perWeekUsedQuota / perWeekTotalQuota / perWeekQuotaNextRefreshTime
//! - 月：perBillMonthUsedQuota / perBillMonthTotalQuota /
//!   perBillMonthQuotaNextRefreshTime（兼容 perMonth* 别名）
//! 重置时间 epoch 秒/毫秒、ISO、`yyyy-MM-dd HH:mm:ss` 均可
//! （cookie_util::parse_time_flexible 自适应）。多实例取第一个有效实例
//! （status=VALID/ACTIVE 优先， expired 类状态沉底，对齐 CodexBar
//! findActiveInstanceInfo）。
//!
//! 错误映射（对齐 CodexBar parseUsageSnapshot 的判定顺序）：
//! - HTTP 401/403 → expired「API Key 无效或已过期」；
//! - 信封 statusCode（数值）非 0/200 且 message 含 "api key"/"unauthorized"
//!   → expired，其余 → error（带 message）；
//! - code/status 字符串含 "needlogin"/"login"，或 message 含 "log in"/
//!   "login"/"console session"/"api key mode may be unavailable" →
//!   error「该账号需要网页登录会话，API Key 模式暂不支持此账号」
//!   （ConsoleNeedLogin：该账号只暴露网页会话接口，API Key 模式拿不到）；
//! - 网络失败 / 缺配额数据 → error。
//!
//! 工程纪律（对齐 provider_quota.rs / minimax.rs）：网络 ureq 同步 + 15s
//! 超时 + codex::resolve_proxy，调用方 spawn_blocking；错误消息中文且不含
//! secret；解析纯函数与网络分离，单测不联网。

use crate::cookie_util::parse_time_flexible;
use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, get_any, now_ms, parse_flexible_f64, quota_http_agent, ProviderQuotaEntry,
    ProviderQuotaWindow,
};

// ============================================================
// region → 端点配置（对齐 CodexBar AlibabaCodingPlanAPIRegion）
// ============================================================

/// 单个 region 的端点与参数集合（纯数据，host_for_region 构造）。
struct RegionConf {
    /// 网关主机（含 https 前缀）
    host: &'static str,
    /// currentRegionId 查询参数
    region_id: &'static str,
    /// 请求体 commodityCode
    commodity_code: &'static str,
    /// Referer（控制台 Coding Plan 页，防风控）
    referer: &'static str,
}

/// region → 端点配置（纯函数，便于单测）：global 走国际站 Model Studio，
/// 其余（None/"cn"/未知值）默认中国站百炼。
fn region_conf(region: Option<&str>) -> RegionConf {
    if region == Some("global") {
        RegionConf {
            host: "https://modelstudio.console.alibabacloud.com",
            region_id: "ap-southeast-1",
            commodity_code: "sfm_codingplan_public_intl",
            referer: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan#/efm/coding_plan",
        }
    } else {
        RegionConf {
            host: "https://bailian.console.aliyun.com",
            region_id: "cn-beijing",
            commodity_code: "sfm_codingplan_public_cn",
            referer: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan",
        }
    }
}

/// 额度查询端点路径与固定查询参数（两站一致，仅 currentRegionId 按站区分）。
const QUOTA_PATH: &str = "/data/api.json\
?action=zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2\
&product=broadscope-bailian\
&api=queryCodingPlanInstanceInfoV2";

// ============================================================
// 网络层（ureq 同步；调用方 spawn_blocking）
// ============================================================

/// 逐凭证查询阿里 Coding Plan 额度（串行；单凭证失败产出 error/expired
/// 条目，不阻塞其他凭证）。由 provider_quota 骨架分发调用。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .map(|cred| {
            let conf = region_conf(cred.region.as_deref());
            let raw = fetch_instance_info_raw(&agent, &conf, &cred.secret);
            entry_from_raw(&cred.id, &cred.label, &raw)
        })
        .collect()
}

/// 单次请求（网络层）：POST {host}{QUOTA_PATH}&currentRegionId=...，
/// 请求体 `{"queryCodingPlanInstanceInfoRequest":{"commodityCode":...}}`，
/// 头含三种鉴权形态（Bearer / x-api-key / X-DashScope-API-Key）+ 浏览器
/// 仿真 UA + Origin/Referer（对齐 CodexBar fetchUsageOnce(apiKey:...)）。
/// 返回展平的 (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因）。
fn fetch_instance_info_raw(
    agent: &ureq::Agent,
    conf: &RegionConf,
    key: &str,
) -> Result<(u16, Option<String>), String> {
    let url = format!(
        "{}{}&currentRegionId={}",
        conf.host, QUOTA_PATH, conf.region_id
    );
    let body = serde_json::json!({
        "queryCodingPlanInstanceInfoRequest": {
            "commodityCode": conf.commodity_code,
        }
    })
    .to_string();
    let resp = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("Authorization", &format!("Bearer {key}"))
        .set("x-api-key", key)
        .set("X-DashScope-API-Key", key)
        .set(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
        )
        .set("Origin", conf.host)
        .set("Referer", conf.referer)
        .send_string(&body);
    flatten_response(resp).map_err(|e| format!("阿里 Coding Plan 额度{e}"))
}

// ============================================================
// 解析纯函数（网络无关，单测直接构造输入）
// ============================================================

/// 嵌套 JSON 字符串展开（对齐 CodexBar expandedJSON）：阿里 console 网关有
/// 时把对象序列化成字符串塞在值里（`"data":"{\"codingPlan...\":{...}}"`），
/// 递归遇到「能完整解析为对象/数组的字符串值」就替换为解析结果再展开。
fn expand_json_strings(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                match serde_json::from_str::<serde_json::Value>(trimmed) {
                    Ok(nested @ (serde_json::Value::Object(_)
                    | serde_json::Value::Array(_))) => expand_json_strings(nested),
                    _ => serde_json::Value::String(s),
                }
            } else {
                serde_json::Value::String(s)
            }
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(expand_json_strings).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, val)| (k, expand_json_strings(val)))
                .collect(),
        ),
        other => other,
    }
}

/// 深度优先找第一个「能解析为数值」的目标键值（本层按键序尝试，再递归
/// 对象值与数组元素；对齐 CodexBar findFirstInt 的先本层后递归策略）。
fn find_first_num<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<f64> {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(val) = map.get(*key) {
                    if let Some(n) = parse_flexible_f64(val) {
                        return Some(n);
                    }
                }
            }
            map.values().find_map(|val| find_first_num(val, keys))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|it| find_first_num(it, keys)),
        _ => None,
    }
}

/// 深度优先找第一个非空字符串目标键值（对齐 CodexBar findFirstString）。
fn find_first_str<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(val) = map.get(*key) {
                    if let Some(s) = val.as_str() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed);
                        }
                    }
                }
            }
            map.values().find_map(|val| find_first_str(val, keys))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|it| find_first_str(it, keys)),
        _ => None,
    }
}

/// 深度优先找第一个「数组值且含任一目标键」的值（对齐 CodexBar
/// findFirstArray(forKeys:) 的递归策略，data 包裹形态也能命中）。
fn find_first_arr_with_keys<'a>(
    v: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a Vec<serde_json::Value>> {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(val) = map.get(*key) {
                    if let Some(arr) = val.as_array() {
                        return Some(arr);
                    }
                }
            }
            map.values().find_map(|val| find_first_arr_with_keys(val, keys))
        }
        serde_json::Value::Array(items) => {
            items.iter().find_map(|it| find_first_arr_with_keys(it, keys))
        }
        _ => None,
    }
}

/// 深度优先找第一个「对象值且含任一目标键」的值（先本层按键序尝试，再
/// 递归对象值与数组元素；对齐 CodexBar findFirstDictionary(forKeys:)）。
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

/// 三窗口字段名（quota info 对象内的直接键，camelCase 优先别名回退）；
/// weekly 平台暂无别名，占位重复同键保持结构统一。
const WINDOW_KEYS: [(&str, [&str; 2], [&str; 2], [&str; 2]); 3] = [
    (
        "hour5",
        ["per5HourUsedQuota", "perFiveHourUsedQuota"],
        ["per5HourTotalQuota", "perFiveHourTotalQuota"],
        ["per5HourQuotaNextRefreshTime", "perFiveHourQuotaNextRefreshTime"],
    ),
    (
        "weekly",
        ["perWeekUsedQuota", "perWeekUsedQuota"],
        ["perWeekTotalQuota", "perWeekTotalQuota"],
        ["perWeekQuotaNextRefreshTime", "perWeekQuotaNextRefreshTime"],
    ),
    (
        "monthly",
        ["perBillMonthUsedQuota", "perMonthUsedQuota"],
        ["perBillMonthTotalQuota", "perMonthTotalQuota"],
        ["perBillMonthQuotaNextRefreshTime", "perMonthQuotaNextRefreshTime"],
    ),
];

/// 配额对象的识别键（used/total 任一命中即视为配额对象）。
const QUOTA_MARKER_KEYS: [&str; 8] = [
    "per5HourUsedQuota",
    "perFiveHourUsedQuota",
    "per5HourTotalQuota",
    "perFiveHourTotalQuota",
    "perWeekUsedQuota",
    "perWeekTotalQuota",
    "perBillMonthUsedQuota",
    "perBillMonthTotalQuota",
];

/// 实例是否「有效」信号分（对齐 CodexBar activeSignalScore）：
/// status=VALID/ACTIVE 或 isActive=true → 3；expired 类状态 → -1；
/// endTime/expireTime 等在未来 → 1；其余 0。
fn active_signal_score(info: &serde_json::Value, now: i64) -> i64 {
    if let Some(status) = find_first_str(info, &["status", "instanceStatus"]) {
        let upper = status.to_ascii_uppercase();
        if upper == "VALID" || upper == "ACTIVE" {
            return 3;
        }
        if matches!(
            upper.as_str(),
            "EXPIRED" | "INVALID" | "INACTIVE" | "DISABLED" | "TERMINATED" | "STOPPED"
        ) {
            return -1;
        }
    }
    if let Some(active) = get_any(info, &["isActive", "active"]) {
        match active {
            serde_json::Value::Bool(b) => return if *b { 3 } else { -1 },
            serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "active" | "valid" => return 3,
                "false" | "0" | "no" | "inactive" | "invalid" | "expired" => return -1,
                _ => {}
            },
            _ => {}
        }
    }
    if let Some(t) = get_any(info, &["endTime", "periodEndTime", "expireTime", "expirationTime"])
        .and_then(parse_time_flexible)
    {
        if t > now {
            return 1;
        }
    }
    0
}

/// 多实例选主（纯函数）：信号分最高的实例优先（同分保留先出现者）；全部
/// 无正分时回退第一条（对齐 CodexBar findActiveInstanceInfo）。空数组 None。
fn pick_instance<'a>(
    infos: &'a [serde_json::Value],
    now: i64,
) -> Option<&'a serde_json::Value> {
    let first = infos.first()?;
    let mut best = first;
    let mut best_score = active_signal_score(first, now);
    for info in infos.iter().skip(1) {
        let score = active_signal_score(info, now);
        if score > best_score {
            best = info;
            best_score = score;
        }
    }
    if best_score > 0 {
        Some(best)
    } else {
        Some(first)
    }
}

/// 查找配额信息对象（对齐 CodexBar findQuotaInfo）：先递归找
/// codingPlanQuotaInfo / coding_plan_quota_info 键的对象值；找不到回退
/// 「含任一窗口 used/total 键的对象」。
fn find_quota_info<'a>(v: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    find_first_obj_with_keys(v, &["codingPlanQuotaInfo", "coding_plan_quota_info"])
        .or_else(|| find_first_obj_with_keys(v, &QUOTA_MARKER_KEYS))
}

/// 实例内取 plan 名（纯函数）：planName > instanceName > packageName，
/// 缺失时回退父负载的顶层字段（对齐 CodexBar findPlanName 的候选顺序）。
fn plan_name_of(info: Option<&serde_json::Value>, payload: &serde_json::Value) -> Option<String> {
    if let Some(info) = info {
        for keys in [
            ["planName", "plan_name"],
            ["instanceName", "instance_name"],
            ["packageName", "package_name"],
        ] {
            if let Some(name) = get_any(info, &keys).and_then(|v| v.as_str()) {
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    for keys in [["planName", "plan_name"], ["packageName", "package_name"]] {
        if let Some(name) = get_any(payload, &keys).and_then(|v| v.as_str()) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// 已用百分比（纯函数）：used/total*100，clamp 0-100；total 无效（<=0/缺失）
/// 时 None（前端只展示 used/total 不算百分比）。
fn used_percent(used: Option<f64>, total: Option<f64>) -> Option<f64> {
    let total = total?;
    if total <= 0.0 {
        return None;
    }
    let used = used?;
    Some((used / total * 100.0).clamp(0.0, 100.0))
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败(error) > 信封错误码（needlogin/login → 专属 error 文案；
/// api key/unauthorized → expired；其余 → error）> 缺配额(error) >
/// 成功(ok + 三窗口 + planName)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), String>,
) -> ProviderQuotaEntry {
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
        return fail(
            "error",
            format!("阿里 Coding Plan 额度{}", raw.as_ref().unwrap_err()),
        );
    };
    // Key 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", "API Key 无效或已过期".to_string());
    }
    if *http_status != 200 {
        return fail(
            "error",
            format!("阿里 Coding Plan 额度查询失败（HTTP {http_status}）"),
        );
    }
    let Some(body) = body.as_deref().filter(|b| !b.trim().is_empty()) else {
        return fail("error", "阿里 Coding Plan 额度响应为空".to_string());
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "阿里 Coding Plan 额度响应解析失败".to_string());
    };
    let v = expand_json_strings(parsed);

    // —— 信封错误码（对齐 CodexBar 判定顺序：数值 statusCode → 字符串
    //    code/status → message 关键词）——
    if let Some(status_code) = find_first_num(&v, &["statusCode", "status_code", "code"]) {
        if status_code != 0.0 && status_code != 200.0 {
            let message = find_first_str(&v, &["statusMessage", "status_msg", "message", "msg"])
                .unwrap_or("unknown error");
            let lower = message.to_ascii_lowercase();
            if lower.contains("api key") || lower.contains("unauthorized") {
                return fail("expired", "API Key 无效或已过期".to_string());
            }
            return fail(
                "error",
                format!("阿里云平台返回错误: {message}（code {status_code}）"),
            );
        }
    }
    // 需要网页登录会话（如 ConsoleNeedLogin）：API Key 模式拿不到该账号数据
    const NEED_LOGIN_MSG: &str = "该账号需要网页登录会话，API Key 模式暂不支持此账号";
    if let Some(code_text) = find_first_str(&v, &["code", "status", "statusCode"]) {
        let lower = code_text.to_ascii_lowercase();
        if lower.contains("needlogin") || lower.contains("login") {
            return fail("error", NEED_LOGIN_MSG.to_string());
        }
    }
    if let Some(message_text) = find_first_str(&v, &["message", "msg", "statusMessage"]) {
        let lower = message_text.to_ascii_lowercase();
        if lower.contains("log in") || lower.contains("login") {
            return fail("error", NEED_LOGIN_MSG.to_string());
        }
        if lower.contains("console session") || lower.contains("api key mode may be unavailable") {
            return fail("error", NEED_LOGIN_MSG.to_string());
        }
    }

    // —— 实例与配额提取 ——
    let now = now_ms();
    let instances = find_first_arr_with_keys(
        &v,
        &["codingPlanInstanceInfos", "coding_plan_instance_infos"],
    )
    .cloned()
    .unwrap_or_default();
    let instance = pick_instance(&instances, now);
    // 配额优先取选中实例内；实例外/顶层也兜底（多实例时 CodexBar 会把配额
    // 限定到选中实例，单实例/顶层结构则直接全局找）
    let quota = instance
        .and_then(find_quota_info)
        .or_else(|| find_quota_info(&v));
    let Some(quota) = quota else {
        return fail("error", "阿里 Coding Plan 响应缺少配额数据".to_string());
    };
    let plan_name = plan_name_of(instance, &v);

    // 三窗口：used/total 任一有值才产出该窗口（unit 统一「次」，CodexBar
    // 未暴露单位，按配额计数口径展示）
    let mut windows = Vec::new();
    for (key, used_keys, total_keys, reset_keys) in WINDOW_KEYS {
        let used = get_any(quota, &used_keys).and_then(parse_flexible_f64);
        let total = get_any(quota, &total_keys).and_then(parse_flexible_f64);
        let resets_at = get_any(quota, &reset_keys).and_then(parse_time_flexible);
        if used.is_none() && total.is_none() {
            continue;
        }
        windows.push(ProviderQuotaWindow {
            key: key.to_string(),
            title: match key {
                "hour5" => "5h",
                "weekly" => "本周",
                _ => "本月",
            }
            .to_string(),
            used_percent: used_percent(used, total),
            used,
            total,
            unit: Some("次".to_string()),
            resets_at,
        });
    }
    if windows.is_empty() {
        return fail("error", "阿里 Coding Plan 响应缺少配额数据".to_string());
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

// ============================================================
// 单元测试（纯函数，不联网）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "ali-1";
    const LABEL: &str = "通义灵码";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 成功样例：data 为 JSON 字符串形态（阿里网关常见），实例内含配额。
    const SAMPLE_BODY: &str = r#"{
        "requestId": "req-1",
        "success": true,
        "data": "{\"codingPlanInstanceInfos\":[{\"planName\":\"Coding Plan Lite\",\"instanceName\":\"实例A\",\"packageName\":\"qwen-coder-lite\",\"status\":\"VALID\",\"codingPlanQuotaInfo\":{\"per5HourUsedQuota\":30,\"per5HourTotalQuota\":100,\"per5HourQuotaNextRefreshTime\":1730018000,\"perWeekUsedQuota\":120,\"perWeekTotalQuota\":500,\"perWeekQuotaNextRefreshTime\":\"2030-10-27 05:06:07\",\"perBillMonthUsedQuota\":300,\"perBillMonthTotalQuota\":1000,\"perBillMonthQuotaNextRefreshTime\":\"2030-10-01T00:00:00Z\"}}]}"
    }"#;

    /// 成功路径（JSON 字符串信封展开）：ok + planName 优先 planName 字段 +
    /// 三窗口（usedPercent = used/total*100）+ 时间字段三种形态自适应。
    #[test]
    fn parses_sample_three_windows_with_string_envelope() {
        let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(SAMPLE_BODY));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert_eq!(entry.plan_name.as_deref(), Some("Coding Plan Lite"));
        assert_eq!(entry.windows.len(), 3);

        // 5h 窗：epoch 秒 → 毫秒
        let hour5 = &entry.windows[0];
        assert_eq!(hour5.key, "hour5");
        assert_eq!(hour5.title, "5h");
        assert_eq!(hour5.used, Some(30.0));
        assert_eq!(hour5.total, Some(100.0));
        assert_eq!(hour5.used_percent, Some(30.0));
        assert_eq!(hour5.unit.as_deref(), Some("次"));
        assert_eq!(hour5.resets_at, Some(1_730_018_000_000));

        // 周窗：`yyyy-MM-dd HH:mm:ss`（无时区按 UTC）
        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.title, "本周");
        assert_eq!(weekly.used_percent, Some(24.0));
        assert_eq!(weekly.resets_at, Some(1_919_307_967_000));

        // 月窗：ISO/RFC3339（2030-10-01T00:00:00Z）
        let monthly = &entry.windows[2];
        assert_eq!(monthly.key, "monthly");
        assert_eq!(monthly.title, "本月");
        assert_eq!(monthly.used_percent, Some(30.0));
        assert_eq!(monthly.resets_at, Some(1_917_043_200_000));
    }

    /// 顶层（无 data 包裹、无 JSON 字符串）形态同样可解析（信封弹性）。
    #[test]
    fn parses_plain_top_level_envelope() {
        let raw = ok_raw(
            r#"{"code":200,"data":{"codingPlanInstanceInfos":[
               {"instanceName":"备用实例名","packageName":"pkg-pro","status":"ACTIVE",
                "codingPlanQuotaInfo":{"per5HourUsedQuota":10,"per5HourTotalQuota":200}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        // planName 缺失 → instanceName 优先于 packageName
        assert_eq!(entry.plan_name.as_deref(), Some("备用实例名"));
        assert_eq!(entry.windows.len(), 1);
        assert_eq!(entry.windows[0].used_percent, Some(5.0));
        assert_eq!(entry.windows[0].resets_at, None);
    }

    /// planName 优先级：planName > instanceName > packageName。
    #[test]
    fn plan_name_prefers_plan_name_field() {
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[
               {"planName":"Plan-Top","instanceName":"Inst-Mid","packageName":"Pkg-Low",
                "codingPlanQuotaInfo":{"perWeekUsedQuota":1,"perWeekTotalQuota":10}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.plan_name.as_deref(), Some("Plan-Top"));

        // 缺 planName → instanceName
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[
               {"instanceName":"Inst-Mid","packageName":"Pkg-Low",
                "codingPlanQuotaInfo":{"perWeekUsedQuota":1,"perWeekTotalQuota":10}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.plan_name.as_deref(), Some("Inst-Mid"));

        // 只剩 packageName
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[
               {"packageName":"Pkg-Low",
                "codingPlanQuotaInfo":{"perWeekUsedQuota":1,"perWeekTotalQuota":10}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.plan_name.as_deref(), Some("Pkg-Low"));
    }

    /// ConsoleNeedLogin（code/status 字符串形态）→ error + 专属文案
    /// （API Key 模式拿不到网页会话账号的数据）。
    #[test]
    fn console_need_login_maps_to_error() {
        for (code_key, code_val) in [
            ("code", serde_json::json!("ConsoleNeedLogin")),
            ("status", serde_json::json!("-1|NEED_LOGIN")),
        ] {
            let body = serde_json::json!({ code_key: code_val, "data": null }).to_string();
            let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(&body));
            assert_eq!(entry.status, "error", "{code_key}={code_val} 应判定 error");
            assert!(entry.windows.is_empty());
            assert_eq!(
                entry.message.as_deref(),
                Some("该账号需要网页登录会话，API Key 模式暂不支持此账号")
            );
        }
        // message 关键词形态
        let body = serde_json::json!({"message": "please log in to console"}).to_string();
        let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(&body));
        assert_eq!(entry.status, "error");
        assert_eq!(
            entry.message.as_deref(),
            Some("该账号需要网页登录会话，API Key 模式暂不支持此账号")
        );
    }

    /// HTTP 401/403 → expired「API Key 无效或已过期」（假 Key 手测链路的
    /// 预期分支）；信封 message 含 "api key" 同判 expired。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some(r#"{"message":"forbidden"}"#.to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(entry.message.as_deref(), Some("API Key 无效或已过期"));
            assert!(entry.windows.is_empty());
        }
        // 信封数值 statusCode + "api key" 关键词
        let raw = ok_raw(r#"{"statusCode":401,"message":"Invalid API Key"}"#);
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "expired");
        assert_eq!(entry.message.as_deref(), Some("API Key 无效或已过期"));
    }

    /// 多实例：取第一个有效实例（status=VALID 的配额），expired 实例沉底；
    /// 全部无效时回退第一条。
    #[test]
    fn multiple_instances_picks_active_one() {
        // VALID 实例在后 → 仍应选中它（分数优先于出现顺序）
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[
               {"planName":"Expired-Plan","status":"EXPIRED",
                "codingPlanQuotaInfo":{"per5HourUsedQuota":99,"per5HourTotalQuota":100}},
               {"planName":"Active-Plan","status":"VALID","isActive":true,
                "codingPlanQuotaInfo":{"per5HourUsedQuota":20,"per5HourTotalQuota":100,
                 "perWeekUsedQuota":5,"perWeekTotalQuota":10}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.plan_name.as_deref(), Some("Active-Plan"));
        assert_eq!(entry.windows[0].used_percent, Some(20.0));

        // 全部无效 → 回退第一条
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[
               {"planName":"First","status":"EXPIRED",
                "codingPlanQuotaInfo":{"per5HourUsedQuota":9,"per5HourTotalQuota":10}},
               {"planName":"Second","status":"STOPPED",
                "codingPlanQuotaInfo":{"per5HourUsedQuota":1,"per5HourTotalQuota":10}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.plan_name.as_deref(), Some("First"));
        assert_eq!(entry.windows[0].used_percent, Some(90.0));
    }

    /// 时间字段自适应：epoch 毫秒原样、秒 ×1000、ISO、日期串、脏值无重置。
    #[test]
    fn time_fields_parse_flexible_shapes() {
        // 毫秒原样
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[{"status":"VALID",
               "codingPlanQuotaInfo":{"per5HourUsedQuota":1,"per5HourTotalQuota":2,
               "per5HourQuotaNextRefreshTime":1730018000000}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.windows[0].resets_at, Some(1_730_018_000_000));

        // 字符串数字（秒）
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[{"status":"VALID",
               "codingPlanQuotaInfo":{"per5HourUsedQuota":1,"per5HourTotalQuota":2,
               "per5HourQuotaNextRefreshTime":"1730018000"}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.windows[0].resets_at, Some(1_730_018_000_000));

        // 脏值 → resets_at 缺省，窗口照常产出
        let raw = ok_raw(
            r#"{"data":{"codingPlanInstanceInfos":[{"status":"VALID",
               "codingPlanQuotaInfo":{"per5HourUsedQuota":1,"per5HourTotalQuota":2,
               "per5HourQuotaNextRefreshTime":"n/a"}}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows[0].resets_at, None);
    }

    /// 网络失败 → error 原因透传；非 200 非 401/403 → error 带状态码；
    /// 空 body / 坏 JSON → error；数值 statusCode 非 0/200 且无关键词 → error。
    #[test]
    fn failures_map_to_error() {
        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));

        let raw = Ok((500, Some("internal".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));

        for body in ["", "not json"] {
            let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(body));
            assert_eq!(entry.status, "error", "body={body} 应判定 error");
        }

        let raw = ok_raw(r#"{"statusCode":1001,"message":"internal error"}"#);
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        let message = entry.message.expect("error 条目必须有原因");
        assert!(message.contains("internal error"), "消息应带 message: {message}");
        assert!(!message.contains("sk-"), "错误消息不得含 secret 片段");
    }

    /// 配额数据缺失：无实例、实例无配额、三窗口键全缺 → error。
    #[test]
    fn missing_quota_maps_to_error() {
        // 无实例数组且顶层无配额
        let entry = entry_from_raw(CRED_ID, LABEL, &ok_raw(r#"{"data":{}}"#));
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("缺少配额数据"));

        // 实例存在但无任何窗口字段
        let entry = entry_from_raw(
            CRED_ID,
            LABEL,
            &ok_raw(r#"{"data":{"codingPlanInstanceInfos":[{"planName":"P","status":"VALID"}]}}"#),
        );
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("缺少配额数据"));
    }

    /// region → 端点配置分流：global 国际站，其余默认中国站。
    #[test]
    fn region_maps_to_conf() {
        let intl = region_conf(Some("global"));
        assert_eq!(intl.host, "https://modelstudio.console.alibabacloud.com");
        assert_eq!(intl.region_id, "ap-southeast-1");
        assert_eq!(intl.commodity_code, "sfm_codingplan_public_intl");

        for region in [None, Some("cn"), Some("weird")] {
            let cn = region_conf(region);
            assert_eq!(cn.host, "https://bailian.console.aliyun.com");
            assert_eq!(cn.region_id, "cn-beijing");
            assert_eq!(cn.commodity_code, "sfm_codingplan_public_cn");
        }
    }

    /// usedPercent 边界：total<=0 / 缺失 → None；超限 clamp 100。
    #[test]
    fn used_percent_clamps_and_guards() {
        assert_eq!(used_percent(Some(50.0), Some(100.0)), Some(50.0));
        assert_eq!(used_percent(Some(150.0), Some(100.0)), Some(100.0));
        assert_eq!(used_percent(Some(1.0), Some(0.0)), None);
        assert_eq!(used_percent(Some(1.0), None), None);
        assert_eq!(used_percent(None, Some(100.0)), None);
    }
}

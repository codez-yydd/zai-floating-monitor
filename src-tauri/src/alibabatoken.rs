//! 阿里 Token Plan（百炼 Token 包）额度查询模块。
//!
//! 凭证型：kind=cookie 的 secret 是用户从百炼控制台订阅页请求复制的 Cookie
//! 请求头（或整段 Copy as cURL 粘贴，由 cookie_util::normalize_cookie_secret
//! 归一）。端点、请求构造与错误分类对齐 CodexBar AlibabaTokenPlan*：
//!
//! 端点（阿里 OneConsole 网关 `/data/api.json`，POST form-urlencoded）：
//! - Team 形态（企业订阅）：action=GetSubscriptionSummary&product=BssOpenAPI-V3，
//!   网关主机即控制台主机；请求体 `params={"ProductCode":"sfm_tokenplanteams_dp_cn|intl"}`
//!   + region；响应为订阅摘要（TotalCount / usedQuota / totalQuota /
//!   remainingQuota / 重置时间），取积分池 used/total/remaining 作单窗口。
//! - Personal/Solo 形态（个人订阅）：action=BroadScopeAspnGateway（国际
//!   IntlBroadScopeAspnGateway）&product=sfm_bailian&api=zeldaHttp.apikeyMgr.
//!   /tokenplan/personal/api/v2/{usage,subscription,quota-config}，主机按站
//!   分流（中国 bailian-cs.console.aliyun.com / 国际
//!   bailian-singapore-cs.alibabacloud.com）；usage 给 5 小时/7 天滚动窗口
//!   百分比与重置时间，subscription 给套餐 specCode，quota-config 按套餐码
//!   给 five_hour/weekly 总量。
//!
//! 形态探测：先查 Team 订阅摘要——命中订阅（TotalCount>0 或有配额数值）时
//! 展示积分池单窗口；TotalCount==0 或 Team 查询失败时降级为 Personal/Solo
//! 的 usage 双窗口（判定失败的形态仅 usage 窗口）。
//!
//! sec_token（Personal 网关部分账号必需，缺失时报
//! `BailianGateway.Workspace.NotAuthorised`）：按 CodexBar 顺序尽力解析——
//! 控制台 HTML 的 SEC_TOKEN → `{gateway}/tool/user/info.json` → Cookie 里的
//! sec_token；解析不到也照常发起请求（对齐源码 cookie-only 降级）。
//!
//! 错误映射（对齐 CodexBar throwIfErrorPayload 判定顺序）：HTTP 401/403、
//! 登录页 HTML、信封 login/token 标识 → expired「会话已失效…」；
//! `BailianGateway.Workspace.NotAuthorised` → error「缺少控制台 sec_token…」
//! （工作区权限问题不淘汰会话）；Team 成功但 TotalCount==0 且 Personal 无
//! 窗口 → error「未找到有效的百炼 Token 包订阅」。
//!
//! 工程纪律（对齐 qoder.rs / mimo.rs / alibaba.rs）：网络 ureq 同步 + 15s
//! 超时 + resolve_proxy，调用方 spawn_blocking；解析纯函数可单测；错误消息
//! 中文且不含 secret；Cookie 值不进任何日志。

use crate::cookie_util::{chrome_like_headers, normalize_cookie_secret, parse_time_flexible};
use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, get_any, now_ms, parse_flexible_f64, quota_http_agent,
    quota_http_agent_timeout, ProviderQuotaEntry, ProviderQuotaWindow,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ============================================================
// 端点常量（对齐 CodexBar AlibabaTokenPlanUsageFetcher）
// ============================================================

/// BSS 订阅摘要服务码与 action（Team 形态）。
const BSS_SERVICE_CODE: &str = "BssOpenAPI-V3";
const SUBSCRIPTION_SUMMARY_ACTION: &str = "GetSubscriptionSummary";
/// Personal 网关的 product 与三个 api 名。
const PERSONAL_CONSOLE_PRODUCT: &str = "sfm_bailian";
const PERSONAL_USAGE_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";
const PERSONAL_SUBSCRIPTION_API: &str =
    "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/subscription";
const PERSONAL_QUOTA_CONFIG_API: &str =
    "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/quota-config";
/// usage 网关偶发 200 成功信封但无滚动窗口字段（服务端瞬态），立即重试通常
/// 可得（对齐源码 personalUsageMaxAttempts / 400ms 间隔）。
const USAGE_MAX_ATTEMPTS: usize = 3;
const USAGE_RETRY_DELAY_MS: u64 = 400;

/// sec_token 探测用的 Safari UA（对齐源码 safariLikeUserAgent；OneConsole
/// shell 仅对真实浏览器文档导航渲染 SEC_TOKEN）。
const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.3 Safari/605.1.15";

/// sec_token 成功缓存 TTL（10 分钟）：探测是 HTML 探测 + user/info.json
///（最多 6+ 请求），120s 轮询下每轮重复探测纯属浪费；sec_token 短期内不变。
const SEC_TOKEN_CACHE_TTL: Duration = Duration::from_secs(600);
/// sec_token 探测失败缓存 TTL（60s）：避免连续轮询反复打探测端点，同时比
/// 成功值更快过期，cookie 换新后能尽快重试恢复。
const SEC_TOKEN_MISS_TTL: Duration = Duration::from_secs(60);

/// sec_token 进程内缓存：credential_id → (解析结果 None=近期探测失败,
/// 写入时间)。key 用 credential_id 而非 cookie 内容，避免多凭证互串；
/// 缓存读写均短持锁，网络请求在锁外（对齐 claude.rs 的 OnceLock 缓存先例）。
static SEC_TOKEN_CACHE: OnceLock<Mutex<HashMap<String, (Option<String>, Instant)>>> =
    OnceLock::new();

fn sec_token_cache() -> &'static Mutex<HashMap<String, (Option<String>, Instant)>> {
    SEC_TOKEN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

// ============================================================
// region → 端点配置（对齐 CodexBar AlibabaTokenPlanAPIRegion）
// ============================================================

/// 单个 region 的端点与参数集合（纯数据，region_conf 构造）。
struct RegionConf {
    /// 控制台/网关主机（Team 端点与 Origin 用）
    gateway: &'static str,
    /// Personal 网关主机（usage/subscription/quota-config 用）
    quota_host: &'static str,
    /// region 参数（ap-southeast-1 / cn-beijing）
    region_id: &'static str,
    /// Personal 网关 action
    gateway_action: &'static str,
    /// cornerstoneParam.consoleSite
    console_site: &'static str,
    /// Team 订阅摘要的 ProductCode
    team_product: &'static str,
    /// Personal subscription 的 commodityCode
    solo_product: &'static str,
    /// Team 订阅页（Referer/feURL）
    team_dashboard: &'static str,
    /// Personal/Solo 订阅页（Referer/feURL）
    personal_dashboard: &'static str,
}

/// region → 端点配置（纯函数，便于单测）：global 走国际站，其余
///（None/"cn"/未知值）默认中国站。
fn region_conf(region: Option<&str>) -> RegionConf {
    if region == Some("global") {
        RegionConf {
            gateway: "https://modelstudio.console.alibabacloud.com",
            quota_host: "https://bailian-singapore-cs.alibabacloud.com",
            region_id: "ap-southeast-1",
            gateway_action: "IntlBroadScopeAspnGateway",
            console_site: "MODELSTUDIO_ALBABACLOUD",
            team_product: "sfm_tokenplanteams_dp_intl",
            solo_product: "sfm_tokenplansolo_public_intl",
            team_dashboard: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=plan#/efm/subscription/token-plan",
            personal_dashboard: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=plan#/efm/subscription/token-plan/personal",
        }
    } else {
        RegionConf {
            gateway: "https://bailian.console.aliyun.com",
            quota_host: "https://bailian-cs.console.aliyun.com",
            region_id: "cn-beijing",
            gateway_action: "BroadScopeAspnGateway",
            console_site: "BAILIAN_ALIYUN",
            team_product: "sfm_tokenplanteams_dp_cn",
            solo_product: "sfm_tokenplansolo_public_cn",
            team_dashboard: "https://bailian.console.aliyun.com/cn-beijing?tab=plan#/efm/subscription/token-plan",
            personal_dashboard: "https://bailian.console.aliyun.com/cn-beijing?tab=plan#/efm/subscription/token-plan/personal",
        }
    }
}

// ============================================================
// 网络层（ureq 同步；调用方 spawn_blocking）
// ============================================================

/// 逐凭证查询阿里 Token Plan（串行；单凭证失败产出 error/expired 条目，
/// 不阻塞其他凭证）。只消费 kind=cookie 的凭证，由 provider_quota 骨架分发。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    // sec_token 探测用更短超时（源码 10s；探测失败不阻塞主查询）
    let probe_agent = quota_http_agent_timeout(10);
    snapshots
        .iter()
        .filter(|cred| cred.kind == "cookie")
        .map(|cred| {
            // Cookie 内容支持裸串 / 整段 cURL 粘贴，查询时归一（保存原样）
            let cookie = normalize_cookie_secret(&cred.secret);
            if cookie.is_empty() {
                return entry_error(
                    &cred.id,
                    &cred.label,
                    "未能从粘贴内容中解析出 Cookie，请重新复制请求头或 cURL 命令",
                );
            }
            let conf = region_conf(cred.region.as_deref());
            // CSRF：OneConsole 写操作要求把 cookie 里的 csrf 值回填双头
            let csrf = cookie_value(&cookie, "login_aliyunid_csrf")
                .or_else(|| cookie_value(&cookie, "csrf"))
                .unwrap_or("")
                .to_string();
            let sec_token = fetch_sec_token_cached(&probe_agent, &conf, &cookie, &cred.id);
            let bundle = fetch_bundle(
                &agent,
                &conf,
                &cookie,
                &csrf,
                sec_token.as_deref(),
            );
            entry_from_bundle(&cred.id, &cred.label, &bundle)
        })
        .collect()
}

/// 单凭证的抓取结果集合（team 必查；usage 仅在 team 未命中订阅时抓取，
/// subscription/quota-config 为 Personal 流程的可选补充）。
struct QuotaBundle {
    team: Result<(u16, Option<String>), String>,
    usage: Option<Result<(u16, Option<String>), String>>,
    subscription: Option<(u16, Option<String>)>,
    quota_config: Option<(u16, Option<String>)>,
}

/// 抓取单凭证全部端点：先 Team 订阅摘要；命中订阅即止（Team 形态只需积分
/// 池），否则补抓 Personal 三端点（subscription/quota-config 失败静默）。
fn fetch_bundle(
    agent: &ureq::Agent,
    conf: &RegionConf,
    cookie: &str,
    csrf: &str,
    sec_token: Option<&str>,
) -> QuotaBundle {
    let team = fetch_team_summary_raw(agent, conf, cookie, csrf, sec_token);
    let team_hit = parse_raw(&team)
        .ok()
        .and_then(|v| parse_team_summary(&v))
        .map(|t| team_has_subscription(&t))
        .unwrap_or(false);
    if team_hit {
        return QuotaBundle {
            team,
            usage: None,
            subscription: None,
            quota_config: None,
        };
    }
    let subscription = fetch_personal_api_raw(
        agent,
        conf,
        cookie,
        csrf,
        sec_token,
        PERSONAL_SUBSCRIPTION_API,
        &[("commodityCode", conf.solo_product)],
    )
    .ok();
    let quota_config =
        fetch_personal_api_raw(agent, conf, cookie, csrf, sec_token, PERSONAL_QUOTA_CONFIG_API, &[])
            .ok();
    let usage = fetch_personal_usage_raw(agent, conf, cookie, csrf, sec_token);
    QuotaBundle {
        team,
        usage: Some(usage),
        subscription,
        quota_config,
    }
}

/// POST form-urlencoded 到 OneConsole 网关（公共头组装）：浏览器仿真头
///（chrome_like_headers：Cookie / Chrome UA / Origin / Referer 等）+ 双
/// CSRF 头 + XHR 标记。返回展平的 (HTTP 状态码, 响应体)。
fn post_form(
    agent: &ureq::Agent,
    url: &str,
    cookie: &str,
    csrf: &str,
    accept: &str,
    origin: &str,
    referer: &str,
    body: &str,
) -> Result<(u16, Option<String>), String> {
    let mut req = agent.post(url);
    for (name, value) in chrome_like_headers(cookie, origin, referer) {
        req = req.set(&name, &value);
    }
    if !csrf.is_empty() {
        req = req.set("x-xsrf-token", csrf).set("x-csrf-token", csrf);
    }
    req = req
        .set("Accept", accept)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("X-Requested-With", "XMLHttpRequest");
    flatten_response(req.send_string(body)).map_err(|e| format!("阿里 Token Plan 额度{e}"))
}

/// 追加 sec_token 到 form 体（可选参数，空值不追加）。
fn append_sec_token(body: &mut String, sec_token: Option<&str>) {
    if let Some(t) = sec_token.map(str::trim).filter(|t| !t.is_empty()) {
        body.push_str("&sec_token=");
        body.push_str(&form_urlencode(t));
    }
}

/// Team 订阅摘要（必需端点）：POST {gateway}/data/api.json?action=
/// GetSubscriptionSummary&product=BssOpenAPI-V3&_tag=。
fn fetch_team_summary_raw(
    agent: &ureq::Agent,
    conf: &RegionConf,
    cookie: &str,
    csrf: &str,
    sec_token: Option<&str>,
) -> Result<(u16, Option<String>), String> {
    let url = format!(
        "{}/data/api.json?action={}&product={}&_tag=",
        conf.gateway, SUBSCRIPTION_SUMMARY_ACTION, BSS_SERVICE_CODE
    );
    let params = serde_json::json!({ "ProductCode": conf.team_product }).to_string();
    let mut body = format!(
        "product={}&action={}&params={}&region={}",
        BSS_SERVICE_CODE,
        SUBSCRIPTION_SUMMARY_ACTION,
        form_urlencode(&params),
        conf.region_id
    );
    append_sec_token(&mut body, sec_token);
    post_form(agent, &url, cookie, csrf, "*/*", conf.gateway, conf.team_dashboard, &body)
}

/// Personal 网关单端点：POST {quota_host}/data/api.json?action=<action>&
/// product=sfm_bailian&api=<api>&_v=undefined，体含 cornerstoneParam
///（对齐源码 personalAPIRequestBody；不带 switchAgent，避免绑定他人工作区
/// 触发 NotAuthorised）。
fn fetch_personal_api_raw(
    agent: &ureq::Agent,
    conf: &RegionConf,
    cookie: &str,
    csrf: &str,
    sec_token: Option<&str>,
    api: &str,
    data_params: &[(&str, &str)],
) -> Result<(u16, Option<String>), String> {
    let url = format!(
        "{}/data/api.json?action={}&product={}&api={}&_v=undefined",
        conf.quota_host, conf.gateway_action, PERSONAL_CONSOLE_PRODUCT, api
    );
    let mut data = serde_json::Map::new();
    for (key, value) in data_params {
        data.insert((*key).to_string(), serde_json::json!(value));
    }
    let mut cornerstone = serde_json::Map::new();
    cornerstone.insert("feTraceId".into(), serde_json::json!(pseudo_uuid()));
    cornerstone.insert("feURL".into(), serde_json::json!(conf.personal_dashboard));
    cornerstone.insert("protocol".into(), serde_json::json!("V2"));
    cornerstone.insert("console".into(), serde_json::json!("ONE_CONSOLE"));
    cornerstone.insert("productCode".into(), serde_json::json!("p_efm"));
    cornerstone.insert("switchUserType".into(), serde_json::json!(3));
    cornerstone.insert(
        "domain".into(),
        serde_json::json!(conf.gateway.trim_start_matches("https://")),
    );
    cornerstone.insert("consoleSite".into(), serde_json::json!(conf.console_site));
    cornerstone.insert("userNickName".into(), serde_json::json!(""));
    cornerstone.insert("userPrincipalName".into(), serde_json::json!(""));
    cornerstone.insert("xsp_lang".into(), serde_json::json!("en-US"));
    if let Some(cna) = cookie_value(cookie, "cna").filter(|c| !c.is_empty()) {
        cornerstone.insert("X-Anonymous-Id".into(), serde_json::json!(cna));
    }
    data.insert("cornerstoneParam".into(), serde_json::Value::Object(cornerstone));
    let params = serde_json::json!({
        "Api": api,
        "V": "1.0",
        "Data": serde_json::Value::Object(data),
    })
    .to_string();
    let mut body = format!(
        "product={}&action={}&region={}&language=en-US&params={}",
        PERSONAL_CONSOLE_PRODUCT,
        conf.gateway_action,
        conf.region_id,
        form_urlencode(&params)
    );
    append_sec_token(&mut body, sec_token);
    post_form(
        agent,
        &url,
        cookie,
        csrf,
        "application/json, text/plain, */*",
        conf.gateway,
        conf.personal_dashboard,
        &body,
    )
}

/// Personal usage（必需端点，带瞬态空窗重试）：成功信封但无滚动窗口字段时
/// 间隔 400ms 重试，至多 3 次（对齐源码 personalUsageMaxAttempts）。
fn fetch_personal_usage_raw(
    agent: &ureq::Agent,
    conf: &RegionConf,
    cookie: &str,
    csrf: &str,
    sec_token: Option<&str>,
) -> Result<(u16, Option<String>), String> {
    let mut last: Option<Result<(u16, Option<String>), String>> = None;
    for attempt in 0..USAGE_MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(USAGE_RETRY_DELAY_MS));
        }
        let raw = fetch_personal_api_raw(agent, conf, cookie, csrf, sec_token, PERSONAL_USAGE_API, &[]);
        let retry = matches!(&raw, Ok((200, Some(body))) if success_payload_without_windows(body));
        last = Some(raw);
        if !retry {
            break;
        }
    }
    last.unwrap_or_else(|| Err("阿里 Token Plan 额度个人用量查询未执行".to_string()))
}

/// 成功信封但缺滚动窗口字段的判定（usage 重试条件；错误信封/解析失败不算）。
fn success_payload_without_windows(body: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
        return false;
    };
    let v = expand_json_strings(parsed);
    if classify_payload(&v).is_err() {
        return false;
    }
    find_first_obj_with_any_key(&v, &["per5HourPercentage", "per1WeekPercentage"]).is_none()
}

/// sec_token 尽力解析（带进程内缓存）：命中成功值（10 分钟 TTL）或失败记录
///（60s TTL）时直接返回，不再走 HTML 探测 + user/info.json（每凭证最多
/// 6+ 请求，120s 轮询下重复探测浪费且拖慢整轮）；未命中才调 fetch_sec_token
/// 并回写缓存。锁内只做缓存读写，网络请求全部在锁外；不打印 token 内容。
fn fetch_sec_token_cached(
    probe_agent: &ureq::Agent,
    conf: &RegionConf,
    cookie: &str,
    credential_id: &str,
) -> Option<String> {
    if let Ok(cache) = sec_token_cache().lock() {
        if let Some((token, at)) = cache.get(credential_id) {
            let ttl = if token.is_some() {
                SEC_TOKEN_CACHE_TTL
            } else {
                SEC_TOKEN_MISS_TTL
            };
            if at.elapsed() < ttl {
                return token.clone();
            }
        }
    }
    let token = fetch_sec_token(probe_agent, conf, cookie);
    if let Ok(mut cache) = sec_token_cache().lock() {
        // 顺手清掉过期槽位，凭证删除后不留残余
        cache.retain(|_, (_, at)| at.elapsed() < SEC_TOKEN_CACHE_TTL);
        cache.insert(credential_id.to_string(), (token.clone(), Instant::now()));
    }
    token
}

/// sec_token 尽力解析（对齐源码 resolveSECToken 顺序）：控制台 HTML 的
/// SEC_TOKEN → tool/user/info.json → Cookie 里的 sec_token；全部失败返回
/// None（调用方照常发起 cookie-only 请求）。
fn fetch_sec_token(probe_agent: &ureq::Agent, conf: &RegionConf, cookie: &str) -> Option<String> {
    // 1) 控制台订阅页 HTML：OneConsole shell 仅对同源文档导航渲染
    //    window.ALIYUN_CONSOLE_CONFIG.SEC_TOKEN，需带完整导航头
    let html_req = probe_agent
        .get(conf.personal_dashboard)
        .set("Cookie", cookie)
        .set("User-Agent", SAFARI_UA)
        .set("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .set("Referer", &format!("{}/", conf.gateway))
        .set("Sec-Fetch-Site", "same-origin")
        .set("Sec-Fetch-Mode", "navigate")
        .set("Sec-Fetch-Dest", "document")
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8");
    if let Ok((200, Some(html))) = flatten_response(html_req.call()) {
        if let Some(token) = extract_sec_token_from_html(&html) {
            return Some(token);
        }
    }
    // 2) 用户信息接口（JSON 内 secToken/sec_token 深度查找）
    let info_req = probe_agent
        .get(&format!("{}/tool/user/info.json", conf.gateway))
        .set("Cookie", cookie)
        .set("User-Agent", SAFARI_UA)
        .set("Accept", "application/json, text/plain, */*")
        .set("Referer", &format!("{}/", conf.gateway));
    if let Ok((200, Some(body))) = flatten_response(info_req.call()) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
            let v = expand_json_strings(parsed);
            if let Some(token) = find_first_str(&v, &["secToken", "sec_token"]) {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    // 3) Cookie 里的 sec_token
    cookie_value(cookie, "sec_token")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

// ============================================================
// 响应解析纯函数（网络无关，单测直接构造输入）
// ============================================================

/// 嵌套 JSON 字符串展开（对齐 CodexBar expandEmbeddedJSON / alibaba.rs 先例）：
/// 阿里网关有时把对象序列化成字符串塞在值里，递归展开。
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

/// 弹性数值解析（数字 / 字符串数字，容忍千分位逗号，如 "1,200"）。
fn num_field(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::String(s) => s.trim().replace(',', "").parse::<f64>().ok(),
        _ => parse_flexible_f64(v),
    }
}

/// 深度优先找第一个能解析为数值的目标键值（先本层按键序，再递归对象值与
/// 数组元素；对齐 CodexBar findFirstInt/Double 策略与 alibaba.rs 先例）。
fn find_first_num(v: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(val) = map.get(*key) {
                    if let Some(n) = num_field(val) {
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

/// 深度优先找第一个非空字符串目标键值。
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

/// 深度优先找第一个「对象值且含任一目标键」的对象（data 包裹形态也能命中）。
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

/// 深度优先找第一个「自身含任一目标键」的对象（对象本身命中即返回）。
fn find_first_obj_with_any_key<'a>(
    v: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            if keys.iter().any(|k| map.get(*k).map_or(false, |v| !v.is_null())) {
                return Some(v);
            }
            map.values().find_map(|val| find_first_obj_with_any_key(val, keys))
        }
        serde_json::Value::Array(items) => {
            items.iter().find_map(|it| find_first_obj_with_any_key(it, keys))
        }
        _ => None,
    }
}

/// 深度优先找第一个目标键的非空值（任意类型；quota-config 按套餐码取值用）。
fn find_first_value_for_any_key<'a>(
    v: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(val) = map.get(*key) {
                    if !val.is_null() {
                        return Some(val);
                    }
                }
            }
            map.values().find_map(|val| find_first_value_for_any_key(val, keys))
        }
        serde_json::Value::Array(items) => {
            items.iter().find_map(|it| find_first_value_for_any_key(it, keys))
        }
        _ => None,
    }
}

/// 深度优先按多键名找时间字段（值形态自适应交给 parse_time_flexible）。
fn find_first_time(v: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    find_first_value_for_any_key(v, keys).and_then(parse_time_flexible)
}

// —— 信封错误分类（对齐 CodexBar throwIfErrorPayload 判定顺序）——

/// 信封错误类别（LoginRequired/InvalidCredentials → expired；NotAuthorised
/// → 缺 sec_token 专属文案；Api → 通用错误）。
enum PayloadError {
    LoginRequired,
    InvalidCredentials,
    WorkspaceNotAuthorised,
    Api(String),
}

/// 布尔弹性解析（true/1/yes/active/valid/normal ↔ false/0/no/inactive/
/// invalid/expired，大小写不敏感；其余 None）。
fn parse_bool(v: Option<&serde_json::Value>) -> Option<bool> {
    match v? {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => Some(n.as_f64()? != 0.0),
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "active" | "valid" | "normal" => Some(true),
            "false" | "0" | "no" | "inactive" | "invalid" | "expired" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// 深度收集所有 success/Success 布尔值（OneConsole 外层 200 内层失败的形态）。
fn collect_bool_values(v: &serde_json::Value, keys: &[&str], out: &mut Vec<bool>) {
    match v {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(b) = parse_bool(map.get(*key)) {
                    out.push(b);
                }
            }
            map.values().for_each(|val| collect_bool_values(val, keys, out));
        }
        serde_json::Value::Array(items) => {
            items.iter().for_each(|it| collect_bool_values(it, keys, out));
        }
        _ => {}
    }
}

/// 深度优先找第一个「自身 success/Success == false」的对象（错误详情优先
/// 从报告失败的同一层读取，避免被外层误导性 200 掩盖）。
fn failing_success_frame<'a>(v: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            if parse_bool(map.get("success")) == Some(false)
                || parse_bool(map.get("Success")) == Some(false)
            {
                return Some(v);
            }
            map.values().find_map(failing_success_frame)
        }
        serde_json::Value::Array(items) => items.iter().find_map(failing_success_frame),
        _ => None,
    }
}

/// code/message 文本 → 信封错误类别（对齐源码 isLoginOrTokenError /
/// isAuthorizationError；Workspace 权限错误不是凭证失效，优先识别并单独
/// 分类，避免淘汰有效会话）。
fn auth_error_of(code: Option<&str>, message: Option<&str>) -> Option<PayloadError> {
    let combined = [code, message]
        .iter()
        .flatten()
        .map(|s| s.trim().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if combined.is_empty() {
        return None;
    }
    if combined.contains("workspace.notauthorised") || combined.contains("workspace.notauthorized")
    {
        return Some(PayloadError::WorkspaceNotAuthorised);
    }
    const LOGIN_MARKERS: [&str; 7] = [
        "needlogin",
        "login",
        "postonlyortokenerror",
        "tokenerror",
        "request has expired",
        "refresh page",
        "请求已经过期",
    ];
    if LOGIN_MARKERS.iter().any(|m| combined.contains(m)) {
        return Some(PayloadError::LoginRequired);
    }
    const AUTH_MARKERS: [&str; 8] = [
        "notauthorised",
        "notauthorized",
        "not authorised",
        "not authorized",
        "unauthorised",
        "unauthorized",
        "access denied",
        "forbidden",
    ];
    if AUTH_MARKERS.iter().any(|m| combined.contains(m)) {
        return Some(PayloadError::InvalidCredentials);
    }
    None
}

/// 信封整体分类（对齐 CodexBar throwIfErrorPayload 的四段判定顺序）。
fn classify_payload(v: &serde_json::Value) -> Result<(), PayloadError> {
    // 1) 顶层 successResponse == false（BSS 信封直接失败）
    if parse_bool(v.get("successResponse")) == Some(false) {
        if let Some(status) = find_first_num(v, &["statusCode", "status_code", "code"]) {
            if status == 401.0 || status == 403.0 {
                return Err(PayloadError::InvalidCredentials);
            }
        }
        let code = find_first_str(v, &["errorCode", "code", "status", "statusCode"]);
        let message = find_first_str(v, &["errorMsg", "message", "msg", "statusMessage"]).or(code);
        return match auth_error_of(code, message) {
            Some(e) => Err(e),
            None => Err(PayloadError::Api(
                message
                    .map(str::to_string)
                    .unwrap_or_else(|| "request was not successful".to_string()),
            )),
        };
    }
    // 2) 深层任一 success/Success == false（外层 200 内层失败的嵌套形态）
    let mut bools = Vec::new();
    collect_bool_values(v, &["Success", "success"], &mut bools);
    if bools.contains(&false) {
        let frame = failing_success_frame(v).unwrap_or(v);
        let code = find_first_str(frame, &["errorCode", "Code", "code"])
            .or_else(|| find_first_str(v, &["errorCode", "Code", "code"]));
        let message = find_first_str(frame, &["errorMsg", "Message", "message", "msg"])
            .or_else(|| find_first_str(v, &["errorMsg", "Message", "message", "msg"]));
        return match auth_error_of(code, message) {
            Some(e) => Err(e),
            None => Err(PayloadError::Api(
                message
                    .map(str::to_string)
                    .or_else(|| code.map(str::to_string))
                    .unwrap_or_else(|| "request was not successful".to_string()),
            )),
        };
    }
    // 3) 数值 statusCode 非 0/200
    if let Some(status) = find_first_num(v, &["statusCode", "status_code", "code"]) {
        if status != 0.0 && status != 200.0 {
            if status == 401.0 || status == 403.0 {
                return Err(PayloadError::InvalidCredentials);
            }
            let message = find_first_str(v, &["statusMessage", "status_msg", "message", "msg"])
                .map(str::to_string)
                .unwrap_or_else(|| format!("status code {status}"));
            return Err(PayloadError::Api(message));
        }
    }
    // 4) 字符串 code/message 关键词兜底（非认证类关键词不算错误）
    let code = find_first_str(v, &["errorCode", "code", "status", "statusCode"]);
    let message = find_first_str(v, &["errorMsg", "message", "msg", "statusMessage"]);
    if let Some(e) = auth_error_of(code, message) {
        return Err(e);
    }
    Ok(())
}

// —— 原始响应 → 展开后的 JSON 或失败类别 ——

/// 原始响应的失败类别（entry 组装时决定状态与文案）。
enum Failure {
    /// 网络层失败（消息已带前缀，直接透传）
    Network(String),
    /// 会话失效（401/403、登录页 HTML、信封 login/token 标识）
    Expired,
    /// 工作区未授权（缺少控制台 sec_token）
    NotAuthorised,
    /// 非 200 非 401/403
    Http(u16),
    /// 200 但响应体为空 / 非 JSON
    Parse(String),
    /// 信封业务错误（带服务端 message）
    Api(String),
}

/// 原始响应 → 展开后的 JSON（成功）或失败类别（统一在此收敛 HTTP/HTML/
/// 信封三层判定，team 与 usage 共用）。
fn parse_raw(raw: &Result<(u16, Option<String>), String>) -> Result<serde_json::Value, Failure> {
    let (status, body) = raw.as_ref().map_err(|e| Failure::Network(e.clone()))?;
    if *status == 401 || *status == 403 {
        return Err(Failure::Expired);
    }
    if *status != 200 {
        return Err(Failure::Http(*status));
    }
    let Some(text) = body.as_deref().map(str::trim).filter(|b| !b.is_empty()) else {
        return Err(Failure::Parse("百炼 Token 包响应为空".to_string()));
    };
    // 会话过期时网关会重定向到登录页（HTML）：JSON 解析必失败，先行识别
    if is_likely_login_html(text) {
        return Err(Failure::Expired);
    }
    let parsed = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|_| Failure::Parse("百炼 Token 包响应解析失败".to_string()))?;
    let v = expand_json_strings(parsed);
    match classify_payload(&v) {
        Ok(()) => Ok(v),
        Err(PayloadError::LoginRequired | PayloadError::InvalidCredentials) => Err(Failure::Expired),
        Err(PayloadError::WorkspaceNotAuthorised) => Err(Failure::NotAuthorised),
        Err(PayloadError::Api(m)) => Err(Failure::Api(m)),
    }
}

/// 登录页 HTML 识别（对齐源码 isLikelyLoginHTML）。
fn is_likely_login_html(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("<html")
        && (lower.contains("login") || lower.contains("sign in") || lower.contains("signin"))
}

// —— Team 订阅摘要解析 ——

/// 订阅摘要字段键名（对齐 CodexBar usedQuotaKeys 等，camelCase/大写开头
/// 双形态共存；weekly/月度周期字段按源码列表全量保留）。
const USED_QUOTA_KEYS: [&str; 12] = [
    "usedQuota",
    "used_quota",
    "usedCredits",
    "usedCredit",
    "consumedCredits",
    "usage",
    "used",
    "usedAmount",
    "consumeAmount",
    "usedValue",
    "UsedValue",
    "consumedValue",
];
const TOTAL_QUOTA_KEYS: [&str; 12] = [
    "totalQuota",
    "total_quota",
    "totalCredits",
    "totalCredit",
    "quota",
    "creditLimit",
    "creditsTotal",
    "monthlyTotalQuota",
    "amount",
    "totalValue",
    "TotalValue",
    "cycleTotalValue",
];
const REMAINING_QUOTA_KEYS: [&str; 12] = [
    "remainingQuota",
    "remainQuota",
    "remainingCredits",
    "remainingCredit",
    "availableCredits",
    "balance",
    "remaining",
    "availableAmount",
    "remainAmount",
    "totalSurplusValue",
    "TotalSurplusValue",
    "surplusValue",
];
const SUBSCRIPTION_COUNT_KEYS: [&str; 4] = [
    "totalCount",
    "TotalCount",
    "subscriptionTotalNumber",
    "SubscriptionTotalNumber",
];
const RESET_DATE_KEYS: [&str; 15] = [
    "nextRefreshTime",
    "resetTime",
    "periodEndTime",
    "billingCycleEnd",
    "billCycleEndTime",
    "expireTime",
    "expirationTime",
    "endTime",
    "validEndTime",
    "instanceEndTime",
    "EndTime",
    "cycleEndTime",
    "CycleEndTime",
    "nearestExpireDate",
    "NearestExpireDate",
];
const PLAN_NAME_KEYS: [&str; 18] = [
    "planName",
    "plan_name",
    "packageName",
    "package_name",
    "commodityName",
    "commodity_name",
    "specType",
    "SpecType",
    "instanceName",
    "instance_name",
    "displayName",
    "display_name",
    "ProductName",
    "productName",
    "name",
    "title",
    "planType",
    "plan_type",
];

/// Team 订阅摘要的归一字段。
struct TeamSummary {
    used: Option<f64>,
    total: Option<f64>,
    remaining: Option<f64>,
    resets_at: Option<i64>,
    total_count: Option<f64>,
    plan_name: Option<String>,
}

/// Team 形态是否命中有效订阅（TotalCount>0 或存在任一配额数值）。
fn team_has_subscription(t: &TeamSummary) -> bool {
    t.total_count.map_or(false, |c| c > 0.0)
        || t.total.is_some()
        || t.used.is_some()
        || t.remaining.is_some()
}

/// 解析 Team 订阅摘要（对齐源码 findSubscriptionSummary：优先进入 Data/data/
/// successResponse 包裹层，摘要内无配额键时向嵌套找含配额键的对象；外层
/// 兜底找含配额/计数键的对象）。仅当摘要含任一目标键时返回 Some。
fn parse_team_summary(v: &serde_json::Value) -> Option<TeamSummary> {
    let quota_markers: Vec<&str> = USED_QUOTA_KEYS
        .iter()
        .chain(TOTAL_QUOTA_KEYS.iter())
        .chain(REMAINING_QUOTA_KEYS.iter())
        .copied()
        .collect();
    let count_markers: Vec<&str> = SUBSCRIPTION_COUNT_KEYS.to_vec();
    let has_any = |obj: &serde_json::Value, keys: &[&str]| {
        keys.iter().any(|k| get_any(obj, &[k]).is_some())
    };

    let summary = if let Some(data) =
        find_first_obj_with_keys(v, &["Data", "data", "successResponse", "success_response"])
    {
        if has_any(data, &quota_markers) {
            data
        } else if let Some(nested) = find_first_obj_with_any_key(data, &quota_markers) {
            nested
        } else {
            data
        }
    } else {
        let all: Vec<&str> = quota_markers.iter().chain(count_markers.iter()).copied().collect();
        find_first_obj_with_any_key(v, &all)?
    };

    // 配额数值：先摘要直取（camelCase/snake_case 键序），缺失再深度兜底
    let direct_num = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| get_any(summary, &[k]))
            .and_then(num_field)
            .or_else(|| find_first_num(summary, keys))
    };
    let total = direct_num(&TOTAL_QUOTA_KEYS);
    let remaining = direct_num(&REMAINING_QUOTA_KEYS);
    let mut used = direct_num(&USED_QUOTA_KEYS);
    // used 缺失时按 total - remaining 回退（clamp 0，对齐源码）
    if used.is_none() {
        if let (Some(t), Some(r)) = (total, remaining) {
            used = Some((t - r).max(0.0));
        }
    }
    let total_count = direct_num(&SUBSCRIPTION_COUNT_KEYS);
    let resets_at = RESET_DATE_KEYS
        .iter()
        .find_map(|k| get_any(summary, &[k]))
        .and_then(parse_time_flexible)
        .or_else(|| find_first_time(summary, &RESET_DATE_KEYS))
        .or_else(|| find_first_time(v, &RESET_DATE_KEYS));

    // plan 名：摘要字段优先；缺失时按 TotalCount>0 / 有 total 兜底 "TOKEN PLAN"
    let plan_name = PLAN_NAME_KEYS
        .iter()
        .find_map(|k| get_any(summary, &[k]))
        .and_then(|p| p.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| find_first_str(summary, &PLAN_NAME_KEYS).map(str::to_string))
        .or_else(|| {
            if total_count.map_or(false, |c| c > 0.0) || total.is_some() {
                Some("TOKEN PLAN".to_string())
            } else {
                None
            }
        });

    if used.is_none() && total.is_none() && remaining.is_none() && total_count.is_none() {
        return None;
    }
    Some(TeamSummary {
        used,
        total,
        remaining,
        resets_at,
        total_count,
        plan_name,
    })
}

// —— Personal/Solo usage 双窗口解析 ——

/// 套餐码展示名（对齐源码 displayPlanName：常见档位首字母大写）。
fn display_plan_name(plan_code: &str) -> String {
    match plan_code {
        "lite" => "Lite".to_string(),
        "standard" => "Standard".to_string(),
        "pro" => "Pro".to_string(),
        "max" => "Max".to_string(),
        other => other.to_string(),
    }
}

/// 可选 Personal 端点原始结果 → 展开后 JSON（失败静默为 None）。
fn optional_payload(raw: Option<&(u16, Option<String>)>) -> Option<serde_json::Value> {
    let (status, body) = raw?;
    parse_raw(&Ok((*status, body.clone()))).ok()
}

/// 解析 Personal/Solo usage（对齐 AlibabaTokenPlanPersonalUsageParser）：
/// usage 给 per5Hour/per1Week 百分比（0-1 比率）与重置时间；subscription 给
/// specCode 套餐码；quota-config 按套餐码给 five_hour/weekly 总量。返回
/// (plan 名, 1-2 个滚动窗口)；usage 无任何窗口字段时 None。
fn parse_personal_usage(
    usage_v: &serde_json::Value,
    subscription_raw: Option<&(u16, Option<String>)>,
    quota_config_raw: Option<&(u16, Option<String>)>,
) -> Option<(Option<String>, Vec<ProviderQuotaWindow>)> {
    // 窗口字段所在对象（深度查找，data 包裹/JSON 字符串形态已展开）
    let usage = find_first_obj_with_any_key(
        usage_v,
        &["per5HourPercentage", "per1WeekPercentage"],
    )?;
    let ratio_to_percent = |keys: &[&str]| {
        get_any(usage, keys)
            .and_then(num_field)
            .map(|ratio| (ratio * 100.0).clamp(0.0, 100.0))
    };
    let five_percent = ratio_to_percent(&["per5HourPercentage"]);
    let weekly_percent = ratio_to_percent(&["per1WeekPercentage"]);
    if five_percent.is_none() && weekly_percent.is_none() {
        return None;
    }
    let five_resets = get_any(usage, &["per5HourResetTime"]).and_then(parse_time_flexible);
    let weekly_resets = get_any(usage, &["per1WeekResetTime"]).and_then(parse_time_flexible);

    // 套餐码：subscription 载荷中含 specCode/spec_code/planName/plan_name 的
    // 对象，按键序取第一个非空值并小写（展示时再映射）
    let plan_code = optional_payload(subscription_raw)
        .as_ref()
        .and_then(|sub| find_first_obj_with_any_key(sub, &["specCode", "spec_code", "planName", "plan_name"]))
        .and_then(|plan| {
            ["specCode", "spec_code", "planName", "plan_name"]
                .iter()
                .find_map(|k| get_any(plan, &[k]))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_ascii_lowercase())
        });
    // 总量：quota-config 中以套餐码为键的对象（five_hour|fiveHour / weekly）
    let totals = plan_code.as_deref().and_then(|code| {
        optional_payload(quota_config_raw).as_ref().and_then(|cfg| {
            find_first_value_for_any_key(cfg, &[code]).and_then(|quota| {
                let five_hour = get_any(quota, &["five_hour", "fiveHour"]).and_then(num_field);
                let weekly = get_any(quota, &["weekly"]).and_then(num_field);
                if five_hour.is_none() && weekly.is_none() {
                    None
                } else {
                    Some((five_hour, weekly))
                }
            })
        })
    });
    let plan_name = plan_code
        .map(|c| display_plan_name(&c))
        .or_else(|| Some("Personal".to_string()));

    // 双窗口：百分比来自 usage，总量来自 quota-config；used = total*percent/100
    let mut windows = Vec::new();
    for (key, title, percent, total, resets_at) in [
        ("hour5", "5h", five_percent, totals.and_then(|t| t.0), five_resets),
        ("weekly", "7天", weekly_percent, totals.and_then(|t| t.1), weekly_resets),
    ] {
        let Some(percent) = percent else { continue };
        let used = total.map(|t| ((t * percent / 100.0) * 100.0).round() / 100.0);
        windows.push(ProviderQuotaWindow {
            key: key.to_string(),
            title: title.to_string(),
            used_percent: Some(percent),
            used,
            total,
            unit: Some("积分".to_string()),
            resets_at,
        });
    }
    if windows.is_empty() {
        return None;
    }
    Some((plan_name, windows))
}

// ============================================================
// 条目组装（纯函数，网络无关，单测直接构造输入）
// ============================================================

/// 失败条目（windows 恒空；message 承载原因）。
fn entry_error(cred_id: &str, label: &str, message: &str) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "error".to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message.to_string()),
        updated_at: now_ms(),
    }
}

/// 会话失效条目（凭证卡显示「已过期」徽章）。
fn entry_expired(cred_id: &str, label: &str) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "expired".to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some("会话已失效，请重新登录百炼控制台后更新 Cookie".to_string()),
        updated_at: now_ms(),
    }
}

/// Failure → 展示文案（Expired 由调用方先行分流，不经过这里）。
fn failure_message(failure: &Failure) -> Option<String> {
    match failure {
        Failure::Network(e) => Some(e.clone()),
        Failure::Expired => None,
        Failure::NotAuthorised => {
            Some("缺少控制台 sec_token，请从百炼订阅页请求中复制完整 Cookie".to_string())
        }
        Failure::Http(status) => Some(format!("百炼 Token 包查询失败（HTTP {status}）")),
        Failure::Parse(msg) => Some(msg.clone()),
        Failure::Api(msg) => Some(format!("百炼平台返回错误: {msg}")),
    }
}

/// 已用/总量 → 百分比（total ≤0 或缺失 → None；结果 clamp 0-100）。
fn used_percent(used: Option<f64>, total: Option<f64>) -> Option<f64> {
    let total = total?;
    if total <= 0.0 {
        return None;
    }
    let used = used?;
    Some((used / total * 100.0).clamp(0.0, 100.0))
}

/// 解析单凭证抓取结果 → 展示条目（纯函数）。
/// 分支：Team 摘要命中订阅 → 积分池单窗口；TotalCount==0 → 降级 Personal
/// usage 双窗口，仍无数据 → 「未找到有效的百炼 Token 包订阅」；Team 失败
///（判定失败的形态）→ 降级仅 usage 窗口；401/403、登录标识 → expired；
/// Workspace.NotAuthorised → 缺 sec_token 专属 error。
fn entry_from_bundle(cred_id: &str, label: &str, bundle: &QuotaBundle) -> ProviderQuotaEntry {
    match parse_raw(&bundle.team) {
        Ok(team_v) => {
            if let Some(team) = parse_team_summary(&team_v).filter(team_has_subscription) {
                // Team 形态：积分池 used/total/remaining 作单窗口
                return ProviderQuotaEntry {
                    credential_id: cred_id.to_string(),
                    label: label.to_string(),
                    status: "ok".to_string(),
                    windows: vec![ProviderQuotaWindow {
                        key: "credits".to_string(),
                        title: "积分池".to_string(),
                        used_percent: used_percent(team.used, team.total),
                        used: team.used,
                        total: team.total,
                        unit: Some("积分".to_string()),
                        resets_at: team.resets_at,
                    }],
                    balance: None,
                    plan_name: team.plan_name,
                    message: None,
                    updated_at: now_ms(),
                };
            }
            // Team 成功但无订阅（TotalCount==0 等）→ Personal/Solo 形态
            personal_entry(cred_id, label, bundle, true, None)
        }
        Err(Failure::Expired) => entry_expired(cred_id, label),
        Err(failure) => {
            // Team 形态判定失败 → 降级为 Personal/Solo usage 窗口
            personal_entry(cred_id, label, bundle, false, Some(failure))
        }
    }
}

/// Personal/Solo 降级链路（team 未命中订阅后走这里；usage 为必需端点）。
fn personal_entry(
    cred_id: &str,
    label: &str,
    bundle: &QuotaBundle,
    no_subscription: bool,
    team_failure: Option<Failure>,
) -> ProviderQuotaEntry {
    let fail = |message: String| entry_error(cred_id, label, &message);
    let Some(usage_raw) = &bundle.usage else {
        // fetch 层 team 未命中必然已抓 usage；直构 bundle 缺失时按无数据报错
        return match team_failure.as_ref().and_then(failure_message) {
            Some(msg) if !no_subscription => fail(msg),
            _ => fail("未找到有效的百炼 Token 包订阅".to_string()),
        };
    };
    match parse_raw(usage_raw) {
        Err(Failure::Expired) => entry_expired(cred_id, label),
        Err(failure) => {
            // 缺 sec_token 的专属文案（usage 或 team 任一侧命中即用）
            if matches!(failure, Failure::NotAuthorised)
                || matches!(team_failure, Some(Failure::NotAuthorised))
            {
                return fail("缺少控制台 sec_token，请从百炼订阅页请求中复制完整 Cookie".to_string());
            }
            if no_subscription {
                return fail("未找到有效的百炼 Token 包订阅".to_string());
            }
            // 两侧都失败：优先透传 usage 失败原因，退回 team 失败原因
            let message = failure_message(&failure)
                .or_else(|| team_failure.as_ref().and_then(failure_message))
                .unwrap_or_else(|| "百炼 Token 包查询失败".to_string());
            fail(message)
        }
        Ok(usage_v) => {
            match parse_personal_usage(&usage_v, bundle.subscription.as_ref(), bundle.quota_config.as_ref()) {
                Some((plan_name, windows)) => ProviderQuotaEntry {
                    credential_id: cred_id.to_string(),
                    label: label.to_string(),
                    status: "ok".to_string(),
                    windows,
                    balance: None,
                    plan_name,
                    message: None,
                    updated_at: now_ms(),
                },
                None if no_subscription => fail("未找到有效的百炼 Token 包订阅".to_string()),
                // usage 成功信封但持续无窗口字段（重试后仍瞬态空窗）
                None => fail("百炼用量暂时不可用，请稍后重试".to_string()),
            }
        }
    }
}

// ============================================================
// 请求构造辅助（form 编码 / cookie 取值 / trace id / sec_token 提取）
// ============================================================

/// application/x-www-form-urlencoded 值编码（RFC 3986 保留字外全部百分号
/// 转义；unreserved：A-Z a-z 0-9 - . _ ~）。
fn form_urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 从归一 cookie 串取指定名的值（名字精确匹配，值取首 `=` 之后全部）。
fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|part| {
        let mut pieces = part.splitn(2, '=');
        let key = pieces.next()?.trim();
        let value = pieces.next()?;
        (key == name).then(|| value.trim())
    })
}

/// 伪 UUID（cornerstoneParam.feTraceId 用，网关不校验格式）：时间戳 +
/// 进程号混合 xorshift 生成 8-4-4-4-12 小写十六进制。
fn pseudo_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = nanos ^ ((std::process::id() as u64) << 32) ^ 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let hex = |v: u64, width: usize| format!("{:0width$x}", v, width = width);
    format!(
        "{}-{}-{}-{}-{}",
        hex(next() & 0xffff_ffff, 8),
        hex(next() & 0xffff, 4),
        hex(next() & 0xffff, 4),
        hex(next() & 0xffff, 4),
        hex(next() & 0xffff_ffff_ffff, 12)
    )
}

/// 从控制台 HTML 提取 sec_token（对齐源码 extractSECToken 的 5 组形态：
/// 带引号键 `"secToken"`/`"sec_token"`，宽松键 `secToken`/`sec_token`/
/// `SEC_TOKEN`；分隔符 `:` 或 `=`，值必须带引号）。
fn extract_sec_token_from_html(html: &str) -> Option<String> {
    for (key, quoted_key) in [
        ("secToken", true),
        ("sec_token", true),
        ("secToken", false),
        ("sec_token", false),
        ("SEC_TOKEN", false),
    ] {
        if let Some(token) = scan_string_value_after_key(html, key, quoted_key) {
            return Some(token);
        }
    }
    None
}

/// 在文本中扫描 `key`（可选引号包裹）后的 `: value` / `= value`，值必须
/// 引号包裹（单/双引号），取引号内内容；命中即返回。多次出现取首个非空。
fn scan_string_value_after_key(text: &str, key: &str, quoted_key: bool) -> Option<String> {
    let mut search_from = 0usize;
    while let Some(pos) = text[search_from..].find(key) {
        let abs = search_from + pos;
        let mut rest = &text[abs + key.len()..];
        if quoted_key {
            // 带引号键形态：跳过键后的闭合引号
            rest = rest.trim_start();
            if rest.starts_with('"') || rest.starts_with('\'') {
                rest = &rest[1..];
            }
        }
        // 分隔符 `:` 或 `=`
        let rest_trim = rest.trim_start();
        if !rest_trim.starts_with(':') && !rest_trim.starts_with('=') {
            search_from = abs + key.len();
            continue;
        }
        // 值必须引号包裹（与源码值捕获组 `[^'"]+` 一致）
        let after_sep = rest_trim[1..].trim_start();
        let open = after_sep.chars().next();
        match open {
            Some(q @ ('"' | '\'')) => {
                let content = &after_sep[1..];
                if let Some(end) = content.find(q) {
                    let value = content[..end].trim();
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
            _ => {}
        }
        search_from = abs + key.len();
    }
    None
}

// ============================================================
// 单元测试（纯函数，不联网）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "abt-1";
    const LABEL: &str = "百炼主号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    fn ok_tuple(body: &str) -> (u16, Option<String>) {
        (200, Some(body.to_string()))
    }

    /// Team 形态：JSON 字符串信封 + Data 包裹，TotalCount=1 + 千分位逗号
    /// 配额 + ISO 重置时间 → ok + 积分池单窗口（usedPercent = used/total）。
    #[test]
    fn parses_team_summary_with_nested_string_envelope() {
        let body = r#"{
            "successResponse": true,
            "data": "{\"TotalCount\":1,\"Data\":{\"TotalCount\":1,\"planName\":\"企业版\",\"usedQuota\":\"1,200\",\"totalQuota\":6000,\"remainingQuota\":4800,\"nextRefreshTime\":\"2030-10-27T05:06:07Z\"}}"
        }"#;
        let bundle = QuotaBundle {
            team: ok_raw(body),
            usage: None,
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert_eq!(entry.plan_name.as_deref(), Some("企业版"));
        assert_eq!(entry.windows.len(), 1);
        let w = &entry.windows[0];
        assert_eq!(w.key, "credits");
        assert_eq!(w.title, "积分池");
        assert_eq!(w.used, Some(1200.0)); // 千分位逗号解析
        assert_eq!(w.total, Some(6000.0));
        assert_eq!(w.used_percent, Some(20.0));
        assert_eq!(w.unit.as_deref(), Some("积分"));
        assert_eq!(w.resets_at, Some(1_919_307_967_000)); // ISO → ms

        // used 缺失时按 total - remaining 回退
        let body = r#"{"successResponse":true,"Data":{"TotalCount":2,"totalQuota":100,
            "remainingQuota":30,"nextRefreshTime":1730018000}}"#;
        let entry = entry_from_bundle(
            CRED_ID,
            LABEL,
            &QuotaBundle {
                team: ok_raw(body),
                usage: None,
                subscription: None,
                quota_config: None,
            },
        );
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows[0].used, Some(70.0));
        assert_eq!(entry.plan_name.as_deref(), Some("TOKEN PLAN")); // 无 plan 字段兜底
    }

    /// Personal/Solo 形态：usage 双百分比窗口（0-1 比率 ×100）+ quota-config
    /// 按套餐码给总量 + subscription specCode → 两窗口与 used 推导、plan 名。
    #[test]
    fn parses_personal_double_windows_with_quota_config() {
        let bundle = QuotaBundle {
            // team 明确无订阅（TotalCount=0）→ 走 Personal
            team: ok_raw(r#"{"successResponse":true,"Data":{"TotalCount":0}}"#),
            usage: Some(ok_raw(
                r#"{"successResponse":true,"data":"{\"per5HourPercentage\":0.25,\"per1WeekPercentage\":0.5,\"per5HourResetTime\":1730018000,\"per1WeekResetTime\":\"2030-10-27 05:06:07\"}"}"#,
            )),
            subscription: Some(ok_tuple(
                r#"{"successResponse":true,"data":{"specCode":"pro"}}"#,
            )),
            quota_config: Some(ok_tuple(
                r#"{"successResponse":true,"data":{"pro":{"five_hour":100,"weekly":500}}}"#,
            )),
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.plan_name.as_deref(), Some("Pro"));
        assert_eq!(entry.windows.len(), 2);

        let hour5 = &entry.windows[0];
        assert_eq!(hour5.key, "hour5");
        assert_eq!(hour5.title, "5h");
        assert_eq!(hour5.used_percent, Some(25.0));
        assert_eq!(hour5.total, Some(100.0));
        assert_eq!(hour5.used, Some(25.0)); // total × percent 推导
        assert_eq!(hour5.resets_at, Some(1_730_018_000_000));

        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.title, "7天");
        assert_eq!(weekly.used_percent, Some(50.0));
        assert_eq!(weekly.total, Some(500.0));
        assert_eq!(weekly.used, Some(250.0));
        assert_eq!(weekly.resets_at, Some(1_919_307_967_000));

        // 无 planCode / quota-config 时：plan 回退 "Personal"，窗口只有百分比
        let bundle = QuotaBundle {
            team: ok_raw(r#"{"successResponse":true,"Data":{"TotalCount":0}}"#),
            usage: Some(ok_raw(
                r#"{"successResponse":true,"data":{"per1WeekPercentage":0.3}}"#,
            )),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.plan_name.as_deref(), Some("Personal"));
        assert_eq!(entry.windows.len(), 1);
        assert_eq!(entry.windows[0].used_percent, Some(30.0));
        assert_eq!(entry.windows[0].total, None);
        assert_eq!(entry.windows[0].used, None);
    }

    /// Team 成功但 TotalCount==0 且 Personal usage 无窗口字段 → error
    /// 「未找到有效的百炼 Token 包订阅」，不画窗口。
    #[test]
    fn team_total_count_zero_without_usage_reports_no_subscription() {
        let bundle = QuotaBundle {
            team: ok_raw(r#"{"successResponse":true,"Data":{"TotalCount":0}}"#),
            usage: Some(ok_raw(r#"{"successResponse":true,"data":{}}"#)),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert!(entry.windows.is_empty());
        assert_eq!(
            entry.message.as_deref(),
            Some("未找到有效的百炼 Token 包订阅")
        );

        // usage 缺窗口字段持续（瞬态空窗后仍为空）同样归入无订阅文案
        let bundle = QuotaBundle {
            team: ok_raw(r#"{"successResponse":true,"Data":{"TotalCount":0}}"#),
            usage: Some(ok_raw(
                r#"{"successResponse":true,"data":{"other":1}}"#,
            )),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert_eq!(entry.message.as_deref(), Some("未找到有效的百炼 Token 包订阅"));
    }

    /// Team 形态判定失败（网络/信封错误）→ 降级为仅 Personal usage 窗口，
    /// 条目仍为 ok（对齐「判定失败的形态降级为仅 usage 窗口」）。
    #[test]
    fn team_failure_degrades_to_personal_windows() {
        for team in [
            Err("阿里 Token Plan 额度网络错误或服务不可用: timeout".to_string()),
            ok_raw(r#"{"successResponse":false,"errorMsg":"internal error"}"#),
            Ok((500, Some("internal".to_string()))),
        ] {
            let bundle = QuotaBundle {
                team,
                usage: Some(ok_raw(
                    r#"{"successResponse":true,"data":{"per5HourPercentage":0.4}}"#,
                )),
                subscription: None,
                quota_config: None,
            };
            let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
            assert_eq!(entry.status, "ok");
            assert_eq!(entry.windows.len(), 1);
            assert_eq!(entry.windows[0].used_percent, Some(40.0));
        }
    }

    /// HTTP 401/403 与登录页 HTML、信封 NeedLogin → expired「会话已失效…」
    ///（假 Cookie 手测链路的预期分支），文案不含 Cookie 内容。
    #[test]
    fn unauthorized_login_html_and_payload_map_expired() {
        for team in [
            Ok((401u16, Some(r#"{"detail":"unauthorized"}"#.to_string()))),
            Ok((403u16, Some("forbidden".to_string()))),
            ok_raw("<!DOCTYPE html><html><body>please login</body></html>"),
            ok_raw(r#"{"successResponse":false,"errorMsg":"NeedLogin"}"#),
            ok_raw(r#"{"code":"NeedLogin","data":{}}"#),
        ] {
            let entry = entry_from_bundle(
                CRED_ID,
                LABEL,
                &QuotaBundle {
                    team,
                    usage: None,
                    subscription: None,
                    quota_config: None,
                },
            );
            assert_eq!(entry.status, "expired");
            assert_eq!(
                entry.message.as_deref(),
                Some("会话已失效，请重新登录百炼控制台后更新 Cookie")
            );
            assert!(entry.windows.is_empty());
        }
    }

    /// 信封 `BailianGateway.Workspace.NotAuthorised` → error「缺少控制台
    /// sec_token…」（工作区权限问题不淘汰会话，非 expired）。
    #[test]
    fn workspace_not_authorized_maps_to_sec_token_error() {
        let body = r#"{"successResponse":false,
            "errorCode":"BailianGateway.Workspace.NotAuthorised",
            "errorMsg":"workspace not authorised"}"#;
        let entry = entry_from_bundle(
            CRED_ID,
            LABEL,
            &QuotaBundle {
                team: ok_raw(body),
                usage: None,
                subscription: None,
                quota_config: None,
            },
        );
        assert_eq!(entry.status, "error");
        assert_eq!(
            entry.message.as_deref(),
            Some("缺少控制台 sec_token，请从百炼订阅页请求中复制完整 Cookie")
        );
        // team 侧 NotAuthorised 但 personal usage 正常 → 仍出窗口
        let bundle = QuotaBundle {
            team: ok_raw(body),
            usage: Some(ok_raw(
                r#"{"successResponse":true,"data":{"per5HourPercentage":0.1}}"#,
            )),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows.len(), 1);
    }

    /// 其他信封错误 / 非 200 / 网络失败 / 坏 JSON → error 且消息不含 secret。
    #[test]
    fn other_failures_map_to_error() {
        // usage 与 team 都失败（非认证类）→ 透传 usage 失败原因
        let bundle = QuotaBundle {
            team: Ok((500, Some("internal".to_string()))),
            usage: Some(Err("阿里 Token Plan 额度网络错误或服务不可用: dns".to_string())),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        let msg = entry.message.expect("error 条目必须有原因");
        assert!(msg.contains("网络错误或服务不可用"), "{msg}");
        assert!(!msg.contains("Cookie="), "错误消息不得含 Cookie 内容");

        // usage 业务错误信封：Team 明确无订阅时无订阅文案优先（更准确的诊断）
        let bundle = QuotaBundle {
            team: ok_raw(r#"{"successResponse":true,"Data":{"TotalCount":0}}"#),
            usage: Some(ok_raw(r#"{"successResponse":false,"errorMsg":"rate limited"}"#)),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert_eq!(entry.message.as_deref(), Some("未找到有效的百炼 Token 包订阅"));

        // Team 失败（非认证类）+ usage 业务错误信封 → 透传服务端 message
        let bundle = QuotaBundle {
            team: Ok((500, Some("internal".to_string()))),
            usage: Some(ok_raw(r#"{"successResponse":false,"errorMsg":"rate limited"}"#)),
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("rate limited"));

        // 200 坏 JSON（非登录页）→ 解析失败
        let bundle = QuotaBundle {
            team: ok_raw("not json"),
            usage: None,
            subscription: None,
            quota_config: None,
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("解析失败"));
    }

    /// region 分流（对齐 CodexBar AlibabaTokenPlanAPIRegion）：global 走
    /// 国际站（Team 网关 modelstudio、Personal 网关 bailian-singapore-cs），
    /// 其余默认中国站（Team 网关 bailian、Personal 网关 bailian-cs）。
    #[test]
    fn region_conf_splits_international_and_china() {
        let intl = region_conf(Some("global"));
        assert_eq!(intl.gateway, "https://modelstudio.console.alibabacloud.com");
        assert_eq!(intl.quota_host, "https://bailian-singapore-cs.alibabacloud.com");
        assert_eq!(intl.region_id, "ap-southeast-1");
        assert_eq!(intl.gateway_action, "IntlBroadScopeAspnGateway");
        assert_eq!(intl.console_site, "MODELSTUDIO_ALBABACLOUD");
        assert_eq!(intl.team_product, "sfm_tokenplanteams_dp_intl");
        assert_eq!(intl.solo_product, "sfm_tokenplansolo_public_intl");
        assert!(intl.personal_dashboard.ends_with("/token-plan/personal"));

        for region in [None, Some("cn"), Some("weird")] {
            let cn = region_conf(region);
            assert_eq!(cn.gateway, "https://bailian.console.aliyun.com");
            assert_eq!(cn.quota_host, "https://bailian-cs.console.aliyun.com");
            assert_eq!(cn.region_id, "cn-beijing");
            assert_eq!(cn.gateway_action, "BroadScopeAspnGateway");
            assert_eq!(cn.console_site, "BAILIAN_ALIYUN");
            assert_eq!(cn.team_product, "sfm_tokenplanteams_dp_cn");
            assert_eq!(cn.solo_product, "sfm_tokenplansolo_public_cn");
        }
    }

    /// 字段弹性：quota-config 值形态（字符串数字/千分位）、specCode 深层
    /// 包裹、百分比 clamp、total 为 0 时无百分比。
    #[test]
    fn flexible_fields_and_percent_guards() {
        // 千分位 / 字符串数字总量 + specCode 嵌套在数组内
        let bundle = QuotaBundle {
            team: ok_raw(r#"{"successResponse":true,"Data":{"TotalCount":0}}"#),
            usage: Some(ok_raw(
                r#"{"successResponse":true,"data":{"per5HourPercentage":2}}"#,
            )),
            subscription: Some(ok_tuple(
                r#"{"successResponse":true,"data":{"list":[{"spec_code":"MAX"}]}}"#,
            )),
            quota_config: Some(ok_tuple(
                r#"{"successResponse":true,"data":{"max":{"five_hour":"1,000"}}}"#,
            )),
        };
        let entry = entry_from_bundle(CRED_ID, LABEL, &bundle);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.plan_name.as_deref(), Some("Max")); // 套餐码小写后映射
        let w = &entry.windows[0];
        assert_eq!(w.used_percent, Some(100.0)); // 比率 2 → 200% clamp 100
        assert_eq!(w.total, Some(1000.0));
        assert_eq!(w.used, Some(1000.0)); // 1000 × 100% 推导

        // Team total=0：有窗口但无百分比
        let body = r#"{"successResponse":true,"Data":{"TotalCount":3,"usedQuota":5,"totalQuota":0}}"#;
        let entry = entry_from_bundle(
            CRED_ID,
            LABEL,
            &QuotaBundle {
                team: ok_raw(body),
                usage: None,
                subscription: None,
                quota_config: None,
            },
        );
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows[0].used_percent, None);
        assert_eq!(entry.windows[0].used, Some(5.0));
        assert_eq!(entry.windows[0].total, Some(0.0));
    }

    /// sec_token 的 HTML 提取五种形态（带引号键 / 宽松键 / 大写
    /// SEC_TOKEN）与未命中 None；cookie 取值精确匹配。
    #[test]
    fn extracts_sec_token_from_html_and_cookie() {
        assert_eq!(
            extract_sec_token_from_html(r#"window.ALIYUN_CONSOLE_CONFIG={SEC_TOKEN: "tok-1",a:1}"#).as_deref(),
            Some("tok-1")
        );
        assert_eq!(
            extract_sec_token_from_html(r#"{"secToken":"tok-2"}"#).as_deref(),
            Some("tok-2")
        );
        assert_eq!(
            extract_sec_token_from_html("sec_token: 'tok-3'").as_deref(),
            Some("tok-3")
        );
        assert_eq!(
            extract_sec_token_from_html(r#"secToken ="tok-4""#).as_deref(),
            Some("tok-4")
        );
        assert_eq!(extract_sec_token_from_html("no token here"), None);
        // 值无引号 → 不命中（与源码捕获组一致）
        assert_eq!(extract_sec_token_from_html("secToken= bare"), None);

        let cookie = "cna=xyz; login_aliyunid_csrf=csrf-1; sec_token=tok-5; uid=1";
        assert_eq!(cookie_value(cookie, "sec_token"), Some("tok-5"));
        assert_eq!(cookie_value(cookie, "login_aliyunid_csrf"), Some("csrf-1"));
        assert_eq!(cookie_value(cookie, "SEC_TOKEN"), None);
        assert_eq!(cookie_value(cookie, "missing"), None);
    }

    /// 非 cookie 凭证被过滤；空 Cookie 归一为空串时直接产出 error 不发请求。
    #[test]
    fn non_cookie_and_empty_cookie_credentials_short_circuit() {
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
                label: "无效 cURL".into(),
                kind: "cookie".into(),
                // 不含 Cookie 头的 cURL → 归一为空串 → error（不发请求）
                secret: "curl 'https://bailian.console.aliyun.com' -H 'User-Agent: x'".into(),
                region: None,
            },
        ];
        let entries = fetch_quota_entries(&snapshots);
        assert_eq!(entries.len(), 1, "apiKey 凭证不应被 cookie 型 provider 消费");
        assert_eq!(entries[0].credential_id, "b");
        assert_eq!(entries[0].status, "error");
        assert!(entries[0].message.as_ref().unwrap().contains("解析出 Cookie"));
    }

    /// form 编码与伪 UUID 形态（离线纯函数）。
    #[test]
    fn form_urlencode_and_pseudo_uuid_shapes() {
        assert_eq!(form_urlencode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(form_urlencode("a b/c=d&e"), "a%20b%2Fc%3Dd%26e");
        assert_eq!(form_urlencode("中文"), "%E4%B8%AD%E6%96%87");
        let uuid = pseudo_uuid();
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.matches('-').count(), 4);
        assert!(uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}

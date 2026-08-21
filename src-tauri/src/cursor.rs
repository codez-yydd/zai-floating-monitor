//! Cursor 用量统计模块。
//!
//! 认证原理（参照 CodexBar）：
//! 1. 读取 Cursor 应用的本地 SQLite（state.vscdb）中的 JWT accessToken
//! 2. 解析 JWT payload 取出 user ID（sub 字段，取 | 后的部分）
//! 3. 拼接 cookie：WorkosCursorSessionToken=<userID>%3A%3A<accessToken>
//! 4. 用该 cookie 调用 cursor.com 的 API：
//!    - GET /api/usage-summary  套餐额度（金额为美分）
//!    - GET /api/auth/me        账户身份
//!    - POST /api/dashboard/get-filtered-usage-events  token 花费明细（分页）

use base64::Engine;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use chrono::{Local, TimeZone};

use crate::db;
use crate::pricing::config_dir;

const CURSOR_BASE: &str = "https://cursor.com";
/// Cursor 应用的 state.vscdb 相对于 config_dir 的路径（跨平台通用）
const CURSOR_DB_REL: &str = "Cursor/User/globalStorage/state.vscdb";
/// events API 的分页上限，防止分页 bug 死循环（200 页 × 1000 = 20 万条）
const MAX_PAGES: usize = 200;
const PAGE_SIZE: usize = 1000;
/// events 结果缓存 TTL（2 分钟，与前端刷新间隔对齐）
const EVENTS_CACHE_TTL: Duration = Duration::from_secs(120);

// ============================================================
// 配置
// ============================================================

/// Cursor 配置（~/.zbar/cursor.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorConfig {
    /// cookie 来源："auto"（读 Cursor 应用本地 DB）| "manual"（手动粘贴）
    #[serde(default = "default_cookie_source")]
    pub cookie_source: String,
    /// 手动 cookie 头（cookie_source=manual 时使用）
    #[serde(default)]
    pub cookie_header: String,
    /// USD→CNY 汇率（汇总页合并花费用），默认 7.2
    #[serde(default = "default_fx_rate")]
    pub usd_cny_rate: f64,
    /// 是否每日自动联网更新汇率（默认 true，开箱即自动）
    #[serde(default = "default_true")]
    pub fx_rate_auto: bool,
    /// 汇率最近一次联网获取的时间（ms 时间戳，None=从未自动获取过）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_rate_fetched_at: Option<i64>,
    /// 汇率最近一次获取成功的来源名（如 "er-api"，用于设置页展示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_rate_source: Option<String>,
}

fn default_cookie_source() -> String {
    "auto".to_string()
}

fn default_fx_rate() -> f64 {
    7.2
}

fn default_true() -> bool {
    true
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            cookie_source: default_cookie_source(),
            cookie_header: String::new(),
            usd_cny_rate: default_fx_rate(),
            fx_rate_auto: default_true(),
            fx_rate_fetched_at: None,
            fx_rate_source: None,
        }
    }
}

pub fn cursor_config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("cursor.json"))
}

pub fn load_cursor_config() -> Result<CursorConfig, String> {
    let path = cursor_config_path()?;
    if !path.exists() {
        return Ok(CursorConfig::default());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 Cursor 配置失败: {e}"))?;
    serde_json::from_str::<CursorConfig>(&data)
        .map_err(|e| format!("解析 Cursor 配置失败: {e}"))
}

/// cursor.json 写互斥：后台每日汇率刷新的"读-改-写"段与设置页保存的
/// 全量写并发时（毫秒级窗口交错），无锁可能互相覆盖（如丢刚保存的
/// cookie）或交错写坏文件。所有写路径统一经此锁串行化。
static CURSOR_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cursor_write_lock() -> &'static Mutex<()> {
    CURSOR_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn save_cursor_config(cfg: &CursorConfig) -> Result<(), String> {
    let _guard = cursor_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_cursor_config_unlocked(cfg)
}

/// 写入 cursor.json。调用方必须已持有 CURSOR_WRITE_LOCK（std::Mutex
/// 不可重入，持锁内不得再调 save_cursor_config）。
fn write_cursor_config_unlocked(cfg: &CursorConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = cursor_config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化 Cursor 配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入 Cursor 配置失败: {e}"))
}

// ============================================================
// 汇率自动获取（多源容错）
// ============================================================

/// 免费无 key 汇率数据源（按优先级）。三源响应结构一致：
/// { rates: { CNY: 7.16, ... } }，任一成功即采用。
const FX_RATE_SOURCES: &[(&str, &str)] = &[
    ("er-api", "https://open.er-api.com/v6/latest/USD"),
    ("frankfurter", "https://api.frankfurter.app/latest?from=USD&to=CNY"),
    ("exchangerate-api", "https://api.exchangerate-api.com/v4/latest/USD"),
];

/// 从单个源拉取 USD→CNY 汇率（超时 15s）
fn fetch_fx_rate_from(source: &str, url: &str) -> Result<f64, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();
    let resp = agent
        .get(url)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| format!("{source} 请求失败: {e}"))?;
    let root: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("{source} 响应解析失败: {e}"))?;
    let rate = root
        .get("rates")
        .and_then(|r| r.get("CNY"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("{source} 响应中缺少有效的 CNY 汇率"))?;
    // 常识范围校验（5~15）：防止解析异常/脏数据写坏配置。
    // USD→CNY 多年正常运行于此区间，超界视为数据源异常，换下一个源。
    if !(5.0..=15.0).contains(&rate) {
        return Err(format!("{source} 汇率 {rate} 超出常识范围 (5~15)，疑似脏数据"));
    }
    Ok(rate)
}

/// 联网获取最新 USD→CNY 汇率并写回 cursor 配置。
/// 依次尝试三个免费源（HTTP 失败/解析失败/数值异常都换下一个），
/// 成功返回 (汇率, 来源名)；全部失败返回 Err 且不改动现有汇率值。
pub fn fetch_fx_rate() -> Result<(f64, String), String> {
    let mut last_err = String::new();
    for (source, url) in FX_RATE_SOURCES {
        match fetch_fx_rate_from(source, url) {
            Ok(rate) => {
                // 成功才落盘：更新汇率 + 获取时间 + 来源。读-改-写全程持锁，
                // 与 set_cursor_config 的全量写在锁上串行化，防毫秒窗口互相覆盖。
                let _guard = cursor_write_lock()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut cfg = load_cursor_config()?;
                cfg.usd_cny_rate = rate;
                cfg.fx_rate_fetched_at = Some(chrono::Utc::now().timestamp_millis());
                cfg.fx_rate_source = Some(source.to_string());
                write_cursor_config_unlocked(&cfg)?;
                return Ok((rate, source.to_string()));
            }
            Err(e) => last_err = e,
        }
    }
    Err(format!("所有汇率数据源均不可达: {last_err}"))
}

// ============================================================
// 宽松数值反序列化（Cursor API 部分数字序列化为字符串）
// ============================================================

fn opt_i64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => {
            Ok(n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)))
        }
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            Ok(s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f as i64)))
        }
        _ => Ok(None),
    }
}

fn opt_f64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_f64()),
        Some(serde_json::Value::String(s)) => Ok(s.trim().parse::<f64>().ok()),
        _ => Ok(None),
    }
}

fn opt_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(if s.is_empty() { None } else { Some(s) }),
        Some(n @ serde_json::Value::Number(_)) => Ok(Some(n.to_string())),
        _ => Ok(None),
    }
}

// ============================================================
// API 响应模型
// ============================================================

/// GET /api/usage-summary 的完整响应
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummary {
    #[serde(default)]
    billing_cycle_start: Option<String>,
    #[serde(default)]
    billing_cycle_end: Option<String>,
    #[serde(default)]
    membership_type: Option<String>,
    #[serde(default)]
    individual_usage: Option<IndividualUsage>,
    #[serde(default)]
    #[allow(dead_code)]
    team_usage: Option<TeamUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualUsage {
    #[serde(default)]
    plan: Option<PlanUsage>,
    #[serde(default)]
    on_demand: Option<OnDemandUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUsage {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "opt_i64")]
    used: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    limit: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    remaining: Option<i64>,
    #[serde(default, deserialize_with = "opt_f64")]
    auto_percent_used: Option<f64>,
    #[serde(default, deserialize_with = "opt_f64")]
    api_percent_used: Option<f64>,
    #[serde(default, deserialize_with = "opt_f64")]
    total_percent_used: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDemandUsage {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "opt_i64")]
    used: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    limit: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    remaining: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct TeamUsage {
    #[serde(default)]
    on_demand: Option<OnDemandUsage>,
    #[serde(default)]
    pooled: Option<OnDemandUsage>,
}

/// GET /api/auth/me 的响应
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthMe {
    #[serde(default, deserialize_with = "opt_string")]
    email: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    #[allow(dead_code)]
    sub: Option<String>,
}

/// events API 单页响应
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsPage {
    #[serde(default, deserialize_with = "opt_i64")]
    total_usage_events_count: Option<i64>,
    #[serde(default)]
    usage_events_display: Vec<UsageEvent>,
}

/// 单条 usage event
/// 注意：必须加 rename_all = "camelCase"，否则 tokenUsage/chargedCents 等
/// 多词字段会按 snake_case 匹配（token_usage），全部解析为 None。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageEvent {
    #[serde(default, deserialize_with = "opt_i64")]
    timestamp: Option<i64>,
    #[serde(default, deserialize_with = "opt_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    kind: Option<String>,
    #[serde(default)]
    token_usage: Option<EventTokenUsage>,
    /// 套餐实际扣费（美分）
    #[serde(default, deserialize_with = "opt_f64")]
    charged_cents: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventTokenUsage {
    #[serde(default, deserialize_with = "opt_i64")]
    input_tokens: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    output_tokens: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    cache_write_tokens: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64")]
    cache_read_tokens: Option<i64>,
    /// API 标价花费（美分）
    #[serde(default, deserialize_with = "opt_f64")]
    total_cents: Option<f64>,
}

// ============================================================
// 认证
// ============================================================

/// 定位 Cursor 的 state.vscdb 路径（跨平台）
fn cursor_db_path() -> Option<PathBuf> {
    // dirs::config_dir():
    //   Windows → %APPDATA% (Roaming)
    //   macOS   → ~/Library/Application Support
    //   Linux   → $XDG_CONFIG_HOME 或 ~/.config
    let base = dirs::config_dir()?;
    let p = base.join(CURSOR_DB_REL);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

/// 从 state.vscdb 读取 cursorAuth/accessToken
fn read_cursor_access_token() -> Result<String, String> {
    let path = cursor_db_path()
        .ok_or_else(|| "未找到 Cursor 应用数据库（state.vscdb），请确认 Cursor 已安装并登录".to_string())?;

    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打开 Cursor 数据库失败: {e}"))?;

    conn.busy_timeout(Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;

    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1",
            rusqlite::params!["cursorAuth/accessToken"],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let token = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "Cursor 数据库中未找到 accessToken，请确认已在 Cursor 应用中登录".to_string())?;

    // 兼容：部分版本 value 可能是 JSON 包裹 {"accessToken":"..."}
    if token.starts_with('{') {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&token) {
            if let Some(inner) = json.get("accessToken").and_then(|v| v.as_str()) {
                return Ok(inner.trim().to_string());
            }
        }
    }

    Ok(token)
}

/// base64url 解码（JWT payload 用）
fn b64url_decode(input: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input.trim())
        .or_else(|_| {
            // 尝试带 padding 的标准 url base64
            base64::engine::general_purpose::URL_SAFE.decode(input.trim())
        })
        .map_err(|e| format!("base64 解码失败: {e}"))
}

/// 解析 JWT payload 的 JSON
fn jwt_payload(token: &str) -> Result<serde_json::Value, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return Err("access token 不是有效的 JWT 格式".into());
    }
    let bytes = b64url_decode(parts[1])?;
    serde_json::from_slice(&bytes).map_err(|e| format!("解析 JWT payload 失败: {e}"))
}

/// 从 JWT 的 sub 字段提取 user ID（取 | 后的最后一段）
fn jwt_user_id(token: &str) -> Result<String, String> {
    let payload = jwt_payload(token)?;
    let sub = payload
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "JWT payload 中缺少 sub 字段".to_string())?;

    let user_id = sub
        .split('|')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(sub)
        .to_string();

    if user_id.is_empty() {
        return Err("JWT sub 字段无法提取 user ID".into());
    }
    Ok(user_id)
}

/// 检查 JWT 是否过期（留 60s 余量）
fn jwt_is_valid(token: &str) -> bool {
    if let Ok(payload) = jwt_payload(token) {
        if let Some(exp) = payload.get("exp").and_then(|v| v.as_i64()) {
            let now = chrono::Utc::now().timestamp();
            return exp > now + 60;
        }
    }
    // 无法解析 exp 时不阻拦，交给 API 判断
    true
}

/// 构建 Cursor 认证 cookie 头
fn build_cookie(user_id: &str, token: &str) -> String {
    format!("WorkosCursorSessionToken={user_id}%3A%3A{token}")
}

/// 解析出有效的 cookie 头。优先级：手动配置 > Cursor 应用本地 DB
pub fn resolve_cookie(cfg: &CursorConfig) -> Result<String, String> {
    // 手动模式：直接用配置中的 cookie
    if cfg.cookie_source == "manual" {
        let header = cfg.cookie_header.trim();
        if header.is_empty() {
            return Err("Cookie 来源为手动模式，但未配置 cookie。请编辑 ~/.zbar/cursor.json 填写 cookie_header 字段".into());
        }
        return Ok(header.to_string());
    }

    // 自动模式：读 Cursor 应用本地 DB
    let token = read_cursor_access_token()?;
    if !jwt_is_valid(&token) {
        return Err("Cursor 登录已过期，请在 Cursor 应用中重新登录".into());
    }
    let user_id = jwt_user_id(&token)?;
    Ok(build_cookie(&user_id, &token))
}

// ============================================================
// HTTP 请求
// ============================================================

/// 创建带超时的 HTTP agent
fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build()
}

/// GET /api/usage-summary
fn fetch_usage_summary(cookie: &str) -> Result<UsageSummary, String> {
    let url = format!("{CURSOR_BASE}/api/usage-summary");
    let resp = http_agent()
        .get(&url)
        .set("Cookie", cookie)
        .set("Accept", "application/json")
        .call();

    let resp = map_http_error(resp, "usage-summary")?;
    resp.into_json::<UsageSummary>()
        .map_err(|e| format!("解析 usage-summary 响应失败: {e}"))
}

/// GET /api/auth/me
fn fetch_auth_me(cookie: &str) -> Result<AuthMe, String> {
    let url = format!("{CURSOR_BASE}/api/auth/me");
    let resp = http_agent()
        .get(&url)
        .set("Cookie", cookie)
        .set("Accept", "application/json")
        .call();

    let resp = map_http_error(resp, "auth/me")?;
    resp.into_json::<AuthMe>()
        .map_err(|e| format!("解析 auth/me 响应失败: {e}"))
}

/// 将 ureq 错误映射为友好提示
fn map_http_error(
    result: Result<ureq::Response, ureq::Error>,
    ctx: &str,
) -> Result<ureq::Response, String> {
    match result {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(code, _)) => {
            if code == 401 {
                Err(format!("Cursor 未登录或会话已过期（{ctx} HTTP {code}），请在 Cursor 应用中重新登录"))
            } else {
                Err(format!("Cursor {ctx} 请求失败: HTTP {code}"))
            }
        }
        Err(e) => Err(format!("Cursor {ctx} 网络请求失败: {e}")),
    }
}

/// POST /api/dashboard/get-filtered-usage-events（分页拉取全部事件）
fn fetch_usage_events(cookie: &str, from_ms: i64, to_ms: i64) -> Result<Vec<UsageEvent>, String> {
    let url = format!("{CURSOR_BASE}/api/dashboard/get-filtered-usage-events");
    let agent = http_agent();

    let start_str = from_ms.to_string();
    let end_str = to_ms.to_string();

    let mut all_events: Vec<UsageEvent> = Vec::new();
    let mut expected_total: Option<i64> = None;

    for page in 1..=MAX_PAGES {
        let body = serde_json::json!({
            "page": page,
            "pageSize": PAGE_SIZE,
            "startDate": start_str,
            "endDate": end_str,
        });

        let resp = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .set("Cookie", cookie)
            // Cursor 对 POST 端点做 CSRF 校验，必须带匹配的 Origin
            .set("Origin", CURSOR_BASE)
            .send_json(body);

        let resp = map_http_error(resp, "events")?;
        let page_data: EventsPage = resp
            .into_json::<EventsPage>()
            .map_err(|e| format!("解析 events 响应失败: {e}"))?;

        if let Some(total) = page_data.total_usage_events_count {
            expected_total = Some(total);
        }

        let page_events = page_data.usage_events_display;
        let count = page_events.len();
        if count == 0 {
            break;
        }
        all_events.extend(page_events);

        // 不足一页 → 已到末尾
        if count < PAGE_SIZE {
            break;
        }
    }

    // 与 Cursor 报告的总数校验：少于期望数说明分页不完整
    if let Some(expected) = expected_total {
        if expected > 0 && (all_events.len() as i64) < expected {
            // 不阻断：返回已获取的部分（避免因边界重复条目导致永远报错）
            // 但在日志中记录差异（这里简单忽略，前端展示已获取数据）
        }
    }

    Ok(all_events)
}

// ============================================================
// 聚合
// ============================================================

/// 单日条目（供前端趋势图）
#[derive(Debug, Clone, Serialize)]
pub struct CursorDailyEntry {
    /// 日期标签 "08-13"（MM-DD）
    pub date: String,
    /// 当日 API 标价花费（美元）
    pub cost_usd: f64,
    /// 当日总 token
    pub total_tokens: i64,
    /// 当日请求数
    pub requests: i64,
}

/// 按模型聚合
#[derive(Debug, Clone, Serialize)]
pub struct CursorModelStat {
    pub model: String,
    pub cost_usd: f64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub requests: i64,
}

/// events 汇总
#[derive(Debug, Clone, Serialize)]
pub struct CursorEventsSummary {
    /// API 标价总花费（美元）
    pub total_cost_usd: f64,
    /// 套餐实际扣费（美元），None 表示部分事件缺 chargedCents
    pub metered_cost_usd: Option<f64>,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub requests: i64,
}

/// 把 events 聚合为（汇总, 每日明细, 按模型）
fn aggregate_events(events: &[UsageEvent]) -> (CursorEventsSummary, Vec<CursorDailyEntry>, Vec<CursorModelStat>) {
    let mut total_cost_usd = 0.0f64;
    let mut metered_ok = true;
    let mut metered_cents = 0.0f64;
    let mut saw_valid_event = false;
    let mut total_tokens = 0i64;
    let mut total_input = 0i64;
    let mut total_output = 0i64;
    let mut total_cache_read = 0i64;
    let mut total_requests = 0i64;

    // 按日聚合: full_date "2026-08-13" → (cost, tokens, requests)
    // 用完整日期做 key，避免跨年自定义范围 "01-01" 碰撞
    let mut daily: HashMap<String, (f64, i64, i64)> = HashMap::new();
    // 按模型聚合: model → (cost, tokens, input, output, cache_read, requests)
    let mut by_model: HashMap<String, (f64, i64, i64, i64, i64, i64)> = HashMap::new();

    for ev in events {
        let ts = match ev.timestamp {
            Some(t) if t > 0 => t,
            _ => continue,
        };
        saw_valid_event = true;

        // 计费花费：对所有有效事件累计 chargedCents（包括无 token 详情的计量事件）。
        // 对齐 CodexBar：任一有效事件缺 chargedCents → 总计不可靠，置 None。
        match ev.charged_cents {
            Some(c) if c >= 0.0 => metered_cents += c,
            None => metered_ok = false,
            _ => {}
        }

        // token 聚合：跳过无 token_usage 或全零的事件（对齐 CodexBar/ccusage）
        let usage = match &ev.token_usage {
            Some(u) => u,
            None => continue,
        };
        let inp = usage.input_tokens.unwrap_or(0).max(0);
        let out = usage.output_tokens.unwrap_or(0).max(0);
        let cw = usage.cache_write_tokens.unwrap_or(0).max(0);
        let cr = usage.cache_read_tokens.unwrap_or(0).max(0);
        let tokens = inp + out + cw + cr;
        if tokens == 0 {
            continue;
        }

        // 花费（API 标价，美分→美元）
        let cost = usage.total_cents.unwrap_or(0.0).max(0.0) / 100.0;
        total_cost_usd += cost;

        total_tokens += tokens;
        total_input += inp;
        total_output += out;
        total_cache_read += cr;
        total_requests += 1;

        // 按日（本地时区，完整日期 key）
        if let Some(dt) = chrono::Local.timestamp_millis_opt(ts).single() {
            let day_key = format!("{}", dt.format("%Y-%m-%d"));
            let entry = daily.entry(day_key).or_insert((0.0, 0, 0));
            entry.0 += cost;
            entry.1 += tokens;
            entry.2 += 1;
        }

        // 按模型
        let model = ev.model.clone().unwrap_or_else(|| "unknown".to_string());
        let m = by_model
            .entry(model)
            .or_insert((0.0, 0, 0, 0, 0, 0));
        m.0 += cost;
        m.1 += tokens;
        m.2 += inp;
        m.3 += out;
        m.4 += cr;
        m.5 += 1;
    }

    // 每日明细：按完整日期排序，显示标签截取 "MM-DD"
    let mut daily_vec: Vec<(String, CursorDailyEntry)> = daily
        .into_iter()
        .map(|(full_date, (cost, tokens, reqs))| {
            let display = if full_date.len() >= 10 {
                full_date[5..].to_string()
            } else {
                full_date.clone()
            };
            (
                full_date,
                CursorDailyEntry {
                    date: display,
                    cost_usd: cost,
                    total_tokens: tokens,
                    requests: reqs,
                },
            )
        })
        .collect();
    daily_vec.sort_by(|a, b| a.0.cmp(&b.0));
    let daily_vec: Vec<CursorDailyEntry> = daily_vec.into_iter().map(|(_, e)| e).collect();

    // 按模型按花费降序
    let mut model_vec: Vec<CursorModelStat> = by_model
        .into_iter()
        .map(|(model, (cost, tokens, inp, out, cr, reqs))| CursorModelStat {
            model,
            cost_usd: cost,
            total_tokens: tokens,
            input_tokens: inp,
            output_tokens: out,
            cache_read_tokens: cr,
            requests: reqs,
        })
        .collect();
    model_vec.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal));

    let summary = CursorEventsSummary {
        total_cost_usd,
        metered_cost_usd: if metered_ok && saw_valid_event {
            Some(metered_cents / 100.0)
        } else {
            None
        },
        total_tokens,
        input_tokens: total_input,
        output_tokens: total_output,
        cache_read_tokens: total_cache_read,
        requests: total_requests,
    };

    (summary, daily_vec, model_vec)
}

// ============================================================
// events 缓存
// ============================================================

/// 缓存槽位上限：前端每 180s 轮刷 today/1d/7d/30d 四个不同窗口，
/// 单槽会互相踢出（命中率≈0），多槽共存让各窗口独立命中；超上限淘汰最旧的。
const EVENTS_CACHE_SLOTS: usize = 8;

/// key → (事件列表, 拉取时间)。事件列表用 Arc 共享：
/// 命中路径只克隆引用计数，不再深拷贝几万条事件。
static EVENTS_CACHE: OnceLock<Mutex<HashMap<String, (Arc<Vec<UsageEvent>>, Instant)>>> =
    OnceLock::new();

fn events_cache() -> &'static Mutex<HashMap<String, (Arc<Vec<UsageEvent>>, Instant)>> {
    EVENTS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 将毫秒时间戳向下取整到 10 分钟刻度，作为缓存 key 的一部分。
/// 原因：today/1d/7d/30d 预设的 to_ms = Date.now()，每次调用都不同，
/// 直接用作 key 会导致缓存永远不命中。取整后同一 10 分钟窗口内复用缓存。
fn round_for_cache(ms: i64) -> i64 {
    const INTERVAL: i64 = 600_000; // 10 分钟
    (ms / INTERVAL) * INTERVAL
}

/// 带缓存的 events 拉取：相同时间窗口（取整后）在 TTL 内复用。
/// 返回 Arc 共享的事件列表（命中/未命中路径均无深拷贝），
/// 调用侧聚合函数吃 &[UsageEvent]，Arc deref 即可只读借用。
fn fetch_events_cached(
    cookie: &str,
    from_ms: i64,
    to_ms: i64,
) -> Result<Arc<Vec<UsageEvent>>, String> {
    // 取整后构建 key，避免 to_ms=Date.now() 导致每次 miss
    let key = format!(
        "{}|{}|{}",
        cookie,
        round_for_cache(from_ms),
        round_for_cache(to_ms)
    );
    {
        let cache = events_cache().lock().map_err(|e| format!("缓存锁失败: {e}"))?;
        if let Some((events, at)) = cache.get(&key) {
            if at.elapsed() < EVENTS_CACHE_TTL {
                // Arc clone（仅引用计数 +1，不复制底层数据）
                return Ok(Arc::clone(events));
            }
        }
    }

    let events = Arc::new(fetch_usage_events(cookie, from_ms, to_ms)?);

    let mut cache = events_cache().lock().map_err(|e| format!("缓存锁失败: {e}"))?;
    // 先清掉已过期的槽位，腾出空间的同时避免过期数据长期占内存
    cache.retain(|_, (_, at)| at.elapsed() < EVENTS_CACHE_TTL);
    // 槽位已满时淘汰最旧的（按拉取时间），保证多窗口共存
    if cache.len() >= EVENTS_CACHE_SLOTS && !cache.contains_key(&key) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, (Arc::clone(&events), Instant::now()));

    Ok(events)
}

// ============================================================
// 对外快照结构
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct CursorPlanInfo {
    pub enabled: Option<bool>,
    /// 已用（美分）
    pub used_cents: Option<i64>,
    /// 上限（美分）
    pub limit_cents: Option<i64>,
    pub remaining_cents: Option<i64>,
    pub total_pct: Option<f64>,
    pub auto_pct: Option<f64>,
    pub api_pct: Option<f64>,
}

/// Cursor 今天事件用量换算出的额度增量（百分比）。
///
/// Cursor 的 usage-summary 百分比可能长时间保持不变，但 events 接口会先
/// 更新实际扣费。这里仅用于今日增量历史，进度条仍使用 plan.auto_pct/api_pct。
#[derive(Debug, Clone, Serialize)]
pub struct CursorTodayQuota {
    pub auto_pct: Option<f64>,
    pub api_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CursorOnDemandInfo {
    pub enabled: Option<bool>,
    pub used_cents: Option<i64>,
    pub limit_cents: Option<i64>,
    pub remaining_cents: Option<i64>,
}

/// 前端使用的完整快照
#[derive(Debug, Clone, Serialize)]
pub struct CursorSnapshot {
    /// 是否成功登录并获取到数据
    pub logged_in: bool,
    pub error: Option<String>,
    /// events 拉取失败时的错误信息（套餐数据可能仍可用）
    pub events_error: Option<String>,
    pub account_email: Option<String>,
    pub account_name: Option<String>,
    pub membership_type: Option<String>,
    pub billing_cycle_start: Option<String>,
    pub billing_cycle_end: Option<String>,
    pub plan: Option<CursorPlanInfo>,
    pub on_demand: Option<CursorOnDemandInfo>,
    pub events: Option<CursorEventsSummary>,
    pub today_quota: Option<CursorTodayQuota>,
    pub daily: Vec<CursorDailyEntry>,
    pub by_model: Vec<CursorModelStat>,
    /// 最近使用的模型（口径：查询时间范围内最新一条带模型名的用量事件）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_model: Option<db::CurrentModelStat>,
}

fn local_day_start_ms() -> i64 {
    let now = Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|value| value.timestamp_millis())
        .unwrap_or_else(|| now.timestamp_millis())
}

fn cursor_event_bucket(kind: Option<&str>) -> Option<&'static str> {
    let kind = kind?.to_ascii_uppercase();
    if kind.contains("INCLUDED") {
        Some("auto")
    } else if kind.contains("ON_DEMAND") || kind.contains("API") {
        Some("api")
    } else {
        None
    }
}

fn daily_pct_from_cents(
    daily_cents: f64,
    cycle_used_cents: Option<i64>,
    cycle_used_pct: Option<f64>,
) -> Option<f64> {
    if daily_cents <= 0.0 {
        return None;
    }
    let used_cents = cycle_used_cents? as f64;
    let used_pct = cycle_used_pct?;
    if used_cents <= 0.0 || used_pct <= 0.0 {
        return None;
    }
    let cycle_limit_cents = used_cents * 100.0 / used_pct;
    if cycle_limit_cents <= 0.0 {
        return None;
    }
    Some((daily_cents / cycle_limit_cents * 100.0).clamp(0.0, 100.0))
}

fn calculate_today_quota(events: &[UsageEvent], plan: &CursorPlanInfo) -> CursorTodayQuota {
    let day_start = local_day_start_ms();
    let now = Local::now().timestamp_millis();
    let mut auto_cents = 0.0;
    let mut api_cents = 0.0;
    for event in events {
        let Some(ts) = event.timestamp else {
            continue;
        };
        if ts < day_start || ts > now {
            continue;
        }
        let Some(cents) = event.charged_cents.filter(|value| *value > 0.0) else {
            continue;
        };
        match cursor_event_bucket(event.kind.as_deref()) {
            Some("auto") => auto_cents += cents,
            Some("api") => api_cents += cents,
            _ => {}
        }
    }
    CursorTodayQuota {
        auto_pct: daily_pct_from_cents(auto_cents, plan.used_cents, plan.auto_pct),
        api_pct: daily_pct_from_cents(api_cents, plan.used_cents, plan.api_pct),
    }
}

/// 拉取 Cursor 完整用量快照（套餐 + events 明细）
pub fn fetch_cursor_snapshot(from_ms: i64, to_ms: i64) -> Result<CursorSnapshot, String> {
    let cfg = load_cursor_config()?;
    let cookie = resolve_cookie(&cfg)?;

    // 并行拉取三路数据（ureq 是同步的，顺序拉取）
    let summary = fetch_usage_summary(&cookie)?;
    let auth = fetch_auth_me(&cookie).unwrap_or_default();

    let plan = summary
        .individual_usage
        .as_ref()
        .and_then(|iu| iu.plan.as_ref())
        .map(|p| CursorPlanInfo {
            enabled: p.enabled,
            used_cents: p.used,
            limit_cents: p.limit,
            remaining_cents: p.remaining,
            total_pct: p.total_percent_used,
            auto_pct: p.auto_percent_used,
            api_pct: p.api_percent_used,
        });

    // events 可能较慢 / 失败，不阻断套餐展示，但透传错误信息
    let mut events_error: Option<String> = None;
    let range_events = fetch_events_cached(&cookie, from_ms, to_ms);
    let today_events = if from_ms <= local_day_start_ms() {
        range_events.as_ref().ok().cloned()
    } else {
        fetch_events_cached(&cookie, local_day_start_ms(), to_ms).ok()
    };
    let today_quota = today_events
        .as_deref()
        .and_then(|events| plan.as_ref().map(|quota| calculate_today_quota(events, quota)));
    // 最近使用模型：范围内最新一条带模型名的有效事件（须在 match 移动 range_events 前计算）
    let current_model = range_events.as_ref().ok().and_then(|events| {
        events
            .iter()
            .filter_map(|ev| {
                let ts = ev.timestamp.filter(|t| *t > 0)?;
                let model = ev.model.as_deref().filter(|m| !m.is_empty())?;
                Some((ts, model))
            })
            .max_by_key(|(ts, _)| *ts)
            .map(|(ts, model)| db::CurrentModelStat {
                model_id: model.to_string(),
                provider_id: "cursor".into(),
                last_used_ms: ts,
            })
    });
    let (events_summary, daily, by_model) = match range_events {
        Ok(events) => {
            if events.is_empty() {
                (None, Vec::new(), Vec::new())
            } else {
                let (s, d, m) = aggregate_events(&events);
                (Some(s), d, m)
            }
        }
        Err(ref e) => {
            // events 失败不阻断套餐展示，但记录错误供前端区分"无数据"和"拉取失败"
            events_error = Some(e.clone());
            (None, Vec::new(), Vec::new())
        }
    };

    let on_demand = summary.individual_usage.as_ref().and_then(|iu| iu.on_demand.as_ref()).map(|od| {
        CursorOnDemandInfo {
            enabled: od.enabled,
            used_cents: od.used,
            limit_cents: od.limit,
            remaining_cents: od.remaining,
        }
    });

    Ok(CursorSnapshot {
        logged_in: true,
        error: None,
        events_error,
        account_email: auth.email,
        account_name: auth.name,
        membership_type: summary.membership_type.clone(),
        billing_cycle_start: summary.billing_cycle_start,
        billing_cycle_end: summary.billing_cycle_end,
        plan,
        on_demand,
        events: events_summary,
        today_quota,
        daily,
        by_model,
        current_model,
    })
}

/// 轻量拉取 Cursor 用量合计（菜单栏标题合并用）。
/// 只走 events（带 120s 缓存），不拉 summary/auth，避免 30s 定时器狂发请求。
/// 返回 (API 标价花费 USD, 总 token)。未配置/未登录/网络失败时返回 Err，调用方静默降级。
pub fn fetch_cursor_usage_totals(from_ms: i64, to_ms: i64) -> Result<(f64, i64), String> {
    let cfg = load_cursor_config()?;
    let cookie = resolve_cookie(&cfg)?;
    let events = fetch_events_cached(&cookie, from_ms, to_ms)?;
    let (summary, _, _) = aggregate_events(&events);
    Ok((summary.total_cost_usd, summary.total_tokens))
}

/// 按指定周期聚合 Cursor Token。
/// Cursor events 接口一次返回原始事件，因此在这里按真实事件时间戳切分，
/// 避免前端只能拿到按日明细时把跨重置日的 Token 错分到某个周期。
pub fn fetch_cursor_period_buckets(
    from_ms: i64,
    to_ms: i64,
    periods: &[(i64, i64)],
) -> Result<Vec<crate::db::PeriodBucket>, String> {
    if periods.is_empty() {
        return Ok(Vec::new());
    }
    let cfg = load_cursor_config()?;
    let cookie = resolve_cookie(&cfg)?;
    let events = fetch_events_cached(&cookie, from_ms, to_ms)?;
    let mut buckets: Vec<crate::db::PeriodBucket> = periods
        .iter()
        .map(|&(reset_at, end_at)| crate::db::PeriodBucket {
            reset_at,
            end_at,
            total_tokens: 0,
            requests: 0,
        })
        .collect();

    for event in events.iter() {
        let ts = match event.timestamp {
            Some(value) if value > 0 => value,
            _ => continue,
        };
        let usage = match &event.token_usage {
            Some(value) => value,
            None => continue,
        };
        let tokens = usage.input_tokens.unwrap_or(0).max(0)
            + usage.output_tokens.unwrap_or(0).max(0)
            + usage.cache_write_tokens.unwrap_or(0).max(0)
            + usage.cache_read_tokens.unwrap_or(0).max(0);
        if tokens <= 0 {
            continue;
        }
        if let Some(index) = periods
            .iter()
            .position(|&(reset_at, end_at)| ts >= reset_at && ts < end_at)
        {
            buckets[index].total_tokens += tokens;
            buckets[index].requests += 1;
        }
    }
    Ok(buckets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_maps_to_cursor_quota_window() {
        assert_eq!(
            cursor_event_bucket(Some("USAGE_EVENT_KIND_INCLUDED_IN_PRO")),
            Some("auto")
        );
        assert_eq!(
            cursor_event_bucket(Some("USAGE_EVENT_KIND_ON_DEMAND")),
            Some("api")
        );
        assert_eq!(cursor_event_bucket(Some("OTHER_EVENT")), None);
    }

    #[test]
    fn today_quota_uses_positive_events_and_plan_denominator() {
        let now = Local::now().timestamp_millis();
        let events = vec![
            UsageEvent {
                timestamp: Some(now),
                model: None,
                kind: Some("USAGE_EVENT_KIND_INCLUDED_IN_PRO".into()),
                token_usage: None,
                charged_cents: Some(412.414),
            },
            UsageEvent {
                timestamp: Some(now),
                model: None,
                kind: Some("USAGE_EVENT_KIND_ON_DEMAND".into()),
                token_usage: None,
                charged_cents: Some(10.0),
            },
            UsageEvent {
                timestamp: Some(now),
                model: None,
                kind: Some("OTHER_EVENT".into()),
                token_usage: None,
                charged_cents: Some(999.0),
            },
        ];
        let plan = CursorPlanInfo {
            enabled: Some(true),
            used_cents: Some(1192),
            limit_cents: Some(30000),
            remaining_cents: Some(28808),
            total_pct: Some(3.9733333333333336),
            auto_pct: Some(3.9733333333333336),
            api_pct: Some(2.0),
        };

        let quota = calculate_today_quota(&events, &plan);
        assert!((quota.auto_pct.unwrap_or_default() - 1.3747).abs() < 0.01);
        assert!((quota.api_pct.unwrap_or_default() - 0.0168).abs() < 0.01);
    }
}

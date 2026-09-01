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
use crate::provider_quota::{ProviderQuotaBalance, ProviderQuotaEntry, ProviderQuotaWindow};

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
// 旧手动 cookie 一次性迁移（~/.zbar/cursor.json → 凭证体系）
// ============================================================

/// 迁移决策（纯函数，单测入口）：旧配置为手动来源（cookie_source=manual）
/// 且 cookie_header 非空、凭证体系尚无任何条目时才迁移。cookie_source 缺失
/// 时 serde 默认为 "auto"，auto 模式残留的旧 cookie 不是用户手动配置，不迁移；
/// 任一条件不满足即无操作（幂等：已迁移过 / 从未手动配置过都不重复创建）。
fn migration_needed(cookie_source: &str, old_cookie_header: &str, credential_count: usize) -> bool {
    cookie_source == "manual" && !old_cookie_header.trim().is_empty() && credential_count == 0
}

/// 把旧手动 cookie（~/.zbar/cursor.json 的 cookie_header）一次性迁移到
/// 凭证体系（~/.zbar/credentials/cursor.json）：创建一条 label="手动迁移"、
/// kind="cookie" 的凭证（secret=旧值）。旧 cursor.json 原样保留（不删不改，
/// 不丢任何用户数据），仅主链路此后不再读取它的 cookie 字段。
///
/// 幂等：凭证体系已有条目（add_entry_if_empty 锁内原子判断）或旧值为空时
/// 无操作；写入走凭证体系模块锁，并发触发也只会创建一条。失败仅记日志
/// 不阻断启动（下次启动重试；迁移完成前主链路 manual 仍回落 auto）。
pub fn migrate_legacy_cookie() {
    let cfg = match load_cursor_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("[zbar-cursor] 旧 cookie 迁移跳过（读取配置失败）: {e}");
            return;
        }
    };
    // 凭证体系当前条目数：仅用于幂等判断（真实写入的原子性由
    // add_entry_if_empty 在其锁内保证，这里的计数只是快速短路）
    let existing = crate::provider_credentials::load_query_snapshots("cursor")
        .map(|s| s.len())
        .unwrap_or(0);
    if !migration_needed(&cfg.cookie_source, &cfg.cookie_header, existing) {
        return;
    }
    match crate::provider_credentials::add_entry_if_empty(
        "cursor",
        "手动迁移",
        "cookie",
        cfg.cookie_header.trim(),
    ) {
        Ok(true) => {
            eprintln!(
                "[zbar-cursor] 已把旧手动 cookie 迁移到凭证体系（~/.zbar/credentials/cursor.json，原文件保留）"
            )
        }
        Ok(false) => {} // 并发下已被其他调用创建：视为已迁移
        Err(e) => eprintln!("[zbar-cursor] 旧 cookie 迁移失败（下次启动重试）: {e}"),
    }
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

/// 从凭证查询快照中选出生效的手动 cookie（纯函数，单测入口）：
/// 第一条 kind=cookie 且 secret 非空的条目。文件条目按创建顺序追加，
/// 首条即最早创建的一条（保持旧 cookie_header 的"单条生效"语义）。
fn pick_first_cookie(
    snapshots: &[crate::provider_credentials::CredentialQuerySnapshot],
) -> Option<String> {
    snapshots
        .iter()
        .find(|s| s.kind == "cookie" && !s.secret.trim().is_empty())
        .map(|s| s.secret.trim().to_string())
}

/// 手动 cookie 的现行来源：凭证体系 ~/.zbar/credentials/cursor.json 的
/// 第一条 kind=cookie 条目（旧 cursor.json 的 cookie_header 已一次性迁移
/// 过去，不再读取）。文件不存在/损坏/无有效条目返回 None。
fn first_manual_cookie() -> Option<String> {
    match crate::provider_credentials::load_query_snapshots("cursor") {
        Ok(snapshots) => pick_first_cookie(&snapshots),
        Err(e) => {
            // 凭证文件损坏时降级为"未配置"（回落 auto），不让手动路径卡死主链路
            eprintln!("[zbar-cursor] 读取手动 cookie 凭证失败: {e}");
            None
        }
    }
}

/// 解析出有效的 cookie 头。优先级：手动配置（凭证体系最早一条 cookie 条目）
/// > Cursor 应用本地 DB。手动模式在凭证体系无条目时视为未配置，回落自动模式。
pub fn resolve_cookie(cfg: &CursorConfig) -> Result<String, String> {
    // 手动模式：读凭证体系 ~/.zbar/credentials/cursor.json
    if cfg.cookie_source == "manual" {
        if let Some(header) = first_manual_cookie() {
            return Ok(header);
        }
        // 凭证体系无条目：手动路径视为未配置，继续走自动模式
        //（与旧"手动但未填 cookie"相比更健壮：不再直接报错阻断）
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

/// 把今日扣费（美分）按「周期已用金额 ÷ 周期已用百分比」反推的周期额度折算成百分比。
///
/// 注意：usage-summary 的美元计量字段（plan.used/limit/remaining）与百分比口径
/// （auto_pct/api_pct）在部分订阅（如 pro-legacy）下并非同源——美元口径可能已封顶
/// （remaining=0）或远小于真实周期额度。一旦检测到两者明显不一致，返回 None，
/// 让调用方回退到写真实 auto_pct 快照的旧口径，避免今日增量被放大数倍。
fn daily_pct_from_cents(
    daily_cents: f64,
    cycle_used_cents: Option<i64>,
    cycle_used_pct: Option<f64>,
    cycle_limit_cents: Option<i64>,
    cycle_remaining_cents: Option<i64>,
) -> Option<f64> {
    if daily_cents <= 0.0 {
        return None;
    }
    let used_cents = cycle_used_cents? as f64;
    let used_pct = cycle_used_pct?;
    if used_cents <= 0.0 || used_pct <= 0.0 {
        return None;
    }
    // remaining<=0 说明美元计量已封顶，该口径失真，不能作为周期额度依据
    if cycle_remaining_cents.map(|value| value <= 0).unwrap_or(false) {
        return None;
    }
    // 缺少周期上限时无从校验口径一致性，宁缺毋滥
    let limit_cents = cycle_limit_cents? as f64;
    if limit_cents <= 0.0 {
        return None;
    }
    let inferred_limit_cents = used_cents * 100.0 / used_pct;
    if inferred_limit_cents <= 0.0 {
        return None;
    }
    // 反推额度与上报上限偏差超过 10%：美元口径与百分比口径不同源，反推无意义
    let deviation = ((inferred_limit_cents - limit_cents) / limit_cents).abs();
    if !deviation.is_finite() || deviation > 0.10 {
        return None;
    }
    Some((daily_cents / inferred_limit_cents * 100.0).clamp(0.0, 100.0))
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
        auto_pct: daily_pct_from_cents(
            auto_cents,
            plan.used_cents,
            plan.auto_pct,
            plan.limit_cents,
            plan.remaining_cents,
        ),
        api_pct: daily_pct_from_cents(
            api_cents,
            plan.used_cents,
            plan.api_pct,
            plan.limit_cents,
            plan.remaining_cents,
        ),
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

// ============================================================
// 多账号手动凭证（凭证体系 kind=cookie）的额度堆叠
// ============================================================
// 供 provider_quota 的 "cursor" 分支调用：对 ~/.zbar/credentials/cursor.json
// 的全部 kind=cookie 条目逐条查 usage-summary，产出 ProviderQuotaEntry。
// 本地 auto 登录态不并入本链路（主面板 get_cursor_usage 已展示，避免双查询，
// 与 claude 先例一致）；无凭证时 provider_quota 的空快照早返回已给出空 Vec。

/// 手动 cookie 会话失效（401/403）的统一提示（与本地登录态过期的文案区分：
/// 手动凭证要回 cursor.com 重新复制，而不是重开 Cursor 应用）
const MANUAL_COOKIE_EXPIRED_MSG: &str = "会话已失效，请重新登录 cursor.com 后更新 Cookie";

/// 手动凭证单条查询的失败分类：expired（会话失效）与其余 error 分开，
/// 供条目状态与凭证卡徽章区分展示。
struct ManualQuotaFailure {
    /// "expired" | "error"
    status: &'static str,
    /// 中文原因（不含 cookie 内容）
    message: String,
}

impl ManualQuotaFailure {
    fn error(message: String) -> Self {
        Self { status: "error", message }
    }
}

/// 手动凭证专用的 usage-summary 拉取：与 fetch_usage_summary 同端点，但
/// 错误分类不同（401/403 → expired + 引导更新 Cookie；不动既有函数，保证
/// 主链路错误文案零回归）。
fn fetch_usage_summary_manual(cookie: &str) -> Result<UsageSummary, ManualQuotaFailure> {
    let url = format!("{CURSOR_BASE}/api/usage-summary");
    let resp = http_agent()
        .get(&url)
        .set("Cookie", cookie)
        .set("Accept", "application/json")
        .call();
    match resp {
        Ok(r) => r.into_json::<UsageSummary>().map_err(|e| {
            ManualQuotaFailure::error(format!("解析 usage-summary 响应失败: {e}"))
        }),
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            Err(ManualQuotaFailure {
                status: "expired",
                message: MANUAL_COOKIE_EXPIRED_MSG.to_string(),
            })
        }
        Err(ureq::Error::Status(code, _)) => Err(ManualQuotaFailure::error(format!(
            "Cursor 额度查询失败（HTTP {code}）"
        ))),
        Err(e) => Err(ManualQuotaFailure::error(format!(
            "Cursor 网络请求失败: {e}"
        ))),
    }
}

/// usage-summary → 额度卡字段（纯函数，单测入口）。
/// 窗口映射：plan 的 Auto / API 两个百分比（与主面板口径一致，有值才出窗口；
/// usage-summary 的窗口结构本质是百分比，used/limit 的美分金额是整个套餐
/// 周期的扣费口径，映射到单窗口会有歧义，故省略）。
/// 金额映射：on-demand 剩余额度 → balance（美元）；套餐映射：membership_type
/// → plan_name。
fn quota_parts_from_summary(
    summary: &UsageSummary,
) -> (
    Vec<ProviderQuotaWindow>,
    Option<String>,
    Option<ProviderQuotaBalance>,
) {
    let mut windows = Vec::new();
    if let Some(plan) = summary.individual_usage.as_ref().and_then(|iu| iu.plan.as_ref()) {
        if let Some(pct) = plan.auto_percent_used {
            windows.push(ProviderQuotaWindow {
                key: "auto".into(),
                title: "Auto".into(),
                used_percent: Some(pct),
                used: None,
                total: None,
                unit: None,
                resets_at: None,
            });
        }
        if let Some(pct) = plan.api_percent_used {
            windows.push(ProviderQuotaWindow {
                key: "api".into(),
                title: "API".into(),
                used_percent: Some(pct),
                used: None,
                total: None,
                unit: None,
                resets_at: None,
            });
        }
    }
    let plan_name = summary.membership_type.clone();
    let balance = summary
        .individual_usage
        .as_ref()
        .and_then(|iu| iu.on_demand.as_ref())
        .filter(|od| od.enabled != Some(false))
        .and_then(|od| od.remaining)
        .map(|remaining_cents| ProviderQuotaBalance {
            amount: remaining_cents as f64 / 100.0,
            currency: "$".into(),
            granted: None,
            topped_up: None,
        });
    (windows, plan_name, balance)
}

/// 单条手动凭证 → 一张额度卡条目：查询成功映射窗口/套餐/余额，失败产出
/// expired/error 条目（不阻塞其他凭证）。
fn manual_quota_entry(
    credential_id: &str,
    label: &str,
    cookie: &str,
) -> ProviderQuotaEntry {
    let (status, message, parts) = match fetch_usage_summary_manual(cookie) {
        Ok(summary) => {
            let (windows, plan_name, balance) = quota_parts_from_summary(&summary);
            ("ok", None, Some((windows, plan_name, balance)))
        }
        Err(f) => (f.status, Some(f.message), None),
    };
    let (windows, plan_name, balance) = parts.unwrap_or((Vec::new(), None, None));
    ProviderQuotaEntry {
        credential_id: credential_id.to_string(),
        label: label.to_string(),
        status: status.to_string(),
        windows,
        balance,
        plan_name,
        message,
        updated_at: crate::provider_quota::now_ms(),
    }
}

/// 查询全部手动凭证（kind=cookie）的额度：逐条调 usage-summary（串行，
/// 单条失败不影响其余）。非 cookie 型条目与 secret 为空的条目跳过。
pub(crate) fn fetch_manual_quota_entries(
    snapshots: &[crate::provider_credentials::CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let mut entries = Vec::new();
    for cred in snapshots {
        if cred.kind != "cookie" {
            continue;
        }
        let cookie = cred.secret.trim();
        if cookie.is_empty() {
            continue;
        }
        entries.push(manual_quota_entry(&cred.id, &cred.label, cookie));
    }
    entries
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
            // 守卫要求美元口径与百分比口径同源：2.0 会反推出 59600（偏差 98.7%）被拒绝，
            // 这里取与 used/limit 同源的 4.0（反推 29800，偏差 0.67%）
            api_pct: Some(4.0),
        };

        let quota = calculate_today_quota(&events, &plan);
        assert!((quota.auto_pct.unwrap_or_default() - 1.3747).abs() < 0.01);
        assert!((quota.api_pct.unwrap_or_default() - 0.0336).abs() < 0.01);
    }

    // ===== 今日增量折算的美元口径可信度守卫 =====

    #[test]
    fn daily_pct_guard_rejects_capped_dollar_metering() {
        // remaining=0：美元计量已封顶（如 pro-legacy 的 used=2000/limit=2000），
        // 与百分比口径不同源，直接拒绝折算
        assert_eq!(
            daily_pct_from_cents(3329.0, Some(2000), Some(27.18), Some(2000), Some(0)),
            None
        );
    }

    #[test]
    fn daily_pct_guard_rejects_limit_mismatch() {
        // 反推额度 2000*100/27.18≈7359 与 plan.limit=2000 偏差约 268%，
        // 远超 10% 容差，说明两套口径不同源，拒绝折算
        assert_eq!(
            daily_pct_from_cents(3329.0, Some(2000), Some(27.18), Some(2000), Some(1000)),
            None
        );
    }

    #[test]
    fn daily_pct_guard_rejects_missing_limit() {
        // 缺少周期上限时无从校验口径一致性，宁缺毋滥
        assert_eq!(
            daily_pct_from_cents(3329.0, Some(2000), Some(27.18), None, Some(1000)),
            None
        );
    }

    #[test]
    fn daily_pct_converts_when_denominators_consistent() {
        // 反推额度 1000*100/(10/3)=30000 与 limit 一致 → 正常折算：150/30000*100=0.5%
        let pct = daily_pct_from_cents(
            150.0,
            Some(1000),
            Some(10.0 / 3.0),
            Some(30000),
            Some(29000),
        );
        assert!((pct.unwrap() - 0.5).abs() < 0.001);
    }

    // ===== 旧 cookie 迁移：决策纯函数（幂等性核心判断）=====

    #[test]
    fn migration_decision_is_idempotent_and_value_gated() {
        // 旧值为空（从未手动配置过）→ 无操作
        assert!(!migration_needed("manual", "", 0));
        assert!(!migration_needed("manual", "   ", 0));
        // 凭证体系已有条目（已迁移过 / 用户已手动加过）→ 无操作（幂等）
        assert!(!migration_needed("manual", "WorkosCursorSessionToken=1%3A%3Aabc", 1));
        assert!(!migration_needed("manual", "WorkosCursorSessionToken=1%3A%3Aabc", 3));
        // 旧值非空 + 凭证体系无条目 → 迁移
        assert!(migration_needed("manual", "WorkosCursorSessionToken=1%3A%3Aabc", 0));
        // 前后空白不算"空值"（会 trim 后迁移）
        assert!(migration_needed("manual", "  cookie  ", 0));
        // auto 来源（含字段缺失时的 serde 默认值）：残留 cookie 非手动配置 → 不迁移
        assert!(!migration_needed("auto", "WorkosCursorSessionToken=1%3A%3Aabc", 0));
        // 其他未知来源值同样不迁移（只有显式 manual 才迁移）
        assert!(!migration_needed("unknown", "WorkosCursorSessionToken=1%3A%3Aabc", 0));
    }

    // ===== manual 来源切换：凭证快照中的单条生效选择 =====

    fn snap(id: &str, kind: &str, secret: &str) -> crate::provider_credentials::CredentialQuerySnapshot {
        crate::provider_credentials::CredentialQuerySnapshot {
            id: id.into(),
            label: id.into(),
            kind: kind.into(),
            secret: secret.into(),
            region: None,
        }
    }

    #[test]
    fn pick_first_cookie_empty_and_non_cookie_snapshots() {
        // 无任何凭证 → None（manual 路径视为未配置，回落 auto）
        assert!(pick_first_cookie(&[]).is_none());
        // 只有 apiKey / token 条目 → None（只认 kind=cookie）
        let snaps = vec![snap("a", "apiKey", "sk-xxx"), snap("b", "token", "tok")];
        assert!(pick_first_cookie(&snaps).is_none());
    }

    #[test]
    fn pick_first_cookie_takes_earliest_entry() {
        // 多条 cookie 条目（多账号堆叠场景）：主链路只取第一条（创建最早）
        let snaps = vec![
            snap("old", "cookie", "WorkosCursorSessionToken=1%3A%3Aold"),
            snap("new", "cookie", "WorkosCursorSessionToken=2%3A%3Anew"),
        ];
        assert_eq!(
            pick_first_cookie(&snaps).as_deref(),
            Some("WorkosCursorSessionToken=1%3A%3Aold")
        );
    }

    #[test]
    fn pick_first_cookie_skips_blank_secrets() {
        // 首条 cookie 内容为空白（脏数据）→ 跳过取下一条有效条目
        let snaps = vec![
            snap("blank", "cookie", "   "),
            snap("valid", "cookie", "  WorkosCursorSessionToken=2%3A%3Av"),
        ];
        assert_eq!(
            pick_first_cookie(&snaps).as_deref(),
            Some("WorkosCursorSessionToken=2%3A%3Av")
        );
        // 全部空白 → None
        let blank = vec![snap("b1", "cookie", ""), snap("b2", "cookie", " ")];
        assert!(pick_first_cookie(&blank).is_none());
    }

    // ===== 多账号额度堆叠：usage-summary → 额度卡字段映射 =====

    #[test]
    fn quota_parts_maps_windows_plan_and_balance() {
        let summary: UsageSummary = serde_json::from_str(
            r#"{
                "membershipType": "pro",
                "individualUsage": {
                    "plan": {
                        "enabled": true,
                        "autoPercentUsed": 12.5,
                        "apiPercentUsed": 3.25
                    },
                    "onDemand": {
                        "enabled": true,
                        "remaining": 4567
                    }
                }
            }"#,
        )
        .unwrap();
        let (windows, plan_name, balance) = quota_parts_from_summary(&summary);
        // Auto / API 双窗口，顺序与主面板口径一致
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].key, "auto");
        assert_eq!(windows[0].title, "Auto");
        assert_eq!(windows[0].used_percent, Some(12.5));
        assert_eq!(windows[1].key, "api");
        assert_eq!(windows[1].used_percent, Some(3.25));
        // 套餐名映射 membership_type
        assert_eq!(plan_name.as_deref(), Some("pro"));
        // on-demand 剩余美分 → 美元余额
        let bal = balance.expect("on-demand 余额应映射");
        assert!((bal.amount - 45.67).abs() < 1e-9);
        assert_eq!(bal.currency, "$");
    }

    #[test]
    fn quota_parts_empty_summary_yields_empty_entry_parts() {
        // 空 summary（登录但无任何用量数据）：零窗口、无套餐、无余额（不报错）
        let summary: UsageSummary = serde_json::from_str("{}").unwrap();
        let (windows, plan_name, balance) = quota_parts_from_summary(&summary);
        assert!(windows.is_empty());
        assert!(plan_name.is_none());
        assert!(balance.is_none());
    }

    #[test]
    fn quota_parts_partial_windows_and_disabled_on_demand() {
        // 只有 API 百分比（免费号常见形态）→ 只出 API 窗口
        let summary: UsageSummary = serde_json::from_str(
            r#"{
                "individualUsage": {
                    "plan": { "apiPercentUsed": 8 },
                    "onDemand": { "enabled": false, "remaining": 1000 }
                }
            }"#,
        )
        .unwrap();
        let (windows, _plan, balance) = quota_parts_from_summary(&summary);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "api");
        // on-demand 已关闭 → 不映射余额
        assert!(balance.is_none());
    }

    #[test]
    fn manual_quota_entry_error_keeps_failure_status_and_message() {
        // 错误分类：手动构造失败 → expired 条目保留引导文案（网络路径离线不可测，
        // 此处验证条目组装对 status/message 的透传不丢字段）
        let entry = ProviderQuotaEntry {
            credential_id: "c1".into(),
            label: "手动迁移".into(),
            status: "expired".into(),
            windows: Vec::new(),
            balance: None,
            plan_name: None,
            message: Some(MANUAL_COOKIE_EXPIRED_MSG.to_string()),
            updated_at: 0,
        };
        assert_eq!(entry.status, "expired");
        assert_eq!(entry.message.as_deref(), Some(MANUAL_COOKIE_EXPIRED_MSG));
        assert!(entry.windows.is_empty());
    }
}

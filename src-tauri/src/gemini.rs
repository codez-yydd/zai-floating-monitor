//! Gemini CLI（Google Code Assist）配额查询模块。
//!
//! 本地凭证型：直读 `~/.gemini/oauth_creds.json`（gemini CLI 的 OAuth 个人
//! 登录态），不进凭证体系、无需用户填 key。`ZBAR_GEMINI_HOME` 可覆盖根目录
//! （测试/便携场景，对齐 opencodego.rs 的 ZBAR_OPENCODE_HOME 先例）。
//!
//! 查询链路（对齐 CodexBar GeminiStatusProbe / gemini-cli 官方行为）：
//! 1. settings.json 的 security.auth.selectedType 显式为 api-key /
//!    vertex-ai → pending 条目（不支持非 OAuth 认证方式）；
//! 2. access_token 缺失或 expiry_date 过期 → POST oauth2.googleapis.com/token
//!    刷新（client_id/secret 优先从本机 gemini-cli 安装的 oauth2.js 提取，
//!    失败用内置常量），成功后原子写回 oauth_creds.json；
//! 3. loadCodeAssist 拿 cloudaicompanionProject；ineligibleTiers 出现
//!    UNSUPPORTED_CLIENT → 「账号不支持 Code Assist」；
//! 4. 无 project → 回退 cloudresourcemanager 项目列表选 gen-lang-client*；
//! 5. retrieveUserQuota 拿 buckets（modelId + remainingFraction + resetTime），
//!    同模型取最低 remainingFraction，Pro 主窗口 / Flash 副窗口。
//!
//! 工程纪律（对齐 provider_quota.rs / opencodego.rs）：
//! - 网络：ureq 同步请求 + 15s 超时 + codex::resolve_proxy，调用方
//!   spawn_blocking；刷新写回用 tmp+rename 原子写（provider_credentials 惯例）；
//! - 安全：错误消息中文且不含 token/client_secret 片段；凭证只在 Rust 内部
//!   构造鉴权头，永不下发前端、不进日志。

use crate::provider_quota::{
    flatten_response, now_ms, quota_http_agent, ProviderQuotaEntry, ProviderQuotaWindow,
};
use std::path::PathBuf;
use std::sync::OnceLock;

/// OAuth 刷新端点（Google 公共）。
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// Code Assist 状态/配额端点（gemini-cli 同款私有 API）。
const LOAD_CODE_ASSIST_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
/// 项目发现回退端点（loadCodeAssist 无 project 时用）。
const PROJECTS_ENDPOINT: &str = "https://cloudresourcemanager.googleapis.com/v1/projects";

/// 内置 OAuth 客户端（兜底常量，仅在本机提取不到 oauth2.js 时使用）。
/// 来源：google-gemini/gemini-cli 开源仓库（Apache-2.0）
/// packages/core/src/code_assist/oauth2.ts 公开发布的 installed application
/// 客户端——该类型客户端的 secret 按 Google 文档不作为机密处理，
/// gemini-cli 官方仓库本身将其直接提交在源码中。
const BUILTIN_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const BUILTIN_CLIENT_SECRET: &str = "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";

/// Gemini 配置根目录（ZBAR_GEMINI_HOME 优先，其次 ~/.gemini/）。
fn gemini_home() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_GEMINI_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gemini")
}

/// 本地登录态是否可用（provider_credentials::has_credentials 特判用）：
/// oauth_creds.json 存在即视为已装/已登录，tab 自动出现。
pub(crate) fn has_local_data() -> bool {
    gemini_home().join("oauth_creds.json").exists()
}

// ============================================================
// 纯函数：凭证解析 / JWT / 判定（单测不联网）
// ============================================================

/// oauth_creds.json 的内存形态（字段全部可选，脏文件不致命）。
#[derive(Debug, Default, Clone, PartialEq)]
struct OAuthCreds {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    /// epoch 毫秒
    expiry_date_ms: Option<i64>,
}

/// 解析 oauth_creds.json 内容（纯函数，坏 JSON / 非对象 → None，调用方按
/// 未登录处理）。
fn parse_creds(content: &str) -> Option<OAuthCreds> {
    let v = serde_json::from_str::<serde_json::Value>(content).ok()?;
    if !v.is_object() {
        return None;
    }
    let non_empty = |key: &str| {
        v.get(key)
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(OAuthCreds {
        access_token: non_empty("access_token"),
        refresh_token: non_empty("refresh_token"),
        id_token: non_empty("id_token"),
        expiry_date_ms: v.get("expiry_date").and_then(|n| n.as_f64()).map(|ms| ms as i64),
    })
}

/// 是否需要刷新（纯函数，now 注入）：access_token 缺失，或 expiry_date
/// 存在且已到期（缺失 expiry 不触发刷新，与 gemini-cli/CodexBar 口径一致）。
fn needs_refresh(creds: &OAuthCreds, now: i64) -> bool {
    if creds.access_token.is_none() {
        return true;
    }
    match creds.expiry_date_ms {
        Some(expiry) => expiry <= now,
        None => false,
    }
}

/// settings.json 是否显式配置了不支持的认证方式（api-key / gemini-api-key /
/// vertex-ai）。security.auth.selectedType 缺失或解析失败 → false（按 OAuth
/// 尝试，不阻断）。
fn auth_type_unsupported(settings: &serde_json::Value) -> bool {
    settings
        .get("security")
        .and_then(|s| s.get("auth"))
        .and_then(|a| a.get("selectedType"))
        .and_then(|t| t.as_str())
        .map(|t| matches!(t.trim(), "api-key" | "gemini-api-key" | "vertex-ai"))
        .unwrap_or(false)
}

/// base64url 解码（JWT payload 用；不足位补 padding）。
fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let trimmed = input.trim();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(trimmed))
        .ok()
}

/// 从 id_token（JWT）payload 解出 email（不验签，仅展示用；损坏/缺 email
/// 返回 None，不阻断查询）。
fn decode_jwt_email(id_token: Option<&str>) -> Option<String> {
    let token = id_token?.trim();
    let payload = token.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    let v = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    v.get("email")
        .and_then(|e| e.as_str())
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
}

/// loadCodeAssist 响应里关心的两个字段。
#[derive(Debug, Default)]
struct CodeAssistStatus {
    /// cloudaicompanionProject（字符串或 {id|projectId} 对象两种形态）
    project_id: Option<String>,
    /// ineligibleTiers[].reasonCode 含 UNSUPPORTED_CLIENT（消费级账号已下线）
    unsupported_client: bool,
}

/// 解析 loadCodeAssist 200 响应（纯函数）：项目 ID 提取 + 不支持客户端标记。
fn parse_code_assist(body: &serde_json::Value) -> CodeAssistStatus {
    let project_id = match body.get("cloudaicompanionProject") {
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        Some(obj @ serde_json::Value::Object(_)) => ["id", "projectId"]
            .iter()
            .find_map(|k| {
                obj.get(*k)
                    .and_then(|s| s.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            }),
        _ => None,
    };
    let unsupported_client = body
        .get("ineligibleTiers")
        .and_then(|t| t.as_array())
        .map(|tiers| {
            tiers.iter().any(|tier| {
                tier.get("reasonCode")
                    .and_then(|c| c.as_str())
                    .map(|c| c.eq_ignore_ascii_case("UNSUPPORTED_CLIENT"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    CodeAssistStatus {
        project_id,
        unsupported_client,
    }
}

/// 从 cloudresourcemanager 项目列表响应选 Gemini 项目（纯函数）：
/// 优先 projectId 前缀 gen-lang-client（gemini-cli 自动创建的项目），
/// 其次带 generative-language 标签的项目；都没有 → None。
fn pick_fallback_project(body: &serde_json::Value) -> Option<String> {
    let projects = body.get("projects")?.as_array()?;
    let mut labeled = None;
    for project in projects {
        let Some(id) = project
            .get("projectId")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if id.starts_with("gen-lang-client") {
            return Some(id.to_string());
        }
        if labeled.is_none()
            && project
                .get("labels")
                .and_then(|l| l.get("generative-language"))
                .is_some()
        {
            labeled = Some(id.to_string());
        }
    }
    labeled
}

/// 单个模型组的聚合结果（同模型多 bucket 取最低 remainingFraction）。
#[derive(Debug, Clone, PartialEq)]
struct ModelQuota {
    /// 剩余比例（0-1）
    remaining: f64,
    /// 该 bucket 的重置时间（毫秒；最低桶自己的 resetTime）
    resets_at_ms: Option<i64>,
}

/// 解析 retrieveUserQuota 响应为「模型 → 最低剩余」映射（纯函数）。
/// bucket 缺 modelId 或 remainingFraction 非法时跳过该桶。
fn parse_quota_buckets(body: &serde_json::Value) -> std::collections::BTreeMap<String, ModelQuota> {
    let mut map = std::collections::BTreeMap::new();
    let Some(buckets) = body.get("buckets").and_then(|b| b.as_array()) else {
        return map;
    };
    for bucket in buckets {
        let (Some(model), Some(fraction)) = (
            bucket.get("modelId").and_then(|m| m.as_str()),
            bucket.get("remainingFraction").and_then(|f| f.as_f64()),
        ) else {
            continue;
        };
        if !fraction.is_finite() {
            continue;
        }
        let resets_at_ms = bucket
            .get("resetTime")
            .and_then(|t| t.as_str())
            .and_then(parse_iso_ms);
        let entry = map
            .entry(model.to_string())
            .or_insert(ModelQuota {
                remaining: fraction,
                resets_at_ms,
            });
        if fraction < entry.remaining {
            entry.remaining = fraction;
            entry.resets_at_ms = resets_at_ms;
        }
    }
    map
}

/// ISO-8601（含/不含毫秒）→ 毫秒时间戳。
fn parse_iso_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 模型分组键：pro / flash（flash-lite 归入 flash 组；不含两者的模型不展示，
/// 与 CodexBar 窗口口径一致）。
fn model_group(model_id: &str) -> Option<&'static str> {
    let lower = model_id.to_lowercase();
    if lower.contains("pro") {
        Some("pro")
    } else if lower.contains("flash") {
        Some("flash")
    } else {
        None
    }
}

/// 模型聚合 → 展示窗口（纯函数）：每组取组内最低剩余；Pro 窗口在前（主窗口），
/// Flash 在后（副窗口）；仅一组时该组即主窗口。usedPercent = (1 - remaining) * 100。
fn build_windows(quotas: &std::collections::BTreeMap<String, ModelQuota>) -> Vec<ProviderQuotaWindow> {
    let mut groups: std::collections::BTreeMap<&'static str, ModelQuota> =
        std::collections::BTreeMap::new();
    for (model, quota) in quotas {
        let Some(group) = model_group(model) else {
            continue;
        };
        match groups.get(group) {
            // 组内继续取最低（跨模型也用最低剩余，展示最紧张的一档）
            Some(existing) if quota.remaining >= existing.remaining => {}
            _ => {
                groups.insert(group, quota.clone());
            }
        }
    }
    // 输出顺序：pro（主窗口）→ flash（副窗口），与 key 排序解耦
    ["pro", "flash"]
        .iter()
        .filter_map(|key| {
            groups.get(*key).map(|q| ProviderQuotaWindow {
                key: key.to_string(),
                title: if *key == "pro" {
                    "Pro 模型".to_string()
                } else {
                    "Flash 模型".to_string()
                },
                used_percent: Some(((1.0 - q.remaining) * 100.0).clamp(0.0, 100.0)),
                used: None,
                total: None,
                unit: None,
                resets_at: q.resets_at_ms,
            })
        })
        .collect()
}

// ============================================================
// OAuth client 提取（oauth2.js 正则等价的手写解析 + 进程内缓存）
// ============================================================

/// OAuth 客户端凭证（刷新 token 用）。
#[derive(Debug, Clone, PartialEq)]
struct ClientCreds {
    id: String,
    secret: String,
}

/// 客户端凭证（进程内 OnceLock 缓存：oauth2.js 内容在运行期不会变）。
fn client_creds() -> &'static ClientCreds {
    static CACHE: OnceLock<ClientCreds> = OnceLock::new();
    CACHE.get_or_init(|| match discover_client_creds() {
        Some(creds) => creds,
        // 本机提取失败 → 内置常量兜底（见常量注释的来源说明）
        None => ClientCreds {
            id: BUILTIN_CLIENT_ID.to_string(),
            secret: BUILTIN_CLIENT_SECRET.to_string(),
        },
    })
}

/// 遍历本机 gemini-cli 常见安装位置读 oauth2.js 提取（每处只读有限几个候选
/// 文件；全部失败返回 None 走内置常量）。
fn discover_client_creds() -> Option<ClientCreds> {
    for path in candidate_oauth2_paths() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(creds) = parse_oauth_js(&content) {
                return Some(creds);
            }
        }
    }
    None
}

/// oauth2.js 候选路径（gemini-cli / gemini-cli-core 两种布局 × 常见安装根目录）。
fn candidate_oauth2_paths() -> Vec<PathBuf> {
    // 包内 oauth2.js 的相对布局（新版拆到 gemini-cli-core，旧版在包自身 dist）
    const REL_LAYOUTS: [&str; 2] = [
        "node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
        "dist/src/code_assist/oauth2.js",
    ];
    let mut roots: Vec<PathBuf> = Vec::new();
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    // Windows npm 全局默认前缀（%APPDATA%\npm）
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = appdata.trim();
        if !appdata.is_empty() {
            roots.push(PathBuf::from(appdata).join("npm").join("node_modules"));
        }
    }
    // 自定义 npm 全局前缀（npm config 常见值）
    roots.push(home.join(".npm-global").join("lib").join("node_modules"));
    // gemini-cli 原生安装器数据目录
    roots.push(home.join(".local").join("share").join("gemini-cli"));
    // macOS 包管理器全局 node_modules
    roots.push(PathBuf::from("/usr/local/lib/node_modules"));
    roots.push(PathBuf::from("/opt/homebrew/lib/node_modules"));

    let mut paths = Vec::new();
    for root in &roots {
        for pkg in ["@google/gemini-cli", "@google/gemini-cli-core"] {
            for rel in REL_LAYOUTS {
                paths.push(root.join(pkg).join(rel));
            }
        }
    }
    paths
}

/// 从 oauth2.js 源码提取 `OAUTH_CLIENT_ID = '...'` / `OAUTH_CLIENT_SECRET = '...'`
/// （手写扫描，语义等价 CodexBar 正则 `(?:const|let|var)?\s*NAME\s*=\s*['"]([\w\-\.]+)['"]`）。
fn parse_oauth_js(content: &str) -> Option<ClientCreds> {
    let id = extract_js_string_const(content, "OAUTH_CLIENT_ID")?;
    let secret = extract_js_string_const(content, "OAUTH_CLIENT_SECRET")?;
    Some(ClientCreds { id, secret })
}

/// 提取 JS 字符串常量赋值 `NAME = 'value'` / `NAME = "value"`：
/// 值只允许字母数字、`-`、`_`、`.`（与正则字符类一致），读到闭引号即止。
fn extract_js_string_const(content: &str, name: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(rel) = content[search_from..].find(name) {
        let at = search_from + rel;
        search_from = at + name.len();
        // 前一字符必须是标识符边界（防 OAUTH_CLIENT_IDXXX 之类误配）
        if at > 0 {
            let prev = content[..at].chars().next_back().unwrap();
            if prev.is_ascii_alphanumeric() || prev == '_' || prev == '$' {
                continue;
            }
        }
        // NAME 之后：空白 → '=' → 空白 → 引号
        let rest = &content[search_from..];
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next() else {
            continue;
        };
        if quote != '\'' && quote != '"' {
            continue;
        }
        let value: String = rest[quote.len_utf8()..]
            .chars()
            .take_while(|&c| c != quote)
            .collect();
        if !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Some(value);
        }
    }
    None
}

// ============================================================
// 网络层（ureq 同步；调用方 spawn_blocking）
// ============================================================

/// Token 刷新结果（成功响应里的三个字段）。
struct RefreshOutcome {
    access_token: String,
    /// 秒
    expires_in: i64,
    id_token: Option<String>,
}

/// POST oauth2.googleapis.com/token 刷新 access_token。
/// 失败返回 Err（中文原因 + HTTP 状态，不含 secret 片段）。
fn refresh_access_token(
    agent: &ureq::Agent,
    refresh_token: &str,
) -> Result<RefreshOutcome, String> {
    let creds = client_creds();
    let resp = agent
        .post(TOKEN_ENDPOINT)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .send_form(&[
            ("client_id", creds.id.as_str()),
            ("client_secret", creds.secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ]);
    let (status, body) =
        flatten_response(resp).map_err(|e| format!("Gemini 令牌刷新请求失败: {e}"))?;
    if status != 200 {
        return Err(format!("Gemini 令牌刷新失败（HTTP {status}）"));
    }
    let body = body.unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "Gemini 令牌刷新响应解析失败".to_string())?;
    let Some(access_token) = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .filter(|t| !t.is_empty())
    else {
        return Err("Gemini 令牌刷新响应缺少 access_token".to_string());
    };
    Ok(RefreshOutcome {
        access_token,
        expires_in: v.get("expires_in").and_then(|n| n.as_i64()).unwrap_or(3600),
        id_token: v
            .get("id_token")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .filter(|t| !t.is_empty()),
    })
}

/// 刷新成功后原子写回 oauth_creds.json（保留原有字段——含 refresh_token——
/// 只更新 access_token / expiry_date / id_token）。
fn write_back_creds(path: &std::path::Path, outcome: &RefreshOutcome) -> Result<(), String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 Gemini 凭证文件失败: {e}"))?;
    let mut v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("解析 Gemini 凭证文件失败: {e}"))?;
    if !v.is_object() {
        return Err("Gemini 凭证文件结构异常".to_string());
    }
    v["access_token"] = serde_json::Value::String(outcome.access_token.clone());
    v["expiry_date"] = serde_json::Value::Number(
        serde_json::Number::from((now_ms() + outcome.expires_in * 1000).max(0)),
    );
    if let Some(id_token) = &outcome.id_token {
        v["id_token"] = serde_json::Value::String(id_token.clone());
    }
    let data = serde_json::to_string_pretty(&v)
        .map_err(|e| format!("序列化 Gemini 凭证失败: {e}"))?;
    crate::provider_credentials::atomic_write(path, &data)
}

/// POST loadCodeAssist（失败静默降级为空状态——项目发现走回退端点）。
fn fetch_code_assist_status(
    agent: &ureq::Agent,
    access_token: &str,
) -> Result<CodeAssistStatus, String> {
    let resp = agent
        .post(LOAD_CODE_ASSIST_ENDPOINT)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .send_string(r#"{"metadata":{"ideType":"GEMINI_CLI","pluginType":"GEMINI"}}"#);
    let (status, body) = flatten_response(resp)?;
    if status != 200 {
        // 非致命：项目走回退发现，账号资格由 retrieveUserQuota 的 403 兜底判定
        return Ok(CodeAssistStatus::default());
    }
    let body = body.unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "loadCodeAssist 响应解析失败".to_string())?;
    Ok(parse_code_assist(&v))
}

/// GET cloudresourcemanager 项目列表，选 Gemini 项目（失败/无匹配 → None）。
fn discover_project(agent: &ureq::Agent, access_token: &str) -> Option<String> {
    let resp = agent
        .get(PROJECTS_ENDPOINT)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Accept", "application/json")
        .call();
    let (status, body) = flatten_response(resp).ok()?;
    if status != 200 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&body.unwrap_or_default()).ok()?;
    pick_fallback_project(&v)
}

/// POST retrieveUserQuota。
fn fetch_user_quota_raw(
    agent: &ureq::Agent,
    access_token: &str,
    project_id: Option<&str>,
) -> Result<(u16, Option<String>), String> {
    let body = match project_id {
        Some(project) => format!(r#"{{"project":"{project}"}}"#),
        None => "{}".to_string(),
    };
    let resp = agent
        .post(RETRIEVE_QUOTA_ENDPOINT)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .send_string(&body);
    flatten_response(resp).map_err(|e| format!("Gemini 配额{e}"))
}

/// 解析 retrieveUserQuota 响应 → 条目（纯函数，单测直接构造输入）。
/// 分支：网络失败(error) > 401(expired 重登) > 403 SUBSCRIPTION_REQUIRED
/// (error 账号不支持) > 非 200(error) > buckets 空(error) > 成功(ok)。
fn entry_from_quota_raw(
    raw: &Result<(u16, Option<String>), String>,
    email: Option<&str>,
) -> ProviderQuotaEntry {
    let fail = |status: &str, message: String| ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "Gemini CLI 账号".to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: email.map(str::to_string),
        message: Some(message),
        updated_at: now_ms(),
    };
    let Ok((http_status, body)) = raw else {
        return fail("error", format!("Gemini 配额{}", raw.as_ref().unwrap_err()));
    };
    if *http_status == 401 {
        return fail("expired", "Gemini 登录态已失效，请在终端重新运行 gemini 登录".to_string());
    }
    let body_text = body.as_deref().unwrap_or_default();
    if *http_status == 403 {
        if body_text.contains("SUBSCRIPTION_REQUIRED") {
            return fail("error", "当前 Google 账号不支持 Code Assist 配额查询".to_string());
        }
        return fail("error", format!("Gemini 配额查询失败（HTTP 403）"));
    }
    if *http_status != 200 {
        return fail("error", format!("Gemini 配额查询失败（HTTP {http_status}）"));
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body_text) else {
        return fail("error", "Gemini 配额响应解析失败".to_string());
    };
    let quotas = parse_quota_buckets(&v);
    let windows = build_windows(&quotas);
    if windows.is_empty() {
        return fail("error", "Gemini 配额响应未包含可用的模型额度".to_string());
    }
    ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "Gemini CLI 账号".to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name: email.map(str::to_string),
        message: None,
        updated_at: now_ms(),
    }
}

// ============================================================
// 主入口（provider_quota 早返回分支调用）
// ============================================================

/// 读取 Gemini CLI 登录态并查询配额，产出单条展示条目（home 注入版，
/// 单测可指向临时目录；后续路径会发起网络请求，单测只覆盖离线分支）：
/// - oauth_creds.json 不存在 → 空 Vec（前端 tab 不出现，presence 同口径）；
/// - settings.json 显式 api-key / vertex-ai → pending 提示；
/// - 凭证解析失败 / 刷新失败 / 查询失败 → error 条目（中文原因）；
/// - 成功 → ok 条目（Pro 主窗 + Flash 副窗，plan_name 为账号 email）。
fn fetch_entries_from(home: &std::path::Path) -> Vec<ProviderQuotaEntry> {
    let creds_path = home.join("oauth_creds.json");
    if !creds_path.exists() {
        return vec![];
    }
    let pending = |message: String| ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "Gemini CLI 账号".to_string(),
        status: "pending".to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message),
        updated_at: now_ms(),
    };
    let error = |message: String| ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "Gemini CLI 账号".to_string(),
        status: "error".to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message),
        updated_at: now_ms(),
    };

    // settings.json 认证方式检查（文件缺失/字段缺失按 OAuth 处理，不阻断）
    if let Ok(settings_raw) = std::fs::read_to_string(home.join("settings.json")) {
        if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&settings_raw) {
            if auth_type_unsupported(&settings) {
                return vec![pending(
                    "Gemini CLI 当前认证方式不受支持（需 OAuth 个人登录）".to_string(),
                )];
            }
        }
    }

    // 凭证读取
    let creds_raw = match std::fs::read_to_string(&creds_path) {
        Ok(raw) => raw,
        Err(e) => return vec![error(format!("读取 Gemini 凭证文件失败: {e}"))],
    };
    let mut creds = match parse_creds(&creds_raw) {
        Some(creds) => creds,
        None => return vec![error("Gemini 凭证文件解析失败，请在终端重新运行 gemini 登录".to_string())],
    };

    let agent = quota_http_agent();

    // 过期 → 刷新（成功原子写回后重新读一次 id_token，保持 email 展示最新）
    if needs_refresh(&creds, now_ms()) {
        let Some(refresh_token) = creds.refresh_token.as_deref().filter(|t| !t.is_empty()) else {
            return vec![error(
                "Gemini 登录态已失效，请在终端重新运行 gemini 登录".to_string(),
            )];
        };
        match refresh_access_token(&agent, refresh_token) {
            Ok(outcome) => {
                // 写回失败不阻断本轮查询（新 token 本轮仍可用），只记日志；
                // 文件未更新的后果是下一轮重新刷新一次，可接受
                if let Err(e) = write_back_creds(&creds_path, &outcome) {
                    eprintln!("[zbar-gemini] 刷新后写回凭证文件失败（下一轮将重新刷新）: {e}");
                }
                creds.access_token = Some(outcome.access_token);
                if outcome.id_token.is_some() {
                    creds.id_token = outcome.id_token;
                }
            }
            Err(e) => {
                return vec![error(format!("{e}，请在终端重新运行 gemini 登录"))];
            }
        }
    }
    let Some(access_token) = creds.access_token.as_deref().filter(|t| !t.is_empty()) else {
        return vec![error("Gemini 登录态已失效，请在终端重新运行 gemini 登录".to_string())];
    };
    let email = decode_jwt_email(creds.id_token.as_deref());

    // 项目发现：loadCodeAssist → cloudresourcemanager 回退
    let mut project_id: Option<String> = None;
    match fetch_code_assist_status(&agent, access_token) {
        Ok(status) => {
            if status.unsupported_client {
                return vec![error("当前 Google 账号不支持 Code Assist 配额查询".to_string())];
            }
            project_id = status.project_id;
        }
        Err(e) => eprintln!("[zbar-gemini] loadCodeAssist 失败（继续回退项目发现）: {e}"),
    }
    if project_id.is_none() {
        project_id = discover_project(&agent, access_token);
    }

    let raw = fetch_user_quota_raw(&agent, access_token, project_id.as_deref());
    vec![entry_from_quota_raw(&raw, email.as_deref())]
}

/// 查询入口（provider_quota 早返回分支调用）：以 ~/.gemini 为根。
pub(crate) fn fetch_quota_entries() -> Vec<ProviderQuotaEntry> {
    fetch_entries_from(&gemini_home())
}

// ============================================================
// 单元测试（纯函数，不联网、不碰真实 ~/.gemini）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造 JWT（不签名，payload 原样可解）。
    fn fake_jwt(payload_json: &str) -> String {
        use base64::Engine;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{\"alg\":\"RS256\"}");
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header}.{payload}.signature")
    }

    /// JWT email 解码：正常解出 / 缺 email / 坏 token 各归其位。
    #[test]
    fn jwt_email_decoding() {
        let token = fake_jwt(r#"{"email":"dev@example.com","hd":"example.com"}"#);
        assert_eq!(decode_jwt_email(Some(&token)).as_deref(), Some("dev@example.com"));
        // payload 无 email → None
        let token = fake_jwt(r#"{"sub":"123"}"#);
        assert_eq!(decode_jwt_email(Some(&token)), None);
        // 结构损坏 / None → None（不 panic、不阻断）
        assert_eq!(decode_jwt_email(Some("not-a-jwt")), None);
        assert_eq!(decode_jwt_email(None), None);
        // 空 email 视为无
        let token = fake_jwt(r#"{"email":"  "}"#);
        assert_eq!(decode_jwt_email(Some(&token)), None);
    }

    /// 刷新触发判定：token 缺失必刷；有过期时间按 now 比对；缺 expiry 不刷。
    #[test]
    fn refresh_trigger() {
        let now = 1_700_000_000_000i64;
        // access_token 缺失 → 刷
        assert!(needs_refresh(
            &OAuthCreds {
                expiry_date_ms: Some(now + 3_600_000),
                ..Default::default()
            },
            now
        ));
        // 未过期 → 不刷
        assert!(!needs_refresh(
            &OAuthCreds {
                access_token: Some("t".into()),
                expiry_date_ms: Some(now + 3_600_000),
                ..Default::default()
            },
            now
        ));
        // 已过期（含恰好等于 now）→ 刷
        assert!(needs_refresh(
            &OAuthCreds {
                access_token: Some("t".into()),
                expiry_date_ms: Some(now),
                ..Default::default()
            },
            now
        ));
        assert!(needs_refresh(
            &OAuthCreds {
                access_token: Some("t".into()),
                expiry_date_ms: Some(now - 1),
                ..Default::default()
            },
            now
        ));
        // 无 expiry → 不刷（与 gemini-cli 口径一致）
        assert!(!needs_refresh(
            &OAuthCreds {
                access_token: Some("t".into()),
                ..Default::default()
            },
            now
        ));
    }

    /// settings.json 认证方式：api-key / gemini-api-key / vertex-ai 不支持，
    /// oauth-personal / 未知 / 缺失放行。
    #[test]
    fn settings_auth_type_gating() {
        let mk = |t: &str| serde_json::json!({ "security": { "auth": { "selectedType": t } } });
        assert!(auth_type_unsupported(&mk("api-key")));
        assert!(auth_type_unsupported(&mk("gemini-api-key")));
        assert!(auth_type_unsupported(&mk("vertex-ai")));
        assert!(!auth_type_unsupported(&mk("oauth-personal")));
        assert!(!auth_type_unsupported(&mk("whatever-new")));
        // settings 缺失 / 结构不符 / 坏值 → 放行（容错）
        assert!(!auth_type_unsupported(&serde_json::json!({})));
        assert!(!auth_type_unsupported(&serde_json::json!({ "auth": "api-key" })));
        assert!(!auth_type_unsupported(
            &serde_json::json!({ "security": { "auth": { "selectedType": 42 } } })
        ));
    }

    /// loadCodeAssist 解析：project 字符串/对象两形态 + UNSUPPORTED_CLIENT 标记。
    #[test]
    fn code_assist_status_parsing() {
        // 字符串形态 + 消费级下线标记
        let v = serde_json::json!({
            "cloudaicompanionProject": "gen-lang-client-abc",
            "currentTier": { "id": "standard-tier" },
            "ineligibleTiers": [{ "tierId": "free-tier", "reasonCode": "UNSUPPORTED_CLIENT" }]
        });
        let s = parse_code_assist(&v);
        assert_eq!(s.project_id.as_deref(), Some("gen-lang-client-abc"));
        assert!(s.unsupported_client);

        // 对象形态（id / projectId 键）
        let v = serde_json::json!({ "cloudaicompanionProject": { "id": "proj-1" } });
        assert_eq!(parse_code_assist(&v).project_id.as_deref(), Some("proj-1"));
        let v = serde_json::json!({ "cloudaicompanionProject": { "projectId": "proj-2" } });
        assert_eq!(parse_code_assist(&v).project_id.as_deref(), Some("proj-2"));

        // 无 project / reasonCode 不匹配 → 均为默认
        let v = serde_json::json!({ "ineligibleTiers": [{ "reasonCode": "OTHER" }] });
        let s = parse_code_assist(&v);
        assert_eq!(s.project_id, None);
        assert!(!s.unsupported_client);
        let s = parse_code_assist(&serde_json::json!({}));
        assert_eq!(s.project_id, None);
        assert!(!s.unsupported_client);
    }

    /// 项目回退：gen-lang-client 前缀优先，其次 generative-language 标签。
    #[test]
    fn fallback_project_selection() {
        let v = serde_json::json!({
            "projects": [
                { "projectId": "my-other-project" },
                { "projectId": "gen-lang-client-xyz", "labels": {} },
                { "projectId": "labeled-project", "labels": { "generative-language": "true" } }
            ]
        });
        assert_eq!(pick_fallback_project(&v).as_deref(), Some("gen-lang-client-xyz"));
        // 无前缀匹配 → 标签兜底
        let v = serde_json::json!({
            "projects": [
                { "projectId": "first" },
                { "projectId": "labeled", "labels": { "generative-language": "1" } }
            ]
        });
        assert_eq!(pick_fallback_project(&v).as_deref(), Some("labeled"));
        // 全不匹配 / 结构不符 → None
        assert_eq!(pick_fallback_project(&serde_json::json!({ "projects": [] })), None);
        assert_eq!(pick_fallback_project(&serde_json::json!({})), None);
    }

    /// 配额桶解析 + 窗口映射：同模型多 bucket 取最低、Pro/Flash 分窗、
    /// usedPercent = (1 - remaining) * 100、仅一组时主窗口唯一。
    #[test]
    fn quota_buckets_and_windows() {
        let v = serde_json::json!({
            "buckets": [
                // pro 两个桶：0.9 与 0.5 → 取 0.5（usedPercent 50）
                { "modelId": "gemini-2.5-pro", "remainingFraction": 0.9,
                  "resetTime": "2026-08-30T10:00:00Z", "tokenType": "INPUT" },
                { "modelId": "gemini-2.5-pro", "remainingFraction": 0.5,
                  "resetTime": "2026-08-30T12:34:56.789Z" },
                // flash 0.8 → 20%
                { "modelId": "gemini-2.5-flash", "remainingFraction": 0.8,
                  "resetTime": "2026-08-30T00:00:00Z" },
                // flash-lite 归入 flash 组且更低 → flash 组取 0.25
                { "modelId": "gemini-2.5-flash-lite", "remainingFraction": 0.25,
                  "resetTime": "2026-08-31T00:00:00Z" },
                // 脏桶：缺 fraction / 缺 modelId → 跳过
                { "modelId": "gemini-2.5-pro", "resetTime": "2026-08-30T10:00:00Z" },
                { "remainingFraction": 0.1 }
            ]
        });
        let quotas = parse_quota_buckets(&v);
        // 3 个模型各自成键；pro 取最低 0.5
        assert_eq!(quotas.len(), 3);
        assert!((quotas["gemini-2.5-pro"].remaining - 0.5).abs() < 1e-9);
        let windows = build_windows(&quotas);
        assert_eq!(windows.len(), 2);
        // 主窗口 Pro 在前
        assert_eq!(windows[0].key, "pro");
        assert_eq!(windows[0].title, "Pro 模型");
        assert!((windows[0].used_percent.unwrap() - 50.0).abs() < 1e-9);
        // 最低桶自己的 resetTime（含毫秒的 ISO 也可解析）
        assert_eq!(windows[0].resets_at, parse_iso_ms("2026-08-30T12:34:56.789Z"));
        // flash 组跨模型取最低（flash-lite 0.25 → 75%）
        assert_eq!(windows[1].key, "flash");
        assert!((windows[1].used_percent.unwrap() - 75.0).abs() < 1e-9);

        // 仅 Flash 一组 → 单窗口即主窗口（windows[0]）
        let v = serde_json::json!({
            "buckets": [{ "modelId": "gemini-2.5-flash", "remainingFraction": 1.0 }]
        });
        let windows = build_windows(&parse_quota_buckets(&v));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "flash");
        assert!((windows[0].used_percent.unwrap() - 0.0).abs() < 1e-9);

        // 全部为未知模型 → 无窗口（entry_from_quota_raw 会转 error）
        let v = serde_json::json!({
            "buckets": [{ "modelId": "gemini-3-ultra", "remainingFraction": 0.5 }]
        });
        assert!(build_windows(&parse_quota_buckets(&v)).is_empty());
        // 无 buckets 键 → 空映射
        assert!(parse_quota_buckets(&serde_json::json!({})).is_empty());
    }

    /// retrieveUserQuota 响应映射：403 SUBSCRIPTION_REQUIRED、401、网络失败、成功。
    #[test]
    fn quota_entry_mapping() {
        // 403 + SUBSCRIPTION_REQUIRED → 账号不支持
        let raw = Ok((403, Some(r#"{"error":{"code":403,"status":"SUBSCRIPTION_REQUIRED"}}"#.into())));
        let entry = entry_from_quota_raw(&raw, None);
        assert_eq!(entry.status, "error");
        assert_eq!(entry.message.as_deref(), Some("当前 Google 账号不支持 Code Assist 配额查询"));
        // 403 其他原因 → 常规错误
        let raw = Ok((403, Some("forbidden".into())));
        let entry = entry_from_quota_raw(&raw, None);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("403"));

        // 401 → expired 重登提示
        let raw = Ok((401, Some("unauthorized".into())));
        let entry = entry_from_quota_raw(&raw, None);
        assert_eq!(entry.status, "expired");
        assert!(entry.message.unwrap().contains("gemini 登录"));

        // 网络层失败 → error 透传原因
        let raw: Result<(u16, Option<String>), String> = Err("网络错误或服务不可用: timeout".into());
        let entry = entry_from_quota_raw(&raw, None);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误"));

        // 成功：ok + email 作 plan_name
        let raw = Ok((200, Some(
            r#"{"buckets":[{"modelId":"gemini-2.5-pro","remainingFraction":0.75,"resetTime":"2026-08-30T00:00:00Z"}]}"#.into(),
        )));
        let entry = entry_from_quota_raw(&raw, Some("dev@example.com"));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, "local");
        assert_eq!(entry.label, "Gemini CLI 账号");
        assert_eq!(entry.plan_name.as_deref(), Some("dev@example.com"));
        assert_eq!(entry.windows.len(), 1);
        assert!((entry.windows[0].used_percent.unwrap() - 25.0).abs() < 1e-9);

        // 200 但 buckets 缺失 → error（不产出空 ok 条目）
        let raw = Ok((200, Some("{}".into())));
        let entry = entry_from_quota_raw(&raw, None);
        assert_eq!(entry.status, "error");
    }

    /// oauth_creds.json 解析：完整字段、空串视为缺失、坏 JSON → None。
    #[test]
    fn creds_file_parsing() {
        let creds = parse_creds(
            r#"{"access_token":"ya29.a","refresh_token":"//rt","id_token":"j.w.s",
                "expiry_date":1735689600000}"#,
        )
        .expect("合法凭证应解析成功");
        assert_eq!(creds.access_token.as_deref(), Some("ya29.a"));
        assert_eq!(creds.refresh_token.as_deref(), Some("//rt"));
        assert_eq!(creds.expiry_date_ms, Some(1_735_689_600_000));
        // 空白串视为缺失
        let creds = parse_creds(r#"{"access_token":"  ","expiry_date":123}"#).unwrap();
        assert_eq!(creds.access_token, None);
        // 坏 JSON / 非对象 → None
        assert_eq!(parse_creds("not json"), None);
        assert_eq!(parse_creds("[1,2]"), None);
    }

    /// oauth2.js 常量提取：单双引号、无修饰符前缀（压缩产物）均可提取；
    /// 值字符非法（含空格等）不采纳。
    #[test]
    fn oauth_js_extraction() {
        let js = "const OAUTH_CLIENT_ID = '681255809395-abc.apps.googleusercontent.com';\n\
                  const OAUTH_CLIENT_SECRET = 'GOCSPX-4uHgMPm-1o7Sk';\n";
        let creds = parse_oauth_js(js).expect("应提取成功");
        assert_eq!(creds.id, "681255809395-abc.apps.googleusercontent.com");
        assert_eq!(creds.secret, "GOCSPX-4uHgMPm-1o7Sk");

        // 双引号 + 压缩形态（let/var 前缀或裸赋值）
        let js = "var x=1,OAUTH_CLIENT_ID=\"id-1.x_y\",OAUTH_CLIENT_SECRET=\"sec-1\";";
        let creds = parse_oauth_js(js).expect("应提取成功");
        assert_eq!(creds.id, "id-1.x_y");
        assert_eq!(creds.secret, "sec-1");

        // 缺 SECRET → None；值含非法字符（空格）→ 该常量不采纳
        assert_eq!(parse_oauth_js("const OAUTH_CLIENT_ID = 'a-b';"), None);
        assert_eq!(
            parse_oauth_js("const OAUTH_CLIENT_ID = 'has space';\nconst OAUTH_CLIENT_SECRET='s';"),
            None
        );
        // 名称误配防护：OAUTH_CLIENT_ID_PLACEHOLDER 不算
        assert_eq!(
            parse_oauth_js("const OAUTH_CLIENT_ID_PLACEHOLDER='x';\nconst OAUTH_CLIENT_SECRET='s';"),
            None
        );
    }

    /// ISO 时间解析：Z 带毫秒 / 不带毫秒 / 带偏移量。
    #[test]
    fn iso_time_parsing() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.500Z"), Some(500));
        assert_eq!(parse_iso_ms("1970-01-01T08:00:00+08:00"), Some(0));
        assert_eq!(parse_iso_ms("garbage"), None);
    }

    /// 离线闭环（临时目录，不联网）：无 oauth_creds.json → 空数组（tab 不
    /// 出现）；settings 显式 api-key → pending；过期且无 refresh_token →
    /// error 提示重登（均不发起网络请求的分支）。
    #[test]
    fn offline_presence_flow() {
        let tmp = std::env::temp_dir().join(format!("zbar-gemini-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let creds = tmp.join("oauth_creds.json");
        let settings = tmp.join("settings.json");

        // 无数据 → 空数组
        assert!(fetch_entries_from(&tmp).is_empty());

        // settings 显式 api-key → pending（需 OAuth 个人登录）
        std::fs::write(
            &creds,
            r#"{"access_token":"t","refresh_token":"r","expiry_date":99999999999999}"#,
        )
        .unwrap();
        std::fs::write(
            &settings,
            r#"{"security":{"auth":{"selectedType":"api-key"}}}"#,
        )
        .unwrap();
        let entries = fetch_entries_from(&tmp);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].credential_id, "local");
        assert_eq!(entries[0].status, "pending");
        assert_eq!(
            entries[0].message.as_deref(),
            Some("Gemini CLI 当前认证方式不受支持（需 OAuth 个人登录）")
        );

        // 凭证已过期且无 refresh_token → error 提示重登（刷新前的离线分支）
        std::fs::remove_file(&settings).unwrap();
        std::fs::write(&creds, r#"{"access_token":"t","expiry_date":1000}"#).unwrap();
        let entries = fetch_entries_from(&tmp);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "error");
        assert!(entries[0].message.as_deref().unwrap().contains("gemini 登录"));

        // 坏 JSON 凭证 → error（解析失败分支，同样不联网）
        std::fs::write(&creds, "not json").unwrap();
        let entries = fetch_entries_from(&tmp);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "error");

        std::fs::remove_dir_all(&tmp).ok();
    }
}

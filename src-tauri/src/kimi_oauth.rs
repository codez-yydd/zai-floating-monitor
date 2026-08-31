//! Kimi OAuth 2.0 设备码登录（RFC 8628 Device Authorization Grant）。
//!
//! 为通用凭证体系（provider="kimi"，kind="token"）提供不依赖 Kimi Code CLI
//! 的网页登录入口：应用向后端发起设备授权 → 用户在浏览器打开验证地址确认
//! → 前端按 interval 定时调用 poll 命令 → 成功后把 refresh_token 作为一条
//! 凭证落入 ~/.zbar/credentials/kimi.json（region 为发起登录时所选）。
//! 参考实现：KimiCodeBar-Windows 的 kimi_oauth.rs（同为 Kimi Code CLI 的
//! OAuth 端点与 client_id）。
//!
//! 与参考实现的差异（本项目工程惯例）：
//! - **单次轮询**：poll 命令只打一次 token 端点立即返回（pending/success/
//!   denied/expired/error），由前端按 interval 定时再调——不在 command 里
//!   做长轮询循环，避免长时间占用阻塞线程池线程；
//! - **device_code 不出后端**：前端只持 session_id，device_code 存后端内存
//!   会话表（锁内只做查改，网络请求在锁外）；
//! - 成功落凭证复用 provider_credentials::add_entry（同一存储、同一校验、
//!   同一原子写护栏），secret 存 refresh_token——实测非 rotation 型可复用，
//!   凭证链路额度查询时按需换新 access_token（见 kimi.rs 凭证区块）。
//!
//! 安全护栏：
//! - 验证地址是远端可控字符串、会在系统浏览器打开：必须 https 且落在
//!   kimi.com / kimi.ai 及其子域，否则按授权服务异常处理（防钓鱼跳转）；
//! - 请求带与 Kimi Code CLI 一致的 X-Msh-* 设备身份头（device_id 与 CLI
//!   共享 <kimi-code 根>/device_id，不存在则生成 UUID v4 写入）；
//! - 错误消息中文且不含 device_code/refresh_token 等敏感值。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::kimi::{build_kimi_agent, kimi_code_root, KIMI_OAUTH_CLIENT_ID};
use crate::provider_credentials;

/// 设备授权端点路径（拼接在 auth 主机后）。
const DEVICE_AUTHORIZATION_PATH: &str = "/api/oauth/device_authorization";
/// token 端点路径（设备码轮询与刷新共用）。
const TOKEN_PATH: &str = "/api/oauth/token";
/// 设备码授权 grant_type（RFC 8628 固定值）。
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// 服务端未给 expires_in 时的本地会话过期兜底（与 CLI 一致按 15 分钟）。
const DEFAULT_EXPIRES_SECS: u64 = 15 * 60;
/// 会话表容量上限（防御异常情况下无限增长；正常同时最多 1-2 个登录流程，
/// 达上限时清掉最旧的——登录是低频交互，简单策略即可）。
const MAX_SESSIONS: usize = 32;

// ============================================================
// 数据结构（command 返回，camelCase 对齐前端 types.ts）
// ============================================================

/// 发起设备码登录的结果：前端展示 user_code 并打开验证地址；
/// device_code 留在后端会话表，前端只持 session_id。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiDeviceAuthInfo {
    pub session_id: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    /// 设备码有效期（秒；前端超时提示用）
    pub expires_in: u64,
    /// 建议轮询间隔（秒；服务端未给时后端兜底 5）
    pub interval: u64,
}

/// 单次轮询结果。status 语义：
/// - "pending"：等待用户在浏览器确认（前端按 interval 继续轮询）；
/// - "success"：登录成功，凭证已保存（前端关闭弹层并刷新凭证列表）；
/// - "denied"：用户拒绝了授权（终止轮询）；
/// - "expired"：设备码过期 / 会话不存在（终止轮询，引导重新发起）；
/// - "error"：其他错误（终止轮询，message 为中文原因）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiDevicePollResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 设备授权端点响应（snake_case；全 Option 容错，缺失字段由调用方判错）。
#[derive(Debug, Deserialize)]
struct DeviceAuthResponse {
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(rename = "verification_uri_complete", default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

/// 内存设备码会话（device_code 不出后端；网络请求全部在锁外）。
struct DeviceAuthSession {
    device_code: String,
    /// 发起登录时所选区域（None/空 = 默认大陆站；成功落凭证时随凭证写入）
    region: Option<String>,
    /// 本地过期时刻（服务端 expires_in 秒后；到期 poll 直接判 expired，
    /// 不再打 token 端点）
    expires_at: Instant,
}

static DEVICE_AUTH_SESSIONS: OnceLock<Mutex<HashMap<String, DeviceAuthSession>>> =
    OnceLock::new();

fn device_sessions() -> &'static Mutex<HashMap<String, DeviceAuthSession>> {
    DEVICE_AUTH_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 会话 id 生成序号（与毫秒时间戳组合避免同毫秒碰撞）。
static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

/// 生成会话 id：毫秒时间戳 36 进制 + 原子序号（与 provider_credentials 的
/// new_entry_id 同思路；仅内存会话键，非安全场景够用）。
fn new_session_id() -> String {
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let now_ms = chrono::Utc::now().timestamp_millis();
    format!("{:x}-{:x}", now_ms, seq & 0xfff)
}

// ============================================================
// 纯函数（单测覆盖）
// ============================================================

/// 登录区域 → OAuth auth 主机（与 kimi.rs endpoints_for_region 的
/// oauth_host 语义对齐）：global → auth.kimi.ai，其余（None/空/未知值，
/// 含大陆站 "cn"）→ auth.kimi.com。
fn auth_host_for_region(region: Option<&str>) -> &'static str {
    match region {
        Some(r) if r.trim() == "global" => "https://auth.kimi.ai",
        _ => "https://auth.kimi.com",
    }
}

/// token 端点轮询错误分类（照 KimiCodeBar 参考实现）：
/// - Pending：authorization_pending，按当前间隔继续；
/// - SlowDown：slow_down，应加大间隔（本项目前端固定 interval，按 pending 处理）；
/// - Expired：expired_token → 终止并提示重新发起；
/// - Denied：access_denied → 终止并提示被拒绝；
/// - Api：其余错误码 → error（带服务端消息）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollAction {
    Pending,
    SlowDown,
    Expired,
    Denied,
    Api,
}

fn classify_poll_error(code: Option<&str>) -> PollAction {
    match code {
        Some("authorization_pending") => PollAction::Pending,
        Some("slow_down") => PollAction::SlowDown,
        Some("expired_token") => PollAction::Expired,
        Some("access_denied") => PollAction::Denied,
        _ => PollAction::Api,
    }
}

/// 验证地址白名单校验（纯函数）：必须 https scheme 且 host 为
/// kimi.com / kimi.ai 或其子域（global 站验证地址落在 auth.kimi.ai）。
/// 先剥 userinfo（防 https://kimi.com@evil.com 伪装）再去端口，
/// 按小写 host 比较。
fn is_trusted_verification_uri(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = host_port
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    host == "kimi.com"
        || host.ends_with(".kimi.com")
        || host == "kimi.ai"
        || host.ends_with(".kimi.ai")
}

/// 按优先级抽取错误消息：error_description > message > detail > error > 原文
/// （服务端错误形态不统一，多层兜底保证用户能看到可读原因）。超长截断到
/// 200 字符（对齐 provider_credentials::record_check 的口径），防止异常
/// 响应把超长原文透传到前端。
fn extract_error_message(body: &str) -> Option<String> {
    let mut message: Option<String> = None;
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        for key in ["error_description", "message", "detail", "error"] {
            if let Some(s) = value.get(key).and_then(|v| v.as_str()) {
                message = Some(s.to_string());
                break;
            }
        }
    }
    if message.is_none() {
        let text = body.trim();
        if !text.is_empty() {
            message = Some(text.to_string());
        }
    }
    message.map(|m| m.chars().take(200).collect())
}

/// 抽取 error 字段（轮询错误分类用）。
fn extract_error_code(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    value.get("error")?.as_str().map(str::to_string)
}

/// 解析 token 端点成功响应中的 refresh_token（trim 后非空才有效）。
/// 本流程只存 refresh_token（access_token 15 分钟短效，由凭证链路按需
/// 换新，不在登录时保存）。
fn extract_refresh_token(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("refresh_token")?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

// ============================================================
// 设备身份头（与 Kimi Code CLI 对齐，照 KimiCodeBar 参考实现）
// ============================================================

/// X-Msh-* 设备身份头：Platform / Version / Device-Name / Device-Model /
/// Os-Version / Device-Id（device_id 与 CLI 共享同一文件）。
fn identity_headers() -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("X-Msh-Platform", "kimi_code_cli".to_string()),
        ("X-Msh-Version", env!("CARGO_PKG_VERSION").to_string()),
        ("X-Msh-Device-Name", device_name()),
        ("X-Msh-Device-Model", device_model()),
        ("X-Msh-Os-Version", os_version_string()),
    ];
    if let Some(device_id) = load_or_create_device_id() {
        headers.push(("X-Msh-Device-Id", device_id));
    }
    headers
}

/// 读取 <kimi-code 根>/device_id（与 CLI 共享）；不存在或为空则生成
/// UUID v4 写入（写失败——如目录不可创建——则不携带该头，不影响流程）。
fn load_or_create_device_id() -> Option<String> {
    let root = kimi_code_root();
    let path = root.join("device_id");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let id = text.trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if std::fs::write(&path, &id).is_err() {
        std::fs::create_dir_all(&root).ok()?;
        std::fs::write(&path, &id).ok()?;
    }
    Some(id)
}

/// 主机名（Windows 上即 COMPUTERNAME）。
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Desktop".to_string())
}

/// 设备型号，如 "Windows 11"（build >= 22000）。
fn device_model() -> String {
    match windows_os_version() {
        Some((major, _, build)) if major >= 10 && build >= 22000 => "Windows 11".to_string(),
        Some((major, _, _)) if major >= 10 => "Windows 10".to_string(),
        _ => "Windows".to_string(),
    }
}

/// 尽量取真实系统版本，取不到给合理默认。
fn os_version_string() -> String {
    match windows_os_version() {
        Some((major, minor, build)) => format!("{major}.{minor}.{build}"),
        None => "10.0".to_string(),
    }
}

/// 通过 ntdll!RtlGetVersion 取真实版本（不受 manifest 影响），无需额外依赖。
#[cfg(windows)]
fn windows_os_version() -> Option<(u32, u32, u32)> {
    #[repr(C)]
    struct OsVersionInfoW {
        os_version_info_size: u32,
        major_version: u32,
        minor_version: u32,
        build_number: u32,
        platform_id: u32,
        csd_version: [u16; 128],
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }

    let mut info = OsVersionInfoW {
        os_version_info_size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major_version: 0,
        minor_version: 0,
        build_number: 0,
        platform_id: 0,
        csd_version: [0; 128],
    };
    // NTSTATUS 0 = STATUS_SUCCESS
    if unsafe { RtlGetVersion(&mut info) } == 0 {
        Some((info.major_version, info.minor_version, info.build_number))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn windows_os_version() -> Option<(u32, u32, u32)> {
    None
}

// ============================================================
// HTTP（ureq 同步；10s 超时 + 代理复用 build_kimi_agent）
// ============================================================

/// POST 表单（带 X-Msh-* 设备身份头），返回 (HTTP 状态码, 响应体)。
/// 4xx/5xx 也展平为 Ok（错误分类按状态码 + body error 字段分支）；
/// 网络层彻底失败返回 Err（中文原因）。
fn auth_post_form(url: &str, params: &[(&str, &str)]) -> Result<(u16, String), String> {
    let mut request = build_kimi_agent()
        .post(url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json");
    for (name, value) in identity_headers() {
        request = request.set(name, &value);
    }
    match request.send_form(params) {
        Ok(resp) => {
            let status = resp.status();
            let body = resp
                .into_string()
                .map_err(|e| format!("读取授权响应失败: {e}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(e) => Err(format!("网络错误或服务不可用: {e}")),
    }
}

// ============================================================
// 设备码登录流程（command 薄封装 + 同步内核）
// ============================================================

/// 发起设备码登录：调 device_authorization 端点，device_code 存入内存
/// 会话表并返回展示信息（user_code / 验证地址 / 有效期 / 轮询间隔）。
#[tauri::command]
pub async fn start_kimi_device_auth(
    region: Option<String>,
) -> Result<KimiDeviceAuthInfo, String> {
    tauri::async_runtime::spawn_blocking(move || start_device_auth(region))
        .await
        .map_err(|e| format!("设备码登录任务失败: {e}"))?
}

fn start_device_auth(region: Option<String>) -> Result<KimiDeviceAuthInfo, String> {
    let auth_host = auth_host_for_region(region.as_deref());
    let (status, body) = auth_post_form(
        &format!("{auth_host}{DEVICE_AUTHORIZATION_PATH}"),
        &[("client_id", KIMI_OAUTH_CLIENT_ID)],
    )?;
    if !(200..300).contains(&status) {
        let message = extract_error_message(&body)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(format!("发起设备码登录失败: {message}"));
    }
    let resp: DeviceAuthResponse = serde_json::from_str(&body)
        .map_err(|_| "授权服务返回了无法解析的响应".to_string())?;
    let device_code = resp
        .device_code
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("授权服务返回了无效的响应（缺少 device_code）")?;
    let user_code = resp
        .user_code
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("授权服务返回了无效的响应（缺少 user_code）")?;
    let verification_uri = resp
        .verification_uri
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("授权服务返回了无效的响应（缺少验证地址）")?;
    // verification_uri_complete 可选（缺失时前端打开 verification_uri）
    let verification_uri_complete = resp
        .verification_uri_complete
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 验证地址会在系统浏览器打开：必须落在 kimi 域白名单内（防钓鱼跳转）
    for uri in [
        verification_uri.as_str(),
        verification_uri_complete.as_deref().unwrap_or(""),
    ] {
        if !uri.is_empty() && !is_trusted_verification_uri(uri) {
            return Err("授权服务返回了非预期的验证地址".to_string());
        }
    }

    let expires_in = resp.expires_in.unwrap_or(DEFAULT_EXPIRES_SECS).max(60);
    // 轮询间隔兜底 5 秒（RFC 8628 建议默认；服务端未给时的合理值）
    let interval = resp.interval.unwrap_or(5).max(1);

    // 入会话表：顺手清理过期会话防膨胀；达容量上限时整体清空（低频交互，
    // 丢会话的代价只是让用户重新发起登录，不会损坏任何数据）
    let session_id = new_session_id();
    {
        let mut sessions = device_sessions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        sessions.retain(|_, s| s.expires_at > now);
        if sessions.len() >= MAX_SESSIONS {
            sessions.clear();
        }
        sessions.insert(
            session_id.clone(),
            DeviceAuthSession {
                device_code,
                region,
                expires_at: now + Duration::from_secs(expires_in),
            },
        );
    }

    Ok(KimiDeviceAuthInfo {
        session_id,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in,
        interval,
    })
}

/// 单次轮询设备码授权结果：查会话 → 打一次 token 端点 → 按错误码分类
/// 返回状态。成功时把 refresh_token 落为凭证（provider="kimi",
/// kind="token"，region 为发起时所选）并从会话表移除该会话。
#[tauri::command]
pub async fn poll_kimi_device_auth(session_id: String) -> Result<KimiDevicePollResult, String> {
    tauri::async_runtime::spawn_blocking(move || poll_device_auth(&session_id))
        .await
        .map_err(|e| format!("设备码轮询任务失败: {e}"))?
}

fn poll_device_auth(session_id: &str) -> Result<KimiDevicePollResult, String> {
    // 锁内只取会话快照与本地过期判断，token 请求在锁外
    let (device_code, region) = {
        let mut sessions = device_sessions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = sessions.get(session_id) else {
            return Ok(KimiDevicePollResult {
                status: "expired".to_string(),
                message: Some("登录会话不存在或已失效，请重新发起网页登录".to_string()),
            });
        };
        if session.expires_at <= Instant::now() {
            sessions.remove(session_id);
            return Ok(KimiDevicePollResult {
                status: "expired".to_string(),
                message: Some("授权码已过期，请重新发起网页登录".to_string()),
            });
        }
        (session.device_code.clone(), session.region.clone())
    };

    let auth_host = auth_host_for_region(region.as_deref());
    let (status, body) = auth_post_form(
        &format!("{auth_host}{TOKEN_PATH}"),
        &[
            ("client_id", KIMI_OAUTH_CLIENT_ID),
            ("device_code", device_code.as_str()),
            ("grant_type", DEVICE_GRANT_TYPE),
        ],
    )?;

    if (200..300).contains(&status) {
        // 成功：refresh_token 落凭证 → 移除会话。落凭证失败（磁盘错误等）
        // 返回 error：前端轮询到 error 即终止、没有「同一会话重试」的入口
        //（重新发起会创建新会话），残留会话由本地过期后的清理逻辑回收
        //（保留无害，不产生重复凭证）。
        let Some(refresh_token) = extract_refresh_token(&body) else {
            return Ok(KimiDevicePollResult {
                status: "error".to_string(),
                message: Some("授权服务返回了无效的响应（缺少 refresh_token）".to_string()),
            });
        };
        // 备注默认「网页登录」（用户可在凭证卡修改），region 随发起时选择
        if let Err(e) = provider_credentials::add_entry(
            "kimi",
            "网页登录",
            "token",
            &refresh_token,
            region.as_deref(),
        ) {
            return Ok(KimiDevicePollResult {
                status: "error".to_string(),
                message: Some(format!("登录成功但保存凭证失败: {e}")),
            });
        }
        device_sessions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
        return Ok(KimiDevicePollResult {
            status: "success".to_string(),
            message: None,
        });
    }

    // 4xx/5xx：按 body 的 error 字段分类（网络层失败在 auth_post_form 已变 Err）
    let code = extract_error_code(&body);
    let message = extract_error_message(&body);
    match classify_poll_error(code.as_deref()) {
        PollAction::Pending | PollAction::SlowDown => Ok(KimiDevicePollResult {
            status: "pending".to_string(),
            message: None,
        }),
        PollAction::Expired => {
            remove_session(session_id);
            Ok(KimiDevicePollResult {
                status: "expired".to_string(),
                message: Some("授权码已过期，请重新发起网页登录".to_string()),
            })
        }
        PollAction::Denied => {
            remove_session(session_id);
            Ok(KimiDevicePollResult {
                status: "denied".to_string(),
                message: Some("已拒绝授权，如需登录请重新发起并确认".to_string()),
            })
        }
        PollAction::Api => Ok(KimiDevicePollResult {
            status: "error".to_string(),
            message: Some(format!(
                "授权服务返回错误: {}",
                message.unwrap_or_else(|| format!("HTTP {status}"))
            )),
        }),
    }
}

/// 从会话表移除指定会话（denied/expired 终态时调用；不存在静默忽略）。
fn remove_session(session_id: &str) {
    device_sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(session_id);
}

// ============================================================
// 单元测试（纯函数部分；网络流程不测）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 登录区域 → auth 主机分流：global → .ai；None/空/未知/大陆值 → 默认 .com。
    #[test]
    fn auth_host_region_routing() {
        assert_eq!(
            auth_host_for_region(Some("global")),
            "https://auth.kimi.ai"
        );
        assert_eq!(
            auth_host_for_region(Some(" global ")),
            "https://auth.kimi.ai"
        );
        for region in [None, Some(""), Some("cn"), Some("mainland-cn"), Some("GLOBAL")] {
            assert_eq!(
                auth_host_for_region(region),
                "https://auth.kimi.com",
                "region={region:?}"
            );
        }
    }

    /// 轮询错误码分类：四个已知码各归其位，未知码 / 缺失 → Api。
    #[test]
    fn classify_poll_error_maps_known_codes() {
        assert_eq!(
            classify_poll_error(Some("authorization_pending")),
            PollAction::Pending
        );
        assert_eq!(classify_poll_error(Some("slow_down")), PollAction::SlowDown);
        assert_eq!(
            classify_poll_error(Some("expired_token")),
            PollAction::Expired
        );
        assert_eq!(
            classify_poll_error(Some("access_denied")),
            PollAction::Denied
        );
        assert_eq!(classify_poll_error(Some("invalid_grant")), PollAction::Api);
        assert_eq!(classify_poll_error(Some("server_error")), PollAction::Api);
        assert_eq!(classify_poll_error(None), PollAction::Api);
    }

    /// 验证地址白名单：放行 kimi.com / kimi.ai 及其子域的 https 地址；
    /// 拒绝非 https、异 host、后缀伪装、userinfo 伪装、危险 scheme。
    #[test]
    fn verification_uri_trust_check() {
        for good in [
            "https://kimi.com/device",
            "https://auth.kimi.com/api/oauth/device?user_code=ABCD",
            "https://auth.kimi.ai/api/oauth/device?user_code=ABCD",
            "https://www.kimi.com:443/x",
            "https://kimi.ai/device",
        ] {
            assert!(is_trusted_verification_uri(good), "应放行: {good}");
        }
        for bad in [
            "",
            "http://kimi.com/device",
            "https://evil.com",
            "https://kimi.com.evil.com",
            "https://evil-kimi.com",
            "https://kimi.com@evil.com/",
            "https://kimi.ai.evil.org/",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/cmd.exe",
        ] {
            assert!(!is_trusted_verification_uri(bad), "应拒绝: {bad}");
        }
    }

    /// 错误消息抽取优先级：error_description > message > detail > error > 原文。
    #[test]
    fn extract_error_message_priority() {
        assert_eq!(
            extract_error_message(r#"{"error":"access_denied","error_description":"用户拒绝"}"#),
            Some("用户拒绝".to_string())
        );
        assert_eq!(
            extract_error_message(r#"{"message":"m","detail":"d","error":"e"}"#),
            Some("m".to_string())
        );
        assert_eq!(
            extract_error_message(r#"{"error":"access_denied"}"#),
            Some("access_denied".to_string())
        );
        assert_eq!(extract_error_message("plain text"), Some("plain text".to_string()));
        assert_eq!(extract_error_message(""), None);
    }

    /// error 字段抽取：存在读值、缺失 / 脏 JSON 返回 None。
    #[test]
    fn extract_error_code_reads_error_field() {
        assert_eq!(
            extract_error_code(r#"{"error":"slow_down"}"#).as_deref(),
            Some("slow_down")
        );
        assert_eq!(extract_error_code(r#"{"message":"x"}"#), None);
        assert_eq!(extract_error_code("not json"), None);
    }

    /// refresh_token 提取：正常值 / 带空白 trim / 空串与缺失与脏 JSON 为 None。
    #[test]
    fn refresh_token_extraction() {
        assert_eq!(
            extract_refresh_token(r#"{"access_token":"at","refresh_token":"rt","expires_in":900}"#)
                .as_deref(),
            Some("rt")
        );
        assert_eq!(
            extract_refresh_token(r#"{"refresh_token":"  rt  "}"#).as_deref(),
            Some("rt")
        );
        assert_eq!(extract_refresh_token(r#"{"refresh_token":""}"#), None);
        assert_eq!(extract_refresh_token(r#"{"refresh_token":"  "}"#), None);
        assert_eq!(extract_refresh_token(r#"{"access_token":"at"}"#), None);
        assert_eq!(extract_refresh_token(r#"{"refresh_token":123}"#), None);
        assert_eq!(extract_refresh_token("not json"), None);
    }
}

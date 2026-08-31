//! 通用 AI 服务凭证存储层（provider credentials）。
//!
//! 为后续接入的 AI 订阅服务（Gemini / Grok / DeepSeek 等）提供统一的凭证
//! 存取基建：每个 provider 一个 JSON 文件（`~/.zbar/credentials/<provider>.json`），
//! 同一 provider 可保存多条凭证（主订阅 + 备用号并存）。本模块只负责本地
//! 存取，不发起任何网络请求——`record_credential_check` 供后续各 provider 的
//! 额度查询模块在查询完成后回写校验状态。
//!
//! 工程护栏（对齐 accounts.rs）：
//! - provider id 只允许小写字母数字，防 `<provider>.json` 路径注入；
//! - 全部写操作持模块级互斥锁串行化，读路径无锁（文件级 tmp+rename 原子写）；
//! - 目录 0700 / 文件 0600（Unix；Windows 走 ACL 不处理）；
//! - secret 永不进入任何日志；错误消息不含 secret 片段；
//! - 前端永远拿不到明文：list 只返回掩码元数据（前 6 后 4），编辑时
//!   secret 留空表示不变更（update 的入参口径）。
//!
//! 字段契约：磁盘 JSON 用 snake_case（version/entries/created_at/last_check），
//! 返回前端的 CredentialMeta 用 camelCase（与 types.ts 的 ProviderCredentialMeta
//! 一一对应，参照 agent_theme 模块的 rename_all 先例）。

use crate::pricing::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// 合法的凭证类型（后续各 provider 接入时按其认证方式选择）。
const KINDS: [&str; 3] = ["apiKey", "cookie", "token"];
/// 合法的区域值（部分 provider 区分国内/国际站）。
const REGIONS: [&str; 2] = ["cn", "global"];
/// 备注（label）长度上限，与 accounts.rs 的 display_name 约束一致。
const LABEL_MAX_CHARS: usize = 32;

// ============================================================
// 数据结构
// ============================================================

/// 最近一次校验结果（后续 provider 额度查询完成后回写）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialCheck {
    /// "ok" | "error"
    pub status: String,
    /// 校验时刻（ms 时间戳）
    pub at: i64,
    /// 失败原因（不含 secret；成功为 None）
    #[serde(default)]
    pub message: Option<String>,
}

/// 磁盘上的单条凭证（<provider>.json 的 entries 元素）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialEntry {
    id: String,
    label: String,
    /// "apiKey" | "cookie" | "token"
    kind: String,
    /// 凭证明文（仅落本机私有目录，永不返回前端）
    secret: String,
    /// Some("cn") | Some("global") | None
    #[serde(default)]
    region: Option<String>,
    created_at: i64,
    updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_check: Option<CredentialCheck>,
}

/// 返回前端的凭证元数据（不含 secret，只有掩码）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMeta {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub masked_secret: String,
    pub region: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_check: Option<CredentialCheck>,
}

impl From<&CredentialEntry> for CredentialMeta {
    fn from(e: &CredentialEntry) -> Self {
        CredentialMeta {
            id: e.id.clone(),
            label: e.label.clone(),
            kind: e.kind.clone(),
            masked_secret: mask_secret(&e.secret),
            region: e.region.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
            last_check: e.last_check.clone(),
        }
    }
}

/// 磁盘上的 provider 凭证文件整体。
#[derive(Debug, Serialize, Deserialize)]
struct ProviderFile {
    version: i32,
    #[serde(default)]
    entries: Vec<CredentialEntry>,
}

// ============================================================
// 存储护栏（照抄 accounts.rs）
// ============================================================

/// 全部写操作共用的互斥锁，防止并发写同一 provider 文件交错损坏。
static PROVIDER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn provider_lock() -> &'static Mutex<()> {
    PROVIDER_LOCK.get_or_init(|| Mutex::new(()))
}

/// 凭证 id 生成序号（与时间戳组合避免同毫秒碰撞）。
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 无 uuid 依赖的短 id：毫秒时间戳 36 进制 + 原子序号（本地文件非安全场景够用）。
fn new_entry_id() -> String {
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", now_ms(), seq & 0xfff)
}

/// provider id 只允许 1..=32 位小写字母数字（防 `<provider>.json` 注入路径）。
fn valid_provider(provider: &str) -> bool {
    !provider.is_empty()
        && provider.len() <= 32
        && provider
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

fn valid_region(region: Option<&str>) -> bool {
    match region {
        None => true,
        Some(r) => REGIONS.contains(&r),
    }
}

/// 凭证目录（~/.zbar/credentials/）。
fn credentials_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("credentials"))
}

fn provider_path(provider: &str) -> Result<PathBuf, String> {
    if !valid_provider(provider) {
        return Err("无效的服务标识".to_string());
    }
    Ok(credentials_dir()?.join(format!("{provider}.json")))
}

/// 目录权限收紧到 0700（仅 Unix；Windows 走 ACL 不处理）。
fn harden_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let _ = dir;
}

/// 文件权限收紧到 0600。
fn harden_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    let _ = path;
}

/// 同目录临时文件 + rename 原子写（凭证文件含 secret，必须避免半截文件）。
/// pub(crate)：gemini.rs 刷新 token 后写回 ~/.gemini/oauth_creds.json 复用
/// 同一惯例（刷新成功与写回之间进程被杀时，半截文件会毁掉用户的登录态）。
pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("路径缺少父目录: {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败: {e}"))?;
    harden_file(&tmp);
    fs::rename(&tmp, path).map_err(|e| format!("替换文件失败: {e}"))
}

/// 读取 provider 文件；不存在视为空（首次添加前无需预建）。
/// 文件损坏（手改出错的 JSON）返回错误提示重写，不静默清空用户凭证。
fn load_provider(provider: &str) -> Result<ProviderFile, String> {
    let path = provider_path(provider)?;
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("凭证文件解析失败（{provider}）: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(ProviderFile { version: 1, entries: vec![] })
        }
        Err(e) => Err(format!("读取凭证文件失败（{provider}）: {e}")),
    }
}

fn save_provider(provider: &str, file: &ProviderFile) -> Result<(), String> {
    let dir = credentials_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建凭证目录失败: {e}"))?;
    harden_dir(&dir);
    let data = serde_json::to_string_pretty(file)
        .map_err(|e| format!("序列化凭证失败: {e}"))?;
    atomic_write(&dir.join(format!("{provider}.json")), &data)
}

/// secret 掩码：前 6 后 4；短串（<=10 字符）只露首尾各 2 位，避免整串泄露。
fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let n = chars.len();
    if n == 0 {
        return "…".to_string();
    }
    if n <= 10 {
        let head: String = chars.iter().take(2).collect();
        let tail: String = chars.iter().skip(n - 2).collect();
        return format!("{head}…{tail}");
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars.iter().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// 备注规范化：去首尾空白 + 32 字截断；空则给默认「凭证 N」。
fn normalize_label(label: &str, next_index: usize) -> String {
    let trimmed: String = label.trim().chars().take(LABEL_MAX_CHARS).collect();
    if trimmed.is_empty() {
        format!("凭证 {next_index}")
    } else {
        trimmed
    }
}

/// secret 预处理：去首尾空白；空串报错（错误不含 secret 内容）。
fn normalize_secret(secret: &str) -> Result<String, String> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        Err("凭证内容不能为空".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// region 预处理：去首尾空白；空串/纯空白视为未选择（None）。
/// 前端无区域下拉的服务（DeepSeek/Gemini/Grok 等）region state 初始为空串，
/// 原样提交 Some("") 会被 valid_region 判非法，这里兜底规范化。
fn normalize_region(region: Option<&str>) -> Option<String> {
    region
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| r.to_string())
}

/// 按 id 找到条目下标（顺带校验 id 格式，非法 id 直接不匹配）。
fn find_entry(file: &ProviderFile, id: &str) -> Option<usize> {
    file.entries.iter().position(|e| e.id == id)
}

// ============================================================
// 纯函数核心（命令薄封装 + 单测入口）
// ============================================================

fn list_meta(provider: &str) -> Result<Vec<CredentialMeta>, String> {
    Ok(load_provider(provider)?
        .entries
        .iter()
        .map(CredentialMeta::from)
        .collect())
}

fn has_credentials(provider: &str) -> Result<bool, String> {
    // 本地型 provider（OpenCode Go / Gemini CLI）特判：无凭证也能出数据——
    // presence = 本地登录态/数据库存在，装了 CLI 的用户 tab 自动出现；
    // 其凭证体系保持可用（可选添加，不做强引导）。
    if provider == "opencodego" {
        return Ok(crate::opencodego::has_local_data());
    }
    if provider == "gemini" {
        return Ok(crate::gemini::has_local_data());
    }
    // grok 为混合型：本地 auth.json 存在即视为有数据（OR 语义）；本地没有
    // 时继续走通用凭证判断（手动 token 条目同样能出 tab）
    if provider == "grok" && crate::grok::has_local_data() {
        return Ok(true);
    }
    Ok(!load_provider(provider)?.entries.is_empty())
}

/// 添加一条凭证的内部内核（command 与 kimi_oauth 设备码登录共用）：
/// kind/region/secret 全量校验后持锁写入。pub(crate)：kimi_oauth 登录
/// 成功后经此落凭证，与手动添加路径共用同一存储护栏与原子写。
pub(crate) fn add_entry(
    provider: &str,
    label: &str,
    kind: &str,
    secret: &str,
    region: Option<&str>,
) -> Result<CredentialMeta, String> {
    if !valid_kind(kind) {
        return Err("无效的凭证类型".to_string());
    }
    // 空串/纯空白先规范化为 None 再校验（无区域下拉的服务提交空串不报错）
    let region = normalize_region(region);
    if !valid_region(region.as_deref()) {
        return Err("无效的区域值".to_string());
    }
    let secret = normalize_secret(secret)?;
    let _guard = provider_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_provider(provider)?;
    let entry = CredentialEntry {
        id: new_entry_id(),
        label: normalize_label(label, file.entries.len() + 1),
        kind: kind.to_string(),
        secret,
        region,
        created_at: now_ms(),
        updated_at: now_ms(),
        last_check: None,
    };
    let meta = CredentialMeta::from(&entry);
    file.entries.push(entry);
    save_provider(provider, &file)?;
    Ok(meta)
}

/// 仅当该 provider 尚无任何条目时添加一条凭证（判断与写入在同一锁窗口内
/// 完成）。供旧配置一次性迁移用：多个线程/进程并发触发迁移也只会创建一条，
/// 天然幂等（已有任何条目即视为已迁移，无操作）。返回是否实际创建了条目。
pub(crate) fn add_entry_if_empty(
    provider: &str,
    label: &str,
    kind: &str,
    secret: &str,
) -> Result<bool, String> {
    if !valid_kind(kind) {
        return Err("无效的凭证类型".to_string());
    }
    let secret = normalize_secret(secret)?;
    let _guard = provider_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_provider(provider)?;
    if !file.entries.is_empty() {
        return Ok(false);
    }
    let entry = CredentialEntry {
        id: new_entry_id(),
        label: normalize_label(label, 1),
        kind: kind.to_string(),
        secret,
        region: None,
        created_at: now_ms(),
        updated_at: now_ms(),
        last_check: None,
    };
    file.entries.push(entry);
    save_provider(provider, &file)?;
    Ok(true)
}

/// 重置某 provider 的凭证文件：删除后重建空骨架（持锁，避免与并发写交错）。
/// 凭证文件损坏（JSON 解析失败）时 list/add/update/remove 全部失败，此为
/// 前端「重置凭证文件」自愈入口的内部实现；provider 合法性由 provider_path
/// 校验（防路径注入）。文件不存在视为已重置（幂等）。
fn reset_provider(provider: &str) -> Result<(), String> {
    let _guard = provider_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = provider_path(provider)?;
    match fs::remove_file(&path) {
        Ok(()) => {}
        // 文件本就不存在：与「已重置」等价，继续重建骨架保证幂等
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("删除凭证文件失败: {e}")),
    }
    save_provider(provider, &ProviderFile { version: 1, entries: vec![] })
}

/// 更新凭证。三个可选字段的语义：
/// - label：Some → 更新（截断，空报错）；None → 不变；
/// - secret：Some 且 trim 非空 → 更新；None 或空串 → 不变（编辑留空占位）；
/// - region：Some("cn"/"global") → 设置；Some("") → 清除；None → 不变。
fn update_entry(
    provider: &str,
    id: &str,
    label: Option<&str>,
    secret: Option<&str>,
    region: Option<&str>,
) -> Result<CredentialMeta, String> {
    if let Some(r) = region {
        if !r.is_empty() && !REGIONS.contains(&r) {
            return Err("无效的区域值".to_string());
        }
    }
    // 先在锁外校验（错误早返回，不占锁）
    let new_secret = match secret.map(str::trim) {
        Some("") | None => None,
        Some(s) => Some(s.to_string()),
    };
    let new_label = match label {
        Some(l) => {
            let name: String = l.trim().chars().take(LABEL_MAX_CHARS).collect();
            if name.is_empty() {
                return Err("备注名称不能为空".to_string());
            }
            Some(name)
        }
        None => None,
    };

    let _guard = provider_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_provider(provider)?;
    let idx = find_entry(&file, id).ok_or_else(|| "未找到该凭证".to_string())?;
    let entry = &mut file.entries[idx];
    if let Some(name) = new_label {
        entry.label = name;
    }
    if let Some(s) = new_secret {
        entry.secret = s;
        // secret 变更后旧校验结论不再可信，清空待下轮查询回写
        entry.last_check = None;
    }
    if let Some(r) = region {
        entry.region = if r.is_empty() { None } else { Some(r.to_string()) };
    }
    entry.updated_at = now_ms();
    let meta = CredentialMeta::from(&*entry);
    save_provider(provider, &file)?;
    Ok(meta)
}

fn remove_entry(provider: &str, id: &str) -> Result<(), String> {
    let _guard = provider_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_provider(provider)?;
    let before = file.entries.len();
    file.entries.retain(|e| e.id != id);
    if file.entries.len() == before {
        return Err("未找到该凭证".to_string());
    }
    // 删空仍保存空文件（version 骨架保留，has_credentials 按 entries 判断）
    save_provider(provider, &file)
}

/// 额度查询用的凭证快照（含明文 secret：仅供 Rust 内部各 provider 额度查询
/// 模块构造鉴权头，永不下发前端、不进任何日志与错误消息）。
#[derive(Debug, Clone)]
pub(crate) struct CredentialQuerySnapshot {
    pub id: String,
    pub label: String,
    /// "apiKey" | "cookie" | "token"（kimi 凭证链路按类型分支：apiKey 直接
    /// 作 Bearer，token 视为 OAuth refresh_token 换新；其余余额型 provider
    /// 只用 apiKey，快照保持完整供后续接入）
    pub kind: String,
    pub secret: String,
    /// Some("cn") | Some("global") | None（区分国内/国际站的 provider 用）
    pub region: Option<String>,
}

/// 读取某 provider 全部凭证的查询快照（只读，不改任何状态）。
/// 供 provider_quota 在持锁窗口外取快照：先快照 → 网络查询 → 再回写，
/// 避免持 PROVIDER_LOCK 做网络请求。文件不存在视为空。
pub(crate) fn load_query_snapshots(
    provider: &str,
) -> Result<Vec<CredentialQuerySnapshot>, String> {
    Ok(load_provider(provider)?
        .entries
        .into_iter()
        .map(|e| CredentialQuerySnapshot {
            id: e.id,
            label: e.label,
            kind: e.kind,
            secret: e.secret,
            region: e.region,
        })
        .collect())
}

/// 回写校验状态（provider 额度查询完成后调用；provider_quota 骨架统一回写）。
pub(crate) fn record_check(
    provider: &str,
    id: &str,
    status: &str,
    message: Option<&str>,
) -> Result<(), String> {
    if !matches!(status, "ok" | "error") {
        return Err("无效的校验状态".to_string());
    }
    let _guard = provider_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_provider(provider)?;
    let idx = find_entry(&file, id).ok_or_else(|| "未找到该凭证".to_string())?;
    file.entries[idx].last_check = Some(CredentialCheck {
        status: status.to_string(),
        at: now_ms(),
        message: message.map(|m| m.chars().take(200).collect()),
    });
    save_provider(provider, &file)
}

// ============================================================
// Tauri commands
// ============================================================
// 全部文件 IO，统一 async + spawn_blocking（与 accounts 命令同款纪律），
// 避免同步命令在主线程执行时拖慢托盘/窗口事件。

/// 列出某 provider 的全部凭证（仅元数据 + 掩码，不含明文 secret）。
#[tauri::command]
pub async fn list_provider_credentials(
    provider: String,
) -> Result<Vec<CredentialMeta>, String> {
    tauri::async_runtime::spawn_blocking(move || list_meta(&provider))
        .await
        .map_err(|e| format!("读取凭证列表任务失败: {e}"))?
}

/// 添加一条凭证（secret 去首尾空白；label 为空时默认「凭证 N」）。
#[tauri::command]
pub async fn add_provider_credentials(
    provider: String,
    label: String,
    kind: String,
    secret: String,
    region: Option<String>,
) -> Result<CredentialMeta, String> {
    tauri::async_runtime::spawn_blocking(move || {
        add_entry(&provider, &label, &kind, &secret, region.as_deref())
    })
    .await
    .map_err(|e| format!("添加凭证任务失败: {e}"))?
}

/// 重置某 provider 的凭证文件（自愈损坏文件的入口；清除该服务全部已存
/// 凭证，前端二次确认后调用）。
#[tauri::command]
pub async fn reset_provider_credentials(provider: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || reset_provider(&provider))
        .await
        .map_err(|e| format!("重置凭证文件任务失败: {e}"))?
}

/// 更新凭证（secret 传 None/空串 = 不变更；编辑回显只有掩码，明文不外发）。
#[tauri::command]
pub async fn update_provider_credential(
    provider: String,
    id: String,
    label: Option<String>,
    secret: Option<String>,
    region: Option<String>,
) -> Result<CredentialMeta, String> {
    tauri::async_runtime::spawn_blocking(move || {
        update_entry(
            &provider,
            &id,
            label.as_deref(),
            secret.as_deref(),
            region.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("更新凭证任务失败: {e}"))?
}

/// 删除一条凭证（仅删本应用保存的记录，不动 provider 服务端）。
#[tauri::command]
pub async fn remove_provider_credentials(
    provider: String,
    id: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || remove_entry(&provider, &id))
        .await
        .map_err(|e| format!("删除凭证任务失败: {e}"))?
}

/// 回写最近一次校验状态（额度查询模块成功/失败后调用）。
#[tauri::command]
pub async fn record_credential_check(
    provider: String,
    id: String,
    status: String,
    message: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        record_check(&provider, &id, &status, message.as_deref())
    })
    .await
    .map_err(|e| format!("回写校验状态任务失败: {e}"))?
}

/// 该 provider 是否已有凭证（前端「有凭证自动显示 tab」判断）。
#[tauri::command]
pub async fn has_provider_credentials(provider: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || has_credentials(&provider))
        .await
        .map_err(|e| format!("查询凭证存在任务失败: {e}"))?
}

// ============================================================
// 单元测试（纯函数部分；文件 IO 走 config_dir 不在测试范围）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // 端到端文件链路（手写 moonshot.json → has/list/mask → 删除 → presence
    // 翻转 → UI 添加路径 → 清理）已在开发机用临时测试实测通过后移除，
    // 避免长期测试反复写真实 ~/.zbar/credentials/ 用户目录。

    #[test]
    fn provider_id_validation() {
        assert!(valid_provider("moonshot"));
        assert!(valid_provider("alibabatoken"));
        assert!(!valid_provider("")); // 空
        assert!(!valid_provider("Moonshot")); // 大写
        assert!(!valid_provider("my-provider")); // 连字符（防路径注入）
        assert!(!valid_provider("../zbar")); // 路径穿越
        assert!(!valid_provider("a".repeat(33).as_str())); // 超长
    }

    #[test]
    fn secret_mask_never_leaks_full_value() {
        // 长串：前 6 后 4
        assert_eq!(mask_secret("sk-1234567890abcdef"), "sk-123…cdef");
        // 短串：只露首尾 2 位
        assert_eq!(mask_secret("abc"), "ab…bc");
        assert_eq!(mask_secret("1234567890"), "12…90");
        assert_eq!(mask_secret(""), "…");
        // 掩码永远短于明文（长串 6+1+4=11 < 明文）
        let m = mask_secret("sk-very-long-secret-value");
        assert!(m.chars().count() < "sk-very-long-secret-value".chars().count());
    }

    #[test]
    fn label_defaults_and_truncation() {
        assert_eq!(normalize_label("", 2), "凭证 2");
        assert_eq!(normalize_label("  ", 3), "凭证 3");
        assert_eq!(normalize_label(" Pro 订阅 ", 1), "Pro 订阅");
        let long = "x".repeat(50);
        assert_eq!(normalize_label(&long, 1).chars().count(), LABEL_MAX_CHARS);
    }

    #[test]
    fn secret_must_be_nonempty_after_trim() {
        assert!(normalize_secret("  ").is_err());
        assert!(normalize_secret(" sk-xxx ").is_ok());
        assert_eq!(normalize_secret(" sk-xxx ").unwrap(), "sk-xxx");
    }

    #[test]
    fn add_region_empty_normalizes_to_none() {
        // 无区域下拉的服务（前端 region 初始为空串）原样提交 Some("")：
        // 规范化为 None 后 valid_region 通过，添加不再报「无效的区域值」，
        // 入库 region 存 None
        assert_eq!(normalize_region(Some("")), None);
        assert_eq!(normalize_region(Some("   ")), None);
        assert_eq!(normalize_region(None), None);
        assert!(valid_region(normalize_region(Some("")).as_deref()));
        // 带空白的合法值 trim 后保留；非法值仍原样返回（由 valid_region 拒绝）
        assert_eq!(normalize_region(Some(" cn ")).as_deref(), Some("cn"));
        assert_eq!(normalize_region(Some("global")).as_deref(), Some("global"));
    }

    #[test]
    fn add_rejects_unknown_region_before_io() {
        // 非法区域值仍报错；region 校验位于 secret 校验与文件 IO 之前
        // （secret 传空串即证明未触达 normalize_secret 之后的写盘路径）
        let err = add_entry("deepseek", "x", "apiKey", "", Some("xx")).unwrap_err();
        assert_eq!(err, "无效的区域值");
    }

    #[test]
    fn disk_json_shape_round_trip() {
        // 磁盘契约：snake_case + version/entries（与手写的测试文件兼容）
        let raw = r#"{
            "version": 1,
            "entries": [
                {
                    "id": "abc-1",
                    "label": "Pro 订阅",
                    "kind": "apiKey",
                    "secret": "sk-1234567890abcdef",
                    "region": null,
                    "created_at": 1730000000000,
                    "updated_at": 1730000000000,
                    "last_check": null
                }
            ]
        }"#;
        let file: ProviderFile = serde_json::from_str(raw).unwrap();
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].kind, "apiKey");
        // 前端元数据契约：camelCase + 掩码
        let meta = CredentialMeta::from(&file.entries[0]);
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["maskedSecret"], "sk-123…cdef");
        assert_eq!(json["createdAt"], serde_json::json!(1730000000000i64));
        assert!(json.get("secret").is_none());
    }
}

//! 多设备同步：配置读写 + 增量上传 + HTTP 调用 + 后台同步线程。
//!
//! 设计要点（见 server/README.md）：
//! - model_usage 是 append-only，用 (device_id, local_rowid) 去重。
//! - 客户端维护游标 last_uploaded_rowid，只上传 rowid > 游标 的记录。
//! - 复用项目现有 ureq HTTP 客户端 + pricing::config_dir() 的 ~/.zbar/ 目录。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::db::{self, UsageRow};
use crate::pricing::config_dir;

// ===== 配置（~/.zbar/sync.json）=====

/// 同步模式。
/// - manual：仅手动点「立即同步」时上传。
/// - auto：后台线程按 interval_seconds 自动上传。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    Manual,
    Auto,
}

impl Default for SyncMode {
    fn default() -> Self {
        SyncMode::Manual
    }
}

/// 同步配置。master_token 不持久化（注册完即丢）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: SyncMode,
    /// auto 模式间隔（秒），默认 60。
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub device_token: String,
    /// 已上传到的本机 rowid 游标。
    #[serde(default)]
    pub last_uploaded_rowid: i64,
    /// 上次成功同步的毫秒时间戳。
    #[serde(default)]
    pub last_sync_at: i64,
}

fn default_interval() -> u64 {
    60
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SyncMode::Manual,
            interval_seconds: default_interval(),
            server_url: String::new(),
            device_id: String::new(),
            device_name: String::new(),
            device_token: String::new(),
            last_uploaded_rowid: 0,
            last_sync_at: 0,
        }
    }
}

/// 配置文件路径：~/.zbar/sync.json
pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("sync.json"))
}

/// 读取同步配置；文件不存在返回默认空配置（不报错）。
pub fn load_sync_config() -> Result<SyncConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(SyncConfig::default());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取同步配置失败: {e}"))?;
    serde_json::from_str::<SyncConfig>(&data)
        .map_err(|e| format!("解析同步配置失败: {e}"))
}

/// 写入同步配置。
pub fn save_sync_config(cfg: &SyncConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化同步配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入同步配置失败: {e}"))
}

// ===== HTTP 请求/响应结构（与服务端对齐）=====

#[derive(Debug, Deserialize)]
pub struct RegisterResponse {
    pub device_id: String,
    pub device_token: String,
    pub device_name: String,
}

#[derive(Debug, Deserialize)]
struct SyncResponse {
    accepted: usize,
    max_rowid: i64,
}

#[derive(Debug, Serialize)]
struct SyncPayload {
    records: Vec<UsageRow>,
    last_rowid: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfoRaw {
    device_id: String,
    device_name: String,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    record_count: Option<i64>,
}

/// /usage 返回的远端聚合（与服务端 UsageResult 对齐，字段名一致）。
/// 前端合并本地 + 远端时用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteUsage {
    pub from_ms: i64,
    pub to_ms: i64,
    pub overall: RemoteOverall,
    pub by_model: Vec<RemoteModelStat>,
    pub trend: Vec<RemoteTrendBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteOverall {
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteModelStat {
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTrendBucketModel {
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTrendBucket {
    pub label: String,
    #[serde(default)]
    pub by_model: Vec<RemoteTrendBucketModel>,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub requests: i64,
}

// 清理相关
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupStatus {
    pub total_records: i64,
    pub devices: Vec<DeviceInfoRaw>,
    pub auto_config: AutoCleanupConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutoCleanupConfig {
    #[serde(default)]
    pub auto_enabled: bool,
    #[serde(default)]
    pub auto_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupResult {
    pub action: String,
    pub records_deleted: i64,
    #[serde(default)]
    pub devices_deleted: Option<i64>,
}

// ===== 注册 =====

/// 向服务器注册设备。验证 master_token 成功后返回完整配置（含 device_token）。
/// master_token 不写入配置文件。
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub server_url: String,
    pub master_token: String,
    pub device_name: String,
}

pub fn register_device(req: RegisterRequest) -> Result<SyncConfig, String> {
    let base = normalize_url(&req.server_url)?;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        device_name: &'a str,
    }
    let resp: RegisterResponse = ureq::post(&format!("{base}/register"))
        .timeout(Duration::from_secs(10))
        .send_json(Body {
            master_token: req.master_token.trim(),
            device_name: req.device_name.trim(),
        })
        .map_err(map_http_err("注册设备"))?
        .into_json()
        .map_err(|e| format!("解析注册响应失败: {e}"))?;

    // 读取现有配置，保留游标等字段
    let mut cfg = load_sync_config()?;
    cfg.enabled = true;
    cfg.server_url = base;
    cfg.device_id = resp.device_id;
    cfg.device_name = resp.device_name;
    cfg.device_token = resp.device_token;
    // 注册不重置游标，沿用已有值（首次为 0，会全量上传）
    save_sync_config(&cfg)?;
    Ok(cfg)
}

// ===== 增量上传 =====

/// 一次同步的结果（供 UI 显示）。
#[derive(Debug, Clone, Serialize)]
pub struct SyncOutcome {
    pub uploaded: usize,
    pub new_max_rowid: i64,
    pub last_sync_at: i64,
}

/// 执行一次增量上传：循环分批上传直到无新数据。
/// 返回总上传条数。失败时返回 Err，游标不前进（下次重试）。
pub fn upload_incremental() -> Result<SyncOutcome, String> {
    let mut cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;

    let mut since = cfg.last_uploaded_rowid;
    let mut total_uploaded = 0usize;
    const BATCH: usize = 500;

    loop {
        let records = db::query_since(since, BATCH)?;
        if records.is_empty() {
            break;
        }
        // 本批最大 rowid（游标必须至少推进到这里，否则死循环）
        let batch_max = records.last().map(|r| r.local_rowid).unwrap_or(since);
        let payload = SyncPayload {
            records,
            last_rowid: Some(batch_max),
        };
        let resp: SyncResponse = ureq::post(&format!("{base}/sync"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(15))
            .send_json(&payload)
            .map_err(map_http_err("上传数据"))?
            .into_json()
            .map_err(|e| format!("解析上传响应失败: {e}"))?;

        total_uploaded += resp.accepted;
        // 游标必须推进到本批最大 rowid（无论服务端是否接受，本地都已处理过这些记录）。
        // 取 max 防止服务端返回的旧游标回退。
        since = resp.max_rowid.max(batch_max);
    }

    let now = chrono::Local::now().timestamp_millis();
    cfg.last_uploaded_rowid = since;
    cfg.last_sync_at = now;
    save_sync_config(&cfg)?;

    Ok(SyncOutcome {
        uploaded: total_uploaded,
        new_max_rowid: since,
        last_sync_at: now,
    })
}

// ===== 远端查询 =====

#[derive(Debug, Deserialize)]
pub struct RemoteUsageRequest {
    pub from_ms: i64,
    pub to_ms: i64,
    /// "hour" 或 "day"
    pub bucket: String,
    /// 排除本机设备（避免重复计算本机数据），逗号分隔多个 device_id。
    /// 为空表示不排除（查全部）。
    #[serde(default)]
    pub exclude_device: String,
    /// 仅查询这些设备（逗号分隔），优先级高于 exclude_device。
    /// 用于设备筛选器选具体设备时只查它。
    #[serde(default)]
    pub devices: String,
}

/// 拉取远端聚合数据。
/// - devices 非空：只查指定设备
/// - 否则 exclude_device 非空：排除本机，拿其他设备（与本机合并）
/// - 都空：查全部
pub fn fetch_remote_usage(req: RemoteUsageRequest) -> Result<RemoteUsage, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;

    let mut url = format!(
        "{base}/usage?from_ms={}&to_ms={}&bucket={}",
        req.from_ms, req.to_ms, req.bucket
    );
    if !req.devices.is_empty() {
        url.push_str(&format!("&devices={}", req.devices));
    } else if !req.exclude_device.is_empty() {
        url.push_str(&format!("&exclude_device={}", req.exclude_device));
    }

    let resp: RemoteUsage = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(map_http_err("查询远端数据"))?
        .into_json()
        .map_err(|e| format!("解析远端数据失败: {e}"))?;
    Ok(resp)
}

/// 拉取设备列表（供前端设备筛选器）。
pub fn fetch_devices() -> Result<Vec<DeviceInfo>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let raw: Vec<DeviceInfoRaw> = ureq::get(&format!("{base}/devices"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(map_http_err("查询设备列表"))?
        .into_json()
        .map_err(|e| format!("解析设备列表失败: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|r| DeviceInfo {
            device_id: r.device_id,
            device_name: r.device_name,
            created_at: r.created_at,
            record_count: r.record_count,
        })
        .collect())
}

// ===== 清理 =====

/// 转发清理请求到服务器。master_token 由客户端临时持有（从 UI 输入）。
#[derive(Debug, Deserialize)]
pub struct CleanupServerRequest {
    pub master_token: String,
    pub action: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub days: u32,
}

pub fn cleanup_server(req: CleanupServerRequest) -> Result<CleanupResult, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        action: &'a str,
        device_id: &'a str,
        days: u32,
    }
    let resp: CleanupResult = ureq::post(&format!("{base}/cleanup"))
        .timeout(Duration::from_secs(10))
        .send_json(Body {
            master_token: req.master_token.trim(),
            action: &req.action,
            device_id: &req.device_id,
            days: req.days,
        })
        .map_err(map_http_err("执行清理"))?
        .into_json()
        .map_err(|e| format!("解析清理响应失败: {e}"))?;
    Ok(resp)
}

/// 查询清理状态（数据量 + 自动清理配置）。device_token 鉴权。
pub fn fetch_cleanup_status() -> Result<CleanupStatus, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let resp: CleanupStatus = ureq::get(&format!("{base}/cleanup/status"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(map_http_err("查询清理状态"))?
        .into_json()
        .map_err(|e| format!("解析清理状态失败: {e}"))?;
    Ok(resp)
}

/// 配置服务端自动清理。
#[derive(Debug, Deserialize)]
pub struct AutoCleanupServerRequest {
    pub master_token: String,
    pub auto_enabled: bool,
    pub auto_days: u32,
}

pub fn set_auto_cleanup(req: AutoCleanupServerRequest) -> Result<AutoCleanupConfig, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body {
        master_token: String,
        auto_enabled: bool,
        auto_days: u32,
    }
    let resp: AutoCleanupConfig = ureq::post(&format!("{base}/cleanup/config"))
        .timeout(Duration::from_secs(10))
        .send_json(Body {
            master_token: req.master_token,
            auto_enabled: req.auto_enabled,
            auto_days: req.auto_days,
        })
        .map_err(map_http_err("配置自动清理"))?
        .into_json()
        .map_err(|e| format!("解析自动清理配置失败: {e}"))?;
    Ok(resp)
}

// ===== 断开连接 =====

/// 清空凭证（保留 enabled=false），不删服务器数据。
pub fn disconnect() -> Result<(), String> {
    let mut cfg = load_sync_config()?;
    cfg.enabled = false;
    cfg.device_id.clear();
    cfg.device_name.clear();
    cfg.device_token.clear();
    cfg.last_uploaded_rowid = 0;
    cfg.last_sync_at = 0;
    // 保留 server_url + mode + interval，方便下次重连
    save_sync_config(&cfg)
}

// ===== 后台同步线程 =====

/// 启动后台同步线程：若 mode=auto 且 enabled，按 interval_seconds 循环上传。
/// 模仿 lib.rs 的 spawn_title_updater 模式。静默失败不影响本地展示。
pub fn spawn_sync_worker() {
    std::thread::spawn(|| loop {
        let cfg = load_sync_config().unwrap_or_default();
        if cfg.enabled && cfg.mode == SyncMode::Auto {
            // 静默失败：出错只记不抛，下次重试
            if let Err(e) = upload_incremental() {
                eprintln!("[zbar-sync] 后台同步失败: {e}");
            }
        }
        let sleep_secs = if cfg.enabled && cfg.mode == SyncMode::Auto {
            cfg.interval_seconds.max(10)
        } else {
            // 非自动模式：每 60s 检查一次配置变化（用户可能切到 auto）
            60
        };
        std::thread::sleep(Duration::from_secs(sleep_secs));
    });
}

// ===== 辅助 =====

/// 规范化服务器 URL：去尾斜杠，补 scheme。
fn normalize_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("服务器地址不能为空".into());
    }
    // 允许 http:// https:// 或裸 IP/域名（无 scheme 时默认 http://）
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("http://{trimmed}"))
    }
}

/// 把 ureq::Error 转为可读中文错误。
fn map_http_err(op: &str) -> impl Fn(ureq::Error) -> String + '_ {
    let op = op.to_string();
    move |e: ureq::Error| {
        let detail = match e {
            ureq::Error::Status(code, resp) => {
                let body = resp.into_string().unwrap_or_default();
                format!("HTTP {code}: {body}")
            }
            ureq::Error::Transport(t) => {
                let kind = t.kind();
                let msg = t.message().unwrap_or("").to_string();
                format!("{kind:?}: {msg}")
            }
        };
        format!("{op}失败: {detail}")
    }
}

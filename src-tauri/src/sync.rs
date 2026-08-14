//! 多设备同步：配置读写 + 增量上传 + HTTP 调用 + 后台同步线程。
//!
//! 设计要点（见 server/README.md）：
//! - model_usage 是 append-only，用 (device_id, local_rowid) 去重。
//! - 客户端维护游标 last_uploaded_rowid，只上传 rowid > 游标 的记录。
//! - 复用项目现有 ureq HTTP 客户端 + pricing::config_dir() 的 ~/.zbar/ 目录。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::db::{self, default_source, UsageRow};
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
    /// 已上传到的 Codex 导入库 rowid 游标（与 zcode 游标相互独立）。
    #[serde(default)]
    pub last_uploaded_codex_rowid: i64,
    /// 已上传到的 Claude 导入库 rowid 游标（与上面两个游标相互独立）。
    #[serde(default)]
    pub last_uploaded_claude_rowid: i64,
    /// Claude 修订行补传游标（updated_at 毫秒时间戳，见 claude 模块修订机制）。
    #[serde(default)]
    pub last_uploaded_claude_rev_ts: i64,
    /// 已上传到的快照 ts 游标（额度快照）。
    #[serde(default)]
    pub last_uploaded_snapshot_ts: i64,
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
            last_uploaded_codex_rowid: 0,
            last_uploaded_claude_rowid: 0,
            last_uploaded_claude_rev_ts: 0,
            last_uploaded_snapshot_ts: 0,
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
    /// 新版服务端返回；旧服务端不返回 → 用默认 0。保留字段以正确反序列化协议。
    #[serde(default)]
    #[allow(dead_code)]
    accepted_snapshots: usize,
    #[serde(default)]
    max_snapshot_ts: Option<i64>,
    /// 服务端协议版本：2 = 支持多来源（usage_records 含 source 列）。
    /// 旧服务端不返回 → 0。codex 上传前据此探测，防止旧服务端按
    /// (device_id, local_rowid) 撞键静默丢弃记录后游标仍推进。
    #[serde(default)]
    proto: u32,
}

#[derive(Debug, Serialize)]
struct SyncPayload {
    records: Vec<UsageRow>,
    last_rowid: Option<i64>,
    /// 额度快照（可选；旧服务端会忽略，向后兼容）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    snapshots: Vec<crate::quota_history::QuotaSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_snapshot_ts: Option<i64>,
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
    /// 数据来源（新版服务端返回；旧服务端不返回时默认 zcode）
    #[serde(default = "default_source")]
    pub source: String,
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

/// 执行一次增量上传（三阶段）：先 zcode 明细（含额度快照），再 Codex、Claude
/// 两个派生库的明细。各来源游标相互独立：
/// - zcode 失败：整体返回 Err，游标不前进（下次重试），与原有行为一致；
/// - codex/claude 失败：不阻断已成功来源的结果，错误仅记日志，游标停在最近
///   成功批次并随配置落盘（下次从断点续传）。
pub fn upload_incremental() -> Result<SyncOutcome, String> {
    let mut cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = cfg.server_url.clone();
    let token = cfg.device_token.clone();

    // 读取待上传的快照（ts > 游标）。读失败不阻断明细同步。
    let mut pending_snaps = crate::quota_history::load_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.ts > cfg.last_uploaded_snapshot_ts)
        .collect::<Vec<_>>();
    let snap_max_ts = pending_snaps.iter().map(|s| s.ts).max();
    let mut snapshot_cursor_advanced = false;

    // ===== 阶段一：zcode 明细（records 固定 source="zcode"）=====
    let mut since = cfg.last_uploaded_rowid;
    let mut total_uploaded = 0usize;
    const BATCH: usize = 500;
    let mut first_batch = true;

    loop {
        let records = db::query_since(since, BATCH)?;
        // 明细耗尽，且还有未发的快照 → 发一个空 records 的批次把快照送出
        let records_empty = records.is_empty();
        if records_empty && pending_snaps.is_empty() {
            break;
        }
        if records_empty && !first_batch {
            // 明细已发完，快照在首批已随带；无更多数据
            break;
        }
        // 本批最大 rowid（游标必须至少推进到这里，否则死循环）
        let batch_max = records.last().map(|r| r.local_rowid).unwrap_or(since);

        // 快照只在首批携带（一次性发完）；之后清空避免重复发
        let snaps_to_send = if first_batch {
            std::mem::take(&mut pending_snaps)
        } else {
            Vec::new()
        };
        let last_snapshot_ts = if first_batch { snap_max_ts } else { None };

        let payload = SyncPayload {
            records,
            last_rowid: Some(batch_max),
            snapshots: snaps_to_send,
            last_snapshot_ts,
        };
        let resp: SyncResponse = ureq::post(&format!("{base}/sync"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(15))
            .send_json(&payload)
            .map_err(map_http_err("上传数据"))?
            .into_json()
            .map_err(|e| format!("解析上传响应失败: {e}"))?;

        total_uploaded += resp.accepted;
        // 快照游标：取服务端返回值（新服务端）或本批最大 ts（旧服务端不回填）。
        if first_batch {
            if let Some(ts) = resp.max_snapshot_ts {
                cfg.last_uploaded_snapshot_ts = ts;
            } else if let Some(ts) = snap_max_ts {
                cfg.last_uploaded_snapshot_ts = ts;
            }
            snapshot_cursor_advanced = true;
        }
        // 游标必须推进到本批最大 rowid（无论服务端是否接受，本地都已处理过这些记录）。
        // 取 max 防止服务端返回的旧游标回退。
        since = resp.max_rowid.max(batch_max);
        first_batch = false;
        // 明细空 + 快照发完 → 结束
        if records_empty {
            break;
        }
    }

    // ===== 阶段二：Codex 明细（records 固定 source="codex"，失败不阻断）=====
    let mut codex_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "codex") {
        Ok(n) => codex_uploaded = n,
        Err(e) => {
            // 游标已随每批成功推进（见函数内），稍后随配置落盘，下次从断点续传
            eprintln!("[zbar-sync] Codex 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段三：Claude 明细（records 固定 source="claude"，失败不阻断）=====
    let mut claude_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "claude") {
        Ok(n) => claude_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] Claude 增量上传失败（下次重试）: {e}");
        }
    }

    let _ = snapshot_cursor_advanced; // 标记已用，避免未读警告
    let now = chrono::Local::now().timestamp_millis();
    cfg.last_uploaded_rowid = since;
    cfg.last_sync_at = now;
    save_sync_config(&cfg)?;

    Ok(SyncOutcome {
        uploaded: total_uploaded + codex_uploaded + claude_uploaded,
        new_max_rowid: since,
        last_sync_at: now,
    })
}

/// 上传派生来源（codex/claude 导入库）的增量（id > 对应游标，循环分批）。
/// 查询前由各模块自行增量导入（原始 jsonl → 派生 sqlite）。两条路径完全同构，
/// 仅数据源与游标字段不同，故按 source 参数泛化：
/// - "codex" → crate::codex + last_uploaded_codex_rowid
/// - "claude" → crate::claude + last_uploaded_claude_rowid
/// 未安装对应 CLI 时静默返回 0（不算错误，避免后台同步每轮刷错误日志）。
/// 首批上传前先探测服务端协议版本（proto ≥ 2 才支持多来源）：
/// 旧服务端无 source 列，记录会按 (device_id, local_rowid) 撞键静默丢弃
/// 且游标推进 → 数据永久丢失，故版本不足时不推进游标直接报错，
/// 升级服务端后自动恢复上传。成功的每批都推进游标（部分失败也保留断点，
/// 由调用方负责落盘）。返回本次成功上传的条数。
fn upload_derived_source_incremental(
    cfg: &mut SyncConfig,
    base: &str,
    token: &str,
    source: &str,
) -> Result<usize, String> {
    let (dir_exists, has_pending, cursor): (bool, bool, i64) = match source {
        "codex" => {
            let dir = crate::codex::sessions_dir().is_ok();
            let pending = dir
                && !crate::codex::query_since(cfg.last_uploaded_codex_rowid, 1)?.is_empty();
            (dir, pending, cfg.last_uploaded_codex_rowid)
        }
        "claude" => {
            let dir = crate::claude::projects_dir().is_ok();
            let pending = dir
                && !crate::claude::query_since(cfg.last_uploaded_claude_rowid, 1)?.is_empty();
            (dir, pending, cfg.last_uploaded_claude_rowid)
        }
        _ => return Err(format!("未知数据来源: {source}")),
    };
    // 未安装对应 CLI：无会话目录，静默跳过（不算错误，避免后台同步每轮刷日志）
    if !dir_exists {
        return Ok(0);
    }
    if !has_pending {
        return Ok(0); // 无待上传数据，跳过（也免去协议探测请求）
    }

    // 协议探测：空批次请求无任何写入副作用，仅读回服务端能力标记
    let probe: SyncResponse = ureq::post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(15))
        .send_json(&SyncPayload {
            records: Vec::new(),
            last_rowid: None,
            snapshots: Vec::new(),
            last_snapshot_ts: None,
        })
        .map_err(map_http_err("探测服务端协议"))?
        .into_json()
        .map_err(|e| format!("解析协议探测响应失败: {e}"))?;
    if probe.proto < 2 {
        return Err(format!(
            "服务端版本过旧（不支持多来源同步），{source} 数据暂不上传，请升级服务端 zbar-sync 后重试"
        ));
    }

    let mut since = cursor;
    let mut total = 0usize;
    const BATCH: usize = 500;
    loop {
        let records = match source {
            "codex" => crate::codex::query_since(since, BATCH)?,
            "claude" => crate::claude::query_since(since, BATCH)?,
            _ => unreachable!("来源已在上文校验"),
        };
        if records.is_empty() {
            break;
        }
        // 本批最大 rowid（游标必须至少推进到这里，否则死循环）
        let batch_max = records.last().map(|r| r.local_rowid).unwrap_or(since);
        let payload = SyncPayload {
            records,
            last_rowid: Some(batch_max),
            snapshots: Vec::new(),
            last_snapshot_ts: None,
        };
        let resp: SyncResponse = ureq::post(&format!("{base}/sync"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(15))
            .send_json(&payload)
            .map_err(map_http_err("上传数据"))?
            .into_json()
            .map_err(|e| format!("解析上传响应失败: {e}"))?;
        total += resp.accepted;
        since = resp.max_rowid.max(batch_max);
        match source {
            "codex" => cfg.last_uploaded_codex_rowid = since,
            "claude" => cfg.last_uploaded_claude_rowid = since,
            _ => unreachable!("来源已在上文校验"),
        }
    }

    // Claude 修订补传：会话流式落盘时，某条消息的中间值可能已随早前批次上传，
    // 终值稍后覆盖本地行（id 不变）→ 上面的 id 游标永远选不出它。这里按
    // updated_at 选出修订行重传，新版服务端以"总量更大者胜" upsert 覆盖修正；
    // 旧服务端（INSERT OR IGNORE）会忽略重传——退化为旧行为（远端保留中间值），
    // 无数据损坏，无需协议协商。after_id 逐批推进防止同批重复选出。
    if source == "claude" {
        let sweep_watermark = chrono::Local::now().timestamp_millis();
        let mut after_id = 0i64;
        loop {
            let records =
                crate::claude::query_revised_since(cfg.last_uploaded_claude_rev_ts, after_id, BATCH)?;
            if records.is_empty() {
                break;
            }
            after_id = records.last().map(|r| r.local_rowid).unwrap_or(after_id);
            let resp: SyncResponse = ureq::post(&format!("{base}/sync"))
                .set("Authorization", &format!("Bearer {token}"))
                .timeout(Duration::from_secs(15))
                .send_json(&SyncPayload {
                    records,
                    // 修订行都是已上传过的 rowid，不携带 last_rowid 以免扰动
                    // 服务端对该来源游标状态的判断
                    last_rowid: None,
                    snapshots: Vec::new(),
                    last_snapshot_ts: None,
                })
                .map_err(map_http_err("上传 Claude 修订数据"))?
                .into_json()
                .map_err(|e| format!("解析上传响应失败: {e}"))?;
            total += resp.accepted;
        }
        // 推进到补传开始前的水位：补传期间新发生的修订（updated_at 更大）
        // 下一轮自然选出
        cfg.last_uploaded_claude_rev_ts = sweep_watermark;
    }
    Ok(total)
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
    /// 数据来源过滤："zcode" | "codex" | "claude"，空 = 全部来源。
    #[serde(default)]
    pub source: String,
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
    if !req.source.is_empty() {
        url.push_str(&format!("&source={}", req.source));
    }
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

// ===== 远端模型清单（价格设置页 / 价格检查用）=====

/// 远端一条模型记录（全部设备 × 全部来源的去重并集）
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteModelInfo {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model_id: String,
}

/// 远端模型清单缓存，条目 = (最近一次尝试时间, 清单)。
/// - 成功后 TTL_REMOTE_MODELS（5 分钟）内直接复用；
/// - 拉取失败写入短负缓存 TTL_REMOTE_MODELS_RETRY（30 秒），期间用旧值/空值顶住，
///   避免服务器不可达时设置页每次打开都等一轮 3s 超时（list_models 与
///   check_pricing_updates 首开即并发调用，无负缓存最坏叠加约 6s）。
static REMOTE_MODELS_CACHE: OnceLock<Mutex<Option<(Instant, Vec<RemoteModelInfo>)>>> =
    OnceLock::new();
const TTL_REMOTE_MODELS: Duration = Duration::from_secs(300);
const TTL_REMOTE_MODELS_RETRY: Duration = Duration::from_secs(30);

/// 实际请求服务端 GET /models（轻量 distinct 清单）。
fn fetch_remote_models_once() -> Result<Vec<RemoteModelInfo>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(Deserialize)]
    struct ModelsResponse {
        #[serde(default)]
        models: Vec<RemoteModelInfo>,
    }
    let resp: ModelsResponse = ureq::get(&format!("{}/models", cfg.server_url))
        .set("Authorization", &format!("Bearer {}", cfg.device_token))
        // 清单接口很轻，超时收紧到 3s：避免服务器不可达时拖慢设置页首次加载
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(map_http_err("查询远端模型清单"))?
        .into_json()
        .map_err(|e| format!("解析远端模型清单失败: {e}"))?;
    Ok(resp
        .models
        .into_iter()
        .filter(|m| !m.model_id.is_empty())
        .collect())
}

/// 远端模型清单（带缓存、负缓存与静默降级），绝不向调用方报错：
/// - 未启用同步 → 恒为空且不写缓存（刚注册设备后立即可见远端模型）；
/// - 拉取失败 → 30s 内不重试，期间沿用上次清单（含过期值）；
/// - 完全无缓存且失败 → 空列表。
pub fn remote_models_cached() -> Vec<RemoteModelInfo> {
    // 未启用同步直接短路，不触碰缓存
    let enabled = load_sync_config()
        .map(|c| c.enabled && !c.device_token.is_empty())
        .unwrap_or(false);
    if !enabled {
        return Vec::new();
    }

    let cache = REMOTE_MODELS_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((last_attempt, models)) = guard.as_ref() {
        // 有数据：5 分钟内复用；空数据（上次失败）：30s 负缓存内不重试
        let ttl = if models.is_empty() {
            TTL_REMOTE_MODELS_RETRY
        } else {
            TTL_REMOTE_MODELS
        };
        if last_attempt.elapsed() < ttl {
            return models.clone();
        }
    }
    match fetch_remote_models_once() {
        Ok(models) => {
            *guard = Some((Instant::now(), models.clone()));
            models
        }
        Err(_) => {
            // 失败：只推进尝试时间戳，保留旧清单继续顶住（无旧值则为空）
            if let Some((last_attempt, models)) = guard.as_mut() {
                *last_attempt = Instant::now();
                models.clone()
            } else {
                *guard = Some((Instant::now(), Vec::new()));
                Vec::new()
            }
        }
    }
}

// ===== 远端额度快照查询（对比页/报告页用）=====

/// 远端单条快照（带 device_id，供前端按设备筛选）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSnapshot {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub ts: i64,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub weekly_pct: u32,
    #[serde(default)]
    pub weekly_reset: Option<i64>,
    #[serde(default)]
    pub hour5_pct: u32,
    #[serde(default)]
    pub mcp_pct: u32,
    #[serde(default)]
    pub mcp_used: Option<i64>,
    #[serde(default)]
    pub mcp_total: Option<i64>,
}

/// /snapshots 返回包装
#[derive(Debug, Deserialize)]
struct SnapshotsResponse {
    #[serde(default)]
    snapshots: Vec<RemoteSnapshot>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RemoteSnapshotRequest {
    pub from_ms: i64,
    pub to_ms: i64,
    #[serde(default)]
    pub exclude_device: String,
    #[serde(default)]
    pub devices: String,
}

/// 拉取远端额度快照（带 device_id）。
pub fn fetch_remote_snapshots(req: RemoteSnapshotRequest) -> Result<Vec<RemoteSnapshot>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;

    let mut url = format!(
        "{base}/snapshots?from_ms={}&to_ms={}",
        req.from_ms, req.to_ms
    );
    if !req.devices.is_empty() {
        url.push_str(&format!("&devices={}", req.devices));
    } else if !req.exclude_device.is_empty() {
        url.push_str(&format!("&exclude_device={}", req.exclude_device));
    }

    let resp: SnapshotsResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(map_http_err("查询远端快照"))?
        .into_json()
        .map_err(|e| format!("解析远端快照失败: {e}"))?;
    Ok(resp.snapshots)
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

// ===== 设备合并 / 改名 =====

#[derive(Debug, Deserialize)]
pub struct MergeDevicesRequest {
    pub master_token: String,
    pub source_device_id: String,
    pub target_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub records_moved: i64,
    pub snapshots_moved: i64,
}

/// 合并设备：把来源设备数据并入目标设备后删除来源。master token 鉴权。
pub fn merge_devices(req: MergeDevicesRequest) -> Result<MergeResult, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        source_device_id: &'a str,
        target_device_id: &'a str,
    }
    let resp: MergeResult = ureq::post(&format!("{base}/device/merge"))
        .timeout(Duration::from_secs(15))
        .send_json(Body {
            master_token: req.master_token.trim(),
            source_device_id: req.source_device_id.trim(),
            target_device_id: req.target_device_id.trim(),
        })
        .map_err(map_http_err("合并设备"))?
        .into_json()
        .map_err(|e| format!("解析合并响应失败: {e}"))?;
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct RenameDeviceRequest {
    pub master_token: String,
    pub device_id: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameResult {
    pub updated: i64,
}

/// 修改设备显示名。master token 鉴权。
pub fn rename_device(req: RenameDeviceRequest) -> Result<RenameResult, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        device_id: &'a str,
        device_name: &'a str,
    }
    let resp: RenameResult = ureq::post(&format!("{base}/device/rename"))
        .timeout(Duration::from_secs(10))
        .send_json(Body {
            master_token: req.master_token.trim(),
            device_id: req.device_id.trim(),
            device_name: req.device_name.trim(),
        })
        .map_err(map_http_err("改名设备"))?
        .into_json()
        .map_err(|e| format!("解析改名响应失败: {e}"))?;
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
    cfg.last_uploaded_codex_rowid = 0;
    cfg.last_uploaded_claude_rowid = 0;
    cfg.last_uploaded_claude_rev_ts = 0;
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

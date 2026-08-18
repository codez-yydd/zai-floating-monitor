//! Codex CLI 用量统计模块。
//!
//! 数据来源：Codex CLI 把每个会话记录在 ~/.codex/sessions/年/月/日/rollout-*.jsonl
//! （append-only，每行一个 JSON 事件）。token 用量在 type=event_msg 且
//! payload.type=token_count 的事件里，取 last_token_usage（单次 API 调用值，
//! 逐条求和 == 会话最后一条的 total_token_usage）。
//!
//! 实现方式（原始文件只读 + 派生自有库）：
//! - 原始 jsonl 不做任何修改；把解析结果导入自有库 ~/.zbar/codex.sqlite，
//!   file_progress 表记录每个文件"已处理到的字节偏移"，之后只增量续读。
//! - (session_id, event_seq) 唯一键 + 冲突 upsert：文件被重写或重复解析时幂等
//!   去重；历史空模型在重新归因后允许只更新模型名。
//! - token_count 事件本身不带模型名，用同文件内最近的 turn_context 事件
//!   payload.model 归因（会话开始前的事件模型留空 ""）。
//! - rate_limits 仅 ChatGPT 订阅登录模式非空（custom provider 全为 null），
//!   最新快照存 rate_limits_state 单行表，resets_at 由 Unix 秒转毫秒。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, Local, TimeZone};

use crate::agent_quota_history;
use crate::db;
use crate::pricing::config_dir;

// ===== 路径定位 =====

/// Codex sessions 目录路径（不做存在性检查，供诊断展示）。
/// 环境变量 ZBAR_CODEX_HOME（指向 .codex 根目录）优先，否则 ~/.codex/sessions。
fn sessions_dir_path() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_CODEX_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("sessions");
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".codex").join("sessions")
}

/// 定位 Codex 会话目录。目录不存在返回友好中文错误（调用方按需容错降级）。
pub fn sessions_dir() -> Result<PathBuf, String> {
    let p = sessions_dir_path();
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!(
            "未找到 Codex 会话目录: {}。请确认 Codex CLI 已安装并使用过，或设置 ZBAR_CODEX_HOME 环境变量指向 .codex 根目录。",
            p.display()
        ))
    }
}

/// 自有导入库路径：~/.zbar/codex.sqlite
fn codex_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("codex.sqlite"))
}

/// 打开（必要时创建）导入库并确保表结构就绪。这是自有库，读写均可用。
fn open_codex_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = codex_db_path()?;
    let conn = Connection::open(&path).map_err(|e| format!("打开 Codex 导入库失败: {e}"))?;
    // 应用可能在启动时被多个实例同时唤起，结构迁移需要给另一实例提交事务留出时间。
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS model_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            event_seq INTEGER NOT NULL,
            started_at INTEGER NOT NULL,
            model_id TEXT NOT NULL DEFAULT '',
            provider_id TEXT NOT NULL DEFAULT 'codex',
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            computed_total_tokens INTEGER NOT NULL DEFAULT 0,
            model_revision_at INTEGER NOT NULL DEFAULT 0,
            UNIQUE(session_id, event_seq)
         );
         CREATE INDEX IF NOT EXISTS idx_codex_model_usage_started ON model_usage(started_at);
         CREATE TABLE IF NOT EXISTS file_progress (
            path   TEXT    PRIMARY KEY,
            offset INTEGER NOT NULL,
            size   INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS rate_limits_state (
            id                 INTEGER PRIMARY KEY CHECK (id = 1),
            observed_at        INTEGER NOT NULL,
            plan_type          TEXT,
            primary_pct        REAL,
            primary_reset_at   INTEGER,
            secondary_pct      REAL,
            secondary_reset_at INTEGER
         );",
    )
    .map_err(|e| format!("初始化 Codex 导入库失败: {e}"))?;
    ensure_model_revision_column(&conn)?;
    Ok(conn)
}

/// 进程内串行化导入库结构迁移；跨进程竞态由 ALTER TABLE 的重复列容错兜底。
static SCHEMA_MIGRATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_migration_lock() -> &'static Mutex<()> {
    SCHEMA_MIGRATION_LOCK.get_or_init(|| Mutex::new(()))
}

/// 为已有导入库补充模型修订时间列。
///
/// 该列只在历史空模型被回填时写入，用于同步客户端把已经上传过的行再次补传。
/// 使用 PRAGMA + ALTER TABLE 而不是版本号，保证旧版数据库和重复启动都能幂等升级。
fn ensure_model_revision_column(conn: &Connection) -> Result<(), String> {
    let _guard = schema_migration_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut stmt = conn
        .prepare("PRAGMA table_info(model_usage)")
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?;
    drop(stmt);
    if !columns.iter().any(|c| c == "model_revision_at") {
        let result = conn.execute(
            "ALTER TABLE model_usage
             ADD COLUMN model_revision_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        if let Err(e) = result {
            // 另一个进程可能在本次 PRAGMA 与 ALTER 之间完成了迁移，
            // 此时重复列就是成功状态，后续查询可直接继续。
            let message = e.to_string().to_ascii_lowercase();
            if !message.contains("duplicate column name") {
                return Err(format!("升级 Codex 导入库结构失败: {e}"));
            }
        }
    }
    Ok(())
}

// ===== jsonl 事件解析结构（全部 Option/默认值容错，坏行只跳过不中断）=====

/// jsonl 单行事件。payload 类型随 type 不同，先存原始 Value 再按需二次解析。
#[derive(Debug, Deserialize)]
struct RolloutLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    line_type: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

/// event_msg 事件 payload（只关心 token_count 一种）
#[derive(Debug, Deserialize)]
struct TokenCountPayload {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    info: Option<TokenCountInfo>,
    #[serde(default)]
    rate_limits: Option<RateLimitsPayload>,
}

#[derive(Debug, Deserialize)]
struct TokenCountInfo {
    #[serde(default)]
    last_token_usage: Option<TokenUsage>,
}

/// 单次 API 调用的 token 用量（last_token_usage）。
/// 字段映射到 zcode 口径：cached_input_tokens → cache_read_input_tokens、
/// reasoning_output_tokens → reasoning_tokens、total_tokens → computed_total_tokens。
#[derive(Debug, Default, Deserialize)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    reasoning_output_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
}

/// ChatGPT 订阅额度快照（custom provider 模式全为 null → Option 化）
#[derive(Debug, Deserialize)]
struct RateLimitsPayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    primary: Option<RateWindowPayload>,
    #[serde(default)]
    secondary: Option<RateWindowPayload>,
}

/// 单个额度窗口（primary=5小时，secondary=周）。resets_at 为 Unix 秒。
#[derive(Debug, Deserialize)]
struct RateWindowPayload {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<i64>,
    /// 窗口时长（分钟）。Plus 账号的 primary 也可能是周窗口，不能只按字段名归类。
    #[serde(default)]
    window_minutes: Option<i64>,
}

/// turn_context 事件 payload（token_count 靠它归因模型）
#[derive(Debug, Deserialize)]
struct TurnContextPayload {
    #[serde(default)]
    model: Option<String>,
}

/// 从一行 turn_context 中取出非空模型名。
fn model_from_turn_context(line: &RolloutLine) -> Option<String> {
    if line.line_type.as_deref() != Some("turn_context") {
        return None;
    }
    let value = line.payload.as_ref()?;
    let context = serde_json::from_value::<TurnContextPayload>(value.clone()).ok()?;
    context.model.filter(|model| !model.is_empty())
}

/// 扫描文件在指定字节偏移之前最近一次出现的模型名。
///
/// Codex 会话文件是追加写入的，但桌面端可能在 turn_context 写入前就触发一次
/// 导入。续读时如果只看导入库中最近一条非空记录，而该会话此前全是空模型，
/// 就会一直丢失上下文。因此这里从文件头恢复一次上下文，避免依赖脏数据自愈。
fn latest_model_before_offset(path: &Path, before_offset: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开 Codex 会话文件失败: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    let mut pos = 0u64;
    let mut latest = String::new();
    let mut buf = Vec::with_capacity(4096);

    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("读取 Codex 会话文件失败: {e}"))?;
        if n == 0 {
            break;
        }
        pos += n as u64;
        if pos > before_offset {
            break;
        }
        if let Ok(line) = serde_json::from_slice::<RolloutLine>(&buf) {
            if let Some(model) = model_from_turn_context(&line) {
                latest = model;
            }
        }
    }
    Ok(latest)
}

/// 查询用的最新额度快照（resets_at 已转为毫秒）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexRateLimits {
    pub plan_type: Option<String>,
    pub primary_pct: Option<f64>,
    /// 5 小时窗口重置时间（毫秒时间戳）
    pub primary_reset_at: Option<i64>,
    pub secondary_pct: Option<f64>,
    /// 周窗口重置时间（毫秒时间戳）
    pub secondary_reset_at: Option<i64>,
}

/// 事件内 rate_limits → 查询结构（resets_at 秒 → 毫秒）
fn to_rate_limits(r: &RateLimitsPayload) -> CodexRateLimits {
    let mut hour5 = (None, None);
    let mut weekly = (None, None);
    for (fallback_key, window) in [("hour5", &r.primary), ("weekly", &r.secondary)] {
        let Some(window) = window.as_ref() else {
            continue;
        };
        let key = classify_rate_window(window, fallback_key);
        let value = (window.used_percent, window.resets_at.map(|s| s * 1000));
        if key == "weekly" {
            weekly = value;
        } else {
            hour5 = value;
        }
    }
    CodexRateLimits {
        plan_type: r.plan_type.clone(),
        primary_pct: hour5.0,
        primary_reset_at: hour5.1,
        secondary_pct: weekly.0,
        secondary_reset_at: weekly.1,
    }
}

/// 把 Codex CLI 的额度窗口归一到前端的 5h / 周两个稳定窗口键。
/// `primary` 并不总是 5h，Plus 账号实测可能只有一个周窗口。
fn classify_rate_window(window: &RateWindowPayload, fallback_key: &str) -> &'static str {
    if window.window_minutes.unwrap_or(0) >= 2 * 24 * 60 {
        "weekly"
    } else if window.window_minutes.is_some() {
        "hour5"
    } else if fallback_key == "weekly" {
        "weekly"
    } else {
        "hour5"
    }
}

fn agent_windows_from_rate_limits(
    limits: &RateLimitsPayload,
) -> Vec<agent_quota_history::AgentQuotaWindow> {
    let mut windows = Vec::new();
    for (fallback_key, window) in [("hour5", &limits.primary), ("weekly", &limits.secondary)] {
        let Some(window) = window.as_ref() else {
            continue;
        };
        let Some(used_pct) = window.used_percent else {
            continue;
        };
        let key = classify_rate_window(window, fallback_key);
        windows.push(agent_quota_history::AgentQuotaWindow {
            key: key.to_string(),
            used_pct,
            reset_at: window.resets_at.map(|s| s * 1000),
        });
    }
    windows
}

/// ISO8601 时间戳（如 2026-06-22T12:41:14.214Z）→ 毫秒时间戳
fn parse_ts_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 从文件名提取会话 uuid（rollout-<时间>-<uuid>.jsonl → <uuid>）。
/// 固定取文件主干末 36 个字符（8-4-4-4-12 标准长度），兼容任何前缀；
/// 不依赖 session_meta 事件——续读偏移后首行可能已越过它，文件名始终稳定。
fn session_id_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem.chars().count() >= 36 {
        let tail: String = stem.chars().rev().take(36).collect();
        let uid: String = tail.chars().rev().collect();
        // 轻量校验：标准 uuid 含 4 个连字符，不匹配则退回完整主干
        if uid.matches('-').count() == 4 {
            return uid;
        }
    }
    stem
}

// ===== 增量导入 =====

/// 导入互斥锁：面板查询 / 托盘标题刷新 / 同步上传可能并发触发导入，
/// 串行化避免同一文件被双份解析（冲突 upsert 可去重，但重复 IO 浪费）。
static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn import_lock() -> &'static Mutex<()> {
    IMPORT_LOCK.get_or_init(|| Mutex::new(()))
}

/// 上次导入时间（节流用）。各查询入口（query_stats/query_trend/…）都会触发
/// 导入，前端 30s 一轮 × 4 个预设范围 × 多个命令会放大到十余次"锁 + 开库 +
/// 递归扫目录"；会话文件是分钟级追加，5 秒节流足够实时且省掉重复扫描。
static LAST_IMPORT_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
static LAST_RATE_LIMIT_BACKFILL_DAY: OnceLock<Mutex<Option<i32>>> = OnceLock::new();

fn last_import_at() -> &'static Mutex<Option<std::time::Instant>> {
    LAST_IMPORT_AT.get_or_init(|| Mutex::new(None))
}

/// 增量导入（查询入口用，5 秒节流；失败也计入节流窗口，避免故障时重试风暴）。
pub fn import_incremental() -> Result<(), String> {
    {
        let mut last = last_import_at()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.map(|t| t.elapsed() < std::time::Duration::from_secs(5)) == Some(true) {
            return Ok(());
        }
        *last = Some(std::time::Instant::now());
    }
    import_incremental_force()
}

/// 递归收集 sessions 目录下所有 rollout-*.jsonl。
/// 目录层级为 年/月/日 三层，逐层手动遍历（不引入额外依赖）；
/// 每层同时收集匹配文件，兼容布局微调。结果排序保证导入顺序稳定。
fn collect_session_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // 最多下钻 3 层（年/月/日），防御异常深层嵌套
            if depth > 0 {
                collect_session_files(&path, depth - 1, out);
            }
        } else {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
}

/// 增量导入（不节流）：遍历 sessions 目录，把每个 rollout 文件新增部分解析入库。
/// 查询入口应调用节流版 import_incremental；诊断等需要"一定执行"的场景用本函数。
/// - file_progress 记录"已处理到的字节偏移"（对齐完整行末尾）；文件变短
///   （被重写）时从头重新解析，UNIQUE 键保证幂等。
/// - 续读时 event_seq 从该会话已入库的最大序号继续，避免序号回退导致
///   冲突 upsert 静默吞掉新事件。
/// - 每个文件一个事务：中途崩溃整体回滚，下次从旧偏移重来。
/// - 单文件失败（被占用/磁盘异常）只记日志跳过，不阻断其他文件。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = sessions_dir()?;
    let mut conn = open_codex_db()?;

    // 预载全部文件进度（单次查询；文件数通常几百，内存开销可忽略）
    let mut progress: HashMap<String, (u64, u64)> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT path, offset, size FROM file_progress")
            .map_err(|e| format!("读取导入进度失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| format!("读取导入进度失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取导入进度失败: {e}"))?;
        for (p, off, sz) in rows {
            progress.insert(p, (off.max(0) as u64, sz.max(0) as u64));
        }
    }

    let mut files = Vec::new();
    collect_session_files(&dir, 3, &mut files);
    files.sort();

    for path in &files {
        let key = path.to_string_lossy().to_string();
        let known = progress.get(&key).copied();
        if let Err(e) = import_one_file(&mut conn, path, known) {
            eprintln!("[zbar-codex] 导入 {} 失败（下次重试）: {e}", path.display());
        }
    }

    // 历史版本可能在模型上下文尚未写入时就落了空 model_id。对这些会话重扫
    // 原始文件，借助同一套事件序号和 upsert 逻辑把模型名补回；没有模型上下文
    // 的文件跳过，避免每次查询都重复扫描无法修复的数据。
    let blank_sessions: HashSet<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT session_id FROM model_usage WHERE model_id = ''")
            .map_err(|e| format!("查询待修复 Codex 会话失败: {e}"))?;
        let sessions = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询待修复 Codex 会话失败: {e}"))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| format!("查询待修复 Codex 会话失败: {e}"))?;
        sessions
    };
    for path in &files {
        let session_id = session_id_from_filename(path);
        if !blank_sessions.contains(&session_id) {
            continue;
        }
        let has_model = latest_model_before_offset(path, u64::MAX)
            .map(|model| !model.is_empty())
            .unwrap_or(false);
        if !has_model {
            continue;
        }
        if let Err(e) = import_one_file(&mut conn, path, None) {
            eprintln!(
                "[zbar-codex] 回填 {} 的模型名失败（下次重试）: {e}",
                path.display()
            );
        }
    }
    Ok(())
}

/// 把今天 Codex CLI 会话中已经记录的有效额度快照补入 Agent 历史。
///
/// 额度历史功能可能在用户当天已经使用了一段时间后才启动；如果只从
/// `wham/usage` 的首次实时采样开始，今日早先的真实使用会被误当成今日
/// 起点。Codex CLI 自己会在每次 token_count 事件中记录 rate_limits，
/// 这些是有效的提供方快照，不是网络失败时的旧回退值，因此可以安全补齐。
/// 每个本地日期最多扫描一次，重复启动通过历史中的“来源 + 同秒”集合去重。
pub fn backfill_today_rate_limit_history() -> Result<usize, String> {
    let now = Local::now();
    let day = now.date_naive();
    let day_key = day.num_days_from_ce();
    let backfill_day = LAST_RATE_LIMIT_BACKFILL_DAY.get_or_init(|| Mutex::new(None));
    {
        let guard = backfill_day
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard == Some(day_key) {
            return Ok(0);
        }
    }

    let Some(day_start) = day
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|value| value.timestamp_millis())
    else {
        return Ok(0);
    };
    let day_dir = sessions_dir_path()
        .join(format!("{:04}", day.year()))
        .join(format!("{:02}", day.month()))
        .join(format!("{:02}", day.day()));
    if !day_dir.is_dir() {
        return Ok(0);
    }

    let mut files = Vec::new();
    collect_session_files(&day_dir, 0, &mut files);
    files.sort();
    if files.is_empty() {
        return Ok(0);
    }

    let mut snapshots = Vec::new();
    for path in files {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in std::io::BufReader::new(file).lines().flatten() {
            let Ok(rollout) = serde_json::from_str::<RolloutLine>(&line) else {
                continue;
            };
            if rollout.line_type.as_deref() != Some("event_msg") {
                continue;
            }
            let Some(payload) = rollout.payload else {
                continue;
            };
            let Ok(token_count) = serde_json::from_value::<TokenCountPayload>(payload) else {
                continue;
            };
            if token_count.msg_type.as_deref() != Some("token_count") {
                continue;
            }
            let Some(limits) = token_count.rate_limits.as_ref() else {
                continue;
            };
            let Some(ts) = rollout.timestamp.as_deref().and_then(parse_ts_ms) else {
                continue;
            };
            if ts < day_start || ts > now.timestamp_millis() {
                continue;
            }
            let windows = agent_windows_from_rate_limits(limits);
            if windows.is_empty() {
                continue;
            }
            snapshots.push(agent_quota_history::AgentQuotaSnapshot {
                source: "codex".to_string(),
                ts,
                plan_type: limits.plan_type.clone(),
                windows,
            });
        }
    }
    snapshots.sort_by_key(|snapshot| snapshot.ts);

    let mut existing_seconds: HashSet<i64> = agent_quota_history::load_all()?
        .into_iter()
        .filter(|snapshot| snapshot.source == "codex")
        .map(|snapshot| snapshot.ts / 1000)
        .collect();
    let mut added = 0usize;
    for snapshot in snapshots {
        let second = snapshot.ts / 1000;
        if !existing_seconds.insert(second) {
            continue;
        }
        agent_quota_history::append_snapshot(&snapshot);
        added += 1;
    }

    let mut guard = backfill_day
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(day_key);
    Ok(added)
}

/// 解析单个 rollout 文件的增量部分。known = 上次记录的 (offset, size)。
fn import_one_file(
    conn: &mut Connection,
    path: &Path,
    known: Option<(u64, u64)>,
) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取文件元信息失败: {e}"))?
        .len();

    // 无进度记录，或文件比记录时更短（被重写）→ 从头解析（序号从 1 重计，幂等）
    let (start_offset, reparse) = match known {
        Some((off, prev_size)) if size >= prev_size => (off, false),
        _ => (0, true),
    };
    if !reparse && start_offset >= size {
        return Ok(()); // 文件无新增内容
    }

    let session_id = session_id_from_filename(path);

    let mut file = std::fs::File::open(path).map_err(|e| format!("打开会话文件失败: {e}"))?;
    let mut reader = std::io::BufReader::new(&mut file);
    reader
        .seek(SeekFrom::Start(start_offset))
        .map_err(|e| format!("定位读取偏移失败: {e}"))?;

    // 续读时 event_seq 接着该会话已入库的最大序号继续（唯一键稳定）；
    // 同时恢复该会话最近一次归因的模型名。若历史记录全为空，则从原始文件
    // 的 start_offset 之前恢复最近上下文，避免继续把新增 token 归到空模型。
    let mut seq: i64 = 0;
    let mut current_model = String::new();
    if !reparse {
        let _ = conn.query_row(
            "SELECT COALESCE(MAX(event_seq), 0),
                    COALESCE((SELECT model_id FROM model_usage
                              WHERE session_id = ?1 AND model_id != ''
                              ORDER BY event_seq DESC LIMIT 1), '')
             FROM model_usage WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| {
                seq = row.get(0)?;
                current_model = row.get(1)?;
                Ok(())
            },
        );
        if current_model.is_empty() {
            current_model = latest_model_before_offset(path, start_offset)?;
        }
    }

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| format!("开启导入事务失败: {e}"))?;

    let mut pos = start_offset;
    let mut last_complete_end = start_offset;
    // 本文件内时间最新的非空额度快照（落库前与已存值比 observed_at 不回退）
    let mut latest_limits: Option<(CodexRateLimits, i64)> = None;
    // 仅在已有空模型行被补回时写入，供同步客户端补传修订行。取数据库中
    // 现有最大值 + 1，保证即使系统时钟回拨或同一毫秒发生多次修订，游标仍
    // 能按严格递增顺序工作。
    let max_model_revision: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(model_revision_at), 0) FROM model_usage",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("读取 Codex 模型修订游标失败: {e}"))?;
    let model_revision_at = chrono::Local::now()
        .timestamp_millis()
        .max(max_model_revision.saturating_add(1));
    let mut pending_model_seqs: Vec<i64> = Vec::new();

    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("读取会话文件失败: {e}"))?;
        if n == 0 {
            break; // EOF
        }
        pos += n as u64;
        // 只把"完整行"（带换行符）计入进度：末尾半行留待下次追加完整后重读
        let complete = buf.last() == Some(&b'\n');
        if complete {
            last_complete_end = pos;
        }

        // 逐行容错：JSON 解析失败/字段异常只跳过该行（偏移仍推进）
        let Ok(line) = serde_json::from_slice::<RolloutLine>(&buf) else {
            continue;
        };
        match line.line_type.as_deref() {
            // turn_context：更新当前模型（token_count 不带模型，靠它归因）
            Some("turn_context") => {
                if let Some(model) = model_from_turn_context(&line) {
                    current_model = model.clone();
                    // 若本次文件扫描中 token_count 先于 turn_context，先前插入的
                    // 空模型也属于这个会话；拿到首个模型上下文后立即补回，兼容
                    // Codex 文件出现事件顺序暂时不稳定的情况。
                    for pending_seq in pending_model_seqs.drain(..) {
                        tx.execute(
                            "UPDATE model_usage
                             SET model_id = ?1, model_revision_at = ?2
                             WHERE session_id = ?3 AND event_seq = ?4 AND model_id = ''",
                            rusqlite::params![
                                &model,
                                model_revision_at,
                                session_id,
                                pending_seq,
                            ],
                        )
                        .map_err(|e| format!("回填 Codex 模型名失败: {e}"))?;
                    }
                }
            }
            // event_msg + token_count：一条 token 用量记录
            Some("event_msg") => {
                let Some(v) = line.payload.as_ref() else { continue };
                let Ok(p) = serde_json::from_value::<TokenCountPayload>(v.clone()) else {
                    continue;
                };
                if p.msg_type.as_deref() != Some("token_count") {
                    continue;
                }
                let Some(usage) = p.info.as_ref().and_then(|i| i.last_token_usage.as_ref())
                else {
                    continue;
                };
                // 没有时间戳无法归入统计区间，跳过（偏移仍推进）
                let Some(started_at) = line.timestamp.as_deref().and_then(parse_ts_ms) else {
                    continue;
                };

                seq += 1;
                tx.execute(
                    "INSERT INTO model_usage
                        (session_id, event_seq, started_at, model_id, provider_id,
                         input_tokens, output_tokens, cache_read_input_tokens,
                         cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                         model_revision_at)
                     VALUES (?1, ?2, ?3, ?4, 'codex', ?5, ?6, ?7, 0, ?8, ?9, 0)
                     ON CONFLICT(session_id, event_seq) DO UPDATE SET
                         model_id = excluded.model_id,
                         model_revision_at = ?10
                     WHERE model_usage.model_id = '' AND excluded.model_id != ''",
                    rusqlite::params![
                        session_id,
                        seq,
                        started_at,
                        current_model,
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cached_input_tokens,
                        usage.reasoning_output_tokens,
                        usage.total_tokens,
                        model_revision_at,
                    ],
                )
                .map_err(|e| format!("写入用量记录失败: {e}"))?;

                if current_model.is_empty() {
                    pending_model_seqs.push(seq);
                }

                // 仅 ChatGPT 订阅登录模式 primary 非空；custom provider 全为 null
                if let Some(limits) = p.rate_limits.as_ref() {
                    if limits.primary.is_some() {
                        latest_limits = Some((to_rate_limits(limits), started_at));
                    }
                }
            }
            _ => {}
        }
    }

    // 进度：对齐到最后一条完整行末尾（末尾半行下次重读，靠唯一键幂等）
    let key = path.to_string_lossy().to_string();
    tx.execute(
        "INSERT INTO file_progress (path, offset, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET offset = ?2, size = ?3",
        rusqlite::params![key, last_complete_end as i64, size as i64],
    )
    .map_err(|e| format!("记录导入进度失败: {e}"))?;

    // 最新额度快照落库（observed_at 不回退，防止旧会话续写覆盖新快照）
    if let Some((snap, observed_at)) = latest_limits {
        let stored: Option<i64> = tx
            .query_row(
                "SELECT observed_at FROM rate_limits_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("读取额度快照失败: {e}"))?;
        if stored.map_or(true, |s| observed_at >= s) {
            tx.execute(
                "INSERT INTO rate_limits_state
                    (id, observed_at, plan_type, primary_pct, primary_reset_at,
                     secondary_pct, secondary_reset_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    observed_at = ?1, plan_type = ?2, primary_pct = ?3,
                    primary_reset_at = ?4, secondary_pct = ?5, secondary_reset_at = ?6",
                rusqlite::params![
                    observed_at,
                    snap.plan_type,
                    snap.primary_pct,
                    snap.primary_reset_at,
                    snap.secondary_pct,
                    snap.secondary_reset_at,
                ],
            )
            .map_err(|e| format!("写入额度快照失败: {e}"))?;
        }
    }

    tx.commit().map_err(|e| format!("提交导入事务失败: {e}"))?;
    Ok(())
}

// ===== 查询函数（与 db.rs 同名同构，查 codex.sqlite；查询前先增量导入）=====

/// 查询 [from_ms, to_ms) 内的统计（口径与 db::query_stats 完全一致）。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_codex_db()?;

    let overall: db::OverallStat = conn
        .query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(cache_creation_input_tokens),0),
                COALESCE(SUM(reasoning_tokens),0),
                COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
            rusqlite::params![from_ms, to_ms],
            |row| {
                Ok(db::OverallStat {
                    requests: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read_tokens: row.get(3)?,
                    cache_write_tokens: row.get(4)?,
                    reasoning_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                })
            },
        )
        .map_err(|e| format!("查询 Codex 整体统计失败: {e}"))?;

    let mut stmt = conn
        .prepare(
            "SELECT
                model_id,
                provider_id,
                COUNT(*),
                COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(cache_creation_input_tokens),0),
                COALESCE(SUM(reasoning_tokens),0),
                COALESCE(SUM(computed_total_tokens),0) AS total_tokens
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider_id, model_id
             ORDER BY total_tokens DESC",
        )
        .map_err(|e| format!("准备 Codex 模型分组查询失败: {e}"))?;

    let by_model = stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok(db::ModelStat {
                model_id: row.get(0)?,
                provider_id: row.get(1)?,
                requests: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                cache_write_tokens: row.get(6)?,
                reasoning_tokens: row.get(7)?,
                total_tokens: row.get(8)?,
            })
        })
        .map_err(|e| format!("读取 Codex 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 模型分组失败: {e}"))?;

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 Codex 时间范围失败: {e}"))?;

    Ok(db::Stats {
        from_ms,
        to_ms,
        overall,
        by_model,
        earliest_ms,
        latest_ms,
    })
}

/// 查询 [from_ms, to_ms) 内的分桶统计（与 db::query_trend 同思路，复用其对齐/标签函数）。
/// bucket 为 "hour" 或 "day"。
pub fn query_trend(
    from_ms: i64,
    to_ms: i64,
    bucket: &str,
) -> Result<Vec<db::TrendBucketRaw>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let width = if bucket == "hour" { 3_600_000 } else { 86_400_000 };

    let mut start = db::align_bucket_start(from_ms, bucket);
    let sql = "SELECT
                model_id,
                provider_id,
                COUNT(*),
                COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider_id, model_id";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("准备 Codex 趋势查询失败: {e}"))?;

    let mut out: Vec<db::TrendBucketRaw> = Vec::new();
    while start < to_ms {
        let end = start + width;
        let by_model: Vec<db::BucketModelStat> = stmt
            .query_map(rusqlite::params![start, end], |row| {
                Ok(db::BucketModelStat {
                    model_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                })
            })
            .map_err(|e| format!("读取 Codex 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 Codex 趋势统计失败: {e}"))?;

        let total_tokens = by_model.iter().map(|m| m.total_tokens).sum();
        let requests = by_model.iter().map(|m| m.requests).sum();

        out.push(db::TrendBucketRaw {
            label: db::bucket_label(start, bucket),
            by_model,
            total_tokens,
            requests,
        });

        start = end;
    }

    Ok(out)
}

/// 查询 id > since 的明细记录（同步上传用）。source 固定 "codex"，local_rowid = id。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, model_id, provider_id,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens
             FROM model_usage
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2",
        )
        .map_err(|e| format!("准备 Codex 增量查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since, limit as i64], |row| {
            Ok(db::UsageRow {
                local_rowid: row.get(0)?,
                started_at: row.get(1)?,
                model_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                provider_id: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                cache_read_input_tokens: row.get(6)?,
                cache_creation_input_tokens: row.get(7)?,
                reasoning_tokens: row.get(8)?,
                computed_total_tokens: row.get(9)?,
                source: "codex".into(),
            })
        })
        .map_err(|e| format!("读取 Codex 增量记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 增量记录失败: {e}"))?;
    Ok(rows)
}

/// 查询模型名被历史回填过的记录（同步补传用）。
///
/// 这些记录的 local_rowid 通常已经小于普通 Codex 上传游标，因此不能复用
/// query_since；按修订时间 + id 双游标分页，上传时不携带 last_rowid，避免
/// 把普通增量游标回退或推进到错误位置。
#[derive(Debug, Clone)]
pub struct ModelRevisionRow {
    pub usage: db::UsageRow,
    pub revision_at: i64,
}

pub fn query_model_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<ModelRevisionRow>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, model_id, provider_id,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                    model_revision_at
             FROM model_usage
             WHERE (model_revision_at > ?1
                    OR (model_revision_at = ?1 AND id > ?2))
               AND model_id != ''
             ORDER BY model_revision_at ASC, id ASC
             LIMIT ?3",
        )
        .map_err(|e| format!("准备 Codex 模型修订查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since_ts, after_id, limit as i64], |row| {
            Ok(ModelRevisionRow {
                usage: db::UsageRow {
                    local_rowid: row.get(0)?,
                    started_at: row.get(1)?,
                    model_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    provider_id: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    input_tokens: row.get(4)?,
                    output_tokens: row.get(5)?,
                    cache_read_input_tokens: row.get(6)?,
                    cache_creation_input_tokens: row.get(7)?,
                    reasoning_tokens: row.get(8)?,
                    computed_total_tokens: row.get(9)?,
                    source: "codex".into(),
                },
                revision_at: row.get(10)?,
            })
        })
        .map_err(|e| format!("读取 Codex 模型修订失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 模型修订失败: {e}"))?;
    Ok(rows)
}

/// 导入库当前最大 rowid（供「待上传条数」显示用）。
pub fn max_rowid() -> Result<i64, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let max: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM model_usage", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("查询 Codex 最大 rowid 失败: {e}"))?;
    Ok(max)
}

/// 列出导入库中出现过的所有 (provider_id, model_id) 组合，供价格配置用。
/// provider_id 恒为 "codex"。
pub fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 Codex 模型列表查询失败: {e}"))?;

    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 Codex 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 模型列表失败: {e}"))?;

    Ok(models)
}

/// 最新一次非空额度快照（导入时存 rate_limits_state 单行表；custom provider 模式为 None）。
/// 快照有天然时效：5 小时/周窗口的百分比只在对应 resets_at 之前有意义，
/// 过期窗口（重置时间已过）直接剔除——否则两个月前切到 API 中转模式的
/// 用户会看到早已失效的订阅额度，误以为数据错误。两窗口都过期返回 None。
pub fn latest_rate_limits() -> Result<Option<CodexRateLimits>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let row = conn
        .query_row(
            "SELECT plan_type, primary_pct, primary_reset_at,
                    secondary_pct, secondary_reset_at
             FROM rate_limits_state WHERE id = 1",
            [],
            |row| {
                Ok(CodexRateLimits {
                    plan_type: row.get(0)?,
                    primary_pct: row.get(1)?,
                    primary_reset_at: row.get(2)?,
                    secondary_pct: row.get(3)?,
                    secondary_reset_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("查询 Codex 额度快照失败: {e}"))?;

    let Some(mut snap) = row else {
        return Ok(None);
    };
    let now_ms = chrono::Local::now().timestamp_millis();
    // 重置时间已过的窗口不再展示（快照观察值早已失效）
    let primary_valid =
        snap.primary_reset_at.map(|t| t > now_ms).unwrap_or(false);
    let secondary_valid =
        snap.secondary_reset_at.map(|t| t > now_ms).unwrap_or(false);
    if !primary_valid {
        snap.primary_pct = None;
        snap.primary_reset_at = None;
    }
    if !secondary_valid {
        snap.secondary_pct = None;
        snap.secondary_reset_at = None;
    }
    if !primary_valid && !secondary_valid {
        return Ok(None);
    }
    Ok(Some(snap))
}

/// 按指定周期聚合 Codex Token。
/// 对比页需要真实的 [reset_at, end_at) 边界，不能用只带 HH:00 的趋势 label 反推跨日周期。
pub fn query_period_buckets(
    periods: &[(i64, i64)],
) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(SUM(computed_total_tokens),0),
                COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 Codex 周期聚合查询失败: {e}"))?;

    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 Codex 周期聚合失败: {e}"))?;
        out.push(db::PeriodBucket {
            reset_at,
            end_at,
            total_tokens,
            requests,
        });
    }
    Ok(out)
}

// ===== 实时额度（ChatGPT backend-api，参照 CodexBar 的实现）=====

/// Codex 登录凭证（只读 ~/.codex/auth.json，绝不修改/刷新——
/// refresh_token 是一次性轮换的，外部写回极易把 Codex CLI 的登录搞坏，
/// 刷新由 Codex CLI 自己按 8 天周期维护）。
struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

/// 读取 auth.json 的 access_token / account_id。
/// 新版结构在 tokens 嵌套对象里，旧版扁平在顶层，两种都兼容。
fn load_codex_auth() -> Result<CodexAuth, String> {
    let root = std::env::var("ZBAR_CODEX_HOME")
        .or_else(|_| std::env::var("CODEX_HOME"))
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    let path = root.join("auth.json");
    if !path.exists() {
        return Err("未找到 Codex 登录凭证（auth.json），请先 codex login".into());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 auth.json 失败: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("解析 auth.json 失败: {e}"))?;
    let token = v
        .pointer("/tokens/access_token")
        .and_then(|t| t.as_str())
        .or_else(|| v.get("access_token").and_then(|t| t.as_str()))
        .filter(|t| !t.is_empty())
        .ok_or("auth.json 中无 access_token")?;
    let account_id = v
        .pointer("/tokens/account_id")
        .and_then(|t| t.as_str())
        .or_else(|| v.get("account_id").and_then(|t| t.as_str()))
        .filter(|t| !t.is_empty())
        .map(|s| s.to_string());
    Ok(CodexAuth {
        access_token: token.to_string(),
        account_id,
    })
}

/// wham/usage 响应结构（字段名与 jsonl 里的 rate_limits 不同：
/// 这里是 primary_window/secondary_window + reset_at 秒）。
/// 注意窗口组合不固定（Plus 账号实测 primary 就是周窗口、secondary 为 null），
/// 展示前需按 limit_window_seconds 归类，不能假定 primary=5h。
#[derive(Debug, Deserialize)]
struct WhamUsageResponse {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<WhamRateLimit>,
}

#[derive(Debug, Deserialize)]
struct WhamRateLimit {
    #[serde(default)]
    primary_window: Option<WhamWindow>,
    #[serde(default)]
    secondary_window: Option<WhamWindow>,
}

#[derive(Debug, Deserialize)]
struct WhamWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    reset_at: Option<i64>,
    /// 窗口时长（秒）：18000=5小时，604800=周
    #[serde(default)]
    limit_window_seconds: Option<i64>,
}

/// 解析代理地址，优先级：HTTPS_PROXY 环境变量 > 系统代理（Windows 注册表 /
/// macOS scutil）> 直连。chatgpt.com 在部分网络需代理才可达，而桌面应用
/// 常见情况是用户开了系统代理但没有设置环境变量。
/// claude 模块的实时额度请求（api.anthropic.com）复用同一探测。
pub(crate) fn resolve_proxy() -> Option<String> {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    system_proxy()
}

/// 读取系统代理设置，返回代理 URL（如 "http://127.0.0.1:7890"）。
/// - Windows：注册表 HKCU\...\Internet Settings 的 ProxyEnable/ProxyServer
/// - macOS：scutil --proxy 输出（HTTPSEnable/HTTPSProxy 等，Clash/Surge 等
///   设置系统代理后即可被读到）
/// - 其他平台：无（仅环境变量）
fn system_proxy() -> Option<String> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let settings = hkcu
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
            .ok()?;
        let enabled: u32 = settings.get_value("ProxyEnable").ok()?;
        if enabled != 1 {
            return None;
        }
        let server: String = settings.get_value("ProxyServer").ok()?;
        let server = server.trim().to_string();
        if server.is_empty() {
            return None;
        }
        // ProxyServer 两种形态：统一代理 "host:port"；分协议 "http=a:b;https=c:d"
        let picked = server
            .split(';')
            .find(|p| p.trim_start().starts_with("https="))
            .map(|p| p.split('=').nth(1).unwrap_or("").trim().to_string())
            .or_else(|| {
                if server.contains('=') {
                    // 分协议但无 https 项：退回 http 项
                    server
                        .split(';')
                        .find(|p| p.trim_start().starts_with("http="))
                        .and_then(|p| p.split('=').nth(1))
                        .map(|s| s.trim().to_string())
                } else {
                    Some(server.clone())
                }
            })?;
        if picked.is_empty() {
            return None;
        }
        Some(normalize_proxy_url(&picked))
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("scutil")
            .arg("--proxy")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        parse_scutil_proxy(&text)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// 裸 "host:port" 补全为 "http://host:port"（已带 scheme 的原样返回）。
/// ureq 的 Proxy 需要完整 URL；socks5:// 由调用方（scutil 解析）自行拼好。
fn normalize_proxy_url(addr: &str) -> String {
    if addr.contains("://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

/// 解析 `scutil --proxy` 输出，取第一个启用的代理（HTTPS > HTTP > SOCKS5）。
/// 输出形如：
/// ```text
/// <dictionary> {
///   HTTPEnable : 1
///   HTTPProxy : 127.0.0.1
///   HTTPPort : 7890
///   HTTPSEnable : 1
///   HTTPSProxy : 127.0.0.1
///   HTTPSPort : 7890
///   SOCKSEnable : 0
/// }
/// ```
/// 仅 macOS 构建与测试编译纳入（Windows/Linux 普通构建下无人调用，
/// 不加条件会触发 dead_code 警告）
#[cfg(any(target_os = "macos", test))]
fn parse_scutil_proxy(text: &str) -> Option<String> {
    // 逐行查找 "key : value"
    let find = |key: &str| -> Option<String> {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim() == key {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
    };
    if find("HTTPSEnable")?.as_str() == "1" {
        let host = find("HTTPSProxy")?;
        let port = find("HTTPSPort")?;
        if !host.is_empty() && !port.is_empty() {
            return Some(format!("http://{host}:{port}"));
        }
    }
    if find("HTTPEnable")?.as_str() == "1" {
        let host = find("HTTPProxy")?;
        let port = find("HTTPPort")?;
        if !host.is_empty() && !port.is_empty() {
            return Some(format!("http://{host}:{port}"));
        }
    }
    if find("SOCKSEnable")?.as_str() == "1" {
        let host = find("SOCKSProxy")?;
        let port = find("SOCKSPort")?;
        if !host.is_empty() && !port.is_empty() {
            return Some(format!("socks5://{host}:{port}"));
        }
    }
    None
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    #[test]
    fn scutil_https_preferred() {
        let out = "<dictionary> {\n  HTTPEnable : 1\n  HTTPProxy : 127.0.0.1\n  HTTPPort : 7890\n  HTTPSEnable : 1\n  HTTPSProxy : 127.0.0.1\n  HTTPSPort : 7891\n  SOCKSEnable : 0\n}\n";
        assert_eq!(
            parse_scutil_proxy(out).as_deref(),
            Some("http://127.0.0.1:7891")
        );
    }

    #[test]
    fn scutil_http_fallback() {
        let out = "<dictionary> {\n  HTTPEnable : 1\n  HTTPProxy : 127.0.0.1\n  HTTPPort : 1087\n  HTTPSEnable : 0\n}\n";
        assert_eq!(
            parse_scutil_proxy(out).as_deref(),
            Some("http://127.0.0.1:1087")
        );
    }

    #[test]
    fn scutil_socks_fallback() {
        let out = "<dictionary> {\n  HTTPEnable : 0\n  HTTPSEnable : 0\n  SOCKSEnable : 1\n  SOCKSProxy : 127.0.0.1\n  SOCKSPort : 1080\n}\n";
        assert_eq!(
            parse_scutil_proxy(out).as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
    }

    #[test]
    fn scutil_all_disabled() {
        let out = "<dictionary> {\n  HTTPEnable : 0\n  HTTPSEnable : 0\n  SOCKSEnable : 0\n}\n";
        assert_eq!(parse_scutil_proxy(out), None);
    }

    #[test]
    fn normalize_proxy_url_adds_scheme() {
        assert_eq!(normalize_proxy_url("127.0.0.1:33210"), "http://127.0.0.1:33210");
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:1080"),
            "socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn primary_week_window_is_classified_as_weekly() {
        let payload: RateLimitsPayload = serde_json::from_str(
            r#"{"plan_type":"plus","primary":{"used_percent":26.0,"window_minutes":10080,"resets_at":1787364605}}"#,
        )
        .expect("额度窗口解析失败");
        let limits = to_rate_limits(&payload);
        assert_eq!(limits.primary_pct, None);
        assert_eq!(limits.secondary_pct, Some(26.0));
        assert_eq!(limits.secondary_reset_at, Some(1_787_364_605_000));
    }
}

/// 实时额度结果缓存（额度与查询范围无关；前端 30s 一轮 × 4 个预设范围
/// 都会触发 get_codex_usage，不加缓存会把 wham/usage 打爆）。60s TTL。
static LIVE_LIMITS_CACHE: OnceLock<Mutex<Option<(std::time::Instant, Option<CodexRateLimits>)>>> =
    OnceLock::new();

/// 拉取实时订阅额度：GET https://chatgpt.com/backend-api/wham/usage
/// （Codex CLI 内部使用的同一端点，需 ChatGPT 订阅登录；API 中转模式
/// 的用量不走该接口，但只要本机登录过订阅账号即可查询账号额度）。
/// 失败返回 Err，调用方降级到本地快照。60 秒内复用上次结果。
#[allow(dead_code)]
pub fn fetch_live_rate_limits() -> Result<Option<CodexRateLimits>, String> {
    fetch_live_rate_limits_with_freshness().map(|(limits, _)| limits)
}

/// 拉取实时额度并标记本次结果是否来自新的 HTTP 请求。
/// 缓存命中仍可用于当前进度展示，但不应作为新的历史采样。
pub fn fetch_live_rate_limits_with_freshness(
) -> Result<(Option<CodexRateLimits>, bool), String> {
    let cache = LIVE_LIMITS_CACHE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, val)) = guard.as_ref() {
            if at.elapsed() < std::time::Duration::from_secs(60) {
                return Ok((val.clone(), false));
            }
        }
    }

    let auth = load_codex_auth()?;
    let mut builder = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10));
    if let Some(url) = resolve_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("[zbar-codex] 代理地址无效（改为直连）: {url} ({e})"),
        }
    }
    let agent = builder.build();

    let mut req = agent
        .get("https://chatgpt.com/backend-api/wham/usage")
        .set("Authorization", &format!("Bearer {}", auth.access_token))
        .set("User-Agent", "codex-cli");
    if let Some(acc) = &auth.account_id {
        req = req.set("ChatGPT-Account-Id", acc);
    }
    let resp: WhamUsageResponse = req
        .call()
        .map_err(|e| format!("实时额度请求失败: {e}"))?
        .into_json()
        .map_err(|e| format!("解析实时额度失败: {e}"))?;

    // 窗口按时长归类到 5h/周两个展示槽（≤1 天视为 5h 窗口，≥2 天视为周窗口），
    // 同槽后出现的覆盖先出现的（secondary 通常更长期）
    let mut hour5: Option<(Option<f64>, Option<i64>)> = None;
    let mut weekly: Option<(Option<f64>, Option<i64>)> = None;
    for win in [resp.rate_limit.as_ref().and_then(|r| r.primary_window.as_ref()),
                resp.rate_limit.as_ref().and_then(|r| r.secondary_window.as_ref())]
        .into_iter()
        .flatten()
    {
        let slot = (win.used_percent, win.reset_at.map(|s| s * 1000));
        let secs = win.limit_window_seconds.unwrap_or(0);
        if secs >= 2 * 86_400 {
            weekly = Some(slot);
        } else {
            hour5 = Some(slot);
        }
    }
    let result = if hour5.is_some() || weekly.is_some() {
        Some(CodexRateLimits {
            plan_type: resp.plan_type,
            primary_pct: hour5.as_ref().and_then(|s| s.0),
            primary_reset_at: hour5.as_ref().and_then(|s| s.1),
            secondary_pct: weekly.as_ref().and_then(|s| s.0),
            secondary_reset_at: weekly.as_ref().and_then(|s| s.1),
        })
    } else {
        None
    };

    *cache.lock().unwrap_or_else(|p| p.into_inner()) =
        Some((std::time::Instant::now(), result.clone()));
    Ok((result, true))
}

// ===== 诊断 =====

/// Codex 诊断信息（排查"无数据"问题）
#[derive(Debug, Clone, Serialize)]
pub struct CodexDebugInfo {
    /// sessions 目录路径
    pub sessions_dir: String,
    /// 目录是否存在
    pub sessions_dir_exists: bool,
    /// 目录下 rollout 文件数
    pub session_files: usize,
    /// 导入库累计记录数
    pub imported_records: i64,
    /// 最新一条用量的时间（毫秒）
    pub latest_session_ms: Option<i64>,
}

/// 诊断信息（cursor_debug 同款用途）。导入失败不阻断——目录不存在时
/// 恰恰要靠这些字段定位问题。诊断必须真实执行导入（绕过节流）。
pub fn debug_info() -> Result<CodexDebugInfo, String> {
    if let Err(e) = import_incremental_force() {
        eprintln!("[zbar-codex] 诊断时增量导入失败: {e}");
    }

    let dir = sessions_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 3, &mut files);
    }

    let (imported_records, latest_session_ms) = open_codex_db()
        .map(|conn| {
            conn.query_row(
                "SELECT COUNT(*), MAX(started_at) FROM model_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, None))
        })
        .unwrap_or((0, None));

    Ok(CodexDebugInfo {
        sessions_dir: dir.display().to_string(),
        sessions_dir_exists: dir.is_dir(),
        session_files: files.len(),
        imported_records,
        latest_session_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟测试：对本机真实 sessions 目录执行增量导入并做全范围查询，
    /// 打印结果供人工核对（无 Codex 环境时跳过，不视为失败）。
    /// 重复运行第二次还会验证幂等性：记录数不因重复导入而变化
    /// （用不节流的 force 版本，绕开 5 秒节流对二次导入的跳过）。
    #[test]
    fn import_and_query_smoke() {
        if sessions_dir().is_err() {
            eprintln!("[test] 本机无 Codex sessions 目录，跳过");
            return;
        }
        import_incremental_force().expect("增量导入失败");
        let first = query_stats(0, i64::MAX).expect("全范围查询失败");
        // 幂等：再导一次，记录数不变
        import_incremental_force().expect("二次导入失败");
        let second = query_stats(0, i64::MAX).expect("二次查询失败");
        assert_eq!(
            first.overall.requests, second.overall.requests,
            "重复导入导致记录数变化"
        );
        assert!(first.overall.requests > 0, "导入了 0 条记录");
        assert_eq!(
            first.overall.total_tokens,
            first.by_model.iter().map(|m| m.total_tokens).sum::<i64>(),
            "overall 与 by_model 汇总不一致"
        );
        assert!(
            first.by_model.iter().all(|m| !m.model_id.is_empty()),
            "Codex 统计中仍存在空模型分组: {:?}",
            first
                .by_model
                .iter()
                .filter(|m| m.model_id.is_empty())
                .collect::<Vec<_>>()
        );
        // 趋势管道：以 [earliest, latest+1h) 小时桶查询，至少有一个非空桶
        if let (Some(e), Some(l)) = (first.earliest_ms, first.latest_ms) {
            let trend = query_trend(e, l + 3_600_000, "hour").expect("趋势查询失败");
            let non_empty = trend.iter().filter(|b| b.total_tokens > 0).count();
            assert!(non_empty > 0, "趋势桶全部为空");
            eprintln!("[test] trend 桶数={} 非空桶={}", trend.len(), non_empty);
        }
        eprintln!(
            "[test] requests={} overall={:?} by_model={:?} rate_limits={:?}",
            first.overall.requests,
            first.overall,
            first
                .by_model
                .iter()
                .map(|m| (m.model_id.clone(), m.requests, m.total_tokens))
                .collect::<Vec<_>>(),
            latest_rate_limits().unwrap_or(None),
        );
        // 实时额度（wham/usage）：网络环境不同结果不同，只打印不断言。
        // chatgpt.com 不可达时应返回 Err（调用方降级到本地快照）。
        match fetch_live_rate_limits() {
            Ok(v) => eprintln!("[test] 实时额度: {v:?}"),
            Err(e) => eprintln!("[test] 实时额度不可用（降级本地快照）: {e}"),
        }
    }
}

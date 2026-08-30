//! Claude Code 用量统计模块。
//!
//! 数据来源：Claude Code 把每个会话记录在
//! ~/.claude/projects/<项目目录>/<会话uuid>.jsonl（append-only，每行一个 JSON
//! 事件；子代理会话在 <会话uuid>/subagents/ 下一层，递归遍历）。
//! token 用量在 type=assistant 行的 message.usage 里（input/output/
//! cache_read/cache_creation，无 total 字段，四者之和即总量）。
//!
//! 与 Codex 模块的关键差异：
//! - **去重键**：同一 message.id 会写多行（CLI 边流式边落盘，usage 逐行累计，
//!   末行为终值；本机实测全部满足"末行 = 最大值"）。按行直接累加会重复计数，
//!   故按 message.id 全局去重（resume/continue 是 fork 语义，新会话文件复制
//!   原历史且保留原 message.id，跨文件也必须合并为一条）；冲突时保留
//!   computed_total 更大的一行，并打上 updated_at 修订标记——同步层据此把
//!   已上传的中间值补传修正为终值（Claude 独有：Codex 记录落盘后不可变）。
//! - **额度来源**：会话文件里没有 rate_limits 快照（Codex 有），订阅额度只能
//!   实时调 OAuth 端点 GET https://api.anthropic.com/api/oauth/usage
//!   （Claude Code CLI 内部同款，需 claude.ai 订阅登录；第三方中转/API Key
//!   模式无凭据，额度块整体不展示）。凭据只读，绝不刷新/写回——refresh_token
//!   是一次性轮换的，外部写回会搞坏 Claude Code 登录。
//!
//! 实现方式与 codex.rs 同构：原始 jsonl 只读 + 派生自有库 ~/.zbar/claude.sqlite，
//! file_progress 偏移增量续读。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::db;
use crate::pricing::config_dir;

// ===== 路径定位 =====

/// Claude projects 目录路径（不做存在性检查，供诊断展示）。
/// 环境变量 ZBAR_CLAUDE_HOME（指向 .claude 根目录）优先，否则 ~/.claude/projects。
fn projects_dir_path() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_CLAUDE_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("projects");
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".claude").join("projects")
}

/// 定位 Claude 会话目录。目录不存在返回友好中文错误（调用方按需容错降级）。
pub fn projects_dir() -> Result<PathBuf, String> {
    let p = projects_dir_path();
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!(
            "未找到 Claude 会话目录: {}。请确认 Claude Code 已安装并使用过，或设置 ZBAR_CLAUDE_HOME 环境变量指向 .claude 根目录。",
            p.display()
        ))
    }
}

/// 自有导入库路径：~/.zbar/claude.sqlite
fn claude_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("claude.sqlite"))
}

/// 打开（必要时创建）导入库并确保表结构就绪。这是自有库，读写均可用。
/// 与 codex.sqlite 同构，但无 rate_limits_state 表（会话文件里没有额度数据）。
///
/// 去重键 dedupe_key（全局唯一）：
/// - 有 message.id 的行 → message.id 本身（Claude Code 的 resume/continue 是
///   fork 语义：新会话文件会复制原历史且保留原 message.id，按会话内去重会把
///   同一次 API 调用算两遍，故 id 行必须跨文件全局去重）；
/// - 无 id 的旧行 → "<session_id>|<行序号>"（仅会话内去重）。
/// 冲突时保留 computed_total 更大的一行（同 id 多行 usage 为累计口径，末行
/// 即终值；取更大者防御末行意外回退的脏数据）。被覆盖的行 updated_at 记录
/// 修订时间，供同步补传（否则已上传的中间值永远不会被终值修正）。

/// 进程内串行化结构迁移（codex.rs SCHEMA_MIGRATION_LOCK 同款）。
static SCHEMA_MIGRATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_migration_lock() -> &'static Mutex<()> {
    SCHEMA_MIGRATION_LOCK.get_or_init(|| Mutex::new(()))
}

fn open_claude_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = claude_db_path()?;
    let conn = Connection::open(&path).map_err(|e| format!("打开 Claude 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;

    // 结构版本迁移：表存在但缺 dedupe_key / duration_ms 列（旧版结构）时整表重建。
    // 本库是从原始 jsonl 全量派生的缓存库，重建后下次导入自动补齐。重建后自增
    // id 从头重排，普通 id 游标选不出低 id 行，靠修订通道（query_revised_since，
    // 重建行 updated_at=导入时刻）全量补传到服务端收敛——勿删修订通道。
    // 进程内互斥：升级后首次启动多个命令并发 open，避免同时判旧+交错 DROP/CREATE
    //（跨进程竞态由 DROP IF EXISTS / CREATE IF NOT EXISTS 幂等兜底）。
    {
        let _guard = schema_migration_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let legacy = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table' AND name = 'model_usage')
                      + (SELECT COUNT(*) FROM pragma_table_info('model_usage')
                         WHERE name = 'dedupe_key')
                      + (SELECT COUNT(*) FROM pragma_table_info('model_usage')
                         WHERE name = 'duration_ms')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c == 1 || c == 2) // 表存在(1)+两列全缺(0)→1；表存在+缺一列→2；表不存在→0 或新结构→3
            .unwrap_or(false);
        if legacy {
            eprintln!("[zbar-claude] 检测到旧版导入库结构，重建（自动从原始会话重新导入）");
            conn.execute_batch("DROP TABLE IF EXISTS model_usage; DROP TABLE IF EXISTS file_progress;")
                .map_err(|e| format!("重建 Claude 导入库失败: {e}"))?;
        }
    }

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS model_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            dedupe_key TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            model_id TEXT NOT NULL DEFAULT '',
            provider_id TEXT NOT NULL DEFAULT 'claude',
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            computed_total_tokens INTEGER NOT NULL DEFAULT 0,
            duration_ms REAL,
            updated_at INTEGER NOT NULL DEFAULT 0,
            cwd TEXT,
            project_key TEXT,
            UNIQUE(dedupe_key)
         );
         CREATE INDEX IF NOT EXISTS idx_claude_model_usage_started ON model_usage(started_at);
         CREATE INDEX IF NOT EXISTS idx_claude_model_usage_updated ON model_usage(updated_at);
         CREATE INDEX IF NOT EXISTS idx_claude_mu_session ON model_usage(session_id);
         CREATE TABLE IF NOT EXISTS file_progress (
            path   TEXT    PRIMARY KEY,
            offset INTEGER NOT NULL,
            size   INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("初始化 Claude 导入库失败: {e}"))?;
    ensure_project_columns(&conn)?;
    Ok(conn)
}

/// 进程内串行化补列迁移（kimi.rs ENSURE_DURATION_LOCK 同款）：
/// 升级后首次启动多个查询命令并发 open，避免同时判缺列 + 交错重复 ALTER。
static ENSURE_PROJECT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 项目维度补列迁移：升级前创建的旧库 model_usage 无 cwd / project_key 列时
/// ALTER TABLE ADD COLUMN 补加（允许 NULL，旧行自然为 NULL，由存量回填补齐）。
/// 先 PRAGMA table_info 检查再 ALTER，幂等；新库已由 CREATE TABLE 直接带列。
fn ensure_project_columns(conn: &Connection) -> Result<(), String> {
    let lock = ENSURE_PROJECT_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for column in ["cwd", "project_key"] {
        if db::has_column(conn, "model_usage", column) {
            continue;
        }
        conn.execute_batch(&format!("ALTER TABLE model_usage ADD COLUMN {column} TEXT"))
            .map_err(|e| format!("迁移 Claude 导入库（补 {column} 列）失败: {e}"))?;
    }
    Ok(())
}

// ===== jsonl 行解析结构（未知字段自动忽略，巨大的 content 数组不会物化）=====

/// jsonl 单行事件。只取关心的时间戳/类型/message。
/// cwd 为行内顶层字段（会话的工作目录，user/assistant 行均携带），
/// 供项目维度聚合使用；旧行/无 cwd 的行为 None。
#[derive(Debug, Deserialize)]
struct TranscriptLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    line_type: Option<String>,
    #[serde(default)]
    message: Option<AssistantMessage>,
    #[serde(default)]
    cwd: Option<String>,
}

/// assistant 行的 message 对象（content 不取，serde 忽略未知字段）。
/// durationMs 为该次调用的总耗时（毫秒），旧版本 CLI 的行无此字段 → None。
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(rename = "durationMs", default)]
    duration_ms: Option<f64>,
    #[serde(default)]
    usage: Option<UsagePayload>,
}

/// 单次 API 调用的 token 用量。Claude 无 total 字段，四项之和即总量；
/// 也无独立 reasoning 字段（thinking 计入 output）。
#[derive(Debug, Default, Deserialize)]
struct UsagePayload {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
}

impl UsagePayload {
    fn computed_total(&self) -> i64 {
        self.input_tokens + self.output_tokens + self.cache_read_input_tokens
            + self.cache_creation_input_tokens
    }
}

/// ISO8601 时间戳（如 2026-06-12T19:12:00.759Z）→ 毫秒时间戳
fn parse_ts_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 从文件名提取会话标识（<uuid>.jsonl → <uuid>）。
/// 会话文件与其子代理文件都以 uuid 命名，天然互不重名；主干即稳定标识。
fn session_id_from_filename(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ===== 增量导入 =====

/// 导入互斥锁：面板查询 / 托盘标题刷新 / 同步上传可能并发触发导入，
/// 串行化避免同一文件被双份解析（唯一键可去重，但重复 IO 浪费）。
static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn import_lock() -> &'static Mutex<()> {
    IMPORT_LOCK.get_or_init(|| Mutex::new(()))
}

/// 上次导入时间（节流用）。各查询入口都会触发导入，前端 30s 一轮 × 多个
/// 命令会放大扫描次数；会话文件分钟级追加，5 秒节流足够实时。
static LAST_IMPORT_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

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

/// 递归收集 projects 目录下所有会话 jsonl。
/// 层级为 <项目目录>/<uuid>.jsonl 与 <项目目录>/<uuid>/subagents/<uuid>.jsonl，
/// 防御性下钻 5 层。结果排序保证导入顺序稳定。
fn collect_session_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_session_files(&path, depth - 1, out);
            }
        } else {
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
}

/// 增量导入（不节流）：遍历 projects 目录，把每个会话文件新增部分解析入库。
/// - file_progress 记录"已处理到的字节偏移"（对齐完整行末尾）；文件变短
///   （被重写）时从头重新解析，UNIQUE 键保证幂等。
/// - 同一 message.id 的后续行（续传/重写场景）靠 ON CONFLICT 覆盖旧值。
/// - 每个文件一个事务：中途崩溃整体回滚，下次从旧偏移重来。
/// - 单文件失败只记日志跳过，不阻断其他文件。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = projects_dir()?;
    let mut conn = open_claude_db()?;

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
    collect_session_files(&dir, 5, &mut files);
    files.sort();

    for path in &files {
        let key = path.to_string_lossy().to_string();
        let known = progress.get(&key).copied();
        if let Err(e) = import_one_file(&mut conn, path, known) {
            eprintln!(
                "[zbar-claude] 导入 {} 失败（下次重试）: {e}",
                path.display()
            );
        }
    }

    // 存量行项目维度回填（节流 + 每批限量，幂等说明见函数注释）
    if let Err(e) = backfill_project_keys_locked(&mut conn) {
        eprintln!("[zbar-claude] 项目维度回填失败（下次重试）: {e}");
    }
    Ok(())
}

/// 上次项目维度回填时间（节流用）。
static LAST_PROJECT_BACKFILL_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

/// 读取会话文件前 N 行，找首个非空 cwd（旧行无行内 cwd 时回填用）。
fn read_first_cwd(path: &Path, max_lines: usize) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    for line in std::io::BufReader::new(file).lines().take(max_lines).flatten() {
        if let Ok(line) = serde_json::from_str::<TranscriptLine>(&line) {
            if let Some(cwd) = line
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// 存量行项目维度回填（调用方必须已持有 import_lock）。
///
/// 幂等性保证（重复调用不重复读文件）：
/// - 只选 project_key 仍为 NULL 的会话（升级前的旧行 / 行内无 cwd 的新行），
///   回填后（含找不到 cwd 而标成 UNKNOWN 哨兵的）不再进入候选集；
/// - 每个候选会话只读一次源文件的前 20 行；找不到 cwd 时把该会话全部
///   NULL 行直接置为 UNKNOWN 哨兵，下一轮候选查询即排除，不会重复 IO；
/// - UPDATE 带 `AND project_key IS NULL` 条件，与增量导入写入的值不冲突；
/// - 30 秒节流 + 每批最多 500 个会话，量大时靠多轮调用分批消化。
fn backfill_project_keys_locked(conn: &mut Connection) -> Result<usize, String> {
    {
        let mut last = LAST_PROJECT_BACKFILL_AT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.map(|t| t.elapsed() < std::time::Duration::from_secs(30)) == Some(true) {
            return Ok(0);
        }
        *last = Some(std::time::Instant::now());
    }

    let missing: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT session_id FROM model_usage WHERE project_key IS NULL LIMIT 500")
            .map_err(|e| format!("查询待回填 Claude 会话失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询待回填 Claude 会话失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("查询待回填 Claude 会话失败: {e}"))?;
        rows
    };
    if missing.is_empty() {
        return Ok(0);
    }

    // session_id → 源文件映射（文件名主干即会话 uuid；目录已消失/文件被删
    // 的会话直接落 UNKNOWN 哨兵）
    let dir = projects_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 5, &mut files);
    }
    let mut by_session: HashMap<String, &Path> = HashMap::new();
    for path in &files {
        by_session
            .entry(session_id_from_filename(path))
            .or_insert(path);
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启 Claude 回填事务失败: {e}"))?;
    for session_id in &missing {
        let cwd = by_session
            .get(session_id.as_str())
            .and_then(|path| read_first_cwd(path, 20));
        match cwd.and_then(|c| {
            crate::projects::normalize_cwd(&c)
                .map(|key| (c, key))
        }) {
            Some((raw, key)) => {
                tx.execute(
                    "UPDATE model_usage SET cwd = ?1, project_key = ?2
                     WHERE session_id = ?3 AND project_key IS NULL",
                    rusqlite::params![raw, key, session_id],
                )
                .map_err(|e| format!("回填 Claude 项目维度失败: {e}"))?;
            }
            None => {
                // 找不到 cwd（文件被删/前 20 行无 cwd）：置哨兵防止重复扫描
                tx.execute(
                    "UPDATE model_usage SET project_key = ?1
                     WHERE session_id = ?2 AND project_key IS NULL",
                    rusqlite::params![crate::projects::UNKNOWN_PROJECT, session_id],
                )
                .map_err(|e| format!("回填 Claude 项目维度哨兵失败: {e}"))?;
            }
        }
    }
    tx.commit()
        .map_err(|e| format!("提交 Claude 回填事务失败: {e}"))?;
    Ok(missing.len())
}

/// 存量行项目维度回填（公开入口，项目浏览器查询前调用）。
/// 与增量导入共用 IMPORT_LOCK 串行；内部自带节流与分批限量。
pub fn backfill_project_keys() -> Result<usize, String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut conn = open_claude_db()?;
    backfill_project_keys_locked(&mut conn)
}

/// 解析单个会话文件的增量部分。known = 上次记录的 (offset, size)。
fn import_one_file(
    conn: &mut Connection,
    path: &Path,
    known: Option<(u64, u64)>,
) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取文件元信息失败: {e}"))?
        .len();

    // 无进度记录，或文件比记录时更短（被重写）→ 从头解析（唯一键幂等）
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

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启导入事务失败: {e}"))?;

    let mut pos = start_offset;
    let mut last_complete_end = start_offset;
    // 无 message.id 的行（旧版本 CLI）退回"会话内行序号"做去重键：
    // - 续读（文件增长）：从该会话已用的最大序号继续，避免 seq:N 与历史行
    //   撞键错乱合并（撞键后"总量更大者胜"会把两条不同请求折叠成一条）；
    // - 重解析（文件变短被重写）：从 0 重计，重放行与旧行撞相同键，
    //   靠"总量更大者胜"幂等去重（与 codex.rs 的 seq 语义一致）。
    let mut line_seq: i64 = 0;
    // 项目维度：行内顶层 cwd（user/assistant 行均携带），任一行出现即刷新，
    // 后续 token 用量行继承当前值（同文件 cwd 一致）；解析到则归一化落库，
    // 整个文件都无 cwd 时保持 NULL（存量回填/查询侧归未知项目）。
    let mut current_cwd: Option<String> = None;
    let mut current_project: Option<String> = None;
    if !reparse {
        let prefix = format!("{session_id}|");
        let keys: Vec<String> = {
            let mut stmt = match tx.prepare(
                "SELECT dedupe_key FROM model_usage WHERE session_id = ?1 AND dedupe_key LIKE ?2",
            ) {
                Ok(s) => s,
                Err(_) => return Err("准备序号恢复查询失败".into()),
            };
            let like = format!("{prefix}%");
            // 先绑定再 match：让 MappedRows 临时值在 stmt 之前析构
            let rows = stmt.query_map(rusqlite::params![session_id, like], |row| {
                row.get::<_, String>(0)
            });
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            }
        };
        line_seq = keys
            .iter()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|s| s.parse::<i64>().ok())
            .max()
            .unwrap_or(0);
    }
    // 项目上下文恢复：续读窗口内可能没有 cwd 行（在上次已读部分），从该会话
    // 已入库行恢复，避免新增行 project_key 退化为 NULL；重解析路径从文件头
    // 重读，cwd 行自然重建上下文。
    if !reparse {
        let _ = tx.query_row(
            "SELECT cwd, project_key FROM model_usage
             WHERE session_id = ?1 AND project_key IS NOT NULL
             LIMIT 1",
            rusqlite::params![session_id],
            |row| {
                current_cwd = row.get(0)?;
                current_project = row.get(1)?;
                Ok(())
            },
        );
    }
    // 修订时间：同一 message.id 的终值晚于中间值落盘，覆盖已入库行时打上
    // 修订标记（同步层据此把修正后的值补传到服务端）
    let now_ms = chrono::Local::now().timestamp_millis();

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
        let Ok(line) = serde_json::from_slice::<TranscriptLine>(&buf) else {
            continue;
        };
        // 任何携带 cwd 的行都刷新项目上下文（不限于 assistant 行）
        if let Some(cwd) = line
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            let cwd = cwd.to_string();
            if let Some(project) = crate::projects::normalize_cwd(&cwd) {
                current_project = Some(project);
            }
            current_cwd = Some(cwd);
        }
        if line.line_type.as_deref() != Some("assistant") {
            continue;
        }
        let Some(msg) = line.message.as_ref() else {
            continue;
        };
        // CLI 内部占位行（如中断提示）模型名为 <synthetic>，跳过
        let model = match msg.model.as_deref() {
            Some(m) if !m.is_empty() && m != "<synthetic>" => m.to_string(),
            _ => continue,
        };
        let Some(usage) = msg.usage.as_ref() else {
            continue;
        };
        let total = usage.computed_total();
        if total <= 0 {
            // 流式过程中的 0 值占位行 / 空调用，跳过（真正终值由后续行携带）
            continue;
        }
        // 没有时间戳无法归入统计区间，跳过（偏移仍推进）
        let Some(started_at) = line.timestamp.as_deref().and_then(parse_ts_ms) else {
            continue;
        };

        line_seq += 1;
        // 有 id 的行全局去重（防 fork 复制历史导致跨文件双计）；
        // 无 id 行仅会话内去重（序号键带 session 前缀，message.id 形如
        // msg_<hex>，不含 "|"，两类键天然不冲突）
        let dedupe_key = msg
            .id
            .as_deref()
            .filter(|id| !id.is_empty() && !id.contains('|'))
            .map(|id| id.to_string())
            .unwrap_or_else(|| format!("{session_id}|{line_seq}"));

        // 同一 message.id 的多行 usage 为累计口径（末行即终值）：后写覆盖先写；
        // 仅当新行总量更大时覆盖，防御末行意外回退为小值的脏数据。
        // 覆盖不改变自增 id（上传 rowid 稳定），但打上 updated_at 修订标记。
        // 项目维度列不参与覆盖更新（fork 复制历史时行归属首个项目，保持稳定）。
        tx.execute(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 duration_ms, updated_at, cwd, project_key)
             VALUES (?1, ?2, ?3, ?4, 'claude', ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(dedupe_key) DO UPDATE SET
                started_at = excluded.started_at,
                model_id = excluded.model_id,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens,
                cache_creation_input_tokens = excluded.cache_creation_input_tokens,
                computed_total_tokens = excluded.computed_total_tokens,
                duration_ms = excluded.duration_ms,
                updated_at = ?11
             WHERE excluded.computed_total_tokens > model_usage.computed_total_tokens",
            rusqlite::params![
                session_id,
                dedupe_key,
                started_at,
                model,
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
                total,
                msg.duration_ms,
                now_ms,
                current_cwd,
                current_project,
            ],
        )
        .map_err(|e| format!("写入用量记录失败: {e}"))?;
    }

    // 进度：对齐到最后一条完整行末尾（末尾半行下次重读，靠唯一键幂等）
    let key = path.to_string_lossy().to_string();
    tx.execute(
        "INSERT INTO file_progress (path, offset, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET offset = ?2, size = ?3",
        rusqlite::params![key, last_complete_end as i64, size as i64],
    )
    .map_err(|e| format!("记录导入进度失败: {e}"))?;

    tx.commit().map_err(|e| format!("提交导入事务失败: {e}"))?;
    Ok(())
}

// ===== 查询函数（与 codex.rs 同名同构，查 claude.sqlite；查询前先增量导入）=====

/// 查询 [from_ms, to_ms) 内的统计（口径与 db::query_stats 完全一致）。
/// Claude 会话无 TTFT 数据（jsonl 只有总耗时 durationMs），首字延迟恒为 None。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let speed = db::speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        false,
    );

    let overall: db::OverallStat = conn
        .query_row(
            &format!(
                "SELECT
                    COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(cache_creation_input_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0),
                    COALESCE(SUM(computed_total_tokens),0)
                    {speed}
                 FROM model_usage
                 WHERE started_at >= ?1 AND started_at < ?2"
            ),
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
                    speed: db::SpeedMetrics {
                        avg_tps: row.get(7)?,
                        max_tps: row.get(8)?,
                        avg_ttft_ms: row.get(9)?,
                    },
                })
            },
        )
        .map_err(|e| format!("查询 Claude 整体统计失败: {e}"))?;

    let mut stmt = conn
        .prepare(&format!(
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
                {speed}
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider_id, model_id
             ORDER BY total_tokens DESC",
        ))
        .map_err(|e| format!("准备 Claude 模型分组查询失败: {e}"))?;

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
                speed: db::SpeedMetrics {
                    avg_tps: row.get(9)?,
                    max_tps: row.get(10)?,
                    avg_ttft_ms: row.get(11)?,
                },
            })
        })
        .map_err(|e| format!("读取 Claude 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 模型分组失败: {e}"))?;

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 Claude 时间范围失败: {e}"))?;

    Ok(db::Stats {
        from_ms,
        to_ms,
        overall,
        by_model,
        earliest_ms,
        latest_ms,
        current_model: db::query_current_model(&conn),
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
    let conn = open_claude_db()?;
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
        .map_err(|e| format!("准备 Claude 趋势查询失败: {e}"))?;

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
            .map_err(|e| format!("读取 Claude 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 Claude 趋势统计失败: {e}"))?;

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

/// 查询 id > since 的明细记录（同步上传用）。source 固定 "claude"，local_rowid = id。
/// proto 5：附带会话 id 与项目维度（project_key / 原始 cwd 为行内列，
/// 未回填的旧行两字段为 None，服务端落 NULL）。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, model_id, provider_id,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                    session_id, project_key, cwd
             FROM model_usage
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2",
        )
        .map_err(|e| format!("准备 Claude 增量查询失败: {e}"))?;
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
                source: "claude".into(),
                session_id: row.get(10)?,
                project_key: row.get(11)?,
                project_display: row.get(12)?,
            })
        })
        .map_err(|e| format!("读取 Claude 增量记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 增量记录失败: {e}"))?;
    Ok(rows)
}

/// 导入库当前最大 rowid（供「待上传条数」显示用）。
pub fn max_rowid() -> Result<i64, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let max: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM model_usage", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("查询 Claude 最大 rowid 失败: {e}"))?;
    Ok(max)
}

/// 查询修订行（updated_at > since_ts 的记录，同步补传用）。
/// Claude 会话流式落盘：同一 message.id 的中间值可能先入库并被上传，
/// 终值稍后覆盖本地行（id 不变）——常规的 id > 游标查询永远选不出它，
/// 服务端会一直保留旧小值。本查询按修订时间选出这些行，上传后服务端
/// 以"总量更大者胜"的 upsert 覆盖修正（见 server/db.py）。
/// after_id 分页：按 id 升序、只取 id > after_id 的行，调用方逐批推进。
pub fn query_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, model_id, provider_id,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                    session_id, project_key, cwd
             FROM model_usage
             WHERE updated_at > ?1 AND id > ?2
             ORDER BY id ASC
             LIMIT ?3",
        )
        .map_err(|e| format!("准备 Claude 修订查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since_ts, after_id, limit as i64], |row| {
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
                source: "claude".into(),
                session_id: row.get(10)?,
                project_key: row.get(11)?,
                project_display: row.get(12)?,
            })
        })
        .map_err(|e| format!("读取 Claude 修订记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 修订记录失败: {e}"))?;
    Ok(rows)
}

/// 列出导入库中出现过的所有 (provider_id, model_id) 组合，供价格配置用。
/// provider_id 恒为 "claude"。
pub fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 Claude 模型列表查询失败: {e}"))?;

    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 Claude 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 模型列表失败: {e}"))?;
    Ok(models)
}

/// 按指定周期聚合 Claude Token。
/// 对比页需要真实的 [reset_at, end_at) 边界，不能用只带 HH:00 的趋势 label 反推跨日周期。
pub fn query_period_buckets(
    periods: &[(i64, i64)],
) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(SUM(computed_total_tokens),0),
                COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 Claude 周期聚合查询失败: {e}"))?;

    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 Claude 周期聚合失败: {e}"))?;
        out.push(db::PeriodBucket {
            reset_at,
            end_at,
            total_tokens,
            requests,
        });
    }
    Ok(out)
}

// ===== 项目/会话维度查询（项目浏览器用，cwd 归一化见 projects.rs）=====

/// 按项目 × 模型聚合 [from_ms, to_ms) 的用量（SQL 侧 GROUP BY，不逐行加载）。
/// project_key 为 NULL/空串（未回填或无 cwd）归入 unknown 哨兵键。
pub fn query_project_model_rows(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(project_key, ''), ?1),
                    model_id,
                    COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?2 AND started_at < ?3
             GROUP BY 1, 2",
        )
        .map_err(|e| format!("准备 Claude 项目聚合查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| {
                Ok(crate::projects::ProjectModelRow {
                    project_key: row.get(0)?,
                    model_id: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                })
            },
        )
        .map_err(|e| format!("读取 Claude 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// 按项目统计 [from_ms, to_ms) 内的会话数（COUNT(DISTINCT session_id)）。
pub fn query_project_session_counts(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(project_key, ''), ?1), COUNT(DISTINCT session_id)
             FROM model_usage
             WHERE started_at >= ?2 AND started_at < ?3
             GROUP BY 1",
        )
        .map_err(|e| format!("准备 Claude 项目会话统计失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("读取 Claude 项目会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目会话统计失败: {e}"))?;
    Ok(rows)
}

/// 项目键 → 原始形态 cwd（保留大小写，取字典序最小者做代表）。
pub fn query_project_display_paths() -> Result<Vec<(String, String)>, String> {
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT project_key, MIN(cwd)
             FROM model_usage
             WHERE project_key IS NOT NULL AND project_key != ?1
               AND cwd IS NOT NULL AND cwd != ''
             GROUP BY project_key",
        )
        .map_err(|e| format!("准备 Claude 项目路径查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![crate::projects::UNKNOWN_PROJECT], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("读取 Claude 项目路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目路径失败: {e}"))?;
    Ok(rows)
}

/// 分页查询指定项目的会话明细（按会话最后活跃时间降序）。
/// 返回 (匹配会话总数, 按会话 × 模型聚合的行)；offset/limit 作用在会话粒度。
/// project_key 传 UNKNOWN_PROJECT 时匹配 NULL/空串/哨兵的会话。
pub fn query_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                SELECT DISTINCT session_id
                FROM model_usage
                WHERE started_at >= ?1 AND started_at < ?2
                  AND COALESCE(NULLIF(project_key, ''), ?3) = ?4
            )",
            rusqlite::params![from_ms, to_ms, crate::projects::UNKNOWN_PROJECT, project_key],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询 Claude 项目会话总数失败: {e}"))?;

    // 会话级速度聚合（口径与 query_stats 的面板统计完全一致，无 TTFT 列 →
    // ttft 恒 None；传 SUM/COUNT 供会话级跨模型合并，避免二次平均偏差）
    let speed = db::session_speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        false,
    );
    let mut stmt = conn
        .prepare(&format!(
            "SELECT session_id, model_id,
                    MIN(started_at), MAX(started_at), COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(cache_creation_input_tokens),0)
                    {speed}
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
               AND COALESCE(NULLIF(project_key, ''), ?3) = ?4
               AND session_id IN (
                   SELECT session_id FROM (
                       SELECT session_id
                       FROM model_usage
                       WHERE started_at >= ?1 AND started_at < ?2
                         AND COALESCE(NULLIF(project_key, ''), ?3) = ?4
                       GROUP BY session_id
                       ORDER BY MAX(started_at) DESC
                       LIMIT ?5 OFFSET ?6
                   )
               )
             GROUP BY session_id, model_id"
        ))
        .map_err(|e| format!("准备 Claude 项目会话查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                from_ms,
                to_ms,
                crate::projects::UNKNOWN_PROJECT,
                project_key,
                limit as i64,
                offset as i64
            ],
            |row| {
                Ok(crate::projects::ProjectSessionModelRow {
                    session_id: row.get(0)?,
                    model_id: row.get(1)?,
                    first_at: row.get(2)?,
                    last_at: row.get(3)?,
                    requests: row.get(4)?,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    cache_read_tokens: row.get(7)?,
                    cache_write_tokens: row.get(8)?,
                    tps_sum: row.get(9)?,
                    tps_count: row.get(10)?,
                    ttft_sum: row.get(11)?,
                    ttft_count: row.get(12)?,
                })
            },
        )
        .map_err(|e| format!("读取 Claude 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

// ===== 实时额度（Anthropic OAuth 端点，参照 CodexBar 的实现）=====

/// Claude 订阅额度（字段口径与 CodexRateLimits 一致，前端同款渲染）。
/// plan_type 来自凭据的订阅类型（pro/max 等；中转模式无凭据 → None）。
/// 模型专属周窗口 / 超额消费为增量字段：API 响应存在才有值（serde 序列化
/// 时 None 直接省略），旧响应与现有渲染完全不受影响。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeRateLimits {
    pub plan_type: Option<String>,
    pub primary_pct: Option<f64>,
    /// 5 小时会话窗口重置时间（毫秒时间戳）
    pub primary_reset_at: Option<i64>,
    pub secondary_pct: Option<f64>,
    /// 周窗口重置时间（毫秒时间戳）
    pub secondary_reset_at: Option<i64>,
    /// Sonnet 模型专属周窗口已用百分比（seven_day_sonnet / limits[].weekly_scoped）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sonnet_weekly_pct: Option<f64>,
    /// Sonnet 模型专属周窗口重置时间（毫秒时间戳）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sonnet_weekly_reset_at: Option<i64>,
    /// Opus 模型专属周窗口已用百分比（seven_day_opus / limits[].weekly_scoped）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_weekly_pct: Option<f64>,
    /// Opus 模型专属周窗口重置时间（毫秒时间戳）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_weekly_reset_at: Option<i64>,
    /// 超额消费已用金额（美元；月度 extra_usage，字段双兼容解析）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_used: Option<f64>,
    /// 超额消费上限金额（美元；缺省时前端只展示已用金额）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_limit: Option<f64>,
}

/// 实时额度失败分类（本地链路缓存与手动凭证链路共用）。授权类失败
/// （OAuth 条目缺失 / token 被拒）是用户可自助修复的问题，透出明确中文
/// 提示；网络类失败保持既有静默降级行为（额度块不展示即可）。
#[derive(Debug, Clone)]
pub enum ClaudeUsageFailure {
    /// 凭据中找不到 Claude OAuth 条目（如 Claude Code 2.1.x 钥匙串条目
    /// 仅含 mcpOAuth / 缺 accessToken）
    MissingOAuth(String),
    /// OAuth token 被服务端拒绝（HTTP 401/403）
    TokenRejected,
    /// 网络/服务/解析等其他失败（消息仅用于日志与诊断，不透出前端）
    Other(String),
}

/// OAuth 条目缺失（含 Claude Code 2.1.x 凭据仅含 mcpOAuth 的场景）的统一
/// 明确提示：区别于模糊网络错误，用户运行一次 claude 即可自助修复。
const MISSING_OAUTH_MESSAGE: &str =
    "未找到 Claude OAuth 凭证，请在终端运行一次 claude 完成登录授权";

/// token 被服务端拒绝（401/403）时的统一提示。
const TOKEN_REJECTED_MESSAGE: &str = "OAuth Token 已失效，请在终端重新运行 claude 授权";

impl ClaudeUsageFailure {
    /// 完整中文消息（日志 / 诊断 / 测试断言用）。
    pub fn message(&self) -> String {
        match self {
            ClaudeUsageFailure::MissingOAuth(m) => m.clone(),
            ClaudeUsageFailure::TokenRejected => TOKEN_REJECTED_MESSAGE.to_string(),
            ClaudeUsageFailure::Other(e) => e.clone(),
        }
    }

    /// 透出给前端的错误提示：仅授权类失败有明确提示（用户可自助修复），
    /// 网络类失败返回 None（保持静默降级，与既有行为一致）。
    pub fn user_message(&self) -> Option<String> {
        match self {
            ClaudeUsageFailure::Other(_) => None,
            _ => Some(self.message()),
        }
    }
}

/// Claude Code 登录凭证（只读，绝不修改/刷新——refresh_token 一次性轮换，
/// 外部写回极易搞坏 Claude Code 登录；token 过期由 Claude Code 自行刷新）。
#[derive(Debug)]
struct ClaudeAuth {
    access_token: String,
    /// 订阅类型徽标（subscriptionType 优先，rateLimitTier 兜底）
    plan_label: Option<String>,
}

/// 订阅套餐展示名归一（纯函数）：rateLimitTier 含乘数档位标识
/// （default_claude_max_5x / default_claude_max_20x 等）时归一为
/// "Max 5x"/"Max 20x"——乘数是用户最关心的信息，原始 tier 串过长不适合
/// 徽标展示；其余原样返回。
fn plan_display_name(label: &str) -> String {
    let lower = label.to_ascii_lowercase();
    if lower.contains("20x") {
        "Max 20x".to_string()
    } else if lower.contains("5x") {
        "Max 5x".to_string()
    } else {
        label.to_string()
    }
}

/// 解析 .credentials.json / Keychain 里的凭据 JSON。
/// 结构：{ "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt"(毫秒),
/// "scopes", "rateLimitTier", "subscriptionType" } }（字段名为 camelCase）。
/// OAuth 条目缺失（含仅 mcpOAuth）/ 缺 accessToken 归 MissingOAuth 明确
/// 错误态；JSON 本身解析失败归 Other。
fn parse_credentials_json(data: &str) -> Result<ClaudeAuth, ClaudeUsageFailure> {
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| ClaudeUsageFailure::Other(format!("解析 Claude 凭据失败: {e}")))?;
    let oauth = v
        .get("claudeAiOauth")
        .ok_or_else(|| ClaudeUsageFailure::MissingOAuth(MISSING_OAUTH_MESSAGE.into()))?;
    let token = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ClaudeUsageFailure::MissingOAuth(MISSING_OAUTH_MESSAGE.into()))?;
    // 套餐名：rateLimitTier 含乘数档位（5x/20x）时优先归一为 "Max 5x"/"Max 20x"
    //（subscriptionType 通常是 pro/max，不携带乘数信息）；否则沿用既有
    // subscriptionType → rateLimitTier 优先级原样展示。
    let tier = oauth
        .get("rateLimitTier")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let sub_type = oauth
        .get("subscriptionType")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let plan_label = tier
        .filter(|t| {
            let lower = t.to_ascii_lowercase();
            lower.contains("5x") || lower.contains("20x")
        })
        .map(plan_display_name)
        .or_else(|| sub_type.map(|s| s.to_string()))
        .or_else(|| tier.map(|t| t.to_string()));
    Ok(ClaudeAuth {
        access_token: token.to_string(),
        plan_label,
    })
}

/// 读取 Claude Code 登录凭证：
/// 1. ZBAR_CLAUDE_HOME 环境变量或 ~/.claude 下的 .credentials.json
///    （Windows/Linux 的标准位置）；
/// 2. macOS 上凭据在登录钥匙串（服务名 "Claude Code-credentials"），经
///    `security find-generic-password -w` 读取（只读；若系统弹窗询问则取消，
///    读取失败按"无凭据"降级，不影响 token 统计）。
fn load_claude_auth() -> Result<ClaudeAuth, ClaudeUsageFailure> {
    let root = std::env::var("ZBAR_CLAUDE_HOME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
        });
    let path = root.join(".credentials.json");
    if path.exists() {
        let data = std::fs::read_to_string(&path).map_err(|e| {
            ClaudeUsageFailure::Other(format!("读取 .credentials.json 失败: {e}"))
        })?;
        return parse_credentials_json(&data);
    }

    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                let data = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !data.is_empty() {
                    return parse_credentials_json(&data);
                }
            }
        }
    }

    Err(ClaudeUsageFailure::Other(
        "未找到 Claude 登录凭证（.credentials.json / 钥匙串），订阅额度不可用（token 统计不受影响）"
            .into(),
    ))
}

/// /api/oauth/usage 响应结构（窗口字段名与 Codex 的 wham/usage 不同：
/// 这里是 five_hour/seven_day + utilization 百分比(0-100) + resets_at ISO8601）。
/// 增量字段：seven_day_sonnet / seven_day_opus 模型专属周窗口、extra_usage
/// 月度超额消费、limits[] 补充窗口数组——均为 Option/默认值，旧响应不受影响。
#[derive(Debug, Deserialize)]
struct OAuthUsageResponse {
    #[serde(default)]
    five_hour: Option<OAuthWindow>,
    #[serde(default)]
    seven_day: Option<OAuthWindow>,
    /// Sonnet 模型专属周窗口（部分套餐返回；结构与 seven_day 相同）
    #[serde(default)]
    seven_day_sonnet: Option<OAuthWindow>,
    /// Opus 模型专属周窗口（结构与 seven_day 相同）
    #[serde(default)]
    seven_day_opus: Option<OAuthWindow>,
    /// 月度超额消费（金额结构；字段名做 used/limit 双兼容尝试，见
    /// parse_extra_usage——保留原始 Value 以兼容字段名差异）
    #[serde(default)]
    extra_usage: Option<serde_json::Value>,
    /// limits[] 补充窗口（带 weekly_scoped 标识的模型专属行，见
    /// apply_scoped_weekly_limit）
    #[serde(default)]
    limits: Vec<OAuthLimit>,
}

#[derive(Debug, Deserialize)]
struct OAuthWindow {
    /// 已用百分比（0-100，社区实测与 CodexBar 均按百分数直读）
    #[serde(default)]
    utilization: Option<f64>,
    /// 重置时间（ISO8601，如 2026-08-14T12:34:56Z）
    #[serde(default)]
    resets_at: Option<String>,
}

/// limits[] 条目（CodexBar 口径）：weekly_scoped=true 表示模型专属周窗口，
/// 进模型专属行；scope 含 "All models" 的通用行留在主周行，不进专属槽位。
#[derive(Debug, Deserialize)]
struct OAuthLimit {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
    /// 窗口类型（"five_hour" / "seven_day"；仅周窗口类型进模型专属周行）
    #[serde(default)]
    limit_type: Option<String>,
    /// 归属范围（"All models" / "Opus" / "Sonnet"，大小写不敏感识别）
    #[serde(default)]
    scope: Option<String>,
    /// 模型专属周窗口标识
    #[serde(default)]
    weekly_scoped: Option<bool>,
}

/// extra_usage（月度超额消费，美元）→ (已用, 上限)。字段名按 utilization
/// 响应惯例做双兼容尝试：已用取 spend|used，上限取 limit|monthly_limit；
/// 两者全缺（或结构不是对象）→ None（不渲染附加信息行）。
fn parse_extra_usage(v: &serde_json::Value) -> Option<(Option<f64>, Option<f64>)> {
    let used = crate::provider_quota::num_any(v, &["spend", "used"]);
    let limit = crate::provider_quota::num_any(v, &["limit", "monthly_limit"]);
    (used.is_some() || limit.is_some()).then_some((used, limit))
}

/// limits[] 中带 weekly_scoped 标识的单条模型专属周窗口补位（纯函数）。
/// 只在对应槽位为空时写入（顶层 seven_day_opus/sonnet 已验证字段优先）；
/// "All models" 通用 scope 与非周窗口类型（five_hour）跳过，无法识别模型名
/// 的 scoped 条目保守忽略（不猜测归属）。
fn apply_scoped_weekly_limit(
    sonnet: &mut Option<OAuthWindow>,
    opus: &mut Option<OAuthWindow>,
    limit: &OAuthLimit,
) {
    if limit.weekly_scoped != Some(true) || limit.utilization.is_none() {
        return;
    }
    // 仅周窗口类型进模型专属周行（five_hour 的 scoped 条目不展示）
    if matches!(limit.limit_type.as_deref(), Some(t)
        if !t.contains("seven_day") && !t.contains("weekly"))
    {
        return;
    }
    let Some(scope) = limit.scope.as_deref().map(str::to_ascii_lowercase) else {
        return;
    };
    if scope.contains("all model") || scope.contains("all_model") {
        return; // 通用 scope 留主周行（seven_day），不进模型专属槽位
    }
    let slot = if scope.contains("opus") {
        opus
    } else if scope.contains("sonnet") {
        sonnet
    } else {
        return;
    };
    if slot.is_none() {
        *slot = Some(OAuthWindow {
            utilization: limit.utilization,
            resets_at: limit.resets_at.clone(),
        });
    }
}

/// /api/oauth/usage 响应体 → 结构化额度（纯函数，本地登录态链路与手动凭证
/// 链路共用；plan_type 由调用方按凭证信息填充，此处恒为 None）。
pub(crate) fn parse_usage_response(
    body: &str,
) -> Result<ClaudeRateLimits, ClaudeUsageFailure> {
    let mut resp: OAuthUsageResponse = serde_json::from_str(body).map_err(|e| {
        ClaudeUsageFailure::Other(format!("解析实时额度失败: {e}"))
    })?;
    // limits[] 中带 weekly_scoped 标识的模型专属窗口补位（顶层字段优先）
    let limits = std::mem::take(&mut resp.limits);
    for limit in &limits {
        apply_scoped_weekly_limit(&mut resp.seven_day_sonnet, &mut resp.seven_day_opus, limit);
    }

    let conv =
        |w: &OAuthWindow| -> (Option<f64>, Option<i64>) { (w.utilization, w.resets_at.as_deref().and_then(parse_ts_ms)) };
    let (primary_pct, primary_reset_at) = resp
        .five_hour
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (secondary_pct, secondary_reset_at) = resp
        .seven_day
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (sonnet_weekly_pct, sonnet_weekly_reset_at) = resp
        .seven_day_sonnet
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (opus_weekly_pct, opus_weekly_reset_at) = resp
        .seven_day_opus
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (extra_used, extra_limit) = resp
        .extra_usage
        .as_ref()
        .and_then(parse_extra_usage)
        .unwrap_or((None, None));

    Ok(ClaudeRateLimits {
        plan_type: None,
        primary_pct,
        primary_reset_at,
        secondary_pct,
        secondary_reset_at,
        sonnet_weekly_pct,
        sonnet_weekly_reset_at,
        opus_weekly_pct,
        opus_weekly_reset_at,
        extra_used,
        extra_limit,
    })
}

/// 实时额度结果缓存（成功 60s / 失败 15s 双 TTL）。
/// 成功缓存：前端多命令高频触发，防止打爆端点（该端点对高频请求返回 429）。
/// 失败负缓存：无凭据/网络不通时同样会被高频触发，一轮 30s tick 内并发
/// 4~5 个真实 HTTP 请求（各 10s 超时）既浪费也可能触发 Anthropic 限流，
/// 故失败结果也短暂缓存。
static LIVE_LIMITS_CACHE: OnceLock<
    Mutex<Option<(std::time::Instant, Result<Option<ClaudeRateLimits>, ClaudeUsageFailure>)>>,
> = OnceLock::new();

/// OAuth 额度端点（Claude Code CLI 内部同款；需 claude.ai 订阅 OAuth 登录，
/// 需带 anthropic-beta: oauth-2025-04-20 头）。
const OAUTH_USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

/// 拉取 Claude 订阅额度：GET https://api.anthropic.com/api/oauth/usage
/// （第三方中转/API Key 模式无凭据，返回 Err 由调用方降级为不展示额度块）。
#[allow(dead_code)]
pub fn fetch_live_rate_limits() -> Result<Option<ClaudeRateLimits>, String> {
    fetch_live_rate_limits_with_freshness()
        .map(|(limits, _)| limits)
        .map_err(|f| f.message())
}

/// 拉取实时额度并标记本次结果是否来自新的 HTTP 请求。
/// 缓存命中仍可用于当前进度展示，但不应作为新的历史采样。
/// 失败按 ClaudeUsageFailure 分类（授权类可透出明确提示，网络类静默降级）。
pub fn fetch_live_rate_limits_with_freshness(
) -> Result<(Option<ClaudeRateLimits>, bool), ClaudeUsageFailure> {
    let cache = LIVE_LIMITS_CACHE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, val)) = guard.as_ref() {
            let ttl = if val.is_ok() { 60 } else { 15 };
            if at.elapsed() < std::time::Duration::from_secs(ttl) {
                return val.clone().map(|limits| (limits, false));
            }
        }
    }

    let result = fetch_live_rate_limits_uncached();
    *cache.lock().unwrap_or_else(|p| p.into_inner()) =
        Some((std::time::Instant::now(), result.clone()));
    result.map(|limits| (limits, true))
}

/// 用指定 OAuth access token 请求 usage 端点（本地登录态链路与手动凭证
/// 链路共用；agent 由调用方构造，各自决定超时）。返回展平后的
/// (HTTP 状态码, 响应体)；401/403 归类为 TokenRejected，网络层失败归 Other。
fn fetch_usage_raw(
    agent: &ureq::Agent,
    token: &str,
) -> Result<(u16, Option<String>), ClaudeUsageFailure> {
    let resp = agent
        .get(OAUTH_USAGE_ENDPOINT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-code/2.1.0")
        .call();
    let (status, body) =
        crate::provider_quota::flatten_response(resp).map_err(ClaudeUsageFailure::Other)?;
    if status == 401 || status == 403 {
        return Err(ClaudeUsageFailure::TokenRejected);
    }
    Ok((status, body))
}

fn fetch_live_rate_limits_uncached() -> Result<Option<ClaudeRateLimits>, ClaudeUsageFailure> {
    let auth = load_claude_auth()?;
    let mut builder = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10));
    // 复用 Codex 模块的代理探测（环境变量 > 系统代理 > 直连）：
    // api.anthropic.com 与 chatgpt.com 一样在部分网络需代理才可达
    if let Some(url) = crate::codex::resolve_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("[zbar-claude] 代理地址无效（改为直连）: {url} ({e})"),
        }
    }
    let agent = builder.build();

    let (status, body) = fetch_usage_raw(&agent, &auth.access_token)?;
    if status != 200 {
        return Err(ClaudeUsageFailure::Other(format!(
            "实时额度查询失败（HTTP {status}）"
        )));
    }
    let mut limits = parse_usage_response(body.as_deref().unwrap_or_default())?;
    // 套餐名：parse_credentials_json 已做乘数档位归一（Max 5x / Max 20x）
    limits.plan_type = auth.plan_label;

    let has_window = limits.primary_pct.is_some()
        || limits.secondary_pct.is_some()
        || limits.opus_weekly_pct.is_some()
        || limits.sonnet_weekly_pct.is_some();
    Ok(has_window.then_some(limits))
}

// ===== 手动凭证多账号查询（provider_quota 的 "claude" 分支调用）=====
// 用户在凭证体系（~/.zbar/credentials/claude.json）手动添加 kind=token 条目
// （sk-ant-oat 开头的 OAuth access token，可从本机/另一台机器的
// .credentials.json 的 claudeAiOauth.accessToken 取得）。本地登录态不并入
// 本链路——本地路径继续走 fetch_live_rate_limits（带 60s 缓存与历史采样），
// 避免同一 token 双查询。

/// ClaudeRateLimits → 通用额度面板窗口列表（手动凭证条目卡渲染用）。
/// 标题与 grok.rs 同为硬编码中文短语（后端产出、前端直展示）；
/// 百分比窗口 clamp 0-100，超额消费为金额行（limit 缺失时前端只展示金额）。
fn quota_windows_from_limits(
    limits: &ClaudeRateLimits,
) -> Vec<crate::provider_quota::ProviderQuotaWindow> {
    use crate::provider_quota::ProviderQuotaWindow;
    let pct_window = |key: &str, title: &str, pct: Option<f64>, reset: Option<i64>| {
        pct.map(|p| ProviderQuotaWindow {
            key: key.to_string(),
            title: title.to_string(),
            used_percent: Some(p.clamp(0.0, 100.0)),
            used: None,
            total: None,
            unit: None,
            resets_at: reset,
        })
    };
    let mut out = Vec::new();
    for w in [
        pct_window("hour5", "5 小时", limits.primary_pct, limits.primary_reset_at),
        pct_window("weekly", "本周", limits.secondary_pct, limits.secondary_reset_at),
        pct_window("opus_weekly", "Opus 周额度", limits.opus_weekly_pct, limits.opus_weekly_reset_at),
        pct_window("sonnet_weekly", "Sonnet 周额度", limits.sonnet_weekly_pct, limits.sonnet_weekly_reset_at),
    ]
    .into_iter()
    .flatten()
    {
        out.push(w);
    }
    if limits.extra_used.is_some() || limits.extra_limit.is_some() {
        out.push(ProviderQuotaWindow {
            key: "extra_usage".to_string(),
            title: "超额消费".to_string(),
            used_percent: None,
            used: limits.extra_used,
            total: limits.extra_limit,
            unit: Some("$".to_string()),
            resets_at: None,
        });
    }
    out
}

/// 手动凭证的单条查询结果 → 展示条目（纯函数，单测直接构造输入）。
/// 分支：401/403(expired「Token 已失效」；fetch_usage_raw 归 TokenRejected，
/// 此处对原始状态码做同款兜底) > 网络失败(error) > 非 200(error)
/// > 解析失败/缺用量(error) > 成功(ok + 各窗口 + 超额消费行)。
/// 手动 token 无凭据 JSON，套餐名无来源（plan_name=None）。
fn entry_from_usage_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), ClaudeUsageFailure>,
) -> crate::provider_quota::ProviderQuotaEntry {
    use crate::provider_quota::{now_ms, ProviderQuotaEntry};
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
    let token_rejected = || {
        fail(
            "expired",
            "Token 已失效，请重新获取并更新 sk-ant-oat 凭证".to_string(),
        )
    };
    let (status, body) = match raw {
        Ok(pair) => pair.clone(),
        Err(f) => {
            let (status, message) = match f {
                ClaudeUsageFailure::TokenRejected => (
                    "expired",
                    "Token 已失效，请重新获取并更新 sk-ant-oat 凭证".to_string(),
                ),
                ClaudeUsageFailure::MissingOAuth(m) => ("error", m.clone()),
                ClaudeUsageFailure::Other(e) => ("error", format!("Claude 额度{e}")),
            };
            return fail(status, message);
        }
    };
    if status == 401 || status == 403 {
        return token_rejected();
    }
    if status != 200 {
        return fail("error", format!("Claude 额度查询失败（HTTP {status}）"));
    }
    let limits = match parse_usage_response(body.as_deref().unwrap_or_default()) {
        Ok(l) => l,
        Err(ClaudeUsageFailure::Other(e)) => return fail("error", e),
        Err(_) => return fail("error", "Claude 额度响应解析失败".to_string()),
    };
    let windows = quota_windows_from_limits(&limits);
    if windows.is_empty() {
        return fail("error", "Claude 额度响应缺少用量数据".to_string());
    }
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name: limits.plan_type.clone(),
        message: None,
        updated_at: now_ms(),
    }
}

/// 查询手动凭证（kind=token）的订阅额度：逐条调同一 OAuth usage 端点
/// （Bearer <手动token> + 同 beta 头），解析复用 parse_usage_response。
/// 单条失败产出 error/expired 条目，不阻塞其他条目；secret 为空的凭证跳过。
pub(crate) fn fetch_manual_quota_entries(
    snapshots: &[crate::provider_credentials::CredentialQuerySnapshot],
) -> Vec<crate::provider_quota::ProviderQuotaEntry> {
    let agent = crate::provider_quota::quota_http_agent();
    let mut entries = Vec::new();
    for cred in snapshots {
        let token = cred.secret.trim();
        if token.is_empty() {
            continue;
        }
        entries.push(entry_from_usage_raw(
            &cred.id,
            &cred.label,
            &fetch_usage_raw(&agent, token),
        ));
    }
    entries
}

// ===== 诊断 =====

/// Claude 诊断信息（排查"无数据"问题）
#[derive(Debug, Clone, Serialize)]
pub struct ClaudeDebugInfo {
    /// projects 目录路径
    pub projects_dir: String,
    /// 目录是否存在
    pub projects_dir_exists: bool,
    /// 目录下会话 jsonl 文件数
    pub session_files: usize,
    /// 导入库累计记录数
    pub imported_records: i64,
    /// 最新一条用量的时间（毫秒）
    pub latest_session_ms: Option<i64>,
}

/// 诊断信息（get_codex_debug 同款用途）。导入失败不阻断——目录不存在时
/// 恰恰要靠这些字段定位问题。诊断必须真实执行导入（绕过节流）。
pub fn debug_info() -> Result<ClaudeDebugInfo, String> {
    if let Err(e) = import_incremental_force() {
        eprintln!("[zbar-claude] 诊断时增量导入失败: {e}");
    }

    let dir = projects_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 5, &mut files);
    }

    let (imported_records, latest_session_ms) = open_claude_db()
        .map(|conn| {
            conn.query_row(
                "SELECT COUNT(*), MAX(started_at) FROM model_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, None))
        })
        .unwrap_or((0, None));

    Ok(ClaudeDebugInfo {
        projects_dir: dir.display().to_string(),
        projects_dir_exists: dir.is_dir(),
        session_files: files.len(),
        imported_records,
        latest_session_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 凭据 JSON 解析：标准结构 / 缺 accessToken 报错 / 无 claudeAiOauth 报错。
    #[test]
    fn parse_credentials_variants() {
        let auth = parse_credentials_json(
            r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r",
                "expiresAt":1755000000000,"scopes":["user:profile"],
                "rateLimitTier":"pro","subscriptionType":"pro"}}"#,
        )
        .expect("标准凭据解析失败");
        assert_eq!(auth.access_token, "tok");
        assert_eq!(auth.plan_label.as_deref(), Some("pro"));

        let tier_only = parse_credentials_json(
            r#"{"claudeAiOauth":{"accessToken":"tok","rateLimitTier":"max5"}}"#,
        )
        .expect("仅 rateLimitTier 的凭据解析失败");
        assert_eq!(tier_only.plan_label.as_deref(), Some("max5"));

        // 无 claudeAiOauth（如 Claude Code 2.1.x 钥匙串条目仅含 mcpOAuth）/
        // 有条目但缺 accessToken：均为明确错误态（用户可运行 claude 自助修复），
        // 而非模糊网络错误
        for broken in [r#"{"mcpOAuth":{}}"#, r#"{"claudeAiOauth":{}}"#] {
            let err = parse_credentials_json(broken).expect_err("应报 MissingOAuth");
            match err {
                ClaudeUsageFailure::MissingOAuth(m) => {
                    assert_eq!(m, MISSING_OAUTH_MESSAGE);
                }
                other => panic!("应为 MissingOAuth，实际 {other:?}"),
            }
        }
        // JSON 本身损坏 → Other（静默降级，不误导为授权问题）
        assert!(matches!(
            parse_credentials_json("not json"),
            Err(ClaudeUsageFailure::Other(_))
        ));
    }

    /// 套餐推断增强：rateLimitTier 含乘数标识（default_claude_max_5x /
    /// default_claude_max_20x）→ 套餐名归一为 "Max 5x"/"Max 20x"；
    /// 无乘数的 tier/subscriptionType 沿用既有优先级原样展示。
    #[test]
    fn plan_tier_multiplier_label() {
        let parse_label = |json: &str| {
            parse_credentials_json(json)
                .expect("凭据解析失败")
                .plan_label
        };
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"default_claude_max_5x"}}"#).as_deref(),
            Some("Max 5x")
        );
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"default_claude_max_20x"}}"#).as_deref(),
            Some("Max 20x")
        );
        // subscriptionType 通常不携带乘数；tier 含乘数时乘数优先
        assert_eq!(
            parse_label(
                r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"default_claude_max_20x","subscriptionType":"max"}}"#
            )
            .as_deref(),
            Some("Max 20x")
        );
        // 无乘数：沿用 subscriptionType → rateLimitTier 优先级
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"max","subscriptionType":"pro"}}"#).as_deref(),
            Some("pro")
        );
        // "max5"（无 x）不是乘数标识，原样保留
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"max5"}}"#).as_deref(),
            Some("max5")
        );
    }

    /// OAuth 额度响应解析口径：utilization 百分比直读、resets_at ISO 转毫秒。
    #[test]
    fn oauth_usage_response_parsing() {
        let v: OAuthUsageResponse = serde_json::from_str(
            r#"{"five_hour":{"utilization":42.5,"resets_at":"2026-08-14T12:00:00Z"},
                "seven_day":{"utilization":13.0,"resets_at":"2026-08-17T03:30:00Z"}}"#,
        )
        .expect("响应解析失败");
        assert_eq!(v.five_hour.as_ref().unwrap().utilization, Some(42.5));
        assert_eq!(v.seven_day.as_ref().unwrap().utilization, Some(13.0));
        // 毫秒换算用纪元原点验证（数值直观不易错），另验证带毫秒小数的时间可解析
        assert_eq!(parse_ts_ms("1970-01-01T00:00:00Z"), Some(0));
        assert!(parse_ts_ms("2026-06-12T19:12:00.759Z").is_some());
    }

    /// 模型专属周窗口：顶层 seven_day_sonnet / seven_day_opus 字段映射到
    /// ClaudeRateLimits 增量字段（含 resets_at 毫秒转换）。
    #[test]
    fn model_scoped_weekly_windows() {
        let limits = parse_usage_response(
            r#"{"five_hour":{"utilization":10.0,"resets_at":"2026-08-14T12:00:00Z"},
                "seven_day":{"utilization":20.0,"resets_at":"2026-08-17T03:30:00Z"},
                "seven_day_sonnet":{"utilization":33.0,"resets_at":"2026-08-17T04:00:00Z"},
                "seven_day_opus":{"utilization":55.5,"resets_at":"2026-08-18T05:00:00Z"}}"#,
        )
        .expect("解析失败");
        assert_eq!(limits.primary_pct, Some(10.0));
        assert_eq!(limits.secondary_pct, Some(20.0));
        assert_eq!(limits.sonnet_weekly_pct, Some(33.0));
        assert_eq!(
            limits.sonnet_weekly_reset_at,
            parse_ts_ms("2026-08-17T04:00:00Z")
        );
        assert_eq!(limits.opus_weekly_pct, Some(55.5));
        assert_eq!(
            limits.opus_weekly_reset_at,
            parse_ts_ms("2026-08-18T05:00:00Z")
        );
        assert_eq!(limits.extra_used, None);
        assert_eq!(limits.extra_limit, None);
    }

    /// limits[] 补充窗口（CodexBar 口径）：weekly_scoped=true 且 scope 可识别
    /// 模型（Opus/Sonnet）→ 补进对应模型专属槽位；"All models" 通用 scope 留
    /// 主周行不覆盖；five_hour 类型的 scoped 条目忽略；顶层字段优先；
    /// 无法识别模型名的 scoped 条目保守忽略。
    #[test]
    fn weekly_scoped_limits_fill() {
        let limits = parse_usage_response(
            r#"{"five_hour":{"utilization":10.0},
                "seven_day":{"utilization":20.0},
                "limits":[
                    {"limit_type":"five_hour","scope":"All models","utilization":11.0},
                    {"limit_type":"seven_day","scope":"All models","utilization":21.0},
                    {"limit_type":"seven_day","scope":"Opus","weekly_scoped":true,"utilization":66.0,"resets_at":"2026-08-18T05:00:00Z"},
                    {"limit_type":"seven_day","scope":"Sonnet","weekly_scoped":true,"utilization":44.0}
                ]}"#,
        )
        .expect("解析失败");
        // 通用 "All models" scope 不覆盖主周行（仍是 seven_day 的 20.0）
        assert_eq!(limits.secondary_pct, Some(20.0));
        assert_eq!(limits.primary_pct, Some(10.0));
        assert_eq!(limits.opus_weekly_pct, Some(66.0));
        assert_eq!(limits.sonnet_weekly_pct, Some(44.0));

        // 顶层 seven_day_opus 优先，limits 里的 scoped 条目不覆盖已有槽位
        let limits = parse_usage_response(
            r#"{"seven_day_opus":{"utilization":5.0},
                "limits":[{"limit_type":"seven_day","scope":"Opus","weekly_scoped":true,"utilization":99.0},
                          {"limit_type":"seven_day","scope":"Haiku","weekly_scoped":true,"utilization":7.0}]}"#,
        )
        .expect("解析失败");
        assert_eq!(limits.opus_weekly_pct, Some(5.0));
        assert_eq!(limits.sonnet_weekly_pct, None); // 未知模型名保守忽略
    }

    /// extra_usage（月度超额消费）：spend/limit 与 used/limit 双兼容取值，
    /// 两者全缺 / 无该字段 → None（不渲染附加信息行）。
    #[test]
    fn extra_usage_parsing() {
        // spend/limit 结构
        let limits =
            parse_usage_response(r#"{"extra_usage":{"spend":3.2,"limit":35}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, Some(3.2));
        assert_eq!(limits.extra_limit, Some(35.0));
        // used/limit 兼容形态 + 仅一侧有值
        let limits =
            parse_usage_response(r#"{"extra_usage":{"used":1.5}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, Some(1.5));
        assert_eq!(limits.extra_limit, None);
        // 字符串数字（数值解析弹性，与 provider_quota::parse_flexible_f64 同口径）
        let limits =
            parse_usage_response(r#"{"extra_usage":{"spend":"12.5","monthly_limit":"100"}}"#)
                .expect("解析失败");
        assert_eq!(limits.extra_used, Some(12.5));
        assert_eq!(limits.extra_limit, Some(100.0));
        // 全缺 / 空对象 / 无字段 → None
        let limits =
            parse_usage_response(r#"{"extra_usage":{"other":1}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, None);
        assert_eq!(limits.extra_limit, None);
        let limits = parse_usage_response(r#"{"five_hour":{"utilization":1.0}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, None);
    }

    /// 旧响应零回归：只有 five_hour/seven_day 的历史响应，解析后增量字段
    /// 全部为 None，主字段口径与改造前一致。
    #[test]
    fn legacy_response_backward_compat() {
        let limits = parse_usage_response(
            r#"{"five_hour":{"utilization":42.5,"resets_at":"2026-08-14T12:00:00Z"},
                "seven_day":{"utilization":13.0,"resets_at":"2026-08-17T03:30:00Z"}}"#,
        )
        .expect("解析失败");
        assert_eq!(limits.plan_type, None);
        assert_eq!(limits.primary_pct, Some(42.5));
        assert_eq!(limits.secondary_pct, Some(13.0));
        assert_eq!(limits.sonnet_weekly_pct, None);
        assert_eq!(limits.sonnet_weekly_reset_at, None);
        assert_eq!(limits.opus_weekly_pct, None);
        assert_eq!(limits.opus_weekly_reset_at, None);
        assert_eq!(limits.extra_used, None);
        assert_eq!(limits.extra_limit, None);
        // 坏 JSON → Other（本地链路静默降级）
        assert!(matches!(
            parse_usage_response("not json"),
            Err(ClaudeUsageFailure::Other(_))
        ));
    }

    /// 手动凭证条目状态映射：401/403 → expired「Token 已失效」；网络失败 /
    /// 非 200 / 坏 JSON → error；完整成功响应 → ok + 各窗口 + 超额消费金额行。
    #[test]
    fn manual_entry_status_mapping() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some("denied".to_string())));
            let entry = entry_from_usage_raw("abc-1", "Max 订阅", &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert!(
                entry.message.as_deref().unwrap().contains("Token 已失效"),
                "expired 消息应包含 Token 已失效"
            );
            assert!(entry.windows.is_empty());
        }
        // 网络层失败 → error
        let raw: Result<(u16, Option<String>), ClaudeUsageFailure> =
            Err(ClaudeUsageFailure::Other("网络错误或服务不可用: timeout".into()));
        let entry = entry_from_usage_raw("abc-1", "手动", &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误"));
        // 500 → error 带状态码
        let entry = entry_from_usage_raw("abc-1", "手动", &Ok((500, Some("oops".into()))));
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
        // 200 坏 JSON → error
        let entry = entry_from_usage_raw("abc-1", "手动", &Ok((200, Some("not json".into()))));
        assert_eq!(entry.status, "error");
    }

    /// 手动凭证成功路径：ok 条目携带 hour5/weekly/opus/sonnet 四个百分比窗口
    /// （标题为后端硬编码中文短语）与超额消费金额行。
    #[test]
    fn manual_entry_ok_windows() {
        let body = r#"{"five_hour":{"utilization":10.0,"resets_at":"2026-08-14T12:00:00Z"},
            "seven_day":{"utilization":20.0,"resets_at":"2026-08-17T03:30:00Z"},
            "seven_day_opus":{"utilization":66.0},
            "seven_day_sonnet":{"utilization":44.0},
            "extra_usage":{"spend":3.2,"limit":35}}"#;
        let entry = entry_from_usage_raw("cred-9", "另一台机器", &Ok((200, Some(body.into()))));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.message, None);
        let keys: Vec<&str> = entry.windows.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["hour5", "weekly", "opus_weekly", "sonnet_weekly", "extra_usage"]);
        assert_eq!(entry.windows[0].title, "5 小时");
        assert_eq!(entry.windows[1].title, "本周");
        assert_eq!(entry.windows[2].title, "Opus 周额度");
        assert_eq!(entry.windows[3].title, "Sonnet 周额度");
        // 超额消费金额行：used/total 为美元金额，unit 为 $
        let extra = &entry.windows[4];
        assert_eq!(extra.title, "超额消费");
        assert_eq!(extra.used, Some(3.2));
        assert_eq!(extra.total, Some(35.0));
        assert_eq!(extra.unit.as_deref(), Some("$"));
        assert_eq!(extra.used_percent, None);
        // 响应无任何窗口 → error（缺用量数据）
        let entry = entry_from_usage_raw("cred-9", "手动", &Ok((200, Some("{}".into()))));
        assert_eq!(entry.status, "error");
    }

    /// 冒烟测试：对本机真实 projects 目录执行增量导入并做全范围查询，
    /// 打印结果供人工核对（无 Claude 环境时跳过，不视为失败）。
    /// 重复运行第二次验证幂等：记录数不因重复导入而变化。
    #[test]
    fn import_and_query_smoke() {
        if projects_dir().is_err() {
            eprintln!("[test] 本机无 Claude projects 目录，跳过");
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
        // 趋势管道：至少有一个非空桶
        if let (Some(e), Some(l)) = (first.earliest_ms, first.latest_ms) {
            let trend = query_trend(e, l + 3_600_000, "hour").expect("趋势查询失败");
            let non_empty = trend.iter().filter(|b| b.total_tokens > 0).count();
            assert!(non_empty > 0, "趋势桶全部为空");
            eprintln!("[test] trend 桶数={} 非空桶={}", trend.len(), non_empty);
        }
        eprintln!(
            "[test] requests={} overall={:?} by_model={:?}",
            first.overall.requests,
            first.overall,
            first
                .by_model
                .iter()
                .map(|m| (m.model_id.clone(), m.requests, m.total_tokens))
                .collect::<Vec<_>>(),
        );
        // 修订查询：单次导入内"部分值行先入库、终值行覆盖"会打上 updated_at，
        // 本机存在多行 message 的会话时应有修订行；无则 0 条（查询本身必须成功）
        let revised = query_revised_since(0, 0, 500).expect("修订查询失败");
        eprintln!(
            "[test] 修订行数量={}（示例 {:?}）",
            revised.len(),
            revised
                .iter()
                .take(3)
                .map(|r| (r.local_rowid, r.computed_total_tokens))
                .collect::<Vec<_>>()
        );
        // 实时额度（OAuth）：本机未登录订阅/网络不通时应返回 Err（额度块不展示）
        match fetch_live_rate_limits() {
            Ok(v) => eprintln!("[test] 实时额度: {v:?}"),
            Err(e) => eprintln!("[test] 实时额度不可用（不展示额度块）: {e}"),
        }
    }
}

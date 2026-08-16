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
fn open_claude_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = claude_db_path()?;
    let conn = Connection::open(&path).map_err(|e| format!("打开 Claude 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;

    // 结构版本迁移：表存在但无 dedupe_key 列（旧版结构）时整表重建。
    // 本库是从原始 jsonl 全量派生的缓存库，重建后下次导入自动补齐，
    // 唯一代价是同步游标之后的记录重新入库（新 id，正常增量上传）。
    let legacy = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'model_usage')
                  + (SELECT COUNT(*) FROM pragma_table_info('model_usage')
                     WHERE name = 'dedupe_key')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c == 1) // 表存在(1) + 无新列(0) → 旧结构；表不存在(0) 或新结构(2) → 无需迁移
        .unwrap_or(false);
    if legacy {
        eprintln!("[zbar-claude] 检测到旧版导入库结构，重建（自动从原始会话重新导入）");
        conn.execute_batch("DROP TABLE IF EXISTS model_usage; DROP TABLE IF EXISTS file_progress;")
            .map_err(|e| format!("重建 Claude 导入库失败: {e}"))?;
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
            updated_at INTEGER NOT NULL DEFAULT 0,
            UNIQUE(dedupe_key)
         );
         CREATE INDEX IF NOT EXISTS idx_claude_model_usage_started ON model_usage(started_at);
         CREATE INDEX IF NOT EXISTS idx_claude_model_usage_updated ON model_usage(updated_at);
         CREATE TABLE IF NOT EXISTS file_progress (
            path   TEXT    PRIMARY KEY,
            offset INTEGER NOT NULL,
            size   INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("初始化 Claude 导入库失败: {e}"))?;
    Ok(conn)
}

// ===== jsonl 行解析结构（未知字段自动忽略，巨大的 content 数组不会物化）=====

/// jsonl 单行事件。只取关心的时间戳/类型/message。
#[derive(Debug, Deserialize)]
struct TranscriptLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    line_type: Option<String>,
    #[serde(default)]
    message: Option<AssistantMessage>,
}

/// assistant 行的 message 对象（content 不取，serde 忽略未知字段）。
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
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
    Ok(())
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
        tx.execute(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 updated_at)
             VALUES (?1, ?2, ?3, ?4, 'claude', ?5, ?6, ?7, ?8, 0, ?9, 0)
             ON CONFLICT(dedupe_key) DO UPDATE SET
                started_at = excluded.started_at,
                model_id = excluded.model_id,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens,
                cache_creation_input_tokens = excluded.cache_creation_input_tokens,
                computed_total_tokens = excluded.computed_total_tokens,
                updated_at = ?10
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
                now_ms,
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
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_claude_db()?;

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
        .map_err(|e| format!("查询 Claude 整体统计失败: {e}"))?;

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
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
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
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens
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

// ===== 实时额度（Anthropic OAuth 端点，参照 CodexBar 的实现）=====

/// Claude 订阅额度（字段口径与 CodexRateLimits 一致，前端同款渲染）。
/// plan_type 来自凭据的订阅类型（pro/max 等；中转模式无凭据 → None）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeRateLimits {
    pub plan_type: Option<String>,
    pub primary_pct: Option<f64>,
    /// 5 小时会话窗口重置时间（毫秒时间戳）
    pub primary_reset_at: Option<i64>,
    pub secondary_pct: Option<f64>,
    /// 周窗口重置时间（毫秒时间戳）
    pub secondary_reset_at: Option<i64>,
}

/// Claude Code 登录凭证（只读，绝不修改/刷新——refresh_token 一次性轮换，
/// 外部写回极易搞坏 Claude Code 登录；token 过期由 Claude Code 自行刷新）。
struct ClaudeAuth {
    access_token: String,
    /// 订阅类型徽标（subscriptionType 优先，rateLimitTier 兜底）
    plan_label: Option<String>,
}

/// 解析 .credentials.json / Keychain 里的凭据 JSON。
/// 结构：{ "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt"(毫秒),
/// "scopes", "rateLimitTier", "subscriptionType" } }（字段名为 camelCase）。
fn parse_credentials_json(data: &str) -> Result<ClaudeAuth, String> {
    let v: serde_json::Value =
        serde_json::from_str(data).map_err(|e| format!("解析 Claude 凭据失败: {e}"))?;
    let oauth = v
        .get("claudeAiOauth")
        .ok_or("Claude 凭据中无 claudeAiOauth（可能仅 MCP OAuth 或第三方中转配置）")?;
    let token = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or("Claude 凭据中无 accessToken")?;
    let plan_label = oauth
        .get("subscriptionType")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            oauth
                .get("rateLimitTier")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
        })
        .map(|s| s.to_string());
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
fn load_claude_auth() -> Result<ClaudeAuth, String> {
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
        let data = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取 .credentials.json 失败: {e}"))?;
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

    Err("未找到 Claude 登录凭证（.credentials.json / 钥匙串），订阅额度不可用（token 统计不受影响）".into())
}

/// /api/oauth/usage 响应结构（窗口字段名与 Codex 的 wham/usage 不同：
/// 这里是 five_hour/seven_day + utilization 百分比(0-100) + resets_at ISO8601）。
#[derive(Debug, Deserialize)]
struct OAuthUsageResponse {
    #[serde(default)]
    five_hour: Option<OAuthWindow>,
    #[serde(default)]
    seven_day: Option<OAuthWindow>,
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

/// 实时额度结果缓存（成功 60s / 失败 15s 双 TTL）。
/// 成功缓存：前端多命令高频触发，防止打爆端点（该端点对高频请求返回 429）。
/// 失败负缓存：无凭据/网络不通时同样会被高频触发，一轮 30s tick 内并发
/// 4~5 个真实 HTTP 请求（各 10s 超时）既浪费也可能触发 Anthropic 限流，
/// 故失败结果也短暂缓存。
static LIVE_LIMITS_CACHE: OnceLock<Mutex<Option<(std::time::Instant, Result<Option<ClaudeRateLimits>, String>)>>> =
    OnceLock::new();

/// 拉取 Claude 订阅额度：GET https://api.anthropic.com/api/oauth/usage
/// （Claude Code CLI 内部同款端点，需 claude.ai 订阅 OAuth 登录；需带
/// anthropic-beta: oauth-2025-04-20 头。第三方中转/API Key 模式无凭据，
/// 返回 Err 由调用方降级为不展示额度块）。
pub fn fetch_live_rate_limits() -> Result<Option<ClaudeRateLimits>, String> {
    let cache = LIVE_LIMITS_CACHE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, val)) = guard.as_ref() {
            let ttl = if val.is_ok() { 60 } else { 15 };
            if at.elapsed() < std::time::Duration::from_secs(ttl) {
                return val.clone();
            }
        }
    }

    let result = fetch_live_rate_limits_uncached();
    *cache.lock().unwrap_or_else(|p| p.into_inner()) =
        Some((std::time::Instant::now(), result.clone()));
    result
}

fn fetch_live_rate_limits_uncached() -> Result<Option<ClaudeRateLimits>, String> {
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

    let resp: OAuthUsageResponse = agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {}", auth.access_token))
        .set("Accept", "application/json")
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-code/2.1.0")
        .call()
        .map_err(|e| format!("实时额度请求失败: {e}"))?
        .into_json()
        .map_err(|e| format!("解析实时额度失败: {e}"))?;

    let conv = |w: &OAuthWindow| -> (Option<f64>, Option<i64>) {
        (
            w.utilization,
            w.resets_at
                .as_deref()
                .and_then(parse_ts_ms),
        )
    };
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

    let result = if primary_pct.is_some() || secondary_pct.is_some() {
        Some(ClaudeRateLimits {
            plan_type: auth.plan_label,
            primary_pct,
            primary_reset_at,
            secondary_pct,
            secondary_reset_at,
        })
    } else {
        None
    };

    Ok(result)
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

        assert!(parse_credentials_json(r#"{"claudeAiOauth":{}}"#).is_err());
        assert!(parse_credentials_json(r#"{"mcpOAuth":{}}"#).is_err());
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
        // 实时额度（OAuth）：本机未登录订阅/网络不通时应返回 Err（额度块不展示）。
        match fetch_live_rate_limits() {
            Ok(v) => eprintln!("[test] 实时额度: {v:?}"),
            Err(e) => eprintln!("[test] 实时额度不可用（不展示额度块）: {e}"),
        }
    }
}
